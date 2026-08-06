//! AI reflexes, fire control, maneuver and morale (§12.3, §12.5-12.7).

use crate::ai::{Ctx, Stance};
use crate::constants::*;
use crate::events::ReportKind;
use crate::game::Game;
use crate::math::*;
use crate::object::*;
use crate::orders::{self, Mounts};
use crate::systems::helm::{apparent_dist, relative_bearing, target_bearing_mark};

// AI-only tuning floats decoded from the asm (dseg twins of unrelated names).
const AI_PHASER_SPREAD: f64 = 10.0; // flt_35FC9 dseg 101715
const AI_FF_TOLERANCE: f64 = 5.0; // flt_35F6E dseg 101702 (twin of the destruct countdown)
const AI_TEMP_LATCH_FACTOR: f64 = 0.9; // dbl_35F44 dseg 101696 (twin of AI_FANATIC_RAM)

/// `sub_1D1F0` 55451: set course-mode helm; reports whether anything changed.
pub(crate) fn set_course_helm(g: &mut Game, id: ObjId, course: f64, warp: f64) -> bool {
    let course = norm360(course);
    let o = g.obj_mut(id);
    if o.helm == HelmMode::Course && o.desired_course == course && o.desired_warp == warp {
        return false;
    }
    o.helm = HelmMode::Course;
    o.desired_course = course;
    o.desired_warp = warp;
    true
}

/// `sub_1D258` 55574: set pursue-mode helm; reports whether anything changed.
pub(crate) fn set_pursue_helm(g: &mut Game, id: ObjId, target: ObjId, warp: f64) -> bool {
    let changed = {
        let o = g.obj(id);
        o.helm != HelmMode::Pursue
            || o.pursue != Some(target)
            || o.desired_warp != warp.clamp(MIN_WARP, 20.0)
    };
    orders::pursue(g, id, target, Some(warp), false);
    changed
}

// ===== §12.3 reflexes (`sub_20C86` 62632) =====

/// Returns true when a reflex acted (blocks the rest of the tree this cycle).
pub fn reflexes(g: &mut Game, ctx: &Ctx) -> bool {
    // docked ships skip everything except the shield re-raise
    if !g.obj(ctx.id).ship.as_ref().unwrap().docked() {
        if life_support_hold(g, ctx) {
            return true;
        }
        if overheat_slowdown(g, ctx) {
            return true;
        }
        if flee_detonation(g, ctx) {
            return true;
        }
        if snap_phasers(g, ctx) {
            return true;
        }
        if point_defense(g, ctx) {
            return true;
        }
        if shield_drop_when_starved(g, ctx) {
            return true;
        }
    }
    shield_reraise(g, ctx)
}

/// Head of `sub_20C86`: life support failing → hold course, warp 1 to save
/// the crew. Acts (blocks the tree) only when that changed the helm.
fn life_support_hold(g: &mut Game, ctx: &Ctx) -> bool {
    if g.obj(ctx.id).ship.as_ref().unwrap().life_failures <= 0 {
        return false;
    }
    let course = g.obj(ctx.id).desired_course;
    set_course_helm(g, ctx.id, course, 1.0)
}

/// `sub_1E59D`: asymmetric drive damage + temperature > 38 → cool at warp 1.
fn overheat_slowdown(g: &mut Game, ctx: &Ctx) -> bool {
    let (max_temp, max_dmg, min_dmg, any_damaged) = {
        let s = g.obj(ctx.id).ship.as_ref().unwrap();
        let mut mt: f64 = 0.0;
        let mut mx = 0;
        let mut mn = 100;
        let mut dam = false;
        for d in &s.drives {
            mt = mt.max(d.temp);
            mx = mx.max(d.sys.dmg);
            mn = mn.min(d.sys.dmg);
            dam |= d.sys.dmg > 0;
        }
        (mt, mx, mn, dam)
    };
    if !any_damaged || max_temp <= AI_OVERHEAT_REFLEX {
        return false;
    }
    let ratio = (100 - max_dmg) as f64 / (100 - min_dmg).max(1) as f64;
    if ratio < AI_DRIVE_RATIO {
        let o = g.obj_mut(ctx.id);
        o.desired_warp = 1.0;
        o.ship.as_mut().unwrap().brain.overheat_latch = true;
        return true;
    }
    false
}

/// Blast-radius estimate (`sub_1D92D` 56325).
fn blast_radius(g: &Game, id: ObjId) -> f64 {
    let o = g.obj(id);
    match o.kind {
        Kind::Ship => {
            let d = &g.data.ships[o.ship.as_ref().unwrap().design];
            (crate::systems::power::gross_warp_power(g, id) / g.tuning.oo_destruct_damage
                + d.destruct)
                * BLAST_RADIUS_SCALE
        }
        Kind::Probe => o.probe.as_ref().unwrap().damage * BLAST_RADIUS_SCALE,
        Kind::Torp => o.torp.as_ref().unwrap().damage * BLAST_RADIUS_SCALE,
    }
}

