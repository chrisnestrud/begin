//! Black-box completion-window checks; original duel observes detection + four world steps.
use begin::commands::{execute, Outcome};
use begin::session::{Out, Session, SessionOpts};
use begin::ui::Display;
use begin_core::math::Vec3;
use begin_core::{Control, Game, GameData, ObjId, Tuning};
fn fixture(tuning: Tuning) -> (Game, ObjId, ObjId) {
    let data = GameData::load();
    let a = data
        .ships
        .iter()
        .position(|d| d.name == "Heavy Cruiser" && d.nation == "Federation")
        .unwrap();
    let b = data
        .ships
        .iter()
        .position(|d| d.name == "Escort" && d.nation == "Klingon")
        .unwrap();
    let mut g = Game::new(data, tuning, 42);
    let me = g
        .spawn_ship(a, "Trenton".into(), Vec3::ZERO, 0.0, Control::Local)
        .unwrap();
    let enemy = g
        .spawn_ship(
            b,
            "Nova".into(),
            Vec3::new(0.0, 1e6, 0.0),
            180.0,
            Control::None,
        )
        .unwrap();
    g.record_start_strengths();
    (g, me, enemy)
}

fn session(tuning: Tuning) -> Session {
    let mut data = GameData::load();
    for d in &mut data.ships {
        if d.name == "Heavy Cruiser" && d.nation == "Federation" {
            d.ship_names = vec!["Trenton".into()];
        }
        if d.name == "Escort" && d.nation == "Klingon" {
            d.ship_names = vec!["Nova".into()];
        }
    }
    {
        let d = data
            .ships
            .iter_mut()
            .find(|d| d.name == "Escort" && d.nation == "Klingon")
            .unwrap();
        d.crew = 0;
    }
    let opts = SessionOpts {
        tuning,
        seed: 42,
        epoch_jd: 2451545.0,
        spawn_body: None,
        flash: false,
        quick: false,
    };
    let (mut s, _) = Session::new(data, opts);
    for input in [
        "Tranche",
        "Federation",
        "Klingon",
        "ally 1 heavy cruiser",
        "enemy 1 escort",
        "begin",
    ] {
        s.feed(input);
    }
    assert!(!s.finished());
    s
}

fn eliminate(g: &mut Game, enemy: ObjId) {
    g.obj_mut(enemy).ship.as_mut().unwrap().survivors = 0;
    g.obj_mut(enemy).warp = 4.0;
    g.obj_mut(enemy).desired_warp = 4.0;
}

#[test]
fn v165_completion_allows_four_more_world_steps_and_status_queries() {
    let (mut g, me, enemy) = fixture(Tuning::begin1());
    eliminate(&mut g, enemy);
    g.run_cycle(); // D0: first elimination detection.
    assert_eq!(g.cycle, 1);
    assert!(g.over.is_none(), "D0 must leave a playable pending window");
    let mut display = Display::default();
    let p = g.obj(enemy).pos;
    for input in ["report", "scan Nova", "status damage"] {
        assert!(matches!(
            execute(&mut g, me, &mut display, input),
            Outcome::Stay
        ));
        assert_eq!(g.cycle, 1);
        assert_eq!(g.obj(enemy).pos, p);
        assert!(g.over.is_none());
    }
    for step in 1..=4 {
        let before = g.obj(enemy).pos;
        assert!(matches!(
            execute(&mut g, me, &mut display, "helm 0 0"),
            Outcome::Advance
        ));
        g.run_cycle();
        assert_eq!(g.cycle, 1 + step);
        assert_ne!(g.obj(enemy).pos, before, "hulk must keep moving at D{step}");
        assert_eq!(g.over.is_some(), step == 4, "completion at D{step}");
    }
    let p = g.obj(enemy).pos;
    g.run_cycle();
    assert_eq!(g.cycle, 5);
    assert_eq!(g.obj(enemy).pos, p);
}

