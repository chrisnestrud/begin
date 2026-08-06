//! A resumable single-player session: command lines in, ANSI output out.
//! Both the native console (`main.rs` pumps stdin into it) and the browser
//! build (`begin-web` feeds it from xterm.js) drive this same state
//! machine, so the two front-ends play identically by construction. The
//! host path drives the same [`Setup`] machine over stdin to build its
//! `Scenario`.

use crate::commands::{self, Outcome};
use crate::fighters;
use crate::ui::{self, Display, BGREEN, CYAN, GREEN, GREY, RED, RESET, WHITE};
use begin_core::scenario::{spawn_fleets, FleetEntry, Scenario, SideConfig};
use begin_core::{Game, GameData, ObjId, Tuning};

/// One unit of session output. The driver decides how to realize timing:
/// the console sleeps on `Hold`, the browser uses a timeout.
#[derive(Debug, Clone)]
pub enum Out {
    /// A full ANSI screen frame (starts with a clear). `flash` frames are
    /// held ~140 ms; the settled frame follows immediately after.
    Frame { ansi: String, flash: bool },
    /// A scrolling line (setup phase and endgame): print + newline.
    Text(String),
    /// A prompt to display without a trailing newline (setup phase; during
    /// play the prompt is part of the frame).
    Prompt(String),
    /// Pause for the given milliseconds before emitting what follows.
    Hold(u32),
    /// The session is over; no further input will be accepted.
    Done,
}

/// The startup banner (the console prints it before its game-type menu;
/// the browser shows it at the top of the terminal).
pub fn banner() -> Vec<Out> {
    vec![
        Out::Text(format!("{WHITE}BEGIN - A Tactical Starship Simulation{RESET}")),
        Out::Text(format!("{GREY}Rust port of Begin 2.00 (c) 1984-1991 Clockwork Software{RESET}")),
        Out::Text(String::new()),
    ]
}

/// Options that mirror the relevant command-line flags.
#[derive(Clone)]
pub struct SessionOpts {
    pub tuning: Tuning,
    pub seed: u64,
    pub epoch_jd: f64,
    pub spawn_body: Option<String>,
    pub flash: bool,
    /// Skip setup: the classic --quick duel as "Admiral".
    pub quick: bool,
}

// ===== the setup machine (name → nations → fleet config) =====

pub enum SetupResult {
    Quit,
    Go(Scenario, String),
}

enum SetupPhase {
    Name,
    AllyNation,
    EnemyNation,
    Fleet,
}

pub struct Setup {
    phase: SetupPhase,
    seed: u64,
    epoch_jd: f64,
    spawn_body: Option<String>,
    name: String,
    ally_nation: String,
    enemy_nation: String,
    ally: SideConfig,
    enemy: SideConfig,
}

fn empty_side() -> SideConfig {
    SideConfig { nation: String::new(), ships: Vec::new(), flagship: None }
}

impl Setup {
    pub fn new(seed: u64, epoch_jd: f64, spawn_body: Option<String>) -> (Setup, Vec<Out>) {
        let s = Setup {
            phase: SetupPhase::Name,
            seed,
            epoch_jd,
            spawn_body,
            name: String::new(),
            ally_nation: String::new(),
            enemy_nation: String::new(),
            ally: empty_side(),
            enemy: empty_side(),
        };
        let out = vec![Out::Prompt(format!("{CYAN}What is your name, commander?{RESET} "))];
        (s, out)
    }

    fn nation_prompt(&self, data: &GameData, q: &str) -> Out {
        let nations: Vec<String> = data.nations.iter().map(|n| n.adjective.clone()).collect();
        Out::Prompt(format!("{CYAN}{q} ({}){RESET} ", nations.join(", ")))
    }

    fn setup_prompt(&self) -> Out {
        Out::Prompt(format!("{CYAN}{} (setup)>{RESET} ", self.name))
    }