/// `sub_1E6AC`: run from anything about to blow whose blast covers me.
fn flee_detonation(g: &mut Game, ctx: &Ctx) -> bool {
    let my_pos = g.obj(ctx.id).pos;
    for id in g.ids() {
        if id == ctx.id {
            continue;
        }
        let ticking = match g.obj(id).kind {
            Kind::Ship => {
                let c = g.obj(id).ship.as_ref().unwrap().destruct_countdown;
                c > 0.0 && c <= DESTRUCT_COUNTDOWN
            }
            Kind::Probe => {
                let p = g.obj(id).probe.as_ref().unwrap();
                p.arm <= 0.0 && p.time <= DESTRUCT_COUNTDOWN
            }
            Kind::Torp => false,
        };
        if !ticking {
            continue;
        }
        let d = (g.obj(id).pos - my_pos).len();
        if d < blast_radius(g, id) {
            let (away, _) = target_bearing_mark(g, id, ctx.id, ctx.side);
            let warp = combat_speed(g, ctx);
            let o = g.obj_mut(ctx.id);
            o.helm = HelmMode::Course;
            o.desired_course = away; // bearing from the bomb to me = outward
            o.desired_warp = warp;
            return true;
        }
    }
    false
}

/// `sub_1E78A`: snap-fire exactly enough banks to break the facing shield of
/// the nearest enemy pointed at us.
fn snap_phasers(g: &mut Game, ctx: &Ctx) -> bool {
    if ctx.charged_banks == 0 {
        return false;
    }
    let Some((threat, dist)) = ctx.nearest_incoming else { return false };
    if dist >= ctx.bank_range || g.get(threat).is_none() {
        return false;
    }
    // does any charged bank bear? (`sub_1DCBC`)
    let bears = {
        let s = g.obj(ctx.id).ship.as_ref().unwrap();
        (0..s.banks.len()).any(|k| {
            let b = &s.banks[k];
            !b.sys.destroyed()
                && b.enabled
                && b.charge >= ctx.bank_charge_full
                && crate::systems::phasers::bank_bears(g, ctx.id, k, threat)
        })
    };
    // locking all banks makes them bear next resolution anyway; the original
    // required one bearing bank before the snap
    if !bears {
        return false;
    }
    // facing shield the threat presents to me
    let face = relative_bearing(g, threat, ctx.id, ctx.side);
    let shield_idx = {
        let ts = g.obj(threat).ship.as_ref().unwrap();
        crate::systems::shields::facing_shield(ts, face)
    };
    let (n, spread) = match shield_idx {
        Some(si) if crate::systems::shields::shield_eu(g, threat, si) > 0.0 => {
            let shield_eu = crate::systems::shields::shield_eu(g, threat, si);
            let angle_off = {
                let (tb, _) = target_bearing_mark(g, ctx.id, threat, ctx.side);
                ang_dist(tb, g.obj(ctx.id).course)
            };
            let spread = (angle_off * 2.0).clamp(SPREAD_MIN, SPREAD_DEFAULT);
            // expected damage per bank (`sub_1DDB8`)
            let expected = ctx.bank_charge_full
                * (1.0 - dist / ctx.bank_range).max(0.0).sqrt()
                * (g.tuning.phaser_dam_mult / spread)
                * PHASER_HALF;
            if expected <= 0.0 {
                return false;
            }
            // sub_1E78A: floor, then 1 when that rounds to zero (Xfloor)
            let n = (shield_eu / expected).floor() as usize;
            let n = if n == 0 { 1 } else { n.min(ctx.charged_banks) };
            (n, spread)
        }
        _ => (1, SPREAD_DEFAULT),
    };
    orders::lock_banks(g, ctx.id, &Mounts::All, threat);
    let fired = orders::fire_n_charged_banks(g, ctx.id, n, spread);
    fired > 0
}

/// `sub_1E8B8`/`sub_20697`: phaser point defense against incoming probes.
fn point_defense(g: &mut Game, ctx: &Ctx) -> bool {
    // skipped when a ship target is in phaser range and we have any phaser
    // ratio at all (sub_1E8B8: dominance > 0, not >= 1)
    if let Some(t) = ctx.target.as_ref() {
        if t.dist < ctx.bank_range && t.phaser_dominance > 0.0 {
            return false;
        }
    }
    if ctx.op_banks == 0 {
        return false;
    }
    let my_pos = g.obj(ctx.id).pos;
    let my_warp = g.obj(ctx.id).warp.abs();
    // nearest hostile probe inside the defense window
    let mut nearest: Option<(ObjId, f64)> = None;
    for p in g.probe_ids() {
        let o = g.obj(p);
        if o.nation == ctx.side || o.det != Det::None {
            continue;
        }
        let d = (o.pos - my_pos).len();
        let window = (my_warp + o.warp.abs()) * 100.0 * AI_PD_WINDOW + ctx.bank_range;
        if d < window && nearest.map(|(_, nd)| d < nd).unwrap_or(true) {
            nearest = Some((p, d));
        }
    }
    let Some((probe, dist)) = nearest else { return false };
    // find a charged bank that bears
    let bearing_bank = {
        let s = g.obj(ctx.id).ship.as_ref().unwrap();
        (0..s.banks.len()).find(|&k| {
            let b = &s.banks[k];
            !b.sys.destroyed()
                && b.enabled
                && b.charge >= ctx.bank_charge_full
                && crate::systems::phasers::bank_bears(g, ctx.id, k, probe)
        })
    };
    match bearing_bank {
        None => {
            // rotate the first charged bank onto the threat (`sub_AF38`)
            let mark = relative_bearing(g, ctx.id, probe, ctx.side);
            let bank = {
                let s = g.obj(ctx.id).ship.as_ref().unwrap();
                (0..s.banks.len()).find(|&k| {
                    let b = &s.banks[k];
                    !b.sys.destroyed() && b.enabled && b.charge >= ctx.bank_charge_full
                })
            };
            if let Some(k) = bank {
                orders::turn_banks(g, ctx.id, &Mounts::List(vec![k + 1]), mark);
                return true;
            }
            false
        }
        Some(k) if dist >= ctx.bank_range => {
            // let a chasing probe close: slow down two factors (min 1);
            // sub_20697 returns 0 here so the rest of the tree still runs
            let o = g.obj_mut(ctx.id);
            o.desired_warp = (o.desired_warp - AI_PURSUE_WARP_BONUS).max(1.0);
            let _ = k;
            false
        }
        Some(k) => {
            let b = &mut g.obj_mut(ctx.id).ship.as_mut().unwrap().banks[k];
            b.fire = true;
            b.spread = SPREAD_DEFAULT;
            true
        }
    }
}

