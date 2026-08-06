//! Peer-to-peer multiplayer over lobbylink (~/dev/lobbylink).
//!
//! True P2P: no game server. The hosting player's process owns the
//! simulation (begin2's control type 2 = remote human, `sub_20F6A`);
//! every other player is a dumb terminal — they see the ANSI frames the
//! host renders for them and type command lines back. Turn-based like the
//! original: the cycle runs once every seated player has entered a
//! turn-ending command (status displays answer immediately).
//!
//! Wire protocol (reliable channel, JSON):
//!   client → host: {"t":"line","text":"fire all phasers"}
//!   host → client: {"t":"frame","data":"<ansi screen>"}
//!                  {"t":"info","text":"..."}   (lobby chatter)
//!                  {"t":"over","text":"evaluation"}

use begin::ui::{GREEN, GREY, RED, RESET, WHITE};
use begin_core::scenario::Scenario;
use begin_core::{GameData, Tuning};
use p2p_lobby_client::{ConnectOptions, CreateOptions, Event, MessageKind, P2PGame, PlayerId};
use std::io::Write;

pub const DEFAULT_SERVER: &str = "https://pqrstuvw.xyz/lobbylink";
const DEFAULT_ORIGIN: &str = "https://pqrstuvw.xyz";

/// The prod signaling server requires an allowlisted Origin header;
/// local/dev servers run with --allow-no-origin and want none.
fn origin_for(server: &str) -> Option<String> {
    if server == DEFAULT_SERVER {
        Some(DEFAULT_ORIGIN.into())
    } else {
        None
    }
}

.to_string())
}

pub struct HostConfig {
    pub server: String,
    pub code: String,
    pub players: u16,
    pub versus: bool,
    pub scenario: Scenario,
    pub tuning: Tuning,
    pub seed: u64,
    pub name: String,
}

/// Host: create the room, wait for everyone, run the simulation.
pub async fn run_host(cfg: HostConfig) -> Result<(), Box<dyn std::error::Error>> {
    println!("{GREY}Creating room on {} for {} players...{RESET}", cfg.server, cfg.players);
    let mut lobby = P2PGame::connect(ConnectOptions {
        server: cfg.server.clone(),
        code: cfg.code.clone(),
        create: Some(CreateOptions {
            wait_until_full: true,
            allow_reconnect: true,
            ..CreateOptions::new(cfg.players)
        }),
        origin: origin_for(&cfg.server),
        ..Default::default()
    })
    .await?;
    println!(
        "{WHITE}Room code: {}{RESET}  (players join with: begin join {}{})",
        lobby.code(),
        lobby.code(),
        if cfg.server == DEFAULT_SERVER { String::new() } else { format!(" --server {}", cfg.server) }
    );

    // wait for the room to fill
    let mut connected: Vec<PlayerId> = lobby
        .players()
        .iter()
        .filter(|p| p.occupied && p.id != lobby.self_id())
        .map(|p| p.id)
        .collect();
    while (connected.len() as u16) + 1 < cfg.players {
        match lobby.next_event().await {
            Some(Event::PlayerJoined { player_id }) => {
                println!("{GREEN}Player {player_id} has joined.{RESET}");
                if !connected.contains(&player_id) {
                    connected.push(player_id);
                }
            }
            Some(Event::PeerState { .. }) | Some(Event::CandidatePair { .. }) => {}
            Some(Event::Started) => break,
            Some(e) => println!("{GREY}{e:?}{RESET}"),
            None => return Err("lobby closed while waiting".into()),
        }
    }
    println!("{GREEN}All hands aboard. Spawning fleets...{RESET}");

    // seat map: sorted player ids <-> seats 1..N in the shared HostCore
    connected.sort_unstable();
    let names: Vec<String> = connected.iter().map(|p| format!("Player {p}")).collect();
    let (mut core, outs) = begin::host::HostCore::new(
        GameData::load(),
        cfg.tuning.clone(),
        cfg.seed,
        &cfg.scenario,
        cfg.versus,
        cfg.name.clone(),
        names,
    )?;
    let mut done = deliver(&mut lobby, &connected, outs).await;

    // local stdin task
    let (tx, mut local_lines) = tokio::sync::mpsc::unbounded_channel::<String>();
    std::thread::spawn(move || {
        use std::io::BufRead;
        let stdin = std::io::stdin();
        for line in stdin.lock().lines() {
            let Ok(line) = line else { break };
            if tx.send(line).is_err() {
                break;
            }
        }
    });

    while !done {
        let outs = tokio::select! {
            line = local_lines.recv() => {
                match line {
                    Some(line) => core.line(0, &line),
                    None => core.line(0, "quit"), // stdin gone: wrap up
                }
            }
            ev = lobby.next_event() => {
                let Some(ev) = ev else { break };
                match ev {
                    Event::Message { from, kind: MessageKind::Reliable, data } => {
                        match serde_json::from_slice::<serde_json::Value>(&data) {
                            Ok(v) if v["t"] == "line" => {
                                match connected.iter().position(|&p| p == from) {
                                    Some(k) => core.line(k + 1, v["text"].as_str().unwrap_or("")),
                                    None => Vec::new(),
                                }
                            }
                            _ => Vec::new(),
                        }
                    }
                    Event::PlayerLeft { player_id, .. } => {
                        match connected.iter().position(|&p| p == player_id) {
                            Some(k) => core.disconnect(k + 1),
                            None => Vec::new(),
                        }
                    }
                    Event::PlayerRejoined { player_id, .. } => {
                        match connected.iter().position(|&p| p == player_id) {
                            Some(k) => core.reconnect(k + 1),
                            None => Vec::new(),
                        }
                    }
                    _ => Vec::new(),
                }
            }
        };
        done = deliver(&mut lobby, &connected, outs).await;
    }

    // let the reliable channel flush the evaluations before we leave
    tokio::time::sleep(std::time::Duration::from_millis(800)).await;
    lobby.close().await?;
    Ok(())
}

