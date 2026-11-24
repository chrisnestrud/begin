# BEGIN 1.65: two differences in turn timing

Two commands behave differently in original BEGIN 1.65 and
[fireflyk64/begin](https://github.com/fireflyk64/begin) at commit
`0fdade8b40bfa78c0d9782ab58c27ff6f91c3872`:

- [Firing when no tubes are ready](torpedo-turn.md): `fire all torps` advances
  the original game by one turn. Rust returns `Stay`.
- [Ending a duel](completion-window.md): after the enemy loses its last crew,
  the original accepts four more advancing orders before completion. Rust ends
  the duel immediately.

Each report links to a proposed patch and tests. The original observations come
from one recorded duel; the tests use constructed scenarios to check each rule.

- [Original transcript](evidence/selected-frames.txt) and
  [screenshot metadata](evidence/selected-frames.json)
- [Reproduction commands](reproduction/README.md) and [test results](VALIDATION.md)
- [Source details and limitations](PROVENANCE.md)
