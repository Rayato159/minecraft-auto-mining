# Third-party notices

## Drosophila reference model

The LIF implementation and independent validation are adapted from the equations
and reference model by Philip Shiu and Nico Spiller:

- Repository: https://github.com/philshiu/Drosophila_brain_model
- Pinned data revision: `91bdd1e7dcf193f3e7ca5a8933497fcef63b7960`
- Paper: https://www.nature.com/articles/s41586-024-07763-9
- MIT notice: [licenses/Drosophila-model-MIT.txt](licenses/Drosophila-model-MIT.txt)

Minecraft encoding, pooling, goal readout, and policy integration are separate
adaptations. The upstream checkout and dataset are not redistributed here.
`fetch-brain.ps1` retrieves two pinned v783 files directly from the authors'
repository; provenance and hashes are in `data/flywire-v783.json`.
Consult upstream sources for dataset attribution and applicable data terms.

## Forge, Gradle, and Minecraft

The bridge build is based on the Forge MDK. Original notices are retained in
[forge-bridge/LICENSE.txt](forge-bridge/LICENSE.txt) and
[forge-bridge/CREDITS.txt](forge-bridge/CREDITS.txt). Gradle wrapper scripts retain
their Apache-2.0 notices; see [licenses/Gradle-Apache-2.0.txt](licenses/Gradle-Apache-2.0.txt).

Minecraft binaries, mappings, the SteamPunk modpack, and Forge dependencies are
not bundled. Build dependencies retain their own licenses. Minecraft is a
trademark of Mojang.

## Rust and validation dependencies

Rust dependencies are listed in `Cargo.toml` and pinned by `Cargo.lock`.
Brian2 and its Python dependencies are optional validation tools and are not
bundled. These dependencies retain their respective licenses.
