# Validation record — 2026-09-17

## Offline checks

- Rust: the complete suite passed, 37 tests. This includes persistent return-to-mine,
  direct voltage stimulation, and a large-mob case with its center 6 blocks away
  but its hitbox 2 blocks away.
- `cargo clippy --offline --all-targets -- -D warnings` and release build passed.
- Forge build, reobfuscation and `validateMiningBridge` passed. The Java checks
  cover 9 crafting resource policies, 2 workstation searches using real
  Minecraft `BlockPos` objects, and 3 passage geometry cases.
- Independent Brian2 check of the final voltage-driven input: 88 spikes at the
  same timesteps; maximum voltage difference 0.000064 mV. See `runtime/brain-reference-validation.json` and
  [the model notes](fly-brain-rust.md) for the exact scope of this check.
- Full FlyWire graph: 138,639 neurons and 15,091,983 signed connections. Last
  extended benchmark median 37.8 ms for 20 simulated ms with AVX2 versus 84.5 ms scalar;
  all 72 window spike counts matched. This is not a mining quality test.

## Live return test

The previous unbuffered checkpoint writer caused 13,939 and 15,149 ms gaps
between scan completion and the next command. With buffered writes, the observed
gaps were 185, 43, 189, 38, 39 and 197 ms.

The second bounded run completed 30 actions in 19.7 seconds of action time,
moving from (633.54, 88, -39.50) to (615.50, 77, -21.45), including straight
walking batches and one-block descents. All actions reported done and health
remained 20. These are consecutive segments of different terrain, not a paired
comparison of complete trips to the same destination. Raw records are in
`runtime/actions.jsonl` and `runtime/live-return-validation.json`.

## Live crafting investigation

The first attempt crafted and placed a crafting table (planks 42 → 38), then
stopped with `Workstation is obstructed`. Iron ingots stayed at 21 and sticks at
7; no pickaxe was claimed as newly made. The cause was retaining Minecraft's
reused mutable block position during a stream reduction. Station selection now
copies positions before retaining candidates; the regression is part of every
Forge build.

Latest installed bridge SHA-256:
`B9807C1FD9231AA072434AEACEA34B2C7DBF45FB69B5A6991951533AB1A75AA4`.

After the workstation fix, real crafting succeeded: iron pickaxes 3 → 4,
iron ingots 21 → 18, sticks 7 → 5, health 20, menu closed and controller idle.
See `runtime/craft-confirmed-before.json` and `runtime/craft-confirmed-after.json`.

A real chest transfer at (641,73,38) succeeded and increased free slots to 26.
The return-to-mine leg paused cleanly at (629,72,53), with health/food 20, because
the old geometry rejected the house's wooden stairs and Chipped doors. A replay
of 332,278 known cells found only 430 reachable positions under those old rules.
Stair support and hand-operated doors/gates have now been added, with the house
digging prohibition retained. Raw diagnostic: `runtime/route-diagnostic.json`.

## Live passage and storage test after the geometry patch

A 90-second bounded run started underground near (669,56,21), opened the passage
at (639,73,48) through normal interaction, and deposited at the chest at
(641,72,41). The transfer reported zero remaining cargo stacks; free slots
increased from 16 at the start to 26 after storage. The home request was
acknowledged and `runtime/resume-mine.json` was saved for (639,80,-18).
The run recorded 117 actions, including one successful `open_passage`, one
successful `store`, and one action interrupted by an opened game menu. It
stopped cleanly with exit code 0 at the time limit before reaching the mine.
No digging actions were issued during this run. The character was idle after
the stop. Raw action IDs begin with `rust-32880-`; compact evidence is in
`runtime/live-passages-storage-validation.json`.

The next bounded run restored the saved mine trip. It paused after a descent
left the checked corridor. The user confirmed they were also walking manually
during this period, so this attempt cannot isolate a movement defect or verify
the staircase descent. Live control was stopped while the user moved around.

After the user stopped moving and explicitly requested another test, a fresh
90-second run restored the saved mine trip from (640,76,33). All 126 recorded
actions completed successfully. It reached (639,80,-18) after 73.5 seconds,
cleared the resume file, and automatically switched to mining. Two
`tfmg:lead_ore` blocks were broken, scoring 8 points. The two finished attempts
updated the bandit from 13 to 15 and the fly readout from 0 to 2; the latter was
confirmed in the saved learning checkpoint. Brain computation took 24.62 and
25.23 ms on these two decisions. Final state was idle, health/food 20, with an
iron pickaxe equipped. Evidence: `runtime/live-mine-resume-validation.json`.

These scores count confirmed broken ore blocks, not inventory receipts: no
lead items were in the inventory at the bounded stop. Storage, persisted
return-to-mine, and automatic mining resumption have now been observed across
separate bounded runs. The intervening manual repositioning means this is not
a measurement of one uninterrupted complete round trip.

Live smelting is still unverified: the user had no raw iron for the furnace
test. Combat and the learning readout's benefit versus the no-brain baseline
also remain unverified. Two real learning updates prove the feedback pipeline,
not improved mining skill. Simulated IPC tests and the numerical brain
comparison do not prove these remaining game flows.
