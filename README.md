# Blink

> [!WARNING]
> **Highly experimental.** Blink is an early, in-progress research project, not a finished
> game. Expect bugs, crashes, missing features, visual differences from the original, and
> breaking changes between versions, including save files, settings and network
> compatibility. Only one map (Lady Boyle's Last Party) is supported. Please report issues,
> but don't expect stability yet.

Blink is a standalone Rust/Bevy runtime that rebuilds the gameplay kernel of *Dishonored*
(2012) with high behavioral fidelity, using content from a legitimately owned local
install of **Dishonored: Definitive Edition (GOG)**. The runtime does not run or
modify `Dishonored.exe`. An optional research tool (`probe_launch`) can start your own
local copy with a measurement DLL, so that the port can be compared against the original.

No retail assets are included in this repository. All content is read at runtime
from your install.

> **Not affiliated with Arkane Studios, Bethesda Softworks or ZeniMax Media.**
> *Dishonored* is a trademark of ZeniMax Media Inc. This is an unofficial fan project; you
> need your own copy of the game to use it. See [Legal](#legal).

## Game modes

- **Free for all** (offline against bots, or multiplayer): everyone fights everyone, using the
  original's sword, pistol, crossbow and powers.
- **Contracts** (multiplayer, or offline against bots): social stealth in a crowd of NPCs. Each player is assigned a target
  and is someone else's target. Find yours, get close without being spotted, and take them out.
- **Custom missions** (single player or co-op): missions written as `missions/*.ron` files, played
  on the original level with its NPCs, dialogue and scripting. See `missions/README.md`.
- **Playground** (offline): a test level for the player mechanics, built from the original's
  assets. See [Playground](#playground).

## How this project is made

Blink is created and maintained entirely by AI coding agents: research, reverse engineering,
code, tests and documentation. A human sets the direction and reviews the results.

**Found a bug?** Please [open an issue](https://github.com/r614/blink/issues). Agents pick up
issues and work on fixes. Include what you did, what you expected, and what happened.
Screenshots, the console output and your game version (GOG Definitive Edition) all help.

## Status

Experimental. Playable, with known bugs, on Lady Boyle's Last Party (`L_Boyle_*`).

| Area | State |
|---|---|
| UE3 packages, IR, map import | done: 470/470 packages; meshes, materials, lightmaps, decals, props |
| Rendering | the original's shaders reproduced (`research/dishonored/RENDERING.md`) |
| Collision, movers, physics volumes, navigation | done |
| Player movement, combat, powers, weapons | measured against the original (`research/dishonored/HOOK_TRIAGE.md`) |
| AI, Kismet, conversations, barks, audio, HUD | done for the target map |
| Launcher | main menu, missions, server browser / creator, every original option (`research/dishonored/MENUS.md`) |

Open questions are tracked in `research/UNKNOWNS.md`. The shape the code is moving to
(subsystems over one tick-stamped event stream, composed by the original's data) is in
`research/ARCHITECTURE.md`.

### Pending features

Not yet ported, or ported only in part:

- **AI**: the whalers' combat behaviour (`DisBehaviorAssassinCombat`: combat and escape
  teleports, the attract / pull spell); the rest of the bark call sites.
- **Powers**: possession of rats and fish.
- **Walls of light**: sword and explosion hits, their sounds and AI noises, possessed
  creatures touching them.
- **Movers**: actors based on other movers (`Base` chains) do not carry their collision
  along (the Bridge drawbridge).
- **Presentation**: shadow maps for every light type, the L_Out sky, the remaining
  post-process effects, tripod animations, speaker props, the tower light beam; Kismet
  highlights of runtime-only targets (object lists, pawns) and the crosshair focus highlight;
  per-player camera fades; the NPC teleport spell's material fade.
- **Not yet checked on screen**: the journal and stats screens, the Kismet highlight, rain
  and camera fade, and the new sprite sizing for non-square particles (rain streaks).
- **HUD**: the "(Unconscious)" crosshair line still uses its own probe instead of the
  crosshair's target; pickup and body prompt texts are unconfirmed against the original.
- **Journal and stats**: the Powers and Bone Charms tabs, the note reader, audiograph
  playback, map items; the stats screen's replay and the coin-bonus charm.
- **Modes**: interaction prompts are shown in missions only (not FFA or Contracts).
- **Coverage**: screenshots of every map, and the oracle capture backlog
  (`fixtures/oracle/capture_queue.txt`).
- **Paired kills**: the master and slave pawns are not yet placed at the animations' authored
  spacing (`research/UNKNOWNS.md` U-969), so the sword can sit in front of the victim's face and
  the player's arm and body can pass through the victim. Also missing: camera shakes
  (`DisNotify_CameraShake`), the gored and eaten sever states, dramatic targets
  (`Front_Special`), double kills and the Tallboy's moves.
- **Campaign**: no campaign entry in the main menu yet. The cooked campaign missions
  (`tools/mission cook`) are not offered by the launcher.

### Known issues

Reported from play testing, not yet fixed:

- **Input**: conversation choices only respond to the gamepad (the prompt shows the A button).
- **Window**: the mouse cursor appears and the game loses focus when a match starts from the
  menu, when going back to it, and when opening or closing the pause menu. The window also minimizes on the way back to the main menu.
- **Settings**: the resolution cannot be changed.
- **HUD and menus**:
  - the crouch (stealth) overlay draws a large square in the middle of the screen;
  - a stray square texture shows on the menu's message box ("All unsaved progress...");
  - the letterbox bars do not reach the screen's edges on ultrawide displays.
- **Rendering**: the Playground pool's water (the Boyle river material
  `boyle_ext_MS.Water_boyle_ext_INST03`) renders as an opaque green surface with dark stripes.
- **Effects**:
  - blood renders black;
  - the camera's lens blood renders as white blobs;
  - particles start early or late against their events and stay too long.
- **Audio**:
  - throwing a grenade plays a gunshot;
  - combat sounds are out of sync with their events;
  - game sounds keep playing while the game is paused or minimized. The original pauses when
    its window loses focus (`BaseEngine.ini` `[Engine.Engine] bPauseOnLossOfFocus=TRUE`).
- **NPC animation**:
  - NPC animations are very jerky;
  - alerted and fighting NPCs move out of step with their animations;
  - death animations and their particles look incomplete.
- **Animation not yet requested by the simulation**: the renderer only draws what the
  simulation plays, so these are missing until the pawn states move there:
  - first person: the walk, run, crouch and jump arms, the left hand's idles, the Blink,
    Windblast and possession casts, equips, parries, mantles and the choke and carry poses;
  - NPCs: hit reactions, blocks, parries, stuns, ambient idles, turns and starts, blinks, the
    assassination and choke victims' moves and the get-ups (a thrown NPC stands up at once).
- **Performance**: frame rates are very low in Free Roam.

## Platforms

| Platform | State |
|---|---|
| Windows 10/11 (x64) | supported and tested |
| Linux | not supported yet: never built or run; game files are looked up with case-sensitive names |
| macOS | not supported yet: never built or run; same file-name issue as Linux |

The renderer (Bevy / wgpu) and the game code are cross-platform. The only Windows-only parts are the
optional research tools (`probe_launch`, `dishonored_probe_dll`). Linux and macOS support is planned.

## Download (Windows)

Pre-built Windows releases are on the [Releases page](https://github.com/r614/blink/releases).
Unzip, then run `dishonored_runtime.exe`. Keep the files together: the launcher starts
`dishonored_server.exe` and `dishonored_master.exe` from the same folder, and reads `missions/`.

You need Dishonored: Definitive Edition from GOG installed. Blink looks for it in the default GOG
folder (`C:\Program Files\GOG Galaxy\Games\Dishonored`). If it's somewhere else, the first launch
asks you to pick the folder and remembers it in `%APPDATA%\dishonored-rs\game_dir.txt`; delete that
file to choose again. `--game-dir DIR` or the `DISHONORED_DIR` environment variable override it,
and the dedicated server takes `--game-dir` too.

## Building from source (Windows)

Requirements:

- Dishonored: Definitive Edition from GOG, installed (found as described above).
- Rust (stable, edition 2024) from [rustup.rs](https://rustup.rs), with the MSVC toolchain
  (Visual Studio Build Tools, "Desktop development with C++").
- A GPU with DirectX 12 or Vulkan support.
- Optional: `ffmpeg` on `PATH`, for gameplay video capture.

```bash
git clone https://github.com/r614/blink.git
```

```bash
cd blink
```

```bash
cargo run --release -p dishonored_runtime
```

The first launch reads the game files and builds caches in `cache/` inside the checkout (set
`DISHONORED_CACHE` to put them elsewhere). That takes a while, and later launches are faster.
Without arguments the launcher opens. To go straight into a map:

```bash
cargo run --release -p dishonored_runtime -- --map L_Boyle_Ext_P
```

## Usage

### Launcher

Without arguments the runtime opens the launcher. This is Dishonored's own main menu, drawn
from the `UI_MainMenu` / `UI_OptionsMenu` movies over the `Dishonored_MainMenu` level, with
the menu music and UI sounds. Its entries:

- Play: offline match against bots.
- Missions: `missions/*.ron`.
- Playground: the test level (see [Playground](#playground)).
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
`--autopilot duelist|gunner|wanderer` lets a test bot play the joined client (it hunts the
other players along the original navmesh). Free-for-all rules, spawn points and the bots:
`research/dishonored/multiplayer/FFA.md`; `harness ffa` plays a headless match and lists every
death with its place and cause.

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

### Playground

A test level for the player mechanics, built only from the original game's assets read from your
install. The layout is new: `maps/playground.ron`, format in `research/dishonored/PLAYGROUND.md`.
It runs offline with every power and item, and has no bots unless you ask for them with
`--bots N`. You can also start it from the launcher's PLAYGROUND entry, or from Missions.

```bash
cargo run --release -p dishonored_runtime -- --playground
```

Each station is labelled in the world and announced when you walk up to it.

- **Start:** a bench that gives every power and item, or the starting loadout back, and the pickups
  (elixirs, ammunition, food, a rune, a bone charm, coins).
- **Row 1 (south):**
  - step heights 30 / 44 / 50 / 56 / 60;
  - slopes of 40 / 45 / 50 / 55 / 60 degrees;
  - mantle ledges from 60 to 360;
  - jump gaps from 100 to 500.
- **Row 2:**
  - a Blink course: targets at 500 / 1100 / 1600, pillars of 400 / 500 / 600, a wall to Blink-mantle;
  - a climbable chain;
  - a 600-deep pool for swimming and drowning;
  - stairs along the east wall with ledges at 880 to 1520 to drop from (fall damage).
- **Row 3:**
  - doors (one of them locked), a locker and a bin;
  - a lean corner;
  - a crouch tunnel;
  - a pain volume;
  - a shooting gallery with targets at 10 / 20 / 40 m;
  - sword-training dummies.
- **Row 4 (north):** a stealth lane with a lit half and a dark half, hiding spots and a patrolling
  guard.

`harness playground` plays every station headless with scripted input and checks each mechanic
against the values measured on the original (step and slope limits, mantle heights, Blink ranges,
fall damage, drowning, doors, pickups, hits, detection in light and dark).

### Map viewer (`--inspect`)

Controls: hold RMB (or Tab to lock the cursor) to look; WASD to move; Q/E down/up;
Shift fast; Alt slow. Options: `--pos x,y,z` and `--yaw`/`--pitch` (Dishonored units),
`--show-hidden` (blockers/invisible collision), per-feature render switches for
comparisons (`--no-lightmaps --no-materials --no-fx --no-dynamic-lights --no-fog --no-post --no-df-shadows --no-distortion --no-bloom
--no-sky --no-reflections --legacy-geometry`, or `--baseline`), `--debug-lighting` (white albedo),
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

- **sccache** (`RUSTC_WRAPPER=sccache`; `CARGO_INCREMENTAL` left unset so the workspace crates build incrementally while dependencies stay cached) caches compiled crates by content,
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
