# FlyWire reservoir in Rust

This implementation follows the LIF equations and constants in the upstream
`model.py` (Shiu et al.). It is a Minecraft
adaptation, not a reproduction of the biological sensory mapping or a claim that
the complete animal has been emulated.

Sources:

- https://github.com/philshiu/Drosophila_brain_model
- https://www.nature.com/articles/s41586-024-07763-9
- https://brian2.readthedocs.io/en/stable/user/refractoriness.html

Run `./fetch-brain.ps1` to fetch the two pinned v783 data files. The optional
upstream research checkout is not included in Git; source links and its MIT
notice are retained in `THIRD_PARTY_NOTICES.md` and `licenses/`.

## Numerical model

In mV and ms:

```
dv/dt = (-52 - v + g) / 20
dg/dt = -g / 5
spike when v > -45; reset v=-52, g=0
refractory=2.2 ms, synaptic delay=1.8 ms
weight=0.275 * signed connectivity count
```

The solver uses the exact linear update for a fixed 0.1 ms timestep:

```
a=exp(-dt/20), b=exp(-dt/5)
v_next=-52+(v+52)*a+g*(a-b)/3
g_next=g*b
```

Both variables are frozen and incoming events are ignored during refractory
periods, matching Brian's `unless refractory`. Update order is integration,
threshold detection, synaptic/input delivery, reset. Stimulated neurons have zero
refractory duration as in the reference. The source reset contains `w=0` although
`w` is not declared in its neuron model; the independent reference check omits
that unused assignment.

The real v783 graph contains **138,639 neurons / 15,091,983 connections**. Rust
imports the CSV and Brotli Parquet directly, preserves signed edge weights and
constructs a sparse CSR graph. The cache is checked against source size/mtime,
dimensions, monotonic offsets, indices and finite weights. Rebuild it by removing
only `runtime/flywire-v783.bin` when changing data. The source files stay intact.

Neuron arrays use f32 structure-of-arrays storage. AVX2 integrates eight neurons
and checks thresholds together; runtime feature detection selects a scalar
fallback on other CPUs. A delay ring stores presynaptic spikes, so only active
neurons' outgoing connections are traversed. There is no dense adjacency matrix
and no per-step log of every neuron in normal operation.

Conductances with magnitude below `1e-30 mV` are set to zero in both kernels.
This is far below the voltage resolution of f32 at the resting potential and
prevents long-decayed values from becoming persistent subnormals that slow the
CPU. It is an explicit numerical cutoff, not an extra biological mechanism.

## Minecraft adaptation and learning

One persistent network advances 20 simulated milliseconds per target selection;
this is not a 1:1 mapping to wall-clock game time. Sixteen bounded input channels
describe health, food, tools, enemies, light, exposed ore, space, elevation,
traversable neighbors, standing state, food availability, distance home and
complementary signals. A deterministic projection stimulates up to 512 neurons
with Bernoulli/Poisson-style input (one source per neuron per dt, at most 150 Hz,
input amplitude 250 * 0.275 mV applied directly to `v`, as in the author's
`PoissonInput(target_var='v')`; recurrent synapses update `g`). The chosen cells are **not asserted to be real
sensory neurons**. Output cells are assigned to the strongest directly connected
input group (excluding the driven cells). Sixteen population firing rates,
normalized by group size and simulated seconds and scaled by 20 Hz, feed an EMA
readout. This prevents the large whole-brain spike count from saturating all
channels to the same value.

The planner first finds safe reachable choices. At most 32 candidates are scored
using the existing ten features plus candidate/context interactions, giving a
32-feature ridge/UCB readout. Finished mining attempts update this readout with
the same observed reward as the existing bandit. `fly_profiles` in
`runtime/bandit.json` is separated by world/player/dimension/target. Existing
`profiles` are retained. `attempts.jsonl` records `brainFeatures`, `brainUpdates`
and the actual reward, while `scoreboard.json` records `brain_updates`.

The connectome's synaptic weights remain fixed. **The readout learns; this release
does not implement dopamine-driven synaptic plasticity inside the fly network.**
Movement, combat, crafting and home protection remain checked game operations.
The neural score cannot authorize a hazardous or protected dig.

## Reproducible checks

```
cargo test --offline
cargo clippy --offline --all-targets -- -D warnings
cargo run --release -- brain-bench
cargo run --release -- brain-trace
python -m pip install --target tmp/brian-validation brian2==2.10.1
python validation/compare_brian.py
cargo run --release -- nav-bench
```

Python/Brian2 is only an independent verification tool, never a runtime
dependency. The reference fixture has 19 neurons, signed recurrent connections,
delayed events, refractory behavior and deterministic input pulses over 100 ms.
On this machine it produced 88 spikes at identical timesteps in Rust and Brian2;
maximum errors were about 0.000064 mV for v and 0.000010 mV for g. This does not
establish bitwise equivalence of the two random-number generators or validate all
biological predictions on the full graph.

The benchmark runs **72 windows (1.44 simulated seconds)**. The final model's
latest median was **37.8 ms AVX2 versus 84.5 ms scalar (about 2.23x)**, with an
AVX2 maximum of 50.0 ms and late-window mean of 41.1 ms. Scalar and AVX2 spike
counts matched in all 72 windows. Actual results are in
`runtime/brain-benchmark.json`; Minecraft was running, and game activity/CPU load
affect cost. These are measurements, not latency bounds. The historical
pre-cutoff benchmark used an intermediate input mapping and is not a benchmark
of the final model; its obsolete output was removed during repository cleanup.

`nav-bench` replays the last recorded position and home against saved terrain;
it never sends game commands. The measured example found the same 18-step
frontier route using the previous search limit and the new 150 ms search budget.
That is a planning-time comparison, not a measurement of total time to get home.

Live return testing then exposed a larger bottleneck: unbuffered JSON checkpoint
writes. Buffering `atomic_json` with 256 KiB reduced measured scan-completion to
next-command gaps from 13,939–15,149 ms to 38–197 ms. The later run completed 30
commands in 19.7 seconds of action time, including checked straight routes and
descending existing stairs from Y=88 to Y=77 with full health. See
`runtime/live-return-validation.json`. These were consecutive, different route
segments, not a controlled end-to-end race to the same home. Normal bounded
cancellation now exits successfully while genuine IPC failures remain errors.

## What remains experimental

There is no measured Minecraft mining advantage over the original bandit yet.
Use `run --no-brain` as the control, restore the same world snapshot between runs,
and compare ore/minute, damage/deaths, return success and decision latency across
multiple trials. Mining changes terrain, so a seed alone is insufficient.
Input mapping, pooling and readout gain need controlled evaluation. A random
reservoir and a small conventional neural network are also appropriate baselines.

## Upstream notice

MIT License

Copyright (c) 2023 Philip Shiu and Nico Spiller

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
