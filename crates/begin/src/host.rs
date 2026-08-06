//! Transport-agnostic multiplayer host: the begin2 "control type 2 = remote
//! human" game loop (seat queues, turn gating, per-seat frames, endgame),
//! extracted from net.rs so the native console (tokio + lobbylink-rust) and
//! the browser (JS + lobbylink-ts via begin-web) run the *same* host.
//!
//! Seat 0 is the hosting player; drivers render its messages locally and
//! ship every other seat's messages over the wire. `Hold` entries are
//! pacing (flash 140 ms, final frame 4 s) realized by the driver.

use crate::commands::{self, Outcome};
use crate::ui::{self, Display};
use crate::ui::{GREEN, RED, RESET, WHITE};
use begin_core::object::{Control, ObjId};
use begin_core::scenario::{spawn_fleets, Scenario};
use begin_core::{Game, GameData, Tuning};
use std::collections::VecDeque;

/// A message addressed to one seat (0 = the hosting player).
#[derive(Debug, Clone)]
pub enum Msg {
    Frame { data: String, flash: bool },
    Info(String),
    Over(String),
}

#[derive(Debug, Clone)]
pub enum HostOut {
    Seat { seat: usize, msg: Msg },
    /// Pause for pacing before emitting what follows.
    Hold(u32),
    /// The game is finished; after flushing, drivers shut down.
    Ended,
}

struct Seat {
    ship: Option<ObjId>,
    disp: Display,
    submitted: bool,
    connected: bool,
    name: String,
    queue: VecDeque<String>,
    pending: Option<(String, String, String)>,
}

fn prompt_of<'a>(pending: &'a Option<(String, String, String)>, name: &'a str) -> &'a str {
    pending.as_ref().map(|(q, _, _)| q.as_str()).unwrap_or(name)
}

fn resolve_pending(
    pending: &mut Option<(String, String, String)>,
    line: &str,
) -> Option<String> {
    match pending.take() {
        Some((_, prefix, default)) => {
            let ans = line.trim();
            let ans = if ans.is_empty() { default } else { ans.to_string() };
            if ans.is_empty() {
                None
            } else {
                Some(format!("{prefix} {ans}"))
            }
        }
        None => Some(line.trim().to_string()),
    }
}

pub struct HostCore {
    game: Game,
    seats: Vec<Seat>,
    host_side: usize,
    quit: bool,
    ended: bool,
}

impl HostCore {
    /// Build the game, assign seats (host takes the ally flagship; remote
    /// players take the enemy flagship first in versus, then remaining ally
    /// ships) and emit everyone's initial frame.
    pub fn new(
        data: GameData,
        tuning: Tuning,
        seed: u64,
        scenario: &Scenario,
        versus: bool,
        host_name: String,
        remote_names: Vec<String>,
    ) -> Result<(HostCore, Vec<HostOut>), String> {
        let mut game = Game::new(data, tuning, seed);
        begin_core::env::setup(&mut game, scenario.epoch_jd, scenario.spawn_body.as_deref(), seed);
        let fleets = spawn_fleets(&mut game, scenario).map_err(|e| e.to_string())?;

        let my_ship = fleets.flagship.or(fleets.ally_ids.first().copied());
        let mut pool: Vec<ObjId> = Vec::new();
        if versus {
            pool.extend(fleets.enemy_ids.iter().copied());
        }
        pool.extend(fleets.ally_ids.iter().copied().filter(|&s| Some(s) != my_ship));

        let mut seats = Vec::new();
        seats.push(Seat {
            ship: my_ship,
            disp: Display::default(),
            submitted: false,
            connected: true,
            name: host_name,
            queue: VecDeque::new(),
            pending: None,
        });
        for (k, pname) in remote_names.into_iter().enumerate() {
            let ship = pool.get(k).copied();
            if let Some(s) = ship {
                game.obj_mut(s).control = Control::Remote(pname.clone());
            }
            seats.push(Seat {
                ship,
                disp: Display::default(),
                submitted: false,
                connected: true,
                name: pname,
                queue: VecDeque::new(),
                pending: None,
            });
        }
        let host_side = my_ship.map(|s| game.obj(s).nation).unwrap_or(0);

        let mut core = HostCore { game, seats, host_side, quit: false, ended: false };
        let mut out = Vec::new();
        for k in 0..core.seats.len() {
            push_chart(&core.game, core.seats[k].ship, &mut core.seats[k].disp);
            out.push(core.frame(k, &[]));
        }
        Ok((core, out))
    }

    pub fn finished(&self) -> bool {
        self.ended
    }

    pub fn seat_count(&self) -> usize {
        self.seats.len()
    }

