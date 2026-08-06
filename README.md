# BEGIN — A Tactical Starship Simulation (Rust)

A faithful Rust port of *BEGIN 2.00* (Clockwork Software, 1984–1991) console
mode, rebuilt from the reverse-engineered `begin2.exe` — including the AI
behavior tree Daniel spent years tuning (see `AI_AND_COMBAT.md` for the
implementation-grade spec with asm line references throughout).

All ship, torpedo, probe and nation data is extracted from the original
binary (`tools/extract_stats.py` → `crates/begin-core/data/*.json`),
including the endgame messages and officer rosters. Weapon mount counts
come from **begin 1.65** (`begin.exe`) — begin 2.00 halved most loadouts,
but the classic K'tinga battle cruiser carries 5 banks and 5 tubes, and
that's what you get.

Damage lands the **begin 1.65** way by default: shields absorb at full
capacity, any hit that gets through at ≥ 50 destroys the ship outright,
and crew losses are percentage-based — the weapon formulas themselves are
identical across versions, so no fudge factors are involved. `--begin2`
selects begin 2.00's damage application instead (quartered shield
absorption, armour pool, hull integrity, with Daniel's `ooDestructDamage`
14.0 multiplayer patch).

## Playing

```
cargo run --release -p begin            # interactive setup (name, nations, fleets)
cargo run --release -p begin -- --quick # instant duel: Fed HC vs 2 Klingon BCs
```

Setup: `ally 2 heavy cruisers flagship`, `enemy 3 frigates`, then `begin`
(or `random` for out-of-sensor-range placement with fog of war).

In play, the full v1.65 + v2.0 command set works — `help` lists it:
`helm course 135 warp 6` (or just `135 6`), `pursue kronos 8`,
`lock all banks on kronos`, `fire all banks 45` / `fire phasers 1 2 spread 10`,
`lock tubes on kronos dispersion 20` (fans the salvo), `load tubes prox 500`,
`fire torpedoes 1 2`, `load launchers prox 800 time 30`,
`fire probes at kronos` (each probe reports its control code),
`status probes`, `detonate probe <code>` / `detonate all probes`,
`raise shields`, `reenforce 1`, `status tubes`, `scan kronos`, `damage`,
`transport 50 exeter`, `tractor cobra`, `dock alpha`, `cloak on`
(Romulans), `destruct` (a broadcast countdown), `tell asp attack kronos`,
`tell group 1 standoff`, `computer ship klingon frigate`, `quit`.
An empty line lets a cycle pass. Phaser fire and detonations flash briefly
on the scope (`flash off` or `--no-flash` disables).

### The port's additions

- **3D**: helm takes `course^mark` (e.g. `helm 320^22 8`); ships spawn
  coplanar and pursue/elude track targets out of plane. `--planar-lock`
  (or the `planarlock on` command) confines everything to the plane.
- **Near-future combat** (Terran Coalition): battlestars with fighter
  bays (`launch fighters`, `recover fighters`), railguns (`lock rails`,
  `fire rails` — 0.01–0.1 % c slugs, hitscan, ammo-limited) and unguided
  kinetic `harpoon` rounds.
- **Solar system**: `--date 2026-07-09 --near Mars:low|geo|high|rings`
  anchors the battle in orbit using JPL approximate planetary positions
  (+ Ceres and 10 major moons) built in. Stations attach to bodies at low
  or geosynchronous orbit. Saturn's rings and the asteroid belt generate
  deterministic debris fields near ships — flying through at warp is a
  bad idea, and so is lithobraking. With the `spice` cargo feature and
  `BEGIN_SPICE_KERNEL=de440.bsp`, positions come from `anise` instead.

## In the browser

**https://fireflyk64.github.io/begin/** runs the same game on anything
with a browser (Chromebooks included): an xterm.js console driving the
identical Rust engine compiled to WebAssembly — the shared
`begin::session::Session` state machine guarantees transcript-identical
play, verified byte-for-byte against the native console
(`tools/wasm_transcript_test.mjs`).

Cross-play: a console player hosts (`begin host`) and browser players
join with the room code — the page is a dumb terminal for the frames the
host renders, exactly like `begin join`. Rebuild and redeploy with
`tools/build_web.sh` (output in `web/dist/`).

## Multiplayer (peer-to-peer via lobbylink)

No game server — the host's process owns the simulation and every other
player is a dumb terminal seeing the text the host renders for them.
Starting `begin` with no arguments asks `single / host / join <code>`;
the equivalent CLI forms:

```
begin host --players 2            # prints a room code; add --coop for same-side play
begin join <CODE>                 # on the other terminal(s)
```

`--server URL` selects the signaling server (default
`https://pqrstuvw.xyz/lobbylink`; for a LAN game run lobbylink's
`p2p-lobby-server --listen-http 127.0.0.1:8789 --allow-no-origin
--allowed-origin http://127.0.0.1:8789`). In `--versus` (default) the
first joiner commands the enemy flagship. The cycle advances when every
seated player has entered a turn-ending command. Disconnected players'
ships revert to their AI captains.

## Workspace

- `crates/begin-core` — the simulation library (no I/O): exact cycle
  pipeline, physics, weapons, damage, repair, boarding, sensors/fog,
  the complete AI (reflexes → targeting → fire control → maneuver →
  morale → missions), environment. Every tuned constant carries its
  `begin2_annotated.asm` line reference (`constants.rs`).
- `crates/begin` — the terminal game: ANSI renderer matching the original
  screen layout, command parser, single-player loop, lobbylink host/join.
- `tools/extract_stats.py` — regenerates the data files from begin2.exe.

`cargo test` runs 32 tests: helm/power/drive physics, weapon and damage
math, boarding capture, the AI duel harness (2v1 resolves in ~250 cycles
with a torpedo standoff phase and a phaser knife-fight), station orbits,
deterministic ring procgen.
