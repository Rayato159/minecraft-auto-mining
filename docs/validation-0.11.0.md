# 0.11.0 — mining-only Cisco build

Validated on 2026-09-27 using JDK 17 and Minecraft 1.20.1 / Forge 47.4.13.
The local Cisco's Fantasy Medieval RPG [Dragonfyre] profile uses that Forge
version; the previous bridge's minimum was 47.4.20.

## Scope

- Removed animal farming, hunting, fish trips, crop collection, food-mode
  commands/telemetry, and diving-equipment crafting from both controller and
  bridge. Deleted their implementation files and current user guide.
- Kept mining, defensive sword combat, eating, tool selection, pickaxe crafting
  and smelting, rear torches, chest trips, air recovery, terrain memory, and
  fly/bandit learning. Combat no longer depends on the deleted farming module.
- Added the explicit `dwarf` mining profile. It requires permanent sky cover for
  ordinary work and travel, including at night, and prevents removing the last
  roof block. Torch light is not treated as sunlight.
- Separated exposed/shaded cells in the scan palette, even when their Minecraft
  block state is identical. Dwarf terrain and learning keys are separate from
  standard-profile data.
- Oxygen rescue for a Dwarf only considers supported horizontal or stair steps,
  never buoyant vertical movement. Air emergencies can override the cover rule;
  ordinary work waits for shelter afterward.

The installed Medieval Origins Revival 6.6.0 Dwarf data defines `dense` and
`darkness_dweller` powers. This profile deliberately stays conservative even
when armor or enchantments might exempt the player from those powers. It does
not edit them or infer an origin through a fragile internal API.

## Checks completed

- `cargo test --locked`: **83 passed**. Includes simulated command/response
  mining, combat, eating, crafting, chest trips, oxygen preemption, Dwarf shelter
  waiting/resumption, terrain isolation, and rejection of exposed routes/roof
  cuts, and startup menu waiting/cancellation. These are automated simulations,
  not a live Minecraft playthrough.
- `cargo clippy --all-targets --locked -- -D warnings`: passed.
- `cargo build --release --locked`: passed.
- Forge `clean build`, followed by `build` after adapting the standalone test
  bootstrap to Forge 47.4.13's eventbus: passed. `validateMiningBridge` exercises
  Dwarf body exposure, palette separation, non-swimmer exits, fluid margins,
  retreat geometry, torch placement, recipes/reserves, and actual Minecraft tool
  and Soul Sand shape behavior. Existing deprecation warnings remain.
- Checked packaged metadata: bridge 0.11.0, Forge `[47.4.13,48)`, MC
  `[1.20.1,1.20.2)`. Removed food/farm/diving implementation classes are absent.
- Installed exactly one FlyMiner jar in the closed Cisco profile; source and
  installed SHA-256 match. Set its `config/flyminer/mining-profile.json` to
  `{"profile":"dwarf"}` and redirected the local controller's `.env`.

## Artifacts

`forge-bridge/build/libs/flyminer-0.11.0.jar`

```text
SHA-256 E8A025E97EAD672AB2AC23F0D817FB78539083FEF131D851D94046B7B0D8CEE7
```

`target/release/miner.exe`

```text
SHA-256 2EF5F0B557B366079DEFBA14D2B8BBA583381A84B55671A93C3A406D90073A63
```

## Remaining live check

An earlier 0.7.1 SteamPunk run reached home, opened a passage, deposited into
multiple chests, retained supplies, and stopped cleanly with full health. It did
not resume mining after storage, and its timing does not establish current Cisco
performance. Superseded per-version notes have been consolidated into this record.

Subsequent client logs confirm Bridge 0.11.0 loaded and the client connected to
the user's new server. Automated character control in Cisco has **not been
live-validated**. Origin effects, Better Combat behavior, pack-specific recipes,
and actual travel still need an in-game smoke test. Start under cover, set new
covered home/mine waypoints with a roofed connecting route, select an ore, then
enable control. No coordinates from another modpack were copied.

No safe supported oxygen exit means manual rescue is still necessary. The
profile cannot make a Dwarf swim, guarantee survival, or automatically shelter a
player who starts in an open field.

To uninstall, close Minecraft and remove only `flyminer-0.11.0.jar` from this
profile. No previous FlyMiner jar was present there. To change player constraints,
edit the profile to `standard` and restart Minecraft; world files are unaffected.

## Startup diagnostic correction

The original Rust startup guard returned the same error for disabled control,
no active world, and an open game menu. That made an enabled client with chat or
the Esc menu open look as though `/flyminer enable` had not worked. Startup now
waits for an enabled client's menu to close, honors cancellation without sending
any action, and distinguishes disconnected/disabled states. It prints the bridge
folder and connected server. The bridge always follows the current Minecraft
connection; no server-address allowlist or hardcoded IP was changed.
