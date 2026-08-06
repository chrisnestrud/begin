//! BEGIN — A Tactical Starship Simulation.
//! Rust port of begin2.exe console mode (see AI_AND_COMBAT.md).

mod net;

use begin::session::{self, Out, SessionOpts};
use begin::ui::{CYAN, GREY, RED, RESET, WHITE};
use begin_core::scenario::Scenario;
use begin_core::{GameData, Tuning};
use std::io::{BufRead, Write};

fn main() {
    let mut args: Vec<String> = std::env::args().collect();
    // multiplayer subcommands: `begin host ...` / `begin join <code> ...`
    let mode = if args.len() > 1 && (args[1] == "host" || args[1] == "join") {
        args.remove(1)
    } else {
        String::new()
    };
    let mut server = net::DEFAULT_SERVER.to_string();
    let mut code = String::new();
    let mut players: u16 = 2;
    let mut versus = true;
    let mut tuning = Tuning::default();
    let mut seed = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(1);
    let mut quick = false;
    let mut flash = true;
    let mut epoch = 0.0f64;
    let mut spawn_body: Option<String> = None;
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--no-flash" => flash = false,
            "--seed" => {
                i += 1;
                seed = args.get(i).and_then(|s| s.parse().ok()).unwrap_or(seed);
            }
            "--quick" => quick = true,
            "--planar-lock" => tuning.planar_lock = true,
            "--begin1" => tuning = Tuning::begin1(), // the default; kept for compat
            "--begin2" => tuning = Tuning::begin2(),
            "--date" => {
                i += 1;
                epoch = args
                    .get(i)
                    .map(|s| begin_core::env::parse_epoch(s))
                    .unwrap_or(0.0);
            }
            "--near" => {
                i += 1;
                spawn_body = args.get(i).cloned();
            }
            "--server" => {
                i += 1;
                server = args.get(i).cloned().unwrap_or(server);
            }
            "--code" => {
                i += 1;
                code = args.get(i).cloned().unwrap_or_default();
            }
            "--players" => {
                i += 1;
                players = args.get(i).and_then(|s| s.parse().ok()).unwrap_or(2);
            }
            "--coop" => versus = false,
            "--versus" => versus = true,
            x if mode == "join" && code.is_empty() && !x.starts_with('-') => {
                code = x.to_string();
            }
            x => {
                eprintln!("unknown option {x}");
                eprintln!("usage: begin [host|join <code>] [--quick] [--seed N] [--planar-lock] [--begin2]");
                eprintln!("             [--date YYYY-MM-DD] [--near Body:low|high|rings] [--no-flash]");
                eprintln!("             [--server URL] [--code C] [--players N] [--coop|--versus]");
                std::process::exit(2);
            }
        }
        i += 1;
    }

    println!("{WHITE}BEGIN - A Tactical Starship Simulation{RESET}");
    println!("{GREY}Rust port of Begin 2.00 (c) 1984-1991 Clockwork Software{RESET}");
    println!();

    // no subcommand given: offer multiplayer from the interactive menu
    let mut mode = mode;
    if mode.is_empty() && !quick {
        let stdin = std::io::stdin();
        let mut lines = stdin.lock().lines();
        loop {
            let Some(ans) = prompt(
                "Game type?  single (Enter) / host [players] / join <code>",
                &mut lines,
            ) else {
                return;
            };
            let w: Vec<&str> = ans.split_whitespace().collect();
            match w.first().map(|s| s.to_ascii_lowercase()).as_deref() {
                None | Some("s" | "single" | "1") => break,
                Some("h" | "host") => {
                    mode = "host".into();
                    if let Some(n) = w.get(1).and_then(|s| s.parse().ok()) {
                        players = n;
                    }
                    println!(
                        "{GREY}Hosting for {players} players via {server} (use --server URL for a LAN lobby).{RESET}"
                    );
                    break;
                }
                Some("j" | "join") => match w.get(1) {
                    Some(c) => {
                        mode = "join".into();
                        code = c.to_uppercase();
                        break;
                    }
                    None => println!("{RED}join needs the room code:  join ABCD{RESET}"),
                },
                Some(_) => println!("{RED}single, host, or join <code>.{RESET}"),
            }
        }
    }

    if mode == "join" {
        if code.is_empty() {
            eprintln!("usage: begin join <code> [--server URL]");
            std::process::exit(2);
        }
        let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
        if let Err(e) = rt.block_on(net::run_client(server, code)) {
            eprintln!("{RED}{e}{RESET}");
            std::process::exit(1);
        }
        return;
    }

    let data = GameData::load();

    if mode == "host" {
        // configure the scenario on the host terminal, then open the room.
        // NB: the stdin lock must be released before run_host spawns its
        // own stdin reader thread.
        let (scenario, name) = if quick {
            let mut sc = Scenario::duel();
            sc.epoch_jd = epoch;
            sc.spawn_body = spawn_body.clone();
            sc.seed = seed;
            (sc, "Admiral".to_string())
        } else {
            let stdin = std::io::stdin();
            let mut lines = stdin.lock().lines();
            match session::setup_over_stdin(&data, seed, epoch, spawn_body, &mut lines) {
                Some(x) => x,
                None => return,
            }
        };
        let cfg = net::HostConfig {
            server,
            code,
            players,
            versus,
            scenario,
            tuning,
            seed,
            name,
        };
        let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
        if let Err(e) = rt.block_on(net::run_host(cfg)) {
            eprintln!("{RED}{e}{RESET}");
            std::process::exit(1);
        }
        return;
    }

    // single player: the shared Session drives setup and play; this loop is
    // just a stdin pump (the browser build pumps the same Session from JS)
    let stdin = std::io::stdin();
    let mut lines = stdin.lock().lines();
    let opts = SessionOpts {
        tuning,
        seed,
        epoch_jd: epoch,
        spawn_body,
        flash,
        quick,
    };
    let (mut sess, outs) = session::Session::new(data, opts);
    emit(&outs);
    while !sess.finished() {
        let Some(Ok(line)) = lines.next() else { break };
        let outs = sess.feed(&line);
        emit(&outs);
    }
}

/// Print session output on the console, realizing Hold as a sleep.
fn emit(outs: &[Out]) {
    let mut out = std::io::stdout().lock();
    for o in outs {
        match o {
            Out::Frame { ansi, .. } => {
                let _ = out.write_all(ansi.as_bytes());
                let _ = out.flush();
            }
            Out::Text(t) => {
                let _ = writeln!(out, "{t}");
            }
            Out::Prompt(p) => {
                let _ = write!(out, "{p}");
                let _ = out.flush();
            }
            Out::Hold(ms) => {
                let _ = out.flush();
                std::thread::sleep(std::time::Duration::from_millis(*ms as u64));
            }
            Out::Done => {}
        }
    }
}

fn prompt(msg: &str, lines: &mut std::io::Lines<std::io::StdinLock>) -> Option<String> {
    print!("{CYAN}{msg}{RESET} ");
    std::io::stdout().flush().ok();
    lines.next()?.ok().map(|s| s.trim().to_string())
}

