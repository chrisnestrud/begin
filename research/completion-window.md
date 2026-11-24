# Four more turns before a duel ends

In the recorded BEGIN 1.65 duel, the final volley is followed by a transmission
notice and a scan showing zero enemy survivors. `scan Nova`, `report` and
`status damage` remain usable without advancing the game. Four `helm 0 0`
orders then precede `Simulation complete.` The enemy ship remains visible and
coasts. See frames 068–076 in the [transcript](evidence/selected-frames.txt).

Rust commit `0fdade8b40bfa78c0d9782ab58c27ff6f91c3872` ends the duel immediately.
Session's 4,000 ms display delay does not run more world cycles.

The [patch](patches/rust_completion_window.patch) changes `game.rs` and
`cycle.rs`. When the game detects elimination, it starts a four-cycle countdown.
The next four cycles run the simulation; the fourth then finalizes the game.
Informational commands do not consume the countdown. Session needs no change,
and the patch adds no transmission message.

The countdown starts only in V165 mode when the recorded starting roster has
exactly two nations, exactly one living ship has `Local` control, and that ship's
nation is the only one with living crew. Manually assembled games must call
`record_start_strengths`. Other initial endings keep the existing behavior.
Once started, the countdown keeps running; the winner is chosen from the living
sides at completion.

The original observation covers this one elimination case. It does not establish
what should happen if the player dies during the countdown, a crew revives, the
local flagship changes, or the game has alliances or more than two sides.
Those cases need more observations before extending the patch.

The [tests](tests/rust_completion_window.rs) set enemy crew to zero and check
continued hulk movement, informational commands, Session's final events and
freezing after completion. See [test results](VALIDATION.md) and
[reproduction commands](reproduction/README.md).