    /// Feed one line; `Some(result)` ends the setup.
    pub fn feed(&mut self, data: &GameData, line: &str) -> (Vec<Out>, Option<SetupResult>) {
        let line = line.trim().to_string();
        let mut out = Vec::new();
        match self.phase {
            SetupPhase::Name => {
                if line.is_empty() {
                    out.push(Out::Prompt(format!(
                        "{CYAN}What is your name, commander?{RESET} "
                    )));
                } else {
                    self.name = line;
                    self.phase = SetupPhase::AllyNation;
                    out.push(self.nation_prompt(data, "Your nation?"));
                }
            }
            SetupPhase::AllyNation => match data.nation(&line) {
                Some(n) => {
                    self.ally_nation = n.adjective.clone();
                    self.phase = SetupPhase::EnemyNation;
                    out.push(self.nation_prompt(data, "Enemy nation?"));
                }
                None => {
                    out.push(Out::Text(format!("{RED}Unknown nation.{RESET}")));
                    out.push(self.nation_prompt(data, "Your nation?"));
                }
            },
            SetupPhase::EnemyNation => match data.nation(&line) {
                Some(n) if n.adjective != self.ally_nation => {
                    self.enemy_nation = n.adjective.clone();
                    self.ally.nation = self.ally_nation.clone();
                    self.enemy.nation = self.enemy_nation.clone();
                    self.phase = SetupPhase::Fleet;
                    self.fleet_banner(data, &mut out);
                    out.push(self.setup_prompt());
                }
                Some(_) => {
                    out.push(Out::Text(format!(
                        "{RED}A civil war?  Pick someone else.{RESET}"
                    )));
                    out.push(self.nation_prompt(data, "Enemy nation?"));
                }
                None => {
                    out.push(Out::Text(format!("{RED}Unknown nation.{RESET}")));
                    out.push(self.nation_prompt(data, "Enemy nation?"));
                }
            },
            SetupPhase::Fleet => return self.fleet_line(data, &line),
        }
        (out, None)
    }

    fn fleet_banner(&self, data: &GameData, out: &mut Vec<Out>) {
        out.push(Out::Text(String::new()));
        out.push(Out::Text(format!(
            "{WHITE}FLEET SETUP{RESET}  (up to {} ships per side)",
            begin_core::scenario::MAX_FLEET
        )));
        out.push(Out::Text(format!(
            "{GREY}Commands: ally <n> <class> [flagship] | enemy <n> <class> | flagship <class>{RESET}"
        )));
        out.push(Out::Text(format!(
            "{GREY}          begin | random (spread out, sensors dark) | quit{RESET}"
        )));
        let ally_classes: Vec<String> =
            data.classes_of(&self.ally_nation).iter().map(|c| c.name.clone()).collect();
        let enemy_classes: Vec<String> =
            data.classes_of(&self.enemy_nation).iter().map(|c| c.name.clone()).collect();
        out.push(Out::Text(format!(
            "{BGREEN}{}{RESET}: {}",
            self.ally_nation,
            ally_classes.join(", ")
        )));
        out.push(Out::Text(format!(
            "{RED}{}{RESET}: {}",
            self.enemy_nation,
            enemy_classes.join(", ")
        )));
    }