/// `sub_1E9BB` 58292: power-starved → drop the weakest live shield to shed
/// load. Blocks the tree when taken.
fn shield_drop_when_starved(g: &mut Game, ctx: &Ctx) -> bool {
    if !g.obj(ctx.id).ship.as_ref().unwrap().brain.shields_starved {
        return false;
    }
    let weakest = {
        let s = g.obj(ctx.id).ship.as_ref().unwrap();
        s.shields
            .iter()
            .enumerate()
            .filter(|(_, sh)| !sh.sys.destroyed() && sh.state != ShieldState::Down)
            .min_by(|a, b| a.1.strength.partial_cmp(&b.1.strength).unwrap())
            .map(|(k, _)| k)
    };
    let Some(k) = weakest else { return false };
    g.obj_mut(ctx.id).ship.as_mut().unwrap().shields[k].state = ShieldState::Down;
    true
}

/// `sub_1EA71` 58384: recovered (not starved, residual covers the shield
/// budget) → re-raise ONE dropped shield. Blocks the tree when taken.
fn shield_reraise(g: &mut Game, ctx: &Ctx) -> bool {
    let (starved, residual, shield_energy) = {
        let o = g.obj(ctx.id);
        let s = o.ship.as_ref().unwrap();
        (s.brain.shields_starved, o.residual, g.data.ships[s.design].shield_energy)
    };
    if starved || residual < shield_energy {
        return false;
    }
    let dropped = {
        let s = g.obj(ctx.id).ship.as_ref().unwrap();
        s.shields.iter().position(|sh| sh.state == ShieldState::Down)
    };
    let Some(k) = dropped else { return false };
    g.obj_mut(ctx.id).ship.as_mut().unwrap().shields[k].state = ShieldState::Up;
    true
}

// ===== §12.5 fire control =====

/// Returns true when a weapons action was taken (`sub_20D07` 62718).
pub fn weapons(g: &mut Game, ctx: &mut Ctx) -> bool {
    if g.obj(ctx.id).ship.as_ref().unwrap().brain.hold_fire {
        return false;
    }
    if fire_phasers_at_target(g, ctx) {
        return true;
    }
    if fire_torpedoes_at_target(g, ctx) {
        return true;
    }
    if launch_probes(g, ctx) {
        return true;
    }
    if board_via_transporter(g, ctx) {
        return true;
    }
    false
}

/// Friendly-fire corridor (`sub_20398` 61531): an ally inside `range` whose
/// bearing is within `tolerance` of the firing line blocks the shot. A block
/// raises the sidestep flag (word_38684) consumed by `torp_range_maneuver`.
fn corridor_blocked(g: &mut Game, ctx: &Ctx, fire_bearing: f64, range: f64, tolerance: f64) -> bool {
    for a in g.ship_ids() {
        if a == ctx.id {
            continue;
        }
        let o = g.obj(a);
        if o.nation != ctx.side {
            continue;
        }
        let (b, _) = target_bearing_mark(g, ctx.id, a, ctx.side);
        let d = apparent_dist(g, ctx.id, a, ctx.side);
        if d < range && ang_dist(b, fire_bearing) < tolerance {
            g.obj_mut(ctx.id).ship.as_mut().unwrap().brain.weave_base = -1.0;
            return true;
        }
    }
    false
}

/// `sub_1EAEF` 58464.
fn fire_phasers_at_target(g: &mut Game, ctx: &Ctx) -> bool {
    let Some(t) = ctx.target.as_ref() else { return false };
    if ctx.op_banks == 0 || t.dist > ctx.bank_range || g.get(t.id).is_none() {
        return false;
    }
    if g.obj(t.id).is_hulk() {
        return false;
    }
    // all operational banks locked on the target?
    let unlocked = {
        let s = g.obj(ctx.id).ship.as_ref().unwrap();
        s.banks
            .iter()
            .any(|b| !b.sys.destroyed() && b.enabled && b.lock != Some(t.id))
    };
    if unlocked {
        orders::lock_banks(g, ctx.id, &Mounts::All, t.id);
        return true; // locking is the action this cycle
    }
    if ctx.charged_banks == 0 {
        return false;
    }
    // sub_1EAEF: corridor is 5° at full bank range
    let bearing = t.bearing;
    if corridor_blocked(g, ctx, bearing, ctx.bank_range, AI_FF_TOLERANCE) {
        return false;
    }
    // sub_1EAEF: aggressive → alpha strike (all banks, only once every
    // operational bank is charged); timid → half the charged battery now
    let aggression = g.obj(ctx.id).ship.as_ref().unwrap().brain.aggression;
    let n = if g.rng.unit() + AI_AGGR_FIRE_BIAS <= aggression {
        if ctx.charged_banks == ctx.op_banks {
            ctx.charged_banks
        } else {
            return false; // wait for a full battery
        }
    } else {
        (ctx.charged_banks + 1) / 2
    };
    orders::fire_n_charged_banks(g, ctx.id, n, AI_PHASER_SPREAD) > 0
}

