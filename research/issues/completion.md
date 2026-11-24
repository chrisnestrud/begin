# V165: simulation finalizes before the observed four-cycle completion window

In a recorded BEGIN 1.65 duel, the enemy loses its last crew and the player can
still run three informational commands, then four `helm 0 0` orders before
`Simulation complete.` appears. The enemy ship remains visible and coasts.
Rust commit `0fdade8b40bfa78c0d9782ab58c27ff6f91c3872` ends the duel immediately;
Session's display delay does not run more world cycles.

The [original recording](https://github.com/chrisnestrud/begin/blob/research/research/completion-window.md)
includes text and screenshots for frames 068–076. The duel was a Federation
Heavy Cruiser versus a Klingon Escort.

The [patch](https://github.com/chrisnestrud/begin/blob/research/research/patches/rust_completion_window.patch)
runs four more world cycles after detecting elimination in V165 mode. It applies
only when the recorded starting roster has two nations, exactly one living ship
has `Local` control, and that ship's nation is the sole surviving side. It adds
no transmission message.

This observation does not settle what should happen if the player dies during
the countdown, a crew revives, the local flagship changes, or the game has
alliances or more than two sides.

The [tests and reproduction commands](https://github.com/chrisnestrud/begin/blob/research/research/reproduction/README.md)
use constructed zero-crew scenarios and call the core and Session APIs. The
unpatched source fails two timing assertions; three control tests pass. With
the patch, all five behavior tests and 36 upstream tests pass; one AI-watch test
is ignored. They check hulk movement, informational commands, final Session
events and freezing after completion.

Would you like a PR for this?
