# Blink

Blink is a standalone Rust/Bevy runtime that rebuilds the gameplay kernel of *Dishonored*
(2012) with high behavioral fidelity, using content from a legitimately owned local
install of **Dishonored: Definitive Edition (GOG)**. The runtime does not run or
modify `Dishonored.exe`. An optional research tool (`probe_launch`) can start your own
local copy with a measurement DLL, so that the port can be compared against the original.

The end goal is **Contracts**, a social-stealth multiplayer mode: every player has
one target and one pursuer, and the loop is observe → identify → stalk → approach →
assassinate → escape.

No retail assets are included in this repository. All content is read at runtime
from your install.

> **Not affiliated with Arkane Studios, Bethesda Softworks or ZeniMax Media.**
> *Dishonored* is a trademark of ZeniMax Media Inc. This is an unofficial fan project; you
> need your own copy of the game to use it. See [Legal](#legal).

## Status

Playable on Lady Boyle's Last Party (`L_Boyle_*`): offline against bots, custom missions,
and Contracts / FFA over a dedicated server.

| Area | State |
|---|---|
| UE3 packages, IR, map import | done: 470/470 packages; meshes, materials, lightmaps, decals, props |
| Rendering | the original's shaders reproduced (`research/dishonored/RENDERING.md`) |
| Collision, movers, physics volumes, navigation | done |
| Player movement, combat, powers, weapons | measured against the original (`research/dishonored/HOOK_TRIAGE.md`) |
| AI, Kismet, conversations, barks, audio, HUD | done for the target map |
| Launcher | main menu, missions, server browser / creator, every original option (`research/dishonored/MENUS.md`) |

Open questions are tracked in `research/UNKNOWNS.md`.

## Layout

```
crates/
  ue3_package        minimal UE3 v801/licensee-30 reader (summary, LZO chunks, tables, properties)
  dishonored_ir      engine-agnostic map representation (Dishonored world space: Z-up, Unreal units)
  dishonored_assets  Dishonored decoders: StaticMesh, Texture2D (+ .tfc streaming), materials, levels
  dishonored_runtime Bevy presentation: renders an imported map with a free camera
  dishonored_audio   Wwise AKPK/soundbank/WEM readers, Wwise Vorbis decoding, event resolution,
                     dialog VO banks (Bk_<GUID>.INT)
  dishonored_conversation  original dialog graphs (DisConversation) read at runtime +
                     deterministic conversation runner (subtitles, VO, choices, story flags)
  dishonored_world   collision world (BVH) and pathfinding on the original navigation mesh
                     (`L_*_Nav` pylons; research/dishonored/NAVIGATION.md)
  dishonored_ai      NPC brains on Arkane's attention model and AI tweaks
                     (research/dishonored/AI.md)
  dishonored_settings player settings modelled on ArkProfileSettings (options, defaults, bindings)
  dishonored_probe   wire protocol + client of the runtime probe of the original game
  dishonored_probe_dll 32-bit DLL the probe injects (research/dishonored/INSTRUMENTATION.md)
tools/
  probe_launch       start the original game with the probe (32-bit build) and measure it
  package_dump       inspect a package: names / imports / exports / properties / hex
                     (package_scan: sweep a whole directory)
  map_dump           run the full map import and report statistics
                     (nav_dump: navmesh survey/islands/OBJ; tweaks: resolve a tweak object)
  audio_dump         Wwise packages/banks: survey, list, resolve an event, decode to WAV
                     under harness_out/ (`audio_dump event Snd_Power_P_Blink_Start Bank_Power_Player`)
research/            evidence log, unknowns register, format notes, prior art
```

## Usage

Requires a GOG install of Dishonored: Definitive Edition. The default path is
`C:\Program Files\GOG Galaxy\Games\Dishonored`; override it with `DISHONORED_DIR`.

```bash
cargo run --release -p dishonored_runtime -- --map L_Boyle_Ext_P
```

### Launcher

Without arguments the runtime opens the launcher. This is Dishonored's own main menu, drawn
from the `UI_MainMenu` / `UI_OptionsMenu` movies over the `Dishonored_MainMenu` level, with
the menu music and UI sounds. Its entries:

- Play: offline match against bots.
- Missions: `missions/*.ron`.
- Multiplayer:
  - Join: server browser through the master, plus direct connect.
  - Host: server creator. It starts a local `dishonored_server` and optionally a local
    `dishonored_master`.
- Options: every original setting, in the original categories.
- Quit.

Settings are saved to `%APPDATA%/dishonored-rs/settings.ron` (`--settings FILE` or
`DISHONORED_RS_SETTINGS` overrides the path). The game reads them at startup. Options the
port does not implement yet are kept and marked "not yet effective". In game, Esc opens the
pause menu: Resume, Options, Back to Main Menu. See `research/dishonored/MENUS.md`.

```bash
cargo run --release -p dishonored_runtime
```

`harness launcher` drives it end to end:

- screenshots of every screen;
- a settings change, persisted and picked up by a launched game;
- the pause menu;
- hosting, browsing and joining a local server.

The launcher also takes `--control PORT` and `--script FILE`; see
`fixtures/scripts/launcher_screens.json`.

### Playing

Offline against bots (FFA by default, `--mode contracts` for the social-stealth mode;
`--spectate` to watch the bots):

```bash
cargo run --release -p dishonored_runtime -- --bots 3 --mode contracts
```

Custom missions (solo, or co-op with `--mission` on the dedicated server). Authoring guide:
`missions/README.md`; how the original game builds missions: `research/dishonored/MISSIONS.md`.

```bash
cargo run --release -p dishonored_runtime -- --mission missions/boyle_masked_host.ron
```

Dedicated server and the optional control plane (an in-memory server list kept alive by
heartbeats):

```bash
cargo run --release -p dishonored_server -- --port 27015 --mode contracts --master 127.0.0.1:27900
```

```bash
cargo run --release -p dishonored_master -- --bind 0.0.0.0:27900
```

Join with `--connect HOST:27015`, or `--browse MASTER` to pick from the list.

In game (the original PC layout): WASD move, mouse look, Space jump, C sneak, Shift sprint,
LMB sword, RMB left hand (hold to aim Blink), Left Ctrl block, F use / talk, Q/E lean, R reload,
J journal, X walk, 1-5 Blink / Windblast / Dark Vision / pistol / crossbow, V third person,
Tab scores, F1 debug readout, F3 collision.
The HUD is Dishonored's own (gauges, emblem, crosshairs, objectives banner and fonts are
read from `UI_HUD_SF`, the icon packs and `DisFonts_SF` at startup).
Sound is the game's own too: Wwise banks are read and their Vorbis media decoded from the
install at startup (the codebook table is located inside `Dishonored.exe`), and simulation
events post the original events (sword, guns, powers, footsteps by surface, pain/death),
plus the Boyle bed, score and placed emitters. `--no-audio` disables it, `--volume V` scales it.
See `research/dishonored/AUDIO_FORMAT.md`.