    /// A line arrived from a seat (0 = local host player).
    pub fn line(&mut self, seat: usize, line: &str) -> Vec<HostOut> {
        let mut out = Vec::new();
        if self.ended || seat >= self.seats.len() {
            return out;
        }
        self.seats[seat].queue.push_back(line.to_string());
        self.pump(&mut out);
        out
    }

    /// A remote seat disconnected: its ship reverts to the AI captain.
    pub fn disconnect(&mut self, seat: usize) -> Vec<HostOut> {
        let mut out = Vec::new();
        if self.ended || seat == 0 || seat >= self.seats.len() {
            return out;
        }
        self.seats[seat].connected = false;
        if let Some(s) = self.seats[seat].ship.filter(|&s| self.game.get(s).is_some()) {
            self.game.obj_mut(s).control = Control::Ai;
        }
        let name = self.seats[seat].name.clone();
        self.seats[0].disp.push(ui::Line::new(
            format!("{RED}{name} has left; their ship reverts to its AI captain.{RESET}"),
            50,
        ));
        out.push(self.frame(0, &[]));
        // a waiting turn may now be complete
        self.pump(&mut out);
        out
    }

    /// A remote seat reconnected: control returns, fresh frame.
    pub fn reconnect(&mut self, seat: usize) -> Vec<HostOut> {
        let mut out = Vec::new();
        if self.ended || seat == 0 || seat >= self.seats.len() {
            return out;
        }
        self.seats[seat].connected = true;
        if let Some(s) = self.seats[seat].ship.filter(|&s| self.game.get(s).is_some()) {
            let name = self.seats[seat].name.clone();
            self.game.obj_mut(s).control = Control::Remote(name);
        }
        out.push(self.frame(seat, &[]));
        out
    }

    fn frame(&self, seat: usize, flashes: &[begin_core::events::Flash]) -> HostOut {
        let s = &self.seats[seat];
        let prompt = if flashes.is_empty() {
            prompt_of(&s.pending, &s.name).to_string()
        } else {
            s.name.clone()
        };
        let data = ui::render(
            &self.game,
            s.ship.filter(|&sh| self.game.get(sh).is_some()),
            &s.disp,
            &prompt,
            flashes,
        );
        HostOut::Seat { seat, msg: Msg::Frame { data, flash: !flashes.is_empty() } }
    }

    /// Pump queued lines into submissions; run cycles as turns complete.
    fn pump(&mut self, out: &mut Vec<HostOut>) {
        loop {
            for k in 0..self.seats.len() {
                while !self.seats[k].submitted && !self.quit {
                    let Some(line) = self.seats[k].queue.pop_front() else { break };
                    self.process(k, &line, out);
                }
                if self.quit {
                    break;
                }
            }
            if self.quit {
                self.finish(out, &[]);
                return;
            }
            let everyone = self.seats[0].submitted
                && self.seats[1..]
                    .iter()
                    .all(|s| s.submitted || !s.connected || s.ship.is_none());
            if !everyone {
                return;
            }
            self.cycle(out);
            if self.ended {
                return;
            }
        }
    }

    fn process(&mut self, k: usize, line: &str, out: &mut Vec<HostOut>) {
        let Some(input) = resolve_pending(&mut self.seats[k].pending, line) else {
            self.seats[k].disp.push(ui::Line::plain("(cancelled)"));
            out.push(self.frame(k, &[]));
            return;
        };
        let ship = self.seats[k].ship.filter(|&s| self.game.get(s).is_some());
        match ship {
            Some(s) => {
                let outcome = commands::execute(&mut self.game, s, &mut self.seats[k].disp, &input);
                match outcome {
                    Outcome::Quit => {
                        if k == 0 {
                            // the host quitting ends the game for everyone
                            self.quit = true;
                        } else {
                            if let Some(o) = self.game.get_mut(s) {
                                o.control = Control::Ai;
                            }
                            self.seats[k].ship = None;
                        }
                    }
                    Outcome::Stay => out.push(self.frame(k, &[])),
                    Outcome::Ask { question, prefix, default } => {
                        self.seats[k].pending = Some((question, prefix, default));
                        out.push(self.frame(k, &[]));
                    }
                    Outcome::Advance => {
                        self.seats[k].submitted = true;
                        out.push(self.frame(k, &[]));
                    }
                    Outcome::Flagship(nid) => {
                        self.seats[k].ship = Some(nid);
                        self.seats[k].submitted = true;
                        out.push(self.frame(k, &[]));
                    }
                }
            }
            None => {
                if k == 0 {
                    self.quit = true;
                }
                // a shipless remote seat's lines are ignored
            }
        }
    }

