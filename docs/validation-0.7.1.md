# Validation record — Rust controller 0.7.1

This patch uses the existing Bridge 0.7.0 / protocol 8. No Java changes or
Minecraft restart are required. Learning checkpoints and terrain remain intact.

## Return-navigation fixes

- Upward frontier detection accounts for jump headroom plus the two-cell liquid
  buffer. A checked staircase can reach an upper scan frontier and request a
  refresh instead of falsely exhausting the search before recognizing it.
- Search time/node limits retain a checked partial route. A fully exhausted,
  sealed component still returns no route. Partial routes are identified in
  status output and do not claim to reach the destination or be globally shortest.
- The dense liquid mask accelerates queries whose complete buffer lies within
  the current scan. At scan boundaries the search still checks the combined
  current/remembered terrain. Unknown cells are never treated as safe air.
- No-route messages distinguish missing pickaxes, low-health digging restrictions
  and other path constraints. They no longer generically suggest a missing tool.

## Offline verification

- The new ascending-shaft regression failed before the fix and passed afterward.
- 46 Rust tests passed, including bounded-search progress, a sealed corridor,
  protected-home detours, tool exhaustion, multi-scan routes, liquid buffers,
  and existing mining/storage/survival/learning coverage.
- `cargo clippy --offline --all-targets -- -D warnings` passed.
- `cargo build --offline --release` passed.
- Replayed the recorded failure position `(615,-1,-17)`, home `(639,72,37)`,
  original protected radius 45 and saved terrain. A 150 ms search produced a
  checked 35-step partial route to `(616,25,-3)`, measured at about 151 ms.
  A deliberately minimal search slice still produced five checked steps.
  This replay does not reproduce the original live timing exactly; the old
  controller also found a partial route in an offline replay. The regression
  fixtures isolate the frontier/budget defects independently of live timing.

## Live verification

Completed on the server with the user's authorization to continue until home.
The live run started near `(700,71,-14)`, after the user moved and changed the
protected radius to 16. This is not an uninterrupted test from the original
underground failure position or with its original radius 45.

- Reached the home chests in one continuous controller run, about 132 seconds
  including navigation, eating, scans and storage. Health/food were both 20 at stop.
- Opened the entrance passage normally. House protection and the two-cell
  liquid/unknown rule remained enabled.
- First chest `(641,72,41)` accepted part of the load and left 17 cargo stacks.
  Controller selected the next chest `(641,73,38)` and confirmed zero remaining
  cargo stacks, leaving 21 inventory slots free while retaining supplies.
- Home request was acknowledged and the mine-resume checkpoint was saved.
  The test watcher requested a clean stop immediately afterward. No mining
  resumed; final recorded standing cell was `(640,72,38)` and the bridge
  acknowledged `All actions stopped.` Both controller and watcher exited with 0.

Compact evidence remains local in `runtime/live-return-validation-0.7.1.json`,
`runtime/navigation-replay-0.7.1.json`, and `rust-29280-*` entries in
`runtime/actions.jsonl`. Large temporary replay snapshots were removed during
repository cleanup. Runtime files contain personal session data and are not
published with the source.

## Publication preparation

Bridge 0.7.1 removes the development player's hard-coded account/server lock.
Explicit in-game enable is now bound to the current world/server, player UUID,
and dimension, and revoked after leaving that session or death. Java tests
check this session guard. The portable bridge builds and passes its validation
suite; the live return described above used Bridge 0.7.0. The publication build
has not been installed into the running game or live-tested.
