//! Real parser/core checks for V165 no-ready-torpedo turn consumption.
use begin::commands::{execute, Outcome};
use begin::ui::Display;
use begin_core::math::Vec3;
use begin_core::orders::{self, Mounts};
use begin_core::{Control, Game, GameData, ObjId, Tuning};
use serde_json::json;
fn fixture(tuning: Tuning, loaded: bool) -> (Game, ObjId, ObjId) {
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
    if loaded {
        orders::load_tubes(&mut g, me, &Mounts::All, Some(200.0));
        for _ in 0..12 {
            if g.obj(me).ship.as_ref().unwrap().loaded_tubes() == 6 {
                break;
            }
            g.run_cycle();
        }
        assert_eq!(g.obj(me).ship.as_ref().unwrap().loaded_tubes(), 6);
        assert!(g.over.is_none());
    }
    (g, me, enemy)
}

#[test]
fn v165_no_ready_order_advances_once_without_arming_or_spawning_a_shot() {
    for input in [
        "fire all torps",
        "fire all torpedoes",
        "fire all torpedos",
        "fire torps 1",
    ] {
        let (mut g, me, _) = fixture(Tuning::begin1(), false);
        g.obj_mut(me).warp = 1.0;
        g.obj_mut(me).desired_warp = 1.0;
        let before = g.obj(me).pos;
        let mut display = Display::default();
        let out = execute(&mut g, me, &mut display, input);
        assert!(display
            .scroll
            .iter()
            .any(|l| l.text.contains("No loaded tubes ready to fire.")));
        assert!(g
            .obj(me)
            .ship
            .as_ref()
            .unwrap()
            .tubes
            .iter()
            .all(|t| !t.fire));
        assert!(g.torp_ids().is_empty());
        assert!(
            matches!(out, Outcome::Advance),
            "recognized V165 no-ready order must consume one turn: {input}"
        );
        g.run_cycle();
        assert_eq!(g.cycle, 1);
        assert_ne!(g.obj(me).pos, before);
        assert!(g.torp_ids().is_empty());
        println!(
            "{}",
            json!({"case":"V165_no_ready", "input":input,"cycle":g.cycle,"torpedoes":0,"position_before":[before.x,before.y,before.z],"position_after":[g.obj(me).pos.x,g.obj(me).pos.y,g.obj(me).pos.z]})
        );
    }
}

#[test]
fn v200_no_ready_order_stays_and_does_not_arm_a_shot() {
    let (mut g, me, _) = fixture(Tuning::begin2(), false);
    let mut display = Display::default();
    assert!(matches!(
        execute(&mut g, me, &mut display, "fire all torps"),
        Outcome::Stay
    ));
    assert_eq!(g.cycle, 0);
    assert!(g
        .obj(me)
        .ship
        .as_ref()
        .unwrap()
        .tubes
        .iter()
        .all(|t| !t.fire));
    assert!(g.torp_ids().is_empty());
    assert!(display
        .scroll
        .iter()
        .any(|l| l.text.contains("No loaded tubes ready to fire.")));
}

#[test]
fn loaded_aliases_arm_six_tubes_and_consume_one_turn_in_both_models() {
    for tuning in [Tuning::begin1(), Tuning::begin2()] {
        for input in ["fire all torps", "fire all torpedoes", "fire all torpedos"] {
            let (mut g, me, _) = fixture(tuning.clone(), true);
            let before = g.cycle;
            let mut display = Display::default();
            assert!(matches!(
                execute(&mut g, me, &mut display, input),
                Outcome::Advance
            ));
            assert_eq!(
                g.obj(me)
                    .ship
                    .as_ref()
                    .unwrap()
                    .tubes
                    .iter()
                    .filter(|t| t.fire)
                    .count(),
                6
            );
            g.run_cycle();
            assert_eq!(g.cycle, before + 1);
            assert!(!g.torp_ids().is_empty());
        }
    }
}

#[test]
fn information_bad_commands_and_missing_arguments_keep_their_policy() {
    for tuning in [Tuning::begin1(), Tuning::begin2()] {
        let (mut g, me, _) = fixture(tuning, false);
        let mut display = Display::default();
        for input in [
            "report",
            "scan Nova",
            "status damage",
            "status tubes",
            "fire all nonsense",
            "frobnicate",
            "fire torps 999",
        ] {
            assert!(
                matches!(execute(&mut g, me, &mut display, input), Outcome::Stay),
                "{input}"
            );
        }
        assert!(matches!(
            execute(&mut g, me, &mut display, "fire"),
            Outcome::Ask { .. }
        ));
        assert!(matches!(
            execute(&mut g, me, &mut display, "load all tubes"),
            Outcome::Ask { .. }
        ));
        assert_eq!(g.cycle, 0);
        assert!(g
            .obj(me)
            .ship
            .as_ref()
            .unwrap()
            .tubes
            .iter()
            .all(|t| !t.fire));
    }
}
