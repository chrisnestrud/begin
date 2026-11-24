# V165: recognized torpedo fire order consumes no turn when tubes cannot fire

In original BEGIN 1.65, `fire all torps` advances the game even when no tubes
can fire. In Rust commit `0fdade8b40bfa78c0d9782ab58c27ff6f91c3872`, the parser
returns `Stay`, so the game does not advance.

In a recorded original duel, the order reports six tubes unable to fire while
range changes from 171 to 131. Frames 041/042 are in the
[transcript](https://github.com/chrisnestrud/begin/blob/research/research/evidence/selected-frames.txt).
The [research notes](https://github.com/chrisnestrud/begin/blob/research/research/torpedo-turn.md)
link screenshots, source details and tests.

The [patch](https://github.com/chrisnestrud/begin/blob/research/research/patches/rust_torpedo_turn.patch)
returns `Advance` in V165 mode when an order selects an existing tube but none
can fire. It keeps the failure message and requests no shot. V200 and invalid
tube selections keep their current behavior.

The patch also covers Rust's accepted aliases and valid numbered selections.
Only the original `fire all torps` case has direct evidence; applying the rule
to the other inputs is a proposed extension.

[Reproduction commands](https://github.com/chrisnestrud/begin/blob/research/research/reproduction/README.md)
exercise the parser and simulation. The unpatched source fails the V165 turn
assertion. With the patch, all four behavior tests and 36 upstream tests pass;
one AI-watch test is ignored. Controls cover V200, loaded firing, informational
commands, prompts and invalid selections.

Would you like a PR for this?