/// `sub_1EC17` 58601 + volley sizing `sub_2047A` 61646.
fn fire_torpedoes_at_target(g: &mut Game, ctx: &mut Ctx) -> bool {
    let Some(t) = ctx.target.as_ref() else { return false };
    if ctx.op_tubes == 0 || g.get(t.id).is_none() || g.obj(t.id).is_hulk() {
        return false;
    }
    if t.dist < t.torp_min_range || t.dist > t.torp_max_range {
        return false;
    }
    let design = {
        let s = g.obj(ctx.id).ship.as_ref().unwrap();
        g.data.ships[s.design].torp.clone()
    };
    let Some(td) = design.as_deref().and_then(|n| g.data.torp(n)).cloned() else { return false };
    // 20° off-axis lead against saturated dodgers (`flt_36004`)
    let lead = if t.pressure > 80 && !td.homing { AI_JINK_LEAD_OFFSET } else { 0.0 };
    // ensure tubes are locked with that offset (`sub_1F2C6`)
    let needs_lock = {
        let s = g.obj(ctx.id).ship.as_ref().unwrap();
        s.tubes.iter().any(|tb| {
            !tb.sys.destroyed() && (tb.lock != Some(t.id) || (tb.lead_offset - lead).abs() > 0.5)
        })
    };
    if needs_lock {
        orders::lock_tubes(g, ctx.id, &Mounts::All, t.id, lead, 0.0);
        {
            let b = &mut g.obj_mut(ctx.id).ship.as_mut().unwrap().brain;
            b.last_lead_offset = lead;
        }
        return true;
    }
    if ctx.loaded_tubes == 0 {
        return false;
    }
    // friendly corridor: the tubes are turreted, so the (lead+12)/2 tolerance
    // is ONLY the corridor half-angle around the firing bearing (sub_1EC17)
    let tolerance = (lead + 12.0) / 2.0;
    let corridor_range = if td.homing { t.dist * AI_HOMING_CORRIDOR } else { t.torp_max_range };
    let bearing = t.bearing;
    if corridor_blocked(g, ctx, bearing, corridor_range, tolerance) {
        return false;
    }
    // volley size from OPERATIONAL tubes (`sub_2047A`)
    let volley = {
        if g.rng.percent(10.0) {
            0
        } else if ctx.op_tubes == 1 || td.homing {
            1
        } else if t.max_warp < 2.0 || t.pressure < 30 || t.pressure > 80 {
            ctx.op_tubes
        } else if t.pressure > 60 {
            if g.rng.percent(35.0) {
                0
            } else {
                1 // 65%: single harassing shot
            }
        } else if g.rng.percent(10.0) {
            0
        } else {
            ctx.op_tubes / 2
        }
    };
    // sub_2047A tail: hold fire until the full volley is loaded so the salvo
    // launches synchronized and merges
    if volley == 0 || volley > ctx.loaded_tubes {
        return false;
    }
    // fire the first `volley` loaded tubes
    let list: Vec<usize> = {
        let s = g.obj(ctx.id).ship.as_ref().unwrap();
        s.tubes
            .iter()
            .enumerate()
            .filter(|(_, tb)| !tb.sys.destroyed() && tb.loaded.is_some() && tb.charge >= 100.0)
            .take(volley)
            .map(|(k, _)| k + 1)
            .collect()
    };
    orders::fire_torpedoes(g, ctx.id, &Mounts::List(list)) > 0
}

/// `sub_1EE1E` 58840 (+ loader `sub_1ED89` 58770).
fn launch_probes(g: &mut Game, ctx: &Ctx) -> bool {
    let Some(t) = ctx.target.as_ref() else { return false };
    if ctx.op_launchers == 0 || g.get(t.id).is_none() {
        return false;
    }
    let (mission, retreating, aggression) = {
        let b = &g.obj(ctx.id).ship.as_ref().unwrap().brain;
        (b.mission.clone(), b.stance == Stance::Retreat, b.aggression)
    };
    let probe_mission = matches!(mission, Some(crate::ai::Mission::Probe { ship }) if ship == t.id);
    let max_range = if probe_mission {
        ctx.probe_max_range / AI_PROBE_MISSION_DIV
    } else {
        ctx.probe_max_range
    };
    if t.dist > max_range {
        return false;
    }
    let probes_chasing = g
        .probe_ids()
        .into_iter()
        .filter(|&p| g.obj(p).pursue == Some(t.id))
        .count();
    let launch = if probe_mission {
        probes_chasing == 0
    } else if matches!(mission, Some(crate::ai::Mission::Attack { .. })) {
        let others_engaging = g.ship_ids().into_iter().any(|s| {
            s != ctx.id
                && g.obj(s).control == Control::Ai
                && g.obj(s)
                    .ship
                    .as_ref()
                    .map(|sh| sh.brain.target == Some(t.id))
                    .unwrap_or(false)
        });
        !others_engaging
            && (probes_chasing as f64) <= aggression * AI_FANATIC_CREW
            && g.rng.percent(50.0)
    } else {
        // opportunistic: target slower than the probe, no charged banks, unchased
        t.max_warp < ctx.probe_velocity && t.banks == 0 && probes_chasing == 0
    };
    if !launch {
        return false;
    }
    // loader gated behind the launch decision (`sub_1ED89`): load all
    // launchers first (prox 1000 retreating / 100 otherwise), fire once full
    if ctx.loaded_launchers < ctx.op_launchers {
        let prox = if retreating { 1000.0 } else { 100.0 };
        return orders::load_launchers(g, ctx.id, &Mounts::All, prox, 0.0) > 0;
    }
    orders::fire_probes(g, ctx.id, &Mounts::All, Some(t.id), None) > 0
}

