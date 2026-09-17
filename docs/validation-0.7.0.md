# Validation record — 0.7.0

Changes: persistent in-run target-vein commitment across per-block reward updates
and survival interruptions; diagonal vein membership and retention of split
branches; descending exploration whenever a safe lower destination is reachable;
two-cell liquid exclusion in all three axes; routine crafting material reserves.

## Checked offline

- 43 Rust tests passed. They include an IPC run where a competing exposed vein
  becomes cheaper after the first ore break, combat/food/torch interruptions,
  and confirmation that the second member of the committed vein is still mined.
- Existing simulated home/storage/mine tests now retain 64 cobblestone marked
  as crafting supplies. The controller must acknowledge storage and resume
  mining rather than loop through chests trying to deposit the reserve.
- Planner tests cover split/diagonal vein membership, new connected members,
  blocked focus without switching veins, exhausted focus, and already-mined
  connectors staying exhausted after leaving the scan.
- Descending choices are filtered before bandit/brain scoring. Liquid tests
  include diagonal, above/below, unknown and scan-boundary cases. The dense
  separable liquid mask is compared with direct 125-cell checks.
- `cargo clippy --offline --all-targets -- -D warnings` passed.
- `cargo build --offline --release` passed.
- Release planner fixture: 35,937 cells / 12 ore candidates planned in 87 ms,
  including the new liquid mask. This is one local synthetic measurement,
  not an end-to-end live latency guarantee.
- Forge build/reobfuscation passed. Java checks include 127 liquid-buffer cases
  and six material-reserve policies, plus existing crafting, station-position
  and passage geometry checks.

## Policy boundaries

No ore means a checked staircase descent, targeting up to six levels below
within the current scan. If no lower cell is reachable, same-level detours are
allowed; no unsafe descent or vertical shaft is generated. A vein that still
contains unreachable/unsafe ore causes a pause rather than a new commitment.
Home/refill commands and target changes cancel the current commitment. It is
held in memory during a run, not restored after closing the controller.

Logs, planks, sticks, coal/charcoal and stone-tool materials reserve 64 per
group when available. Transfers use whole acknowledged stacks: a partial stack
plus the stack crossing the quota may total more than 64. Later stacks are
deposited. Metals/stations retain the existing emergency crafting policy.

Protocol 8 is required because the live bridge must recheck the liquid buffer
and label retained crafting stacks. Existing learned weights and terrain files
are preserved. The source terrain format is unchanged; the new exclusion is
computed from stored block/fluid properties and rechecked against fresh state.

## Live verification

At initial delivery this build had not yet been tested live. The prior controller was stopped
cleanly before this update. After the user closed the game, 0.7.0 was installed
in the SteamPunk instance and its SHA-256 matched the tested build. The previous
bridge was backed up to `runtime/mod-backups/20260917-234347`. The instance now
contains only `flyminer-0.7.0.jar` among active FlyMiner jars. The user later
loaded it and reported a return-navigation pause. See [validation-0.7.1.md](validation-0.7.1.md)
for the Rust-only fix, regression checks, and completed live return/storage run
using this same bridge.

Built bridge SHA-256:
`D0911BD1BA26752F01F1FFA6A68AF8FAF2A2BBE8B8D816EF1A2F4EC66AF79B18`.