fn assert_no_final(out: &[Out]) {
    assert!(!out.iter().any(|o| matches!(o, Out::Hold(_) | Out::Done)));
    assert!(!out
        .iter()
        .any(|o| matches!(o, Out::Text(t) if t.contains("Performance Evaluation:"))));
}
fn assert_final(out: &[Out]) {
    assert_eq!(
        out.iter().filter(|o| matches!(o, Out::Hold(4000))).count(),
        1
    );
    assert_eq!(out.iter().filter(|o| matches!(o, Out::Done)).count(), 1);
    assert!(out
        .iter()
        .any(|o| matches!(o, Out::Text(t) if t.contains("Performance Evaluation:"))));
}

#[test]
fn v165_session_defers_evaluation_and_done_until_fourth_additional_order() {
    let mut s = session(Tuning::begin1());
    assert_no_final(&s.feed("helm 0 0"));
    assert!(!s.finished());
    for input in ["report", "scan Nova", "status damage"] {
        let out = s.feed(input);
        assert_no_final(&out);
        assert!(!s.finished());
        if input == "scan Nova" {
            assert!(out.iter().any(|o| matches!(o, Out::Frame {ansi, ..} if ansi.contains("Nova") && ansi.contains("SURVIVORS"))));
            assert!(!out
                .iter()
                .any(|o| matches!(o, Out::Frame {ansi, ..} if ansi.contains("No ship named"))));
        }
    }
    for _ in 0..3 {
        assert_no_final(&s.feed("helm 0 0"));
        assert!(!s.finished());
    }
    assert_final(&s.feed("helm 0 0"));
    assert!(s.finished());
    assert!(s.feed("report").is_empty());
}

#[test]
fn v200_still_ends_on_first_check_and_freezes() {
    let (mut g, _, enemy) = fixture(Tuning::begin2());
    eliminate(&mut g, enemy);
    g.run_cycle();
    assert!(g.over.is_some());
    let p = g.obj(enemy).pos;
    g.run_cycle();
    assert_eq!(g.cycle, 1);
    assert_eq!(g.obj(enemy).pos, p);
    let mut s = session(Tuning::begin2());
    assert_final(&s.feed("helm 0 0"));
    assert!(s.finished());
}

#[test]
fn ordinary_nonterminal_commands_keep_their_turn_policy() {
    for tuning in [Tuning::begin1(), Tuning::begin2()] {
        let (mut g, me, _) = fixture(tuning);
        let mut display = Display::default();
        for input in ["report", "scan Nova", "status damage", "fire all torps"] {
            assert!(matches!(
                execute(&mut g, me, &mut display, input),
                Outcome::Stay
            ));
        }
        assert!(matches!(
            execute(&mut g, me, &mut display, "load all tubes"),
            Outcome::Ask { .. }
        ));
        assert_eq!(g.cycle, 0);
        for _ in 0..5 {
            assert!(matches!(
                execute(&mut g, me, &mut display, "helm 0 0"),
                Outcome::Advance
            ));
            g.run_cycle();
            assert!(g.over.is_none());
        }
        assert_eq!(g.cycle, 5);
    }
}

#[test]
fn unvalidated_player_loss_and_multiside_keep_baseline_policy() {
    let (mut g, me, _) = fixture(Tuning::begin1());
    g.obj_mut(me).ship.as_mut().unwrap().survivors = 0;
    g.run_cycle();
    assert!(g.over.is_some());
    let (mut g, _, enemy) = fixture(Tuning::begin1());
    let design = g
        .data
        .ships
        .iter()
        .position(|d| d.nation == "Romulan")
        .unwrap();
    let third = g
        .spawn_ship(
            design,
            "Third".into(),
            Vec3::new(1e6, 0.0, 0.0),
            0.0,
            Control::None,
        )
        .unwrap();
    g.record_start_strengths();
    eliminate(&mut g, enemy);
    eliminate(&mut g, third);
    g.run_cycle();
    assert!(g.over.is_some());
}
