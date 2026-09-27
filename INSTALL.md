# FlyMiner 0.11.0 — unpack, pack lunch, mine! ⛏️

The Windows ZIP contains the Rust controller and the Forge client mod.
Use Minecraft Java **1.20.1 with Forge 47.4.13 or newer within Forge 47**.
The controller is for Windows x64. Other platforms can build from source.

## Install

1. Extract the ZIP into a folder you will keep. Run the controller from this
   folder so it can find its settings and save its learning.
2. Close Minecraft. Back up any previous `flyminer-*.jar` **outside** the instance's
   `mods` folder, then put `flyminer-0.11.0.jar` into `mods`. Keep exactly one
   FlyMiner jar. Other mods stay in place. The server does not need this client mod.
3. Copy `.env.example` to `.env`. Set `MC_INSTANCE` to the **instance folder**,
   not its `mods` subfolder:

   ```dotenv
   MC_INSTANCE="C:\path\to\your\Minecraft instance"
   ```

4. For a Dwarf who cannot swim and suffers in sunlight, create
   `config/flyminer/mining-profile.json` inside that instance before launching:

   ```json
   {"profile":"dwarf"}
   ```

   Otherwise leave the default `standard` profile. Restart Minecraft after a
   profile change. Dwarf work/travel requires a roof, even at night; normal routes
   never swim. Use covered chest/mine locations connected by a roofed path or
   tunnel. An unsupported underwater exit may still require manual rescue.

## Start with the lightweight controller

Open the game normally and join your world/server. No Microsoft credentials go
into FlyMiner. It follows your current game connection automatically.

Carry a suitable pickaxe, sword, food, torches, and crafting materials. Near your
home chests, type `/flyminer home set`; use `/flyminer home radius 16` to protect
the house from digging at every height. At the mining entrance, type
`/flyminer mine set`. Then choose ore and enable local control:

```text
/flyminer target minecraft:iron_ore
/flyminer enable
```

Open PowerShell in the extracted folder:

```powershell
.\miner.exe run --no-brain
```

Return to Minecraft and close chat, inventory, and the Esc menu. Enabled control
waits for menus to close. **F3 + P** disables pausing on lost focus if switching
windows keeps opening the pause menu. Stop with **Ctrl+C** or **`/flyminer stop`**.

`/flyminer home` requests a deposit/refill trip while the controller is running.
`run` resumes unfinished trips. To cancel an old trip and mine here instead, use
`.\miner.exe mine --no-brain`; add `--waypoint` to return to the saved mine first.

## Add the experimental fly brain

The large dataset is downloaded separately from its pinned upstream source:

```powershell
.\fetch-brain.ps1
.\miner.exe run
```

Mining checks still constrain actions. The network and learned readout rank
choices; this project has not demonstrated that they outperform the baseline.
The ZIP does not include anyone else's world maps, learning, login data, or
modpack. Fresh installations start with fresh state.

## Verify or roll back

Compare downloaded files with the release's `SHA256SUMS.txt` using
`Get-FileHash -Algorithm SHA256 <file>`. Close Minecraft before replacing the jar
or restoring your backed-up jar. Keep the matching controller from that release.
Do not delete your world or `runtime` folder to uninstall the bridge; `runtime`
holds local maps and learning you may want to keep.

Automated checks pass, but the current Cisco/Dwarf mining flow still needs a
live gameplay smoke test. See the source repository's validation record and
README for the complete behavior and limitations:
https://github.com/Rayato159/minecraft-auto-mining