/// `sub_1E018` 57219 + `sub_20C3C`: board when we can overwhelm them.
fn board_via_transporter(g: &mut Game, ctx: &Ctx) -> bool {
    let Some(t) = ctx.target.as_ref() else { return false };
    if g.get(t.id).is_none() {
        return false;
    }
    if !boarding_favorable(g, ctx) {
        return false;
    }
    match crate::systems::boarding::transport(g, ctx.id, t.id, i32::MAX) {
        Ok(n) if n > 0 => {
            let name = g.obj(ctx.id).name.clone();
            let tname = g.obj(t.id).name.clone();
            g.say(
                None,
                "",
                format!("{name} is beaming boarders to the {tname}!"),
                ReportKind::Alert,
            );
            true
        }
        _ => false,
    }
}

/// `sub_1E018`: my survivors ≥ target's × (1−aggression) × 10 and more than
/// 10 crew transportable.
pub fn boarding_favorable(g: &Game, ctx: &Ctx) -> bool {
    let Some(t) = ctx.target.as_ref() else { return false };
    let Some(target) = g.get(t.id) else { return false };
    let Some(ts) = target.ship.as_ref() else { return false };
    let s = g.obj(ctx.id).ship.as_ref().unwrap();
    let aggression = s.brain.aggression;
    if (s.survivors as f64) < ts.survivors as f64 * (1.0 - aggression) * AI_APPROACH_CONE {
        return false;
    }
    crate::systems::boarding::beam_capacity(g, ctx.id, t.id) > 10
}

// ===== §12.6 maneuver =====

/// Combat cruise speed (`sub_1DE53` 56985 + `sub_205DF` 61853).
pub fn combat_speed(g: &mut Game, ctx: &Ctx) -> f64 {
    let (battery_frac, max_dmg, latch, max_temp) = {
        let s = g.obj(ctx.id).ship.as_ref().unwrap();
        let max_dmg = s.drives.iter().map(|d| d.sys.dmg).max().unwrap_or(0);
        (s.brain.battery_frac, max_dmg, s.brain.overheat_latch, s.max_drive_temp())
    };
    if battery_frac < AI_BATTERY_CRAWL || ctx.max_warp <= 1.0 {
        return 1.0;
    }
    let (warp_eff,) = {
        let s = g.obj(ctx.id).ship.as_ref().unwrap();
        (g.data.ships[s.design].warp_efficiency,)
    };
    let mut w = ctx.max_warp * (100 - max_dmg) as f64 / 100.0 * warp_eff;
    // temperature panic hysteresis (sub_1DE53: enter >0.85 at ×0.5; while
    // latched, recover at ×0.9 until norm-temp drops below 0.4)
    let norm = (max_temp - AI_TEMP_NORM_BASE) / AI_TEMP_NORM_RANGE;
    if norm > AI_TEMP_PANIC_ENTER {
        g.obj_mut(ctx.id).ship.as_mut().unwrap().brain.overheat_latch = true;
        w *= 0.5;
    } else if latch {
        if norm < AI_TEMP_PANIC_EXIT {
            g.obj_mut(ctx.id).ship.as_mut().unwrap().brain.overheat_latch = false;
        } else {
            w *= AI_TEMP_LATCH_FACTOR;
        }
    }
    w.max(1.0)
}

/// Weave executor (`sub_2029B` 61410): course = base ± amplitude, cadence
/// every 2 cycles (counter bumped in `Ctx::build`); warp from combat_speed
/// reduced when the turn is sharp. Returns whether the helm changed.
pub fn weave_to(g: &mut Game, ctx: &Ctx, mode: u8) -> bool {
    let Some(t) = ctx.target.as_ref() else { return false };
    weave_exec(g, ctx, mode, t.bearing)
}