    fn fleet_line(&mut self, data: &GameData, line: &str) -> (Vec<Out>, Option<SetupResult>) {
        let mut out = Vec::new();
        let lower = line.to_ascii_lowercase();
        let w: Vec<&str> = lower.split_whitespace().collect();
        if w.is_empty() {
            out.push(self.setup_prompt());
            return (out, None);
        }
        match w[0] {
            "begin" | "random" => {
                if self.ally.ships.is_empty() || self.enemy.ships.is_empty() {
                    out.push(Out::Text(format!(
                        "{RED}Both fleets need at least one ship.{RESET}"
                    )));
                    out.push(self.setup_prompt());
                    return (out, None);
                }
                let mut ally = std::mem::replace(&mut self.ally, empty_side());
                let enemy = std::mem::replace(&mut self.enemy, empty_side());
                if ally.flagship.is_none() {
                    ally.flagship = ally.ships.first().map(|e| e.class.clone());
                }
                let sc = Scenario {
                    ally,
                    enemy,
                    stations: Vec::new(),
                    random_placement: w[0] == "random",
                    seed: self.seed,
                    epoch_jd: self.epoch_jd,
                    spawn_body: self.spawn_body.clone(),
                };
                return (out, Some(SetupResult::Go(sc, self.name.clone())));
            }
            "quit" | "exit" => return (out, Some(SetupResult::Quit)),
            "ally" | "enemy" | "config" | "configure" => {
                let mut idx = 1;
                let is_ally = if w[0] == "config" || w[0] == "configure" {
                    let side = w.get(1).copied().unwrap_or("ally");
                    idx = 2;
                    side == "ally"
                } else {
                    w[0] == "ally"
                };
                let nation =
                    if is_ally { self.ally_nation.clone() } else { self.enemy_nation.clone() };
                let mut adds: Vec<(String, usize, bool)> = Vec::new();
                let mut k = idx;
                let mut added = false;
                while k < w.len() {
                    let Ok(n) = w[k].parse::<usize>() else { break };
                    k += 1;
                    let mut class_words = Vec::new();
                    let mut flag = false;
                    while k < w.len() && w[k].parse::<usize>().is_err() {
                        if w[k] == "flagship" {
                            flag = true;
                        } else {
                            class_words.push(w[k]);
                        }
                        k += 1;
                    }
                    let class = class_words.join(" ");
                    match data.find_class(&nation, &class) {
                        Some(d) => {
                            adds.push((d.name.clone(), n.clamp(1, 9), flag));
                            added = true;
                        }
                        None => out.push(Out::Text(format!(
                            "{RED}No {nation} class '{class}'.{RESET}"
                        ))),
                    }
                }
                let side_cfg = if is_ally { &mut self.ally } else { &mut self.enemy };
                for (cname, n, flag) in adds {
                    side_cfg.ships.push(FleetEntry { class: cname.clone(), count: n });
                    if flag && is_ally {
                        side_cfg.flagship = Some(cname.clone());
                    }
                    out.push(Out::Text(format!(
                        "{GREEN}{} {} x{}{}{RESET}",
                        nation,
                        cname,
                        n,
                        if flag { " (flagship)" } else { "" }
                    )));
                }
                if !added {
                    out.push(Out::Text(format!(
                        "{GREY}e.g.  ally 2 heavy cruisers flagship{RESET}"
                    )));
                }
                out.push(self.setup_prompt());
            }
            "flagship" => {
                let class = w[1..].join(" ");
                match data.find_class(&self.ally_nation, &class) {
                    Some(d) => {
                        let name = d.name.clone();
                        self.ally.flagship = Some(name.clone());
                        out.push(Out::Text(format!("{GREEN}Flagship: {name}{RESET}")));
                    }
                    None => out.push(Out::Text(format!("{RED}No such class.{RESET}"))),
                }
                out.push(self.setup_prompt());
            }
            _ => {
                out.push(Out::Text(format!(
                    "{RED}Setup commands: ally/enemy/flagship/begin/random/quit{RESET}"
                )));
                out.push(self.setup_prompt());
            }
        }
        (out, None)
    }
}

/// Drive the setup machine over stdin (console single-player and host).
pub fn setup_over_stdin(
    data: &GameData,
    seed: u64,
    epoch_jd: f64,
    spawn_body: Option<String>,
    lines: &mut std::io::Lines<std::io::StdinLock>,
) -> Option<(Scenario, String)> {
    let (mut setup, outs) = Setup::new(seed, epoch_jd, spawn_body);
    emit_setup(&outs);
    loop {
        let line = lines.next()?.ok()?;
        let (outs, result) = setup.feed(data, &line);
        emit_setup(&outs);
        match result {
            Some(SetupResult::Go(sc, name)) => return Some((sc, name)),
            Some(SetupResult::Quit) => return None,
            None => {}
        }
    }

    fn emit_setup(outs: &[Out]) {
        use std::io::Write;
        for o in outs {
            match o {
                Out::Text(t) => println!("{t}"),
                Out::Prompt(p) => {
                    print!("{p}");
                    std::io::stdout().flush().ok();
                }
                _ => {}
            }
        }
    }
}

// ===== the play session =====

enum Phase {
    Setup(Setup),
    Playing,
    Finished,
}

pub struct Session {
    data: Option<GameData>, // taken when the game starts
    opts: SessionOpts,
    phase: Phase,
    name: String,
    game: Option<Game>,
    me: Option<ObjId>,
    side: usize,
    disp: Display,
    pending: Option<(String, String, String)>,
}

