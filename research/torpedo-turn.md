# Firing when no tubes are ready

In original BEGIN 1.65, `fire all torps` reports six tubes unable to fire and
advances the game: range changes from 171 to 131 in frames 041/042. See the
[transcript](evidence/selected-frames.txt) and
[screenshots](evidence/selected-frames.json).

At Rust commit `0fdade8b40bfa78c0d9782ab58c27ff6f91c3872`, the same command returns
`Stay`, so the game does not advance.

The [patch](patches/rust_torpedo_turn.patch) changes
`crates/begin/src/commands.rs`. In V165 mode, selecting an existing tube returns
`Advance` even if none can fire. The frontend then runs one world cycle, as it
does for other `Advance` results. The failure message remains and no shot is
requested. V200 and selections containing no valid tubes still return `Stay`.
Loaded firing, informational commands and `Ask` prompts keep their behavior.

The patch also covers Rust's accepted aliases and numbered tube selections.
Only `fire all torps` has been observed in the original for this case. Applying
the rule to those other inputs is a proposed extension. Destroyed or recharging
tubes and malformed trailing arguments need more original-game checks. The
original rejects the manual's `fire all torpedoes` spelling; this patch preserves
Rust's accepted spellings.

The [tests](tests/rust_torpedo_turn.rs) call the parser and simulation libraries.
They check movement, the failure message and the absence of a shot, plus controls
for V200, loaded tubes, invalid selections, informational commands and prompts.
They use seed 42 and stock Federation Heavy Cruiser/Klingon Escort data.
See [test results](VALIDATION.md) and [reproduction commands](reproduction/README.md).
