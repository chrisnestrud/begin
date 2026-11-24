# Sources and limitations

The original game was recorded on 2026-11-19:

- BEGIN Version 1.65, 72,514 bytes, SHA-256
  `7796985677fe231eb70270cac6b3c8d3eac3caa1f2e7e8132c04ab870453dc9e`
- DOSBox-X 2026.10.01 SDL2, executable SHA-256
  `e6c3d41ec7c4b3624864773661cc4f168c0cecf1d807a75f7a4e110d34762299`

The duel was Federation Trenton Heavy Cruiser versus Klingon Nova Escort.
Initial odds were 3.2 to 1, with crew counts of 450 and 95. The final displayed
score was 0.11, with crew counts of 450 and 0. The original random seed was
not controlled.

The debugger made 239 read-only requests and no state writes. The emulator was
paused between inputs for screenshots and video reads, so the recording does
not measure the wall-clock delay before evaluation.

The PNGs are unchanged copies of the captured screenshots. Text rows were decoded
from 80×25 CP437 video buffers; low control codes remain escaped in JSON. Use the
PNGs to check display glyphs. `selected-frames.json` records video-buffer hashes,
input times, frame numbers and PNG hashes. Executables, video buffers and memory
dumps are retained privately. The Rust tests do not require an original executable.

The tests and patches are unchanged from the 2026-11-19 experiments. The tests
use constructed scenarios to isolate turn timing. They do not reproduce the
original duel's damage, random sequence or score, or establish that the games
match in other respects.

The separate Cargo workspace builds the upstream core and frontend libraries.
It pins eleven registry dependencies and includes all four upstream core
integration-test files. It does not build optional NET/SPICE features, the web
frontend or application binaries, or change upstream manifests.

[SHA256SUMS](SHA256SUMS) covers every package file except itself.
[Test results](VALIDATION.md) include the source and dependency checks.