impl Session {
    /// Create the session; the returned outputs are the opening prompts,
    /// or (in quick mode) the first frame.
    pub fn new(data: GameData, opts: SessionOpts) -> (Session, Vec<Out>) {
        let quick = opts.quick;
        let (setup, mut out) =
            Setup::new(opts.seed, opts.epoch_jd, opts.spawn_body.clone());
        let mut s = Session {
            data: Some(data),
            opts,
            phase: Phase::Setup(setup),
            name: String::new(),
            game: None,
            me: None,
            side: 0,
            disp: Display::default(),
            pending: None,
        };
        if quick {
            out.clear();
            let mut sc = Scenario::duel();
            sc.epoch_jd = s.opts.epoch_jd;
            sc.spawn_body = s.opts.spawn_body.clone();
            sc.seed = s.opts.seed;
            s.name = "Admiral".into();
            s.start_game(sc, &mut out);
        }
        (s, out)
    }

    pub fn finished(&self) -> bool {
        matches!(self.phase, Phase::Finished)
    }

    /// Feed one input line; returns everything to display.
    pub fn feed(&mut self, line: &str) -> Vec<Out> {
        match &mut self.phase {
            Phase::Setup(setup) => {
                let data = self.data.as_ref().unwrap();
                let (mut out, result) = setup.feed(data, line);
                match result {
                    Some(SetupResult::Go(sc, name)) => {
                        self.name = name;
                        self.start_game(sc, &mut out);
                    }
                    Some(SetupResult::Quit) => {
                        self.phase = Phase::Finished;
                        out.push(Out::Done);
                    }
                    None => {}
                }
                out
            }
            Phase::Playing => {
                let mut out = Vec::new();
                self.play_line(line, &mut out);
                out
            }
            Phase::Finished => Vec::new(),
        }
    }

    /// Spawn the fleets and enter play (mirrors the pre-loop part of run()).
    fn start_game(&mut self, sc: Scenario, out: &mut Vec<Out>) {
        let data = self.data.take().unwrap();
        let mut game = Game::new(data, self.opts.tuning.clone(), self.opts.seed);
        begin_core::env::setup(&mut game, sc.epoch_jd, sc.spawn_body.as_deref(), sc.seed);
        let fleets = match spawn_fleets(&mut game, &sc) {
            Ok(f) => f,
            Err(e) => {
                out.push(Out::Text(format!("{RED}{e}{RESET}")));
                self.phase = Phase::Finished;
                out.push(Out::Done);
                return;
            }
        };
        let me = fleets.flagship.expect("ally flagship");
        let side = game.obj(me).nation;
        // Spock's pregame estimate (1.65 fmt at file 0x244AC)
        {
            let (odds, against) = game.pregame_odds(side);
            let officer =
                game.data.nations[side].officers.first().cloned().unwrap_or_default();
            let mut num = format!("{odds:.2}");
            while num.ends_with('0') {
                num.pop();
            }
            let num = num.replace('.', " point ").trim_end_matches(" point ").to_string();
            game.say(
                Some(side),
                &officer,
                format!(
                    "I estimate our chance of defeating the enemy to be {num} to 1{}.",
                    if against { " against" } else { "" }
                ),
                begin_core::events::ReportKind::Ally,
            );
        }
        self.disp = Display::default();
        self.disp.flash = self.opts.flash;
        self.disp.push_plain("");
        let mut chart = Vec::new();
        ui::chart_lines(&game, me, &mut chart);
        for l in chart {
            self.disp.push(l);
        }
        self.game = Some(game);
        self.me = Some(me);
        self.side = side;
        self.phase = Phase::Playing;
        out.push(self.frame(&[]));
    }

    fn frame(&self, flashes: &[begin_core::events::Flash]) -> Out {
        let game = self.game.as_ref().unwrap();
        let me = self.me.unwrap();
        let prompt =
            self.pending.as_ref().map(|(q, _, _)| q.as_str()).unwrap_or(&self.name);
        let viewer = game.get(me).map(|_| me);
        Out::Frame {
            ansi: ui::render(game, viewer, &self.disp, prompt, flashes),
            flash: !flashes.is_empty(),
        }
    }