/// The executor proper; `bearing` is the raw base (mode 2 flips it 180°,
/// `sub_2051F`).
fn weave_exec(g: &mut Game, ctx: &Ctx, mode: u8, bearing: f64) -> bool {
    let Some(t) = ctx.target.as_ref() else { return false };
    // steering cadence (brain+1Ch)
    {
        let b = &mut g.obj_mut(ctx.id).ship.as_mut().unwrap().brain;
        if b.steering_cooldown < 2 {
            return false;
        }
        b.steering_cooldown = 0;
        b.weave_mode = mode;
    }
    let base = if mode == 2 { norm360(bearing + 180.0) } else { norm360(bearing) };
    // amplitude gated ONLY on the nearest incoming attacker (`sub_2051F`):
    // approach dead straight when nothing is boring in under 25000
    let closest_threat = ctx.nearest_incoming.map(|(_, d)| d).unwrap_or(f64::MAX);
    let (aggression, side_flip) = {
        let b = &mut g.obj_mut(ctx.id).ship.as_mut().unwrap().brain;
        b.weave_side = -b.weave_side;
        (b.aggression, b.weave_side)
    };
    let amp = if closest_threat < AI_WEAVE_THREAT_RANGE {
        let cap = (AI_WEAVE_AGGR_BASE - aggression)
            * (AI_WEAVE_AMP_MAX - closest_threat / AI_WEAVE_DIST_DIV).max(0.0);
        g.rng.range(0.0, cap.max(0.0))
    } else {
        0.0
    };
    let course = norm360(base + side_flip * amp);
    let mut warp = combat_speed(g, ctx);
    // slow down to complete sharp turns
    let (cur_course, cur_warp, w1turn) = {
        let o = g.obj(ctx.id);
        let d = &g.data.ships[o.ship.as_ref().unwrap().design];
        (o.course, o.warp.max(1.0), d.w1turn)
    };
    let turn_needed = ang_dist(cur_course, course);
    if turn_needed > (w1turn / cur_warp) * 2.0 {
        warp = (w1turn * 2.0 / turn_needed).clamp(1.0, warp);
    }
    // 3D: chase the target's elevation (toward) or invert it (away)
    let mark = (if mode == 2 { -t.mark } else { t.mark }).clamp(-90.0, 90.0);
    {
        let o = g.obj_mut(ctx.id);
        let changed = o.helm != HelmMode::Course
            || o.desired_course != course
            || o.desired_mark != mark
            || o.desired_warp != warp;
        o.helm = HelmMode::Course;
        o.desired_course = course;
        o.desired_mark = mark;
        o.desired_warp = warp;
        let b = &mut o.ship.as_mut().unwrap().brain;
        b.weave_base = base;
        b.weave_amp = amp;
        changed
    }
}

/// Random jink (`sub_2034B` 61490).
fn jink(g: &mut Game, ctx: &Ctx) -> bool {
    let course = g.rng.range(0.0, 360.0);
    let warp = combat_speed(g, ctx);
    set_course_helm(g, ctx.id, course, warp)
}

/// Wander when no target (`sub_1F200` 59343): player-side allies hold; the
/// rest keep course at cruise speed, turning ±90° on 5% of cycles.
fn wander(g: &mut Game, ctx: &Ctx) -> bool {
    let has_local_friend = g.ship_ids().into_iter().any(|s| {
        g.obj(s).nation == ctx.side && matches!(g.obj(s).control, Control::Local | Control::Remote(_))
    });
    if has_local_friend {
        // allies of a human hold position
        return set_course_helm(g, ctx.id, 0.0, 0.0);
    }
    let turn = if g.rng.percent(5.0) {
        if g.rng.unit() < 0.5 {
            -AI_SIDESTEP
        } else {
            AI_SIDESTEP
        }
    } else {
        0.0 // 95%: hold course
    };
    let warp = combat_speed(g, ctx);
    let course = g.obj(ctx.id).desired_course + turn;
    set_course_helm(g, ctx.id, course, warp)
}

/// Phaser-range maneuver (`sub_20120` 61222).
pub fn phaser_range_maneuver(g: &mut Game, ctx: &Ctx) -> bool {
    let Some(t) = ctx.target.as_ref() else { return false };
    if t.dist < ctx.bank_range {
        // inside the envelope: jink if our facing shield is nearly gone
        let face = relative_bearing(g, ctx.id, t.id, ctx.side);
        let weak = {
            let s = g.obj(ctx.id).ship.as_ref().unwrap();
            match crate::systems::shields::facing_shield(s, face) {
                Some(k) => s.shields[k].effective < AI_APPROACH_CONE,
                None => true,
            }
        };
        if weak {
            jink(g, ctx)
        } else {
            // sub_20120: target's CURRENT warp + 1
            let warp = g.obj(t.id).warp + 1.0;
            set_pursue_helm(g, ctx.id, t.id, warp)
        }
    } else if t.dist > ctx.bank_range * 2.0 {
        weave_to(g, ctx, 1)
    } else {
        // sub_20120: target's CURRENT warp + aggression × 4
        let aggression = g.obj(ctx.id).ship.as_ref().unwrap().brain.aggression;
        let warp = g.obj(t.id).warp + aggression * AI_APPROACH_WARP_AGGR;
        set_pursue_helm(g, ctx.id, t.id, warp)
    }
}