    fn cycle(&mut self, out: &mut Vec<HostOut>) {
        self.game.run_cycle();
        crate::fighters::absorb_docked_fighters(&mut self.game);
        let reports = self.game.reporter.take();
        let flashes = self.game.take_flashes();
        for k in 0..self.seats.len() {
            self.seats[k].submitted = false;
            let ship = self.seats[k].ship.filter(|&s| self.game.get(s).is_some());
            match ship {
                Some(s) => {
                    let side = self.game.obj(s).nation;
                    for r in &reports {
                        if r.visible_to(side) {
                            self.seats[k].disp.push(ui::report_line(r));
                        }
                    }
                    push_chart(&self.game, Some(s), &mut self.seats[k].disp);
                }
                None if self.seats[k].ship.is_some() => {
                    self.seats[k].disp.push(ui::Line::new(
                        format!("{}Your ship has been destroyed.{RESET}", ui::RBLINK),
                        30,
                    ));
                    if k != 0 {
                        self.seats[k].ship = None;
                    }
                }
                None => {}
            }
        }
        let my_alive =
            self.seats[0].ship.map(|s| self.game.get(s).is_some()).unwrap_or(false);

        // brief weapon-flash frames first (remote clients pause on
        // flash:true; the local driver pauses on the Hold)
        if !flashes.is_empty() {
            for k in 1..self.seats.len() {
                if self.seats[k].connected && self.seats[k].disp.flash && self.seats[k].ship.is_some()
                {
                    out.push(self.frame(k, &flashes));
                }
            }
            if self.seats[0].disp.flash && my_alive {
                out.push(self.frame(0, &flashes));
                out.push(HostOut::Hold(140));
            }
        }

        // when the game just ended, keep the blast asterisks flashing on the
        // final frame everyone stares at before the evaluations land
        let ending = self.game.over.is_some() || !my_alive;
        for k in 1..self.seats.len() {
            if self.seats[k].connected {
                let fx: &[begin_core::events::Flash] =
                    if ending && self.seats[k].disp.flash && !flashes.is_empty() {
                        &flashes
                    } else {
                        &[]
                    };
                out.push(self.frame(k, fx));
            }
        }
        let final_fx: &[begin_core::events::Flash] =
            if ending && self.seats[0].disp.flash { &flashes } else { &[] };
        out.push(self.frame(0, final_fx));
        if ending {
            out.push(HostOut::Hold(4000));
            self.finish(out, &flashes);
        }
    }

    /// Evaluations for every connected seat, then Ended.
    fn finish(&mut self, out: &mut Vec<HostOut>, _flashes: &[begin_core::events::Flash]) {
        self.ended = true;
        let roster: Vec<(usize, String)> = self.game.survivors_roster();
        for k in 1..self.seats.len() {
            if !self.seats[k].connected {
                continue;
            }
            let side = self.seats[k]
                .ship
                .and_then(|s| self.game.get(s).map(|o| o.nation))
                .unwrap_or(self.host_side);
            let lost = self.seats[k].ship.map_or(true, |sid| self.game.get(sid).is_none());
            let (_t, score, text) = self.game.evaluation(side, lost);
            let n = &self.game.data.nations[side];
            let mut labeled = format!(
                "{} \u{2014} {}\r\n{text}\r\nPerformance Evaluation: {score:.2}",
                n.name, n.command
            );
            for (_nat, line) in &roster {
                labeled.push_str("\r\n    ");
                labeled.push_str(line);
            }
            out.push(HostOut::Seat { seat: k, msg: Msg::Over(labeled) });
        }
        let host_lost = self.seats[0].ship.map_or(true, |sid| self.game.get(sid).is_none());
        let (_t, score, text) = self.game.evaluation(self.host_side, host_lost);
        let n = &self.game.data.nations[self.host_side];
        let mut mine = format!(
            "{WHITE}{} \u{2014} {}{RESET}\r\n{GREEN}{text}{RESET}\r\n{GREEN}Performance Evaluation: {score:.2}{RESET}",
            n.name, n.command
        );
        for (nat, line) in &roster {
            let color = if *nat == self.host_side { GREEN } else { RED };
            mine.push_str(&format!("\r\n    {color}{line}{RESET}"));
        }
        out.push(HostOut::Seat { seat: 0, msg: Msg::Over(mine) });
        out.push(HostOut::Ended);
    }
}

fn push_chart(game: &Game, ship: Option<ObjId>, disp: &mut Display) {
    if let Some(s) = ship.filter(|&s| game.get(s).is_some()) {
        disp.push(ui::Line::plain(""));
        let mut chart = Vec::new();
        ui::chart_lines(game, s, &mut chart);
        for l in chart {
            disp.push(l);
        }
    }
}