    /// One line of play (mirrors the main loop body of run()).
    fn play_line(&mut self, line: &str, out: &mut Vec<Out>) {
        let input = match self.pending.take() {
            Some((_, prefix, default)) => {
                let ans = line.trim();
                let ans = if ans.is_empty() { default } else { ans.to_string() };
                if ans.is_empty() {
                    self.disp.push(ui::Line::plain("(cancelled)"));
                    out.push(self.frame(&[]));
                    return;
                }
                format!("{prefix} {ans}")
            }
            None => line.trim().to_string(),
        };
        let (mut me, side) = (self.me.unwrap(), self.side);
        let game = self.game.as_mut().unwrap();
        let outcome = if game.get(me).is_some() && game.over.is_none() {
            commands::execute(game, me, &mut self.disp, &input)
        } else {
            Outcome::Quit
        };
        match outcome {
            Outcome::Quit => {
                self.endgame(out);
                return;
            }
            Outcome::Stay => {
                out.push(self.frame(&[]));
            }
            Outcome::Ask { question, prefix, default } => {
                self.pending = Some((question, prefix, default));
                out.push(self.frame(&[]));
            }
            Outcome::Flagship(new_me) => {
                me = new_me;
                self.me = Some(me);
                let game = self.game.as_mut().unwrap();
                game.run_cycle();
                fighters::absorb_docked_fighters(game);
                for r in game.reporter.take() {
                    if r.visible_to(side) {
                        self.disp.push(ui::report_line(&r));
                    }
                }
                let mut chart = Vec::new();
                ui::chart_lines(game, me, &mut chart);
                self.disp.push_plain("");
                for l in chart {
                    self.disp.push(l);
                }
                out.push(self.frame(&[]));
            }
            Outcome::Advance => {
                let game = self.game.as_mut().unwrap();
                game.run_cycle();
                fighters::absorb_docked_fighters(game);
                for r in game.reporter.take() {
                    if r.visible_to(side) {
                        self.disp.push(ui::report_line(&r));
                    }
                }
                let flashes = game.take_flashes();
                let alive = game.get(me).is_some();
                if alive {
                    let mut chart = Vec::new();
                    ui::chart_lines(game, me, &mut chart);
                    self.disp.push_plain("");
                    for l in chart {
                        self.disp.push(l);
                    }
                } else {
                    self.disp.push(ui::Line::new(
                        format!("{}Your ship has been destroyed.{RESET}", ui::RBLINK),
                        30,
                    ));
                }
                let over = self.game.as_ref().unwrap().over.is_some();
                if !alive || over {
                    // final events with the blast asterisks flashing, held
                    // 4 s before the evaluation
                    let fx: Vec<begin_core::events::Flash> =
                        if self.disp.flash { flashes } else { Vec::new() };
                    out.push(self.frame(&fx));
                    out.push(Out::Hold(4000));
                    self.endgame(out);
                    return;
                }
                // brief weapon-flash frame between cycles
                if self.disp.flash && !flashes.is_empty() {
                    out.push(self.frame(&flashes));
                    out.push(Out::Hold(140));
                }
                out.push(self.frame(&[]));
            }
        }
    }

    /// The evaluation block (mirrors the tail of run()).
    fn endgame(&mut self, out: &mut Vec<Out>) {
        let game = self.game.as_ref().unwrap();
        let me = self.me.unwrap();
        let side = self.side;
        out.push(Out::Text("\r\n".into()));
        let player_lost = game.get(me).is_none();
        let (_tier, score, text) = game.evaluation(side, player_lost);
        let nation = &game.data.nations[side];
        out.push(Out::Text(format!(
            "{WHITE}{} \u{2014} {}{RESET}",
            nation.name, nation.command
        )));
        out.push(Out::Text(format!("{GREEN}{text}{RESET}")));
        out.push(Out::Text(String::new()));
        out.push(Out::Text(format!(
            "{GREEN}Performance Evaluation: {score:.2}{RESET}"
        )));
        out.push(Out::Text(String::new()));
        out.push(Out::Text(format!("{GREEN}Survivors Reported:{RESET}")));
        for (nat, line) in game.survivors_roster() {
            let color = if nat == side { GREEN } else { RED };
            out.push(Out::Text(format!("    {color}{line}{RESET}")));
        }
        self.phase = Phase::Finished;
        out.push(Out::Done);
    }
}