/// Realize a batch of HostCore outputs: seat 0 to this terminal, other
/// seats over the wire, Hold as a sleep. Returns whether the game ended.
async fn deliver(
    lobby: &mut P2PGame,
    pids: &[PlayerId],
    outs: Vec<begin::host::HostOut>,
) -> bool {
    use begin::host::{HostOut, Msg};
    let mut ended = false;
    for o in outs {
        match o {
            HostOut::Seat { seat: 0, msg } => match msg {
                Msg::Frame { data, .. } => {
                    let mut out = std::io::stdout().lock();
                    let _ = out.write_all(data.as_bytes());
                    let _ = out.flush();
                }
                Msg::Info(t) => println!("{GREY}{t}{RESET}"),
                Msg::Over(t) => println!("\r\n{t}"),
            },
            HostOut::Seat { seat, msg } => {
                let Some(&pid) = pids.get(seat - 1) else { continue };
                let payload = match msg {
                    Msg::Frame { data, flash } => {
                        if flash {
                            serde_json::json!({"t":"frame","data":data,"flash":true}).to_string()
                        } else {
                            serde_json::json!({"t":"frame","data":data}).to_string()
                        }
                    }
                    Msg::Info(t) => serde_json::json!({"t":"info","text":t}).to_string(),
                    Msg::Over(t) => serde_json::json!({"t":"over","text":t}).to_string(),
                };
                let _ = lobby.send_reliable(pid, bytes::Bytes::from(payload)).await;
            }
            HostOut::Hold(ms) => {
                tokio::time::sleep(std::time::Duration::from_millis(ms as u64)).await
            }
            HostOut::Ended => ended = true,
        }
    }
    ended
}

/// Client: a dumb terminal. Print what the host sends; send what we type.
pub async fn run_client(server: String, code: String) -> Result<(), Box<dyn std::error::Error>> {
    println!("{GREY}Joining room {code} on {server}...{RESET}");
    let origin = origin_for(&server);
    let mut lobby = P2PGame::connect(ConnectOptions {
        server,
        code,
        origin,
        ..Default::default()
    })
    .await?;
    println!("{GREEN}Connected as player {}. Waiting for the host to start...{RESET}", lobby.self_id());

    // the host is the lowest-numbered occupied seat that isn't us
    let host: PlayerId = lobby
        .players()
        .iter()
        .filter(|p| p.occupied && p.id != lobby.self_id())
        .map(|p| p.id)
        .min()
        .unwrap_or(0);

    let (tx, mut local_lines) = tokio::sync::mpsc::unbounded_channel::<String>();
    std::thread::spawn(move || {
        use std::io::BufRead;
        let stdin = std::io::stdin();
        for line in stdin.lock().lines() {
            let Ok(line) = line else { break };
            if tx.send(line).is_err() {
                break;
            }
        }
    });

    loop {
        tokio::select! {
            line = local_lines.recv() => {
                let Some(line) = line else { break };
                let payload = serde_json::json!({"t": "line", "text": line}).to_string();
                if lobby.send_reliable(host, bytes::Bytes::from(payload)).await.is_err() {
                    println!("{RED}Lost the host.{RESET}");
                    break;
                }
                if line.trim().eq_ignore_ascii_case("quit") {
                    break;
                }
            }
            ev = lobby.next_event() => {
                let Some(ev) = ev else { break };
                match ev {
                    Event::Message { kind: MessageKind::Reliable, data, .. } => {
                        let Ok(v) = serde_json::from_slice::<serde_json::Value>(&data) else { continue };
                        match v["t"].as_str() {
                            Some("frame") => {
                                {
                                    let mut out = std::io::stdout().lock();
                                    let _ = out.write_all(v["data"].as_str().unwrap_or("").as_bytes());
                                    let _ = out.flush();
                                }
                                // hold weapon-flash frames briefly; the
                                // settled frame follows right behind
                                if v["flash"].as_bool().unwrap_or(false) {
                                    tokio::time::sleep(std::time::Duration::from_millis(140)).await;
                                }
                            }
                            Some("info") => println!("{GREY}{}{RESET}", v["text"].as_str().unwrap_or("")),
                            Some("over") => {
                                println!("\r\n{GREEN}{}{RESET}", v["text"].as_str().unwrap_or(""));
                                lobby.close().await?;
                                return Ok(());
                            }
                            _ => {}
                        }
                    }
                    Event::PlayerLeft { player_id, .. } if player_id == host => {
                        // catch a final evaluation that may still be in flight
                        let grace = tokio::time::sleep(std::time::Duration::from_millis(700));
                        tokio::pin!(grace);
                        loop {
                            tokio::select! {
                                _ = &mut grace => break,
                                ev = lobby.next_event() => {
                                    let Some(Event::Message { kind: MessageKind::Reliable, data, .. }) = ev else { continue };
                                    let Ok(v) = serde_json::from_slice::<serde_json::Value>(&data) else { continue };
                                    if v["t"] == "over" {
                                        println!("\r\n{GREEN}{}{RESET}", v["text"].as_str().unwrap_or(""));
                                        break;
                                    }
                                }
                            }
                        }
                        println!("{RED}The host has left. Game over.{RESET}");
                        break;
                    }
                    Event::SignalingClosed { code, .. } if code != "connection-lost" => break,
                    _ => {}
                }
            }
        }
    }
    lobby.close().await?;
    Ok(())
}