### Map viewer (`--inspect`)

Controls: hold RMB (or Tab to lock the cursor) to look; WASD to move; Q/E down/up;
Shift fast; Alt slow. Options: `--pos x,y,z` and `--yaw`/`--pitch` (Dishonored units),
`--show-hidden` (blockers/invisible collision), per-feature render switches for
comparisons (`--no-lightmaps --no-materials --no-fx --no-dynamic-lights --no-fog --no-post --no-df-shadows --no-distortion --no-bloom
--no-sky --legacy-geometry`, or `--baseline`), `--debug-lighting` (white albedo),
`--max-tex N`. `harness fidelity` renders fixed Boyle Ext viewpoints with each feature
on/off and writes before/after pairs to `harness_out/<run>/REPORT.md`. How the level shading follows the original's shaders: `research/dishonored/RENDERING.md`.

```bash
cargo run --release -p package_dump -- "<install>/DishonoredGame/CookedPCConsole/L_Boyle_Ext_Env.upk" --exports
```

```bash
cargo run --release -p map_dump -- L_Boyle_Ext_P
```

```bash
cargo test --release
```

The retail regression tests skip themselves when no install is found.

### Parallel builds / worktrees

Several checkouts or `git worktree`s can build and test at once without rebuilding everything:

```bash
cargo install sccache --locked   # once
source scripts/dev-env.sh        # PowerShell: . .\scripts\dev-env.ps1
```

- **sccache** (`RUSTC_WRAPPER=sccache`, `CARGO_INCREMENTAL=0`) caches compiled crates by content,
  so a new worktree reuses most of the dependencies another checkout already built. Measured on a
  fresh worktree's release build: 219 of 342 cacheable compiles hit (64%).
  - Still compiled per worktree: the workspace's own crates, plus dependencies whose build-script
    output depends on the target dir and everything that depends on them.
  - Not cacheable at all: proc-macros, build scripts and the final link.
  - `sccache --show-stats` shows the hit rate.
- **One `target/` per worktree** (cargo's default). Separate target dirs mean no waiting on another
  build's cargo lock, and no build linking another worktree's half-finished crates.
  - Don't set `CARGO_TARGET_DIR` or per-checkout `RUSTFLAGS`: sccache hashes `CARGO_*` variables and
    the compiler flags, so either one makes every crate miss. The env script unsets both.
- **`DISHONORED_CACHE`** is one shared absolute directory (default
  `%LOCALAPPDATA%\dishonored-rs\cache`) for the derived game data: collision BVHs, navmeshes,
  gameplay config, NPC, travel, look and dialog caches. A new worktree reuses them instead of
  re-extracting from the install. Files are versioned by name: bump a file's name when you change
  its format.
- Test runs that must stay quiet: `DISHONORED_MUTE=1` silences every sound the game and launcher
  play; audio still decodes, so the audio checks keep working.

## Principles

- Behavioral compatibility over approximation. Unknown original behavior is
  measured, not invented (`research/UNKNOWNS.md`, `research/LOG.md`).
- Gameplay simulation lives in portable, fixed-step Rust. Bevy is presentation.
- Binary-level findings are tagged with the `Dishonored.exe` SHA-256 they apply to.

## Legal

- This project is not affiliated with, endorsed by or sponsored by Arkane Studios, Bethesda
  Softworks or ZeniMax Media. *Dishonored* and related names are trademarks of ZeniMax Media Inc.,
  used here only to identify the game this project is compatible with.
- The repository contains no game assets, extracted content or original code. The runtime reads
  data from a copy of the game you own, at runtime, and writes derived caches only to your machine.
- It does not circumvent copy protection. The research tools only read the game's files and,
  optionally, measure your own running copy.

## License

Copyright (C) 2026 Blink contributors.

This program is free software: you can redistribute it and/or modify it under the terms of the
GNU General Public License as published by the Free Software Foundation, either version 3 of the
License, or (at your option) any later version. See [`LICENSE`](LICENSE).

The license covers this project's code and documentation only, not the game or its content.