/// Torpedo-range maneuver (`sub_201C7` 61310).
pub fn torp_range_maneuver(g: &mut Game, ctx: &Ctx) -> bool {
    let Some(t) = ctx.target.as_ref() else { return false };
    // friendly blocked the corridor last shot (word_38684) → sidestep ±90,
    // primed through the weave executor
    let blocked = {
        let b = &mut g.obj_mut(ctx.id).ship.as_mut().unwrap().brain;
        let was = b.weave_base < 0.0;
        if was {
            b.weave_base = 0.0;
            b.steering_cooldown = 2; // sub_201C7 primes the cadence
        }
        was
    };
    if blocked {
        let side = if g.rng.unit() > 0.5 { AI_SIDESTEP } else { -AI_SIDESTEP };
        let bearing = t.bearing + side;
        return weave_exec(g, ctx, 1, bearing);
    }
    let aggression = g.obj(ctx.id).ship.as_ref().unwrap().brain.aggression;
    let standoff = t.torp_min_range + (1.0 - aggression) * (t.torp_max_range - t.torp_min_range);
    if t.dist < standoff {
        weave_to(g, ctx, 2) // extend
    } else {
        weave_to(g, ctx, 1) // approach
    }
}

/// Default combat maneuver (`sub_2001A` 61052).
pub fn default_maneuver(g: &mut Game, ctx: &Ctx) -> bool {
    if !ctx.can_move {
        return false;
    }
    if g.obj(ctx.id).ship.as_ref().unwrap().docked() {
        return set_course_helm(g, ctx.id, 0.0, 0.0);
    }
    let Some(t) = ctx.target.as_ref() else {
        return wander(g, ctx);
    };
    let my_probes_chasing = g
        .probe_ids()
        .into_iter()
        .any(|p| g.obj(p).owner == Some(ctx.id) && g.obj(p).pursue == Some(t.id));
    if my_probes_chasing {
        return weave_to(g, ctx, 2); // stand off, the probes are working
    }
    // sub_2001A loc_2009C: phasers dominate iff p>0 && p>t; torpedoes iff
    // t>0 && t>=p (ties → torpedo standoff); neither → full stop
    let p = t.phaser_dominance;
    let td = t.torp_dominance;
    if boarding_favorable(g, ctx) || (p > 0.0 && p > td) {
        phaser_range_maneuver(g, ctx)
    } else if td > 0.0 && td >= p {
        torp_range_maneuver(g, ctx)
    } else {
        set_course_helm(g, ctx.id, 0.0, 0.0) // helpless: full stop
    }
}

/// `sub_1DB12` 56572: recovered from retreat? No target counts as recovered;
/// no weapons at all means never recovered.
fn retreat_recovered(g: &Game, ctx: &Ctx) -> bool {
    if ctx.target.is_none() {
        return true;
    }
    if ctx.op_tubes == 0 && ctx.op_banks == 0 {
        return false;
    }
    let (ratio, bravery) = damage_ratio(g, ctx.id);
    ratio < bravery * AI_RETREAT_RECOVER
}

/// Stance helm (`sub_1EF17` 58970). Returns whether the helm changed.
pub fn stance_helm(g: &mut Game, ctx: &Ctx) -> bool {
    let stance = g.obj(ctx.id).ship.as_ref().unwrap().brain.stance;
    match stance {
        Stance::DestructRam => {
            if let Some(t) = ctx.target.as_ref() {
                set_pursue_helm(g, ctx.id, t.id, ctx.max_warp)
            } else {
                set_course_helm(g, ctx.id, 0.0, 0.0)
            }
        }
        Stance::Retreat => {
            if retreat_recovered(g, ctx) {
                g.obj_mut(ctx.id).ship.as_mut().unwrap().brain.stance = Stance::Normal;
                return false;
            }
            // sub_1EF17: retreat runs through the weave executor (mode 2,
            // jinking away), not a straight line
            weave_to(g, ctx, 2)
        }
        Stance::Normal => false,
    }
}

// ===== §12.7 morale =====

/// damage ratio = 1 − strength/max. The combat-strength estimate (charged
/// banks / loaded tubes) dips every time a volley fires, which would make
/// morale oscillate with ammo state; for morale we compare *operational*
/// mounts (and crew) against the design, so the ratio tracks real damage.
fn damage_ratio(g: &Game, id: ObjId) -> (f64, f64) {
    let s = g.obj(id).ship.as_ref().unwrap();
    let d = &g.data.ships[s.design];
    let now = 1.0
        + s.operational_banks() as f64 * d.banks_range / STRENGTH_DIVISOR
        + s.operational_tubes() as f64 * 6.0
        + s.survivors as f64 / d.crew.max(1) as f64 * 4.0;
    let max = 1.0
        + d.banks as f64 * d.banks_range / STRENGTH_DIVISOR
        + d.tubes as f64 * 6.0
        + 4.0;
    let ratio = (1.0 - now / max).max(0.0);
    (ratio, s.brain.bravery)
}

/// `sub_1EFC8` 59066: retreat and last-stand checks. Retreat is evaluated
/// FIRST; last-stand only when not retreating.
pub fn morale(g: &mut Game, ctx: &Ctx) {
    if retreat_check(g, ctx) {
        return;
    }
    last_stand(g, ctx);
}

/// `sub_1DB78` 56631. Returns whether retreat was triggered.
fn retreat_check(g: &mut Game, ctx: &Ctx) -> bool {
    let Some(t) = ctx.target.as_ref() else { return false };
    let docked = g.obj(ctx.id).ship.as_ref().unwrap().docked();
    if docked || !ctx.can_move {
        return false;
    }
    let (ratio, bravery) = damage_ratio(g, ctx.id);
    if t.dist > (AI_RETREAT_RANGE_BASE - bravery) * AI_RETREAT_RANGE_SCALE {
        return false;
    }
    // sub_1DB78: no working banks AND no tubes → retreat unconditionally
    let weaponless = ctx.op_tubes == 0 && ctx.op_banks == 0;
    if !weaponless && ratio <= bravery * AI_RETREAT_DAMAGE {
        return false;
    }
    let already = g.obj(ctx.id).ship.as_ref().unwrap().brain.stance == Stance::Retreat;
    {
        let b = &mut g.obj_mut(ctx.id).ship.as_mut().unwrap().brain;
        b.stance = Stance::Retreat;
    }
    if !already {
        // prefer a dockable friendly base (`sub_1623D`)
        let base = g.ship_ids().into_iter().find(|&b| {
            b != ctx.id
                && g.obj(b).nation == ctx.side
                && crate::systems::tractor::can_dock(g, ctx.id, b).is_ok()
        });
        let captain = g.obj(ctx.id).ship.as_ref().unwrap().captain.clone();
        let announced = g.obj(ctx.id).ship.as_ref().unwrap().brain.retreat_announced;
        let line = if let Some(b) = base {
            let bname = g.obj(b).name.clone();
            {
                let brain = &mut g.obj_mut(ctx.id).ship.as_mut().unwrap().brain;
                brain.mission = Some(crate::ai::Mission::Dock { base: b });
            }
            format!("Retreating to the {bname}.")
        } else if announced {
            "Continuing retreat.".to_string()
        } else {
            let lines = &g.data.messages.retreat;
            lines[g.rng.irange(0, lines.len() as i32 - 1).clamp(0, lines.len() as i32 - 1) as usize]
                .clone()
        };
        g.obj_mut(ctx.id).ship.as_mut().unwrap().brain.retreat_announced = true;
        g.say(Some(ctx.side), &captain, line, ReportKind::Ally);
    }
    true
}

/// `sub_1DA17` 56460: set the self-destruct and ram.
fn last_stand(g: &mut Game, ctx: &Ctx) {
    let (boarders, survivors, fanaticism, bravery, countdown) = {
        let s = g.obj(ctx.id).ship.as_ref().unwrap();
        (s.boarders, s.survivors, s.brain.fanaticism, s.brain.bravery, s.destruct_countdown)
    };
    if countdown >= 0.0 {
        return; // already ticking
    }
    let overwhelmed = boarders > survivors;
    let hopeless = (survivors as f64) < (fanaticism + 1.0) * AI_FANATIC_CREW && g.rng.unit() >= bravery;
    let ramming = fanaticism > AI_FANATIC_RAM
        && ctx
            .target
            .as_ref()
            .map(|t| t.dist < blast_radius(g, ctx.id))
            .unwrap_or(false);
    if !(overwhelmed || hopeless || ramming) {
        return;
    }
    let _ = orders::self_destruct(g, ctx.id);
    {
        let b = &mut g.obj_mut(ctx.id).ship.as_mut().unwrap().brain;
        b.stance = Stance::DestructRam;
    }
    let captain = g.obj(ctx.id).ship.as_ref().unwrap().captain.clone();
    let lines = &g.data.messages.destruct;
    let line =
        lines[g.rng.irange(0, lines.len() as i32 - 1).clamp(0, lines.len() as i32 - 1) as usize].clone();
    g.say(Some(ctx.side), &captain, line, ReportKind::Ally);
    // 50%: the enemy side's science officer detects the power buildup
    if g.rng.percent(50.0) {
        let name = g.obj(ctx.id).name.clone();
        for other_side in 0..g.data.nations.len() {
            if other_side != ctx.side {
                g.officer_say(
                    other_side,
                    format!("Energy buildup aboard the {name} — she's going to blow!"),
                    ReportKind::Alert,
                );
            }
        }
    }
}

/// `sub_20BF2` 62550: cloak only when there is NO target at all (and not
/// docked, cloak undamaged, not already cloaked). No auto-decloak here.
pub fn cloak_when_idle(g: &mut Game, ctx: &Ctx) -> bool {
    if ctx.target.is_some() {
        return false;
    }
    let (capable, cloaked, docked) = {
        let s = g.obj(ctx.id).ship.as_ref().unwrap();
        (s.cloak_capable && !s.cloak.destroyed(), s.cloaked, s.docked())
    };
    if !capable || docked || cloaked {
        return false;
    }
    let _ = orders::set_cloak(g, ctx.id, true);
    true
}

/// `sub_20AF8` 62435: remote-detonate a probe the target is outrunning.
pub fn remote_detonate(g: &mut Game, ctx: &Ctx) -> bool {
    let my_pos = g.obj(ctx.id).pos;
    for p in g.probe_ids() {
        if g.obj(p).owner != Some(ctx.id) {
            continue;
        }
        let Some(tid) = g.obj(p).pursue.filter(|&t| g.get(t).is_some()) else { continue };
        let target_warp = g.obj(tid).warp.abs();
        let probe_warp = g.obj(p).warp.abs();
        if target_warp <= probe_warp {
            continue;
        }
        let radius = blast_radius(g, p);
        let d_target = (g.obj(tid).pos - g.obj(p).pos).len();
        let d_me = (my_pos - g.obj(p).pos).len();
        if d_target < radius && d_me > radius {
            crate::systems::probes::detonate_probe(g, p);
            return true;
        }
    }
    false
}
