# dishonored-rs

A standalone Rust/Bevy runtime that rebuilds the gameplay kernel of *Dishonored*
(2012) with high behavioral fidelity, using content from a legitimately owned local
install of **Dishonored: Definitive Edition (GOG)**. It does not run, inject into or
modify `Dishonored.exe`.

The end goal is **Contracts**, a social-stealth multiplayer mode: every player has
one target and one pursuer, and the loop is observe → identify → stalk → approach →
assassinate → escape.

No retail assets are included in this repository. All content is read at runtime
from your install.

## Status

| Milestone | State |
|---|---|
| M0 install/repo reconnaissance | done: `research/dishonored/GAME_FILES.md` |
| M1 UE3 package parsing | done: 470/470 packages, 1.26M objects |
| M2 normalized IR | done: `dishonored_ir` |
| M3 target map | done: Lady Boyle's Last Party (`research/dishonored/maps/TARGET_MAP.md`) |
| M4 render map | **done: Boyle estate renders in Bevy** |
| M5 collision | next (kDOP collision triangles already extracted) |
| Launcher | main menu, missions, server browser / creator, every original option (`research/dishonored/MENUS.md`) |

## Layout

```
crates/
  ue3_package        minimal UE3 v801/licensee-30 reader (summary, LZO chunks, tables, properties)
  dishonored_ir      engine-agnostic map representation (Dishonored world space: Z-up, Unreal units)
  dishonored_assets  Dishonored decoders: StaticMesh, Texture2D (+ .tfc streaming), materials, levels
  dishonored_runtime Bevy presentation: renders an imported map with a free camera
  dishonored_audio   Wwise AKPK/soundbank/WEM readers, Wwise Vorbis decoding, event resolution
  dishonored_settings player settings modelled on ArkProfileSettings (options, defaults, bindings)
tools/
  package_dump       inspect a package: names / imports / exports / properties / hex
                     (package_scan: sweep a whole directory)
  map_dump           run the full map import and report statistics
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
comparisons (`--no-lightmaps --no-materials --no-fx --no-dynamic-lights --no-fog --no-post
--no-sky --legacy-geometry`, or `--baseline`), `--debug-lighting` (white albedo),
`--max-tex N`. `harness fidelity` renders fixed Boyle Ext viewpoints with each feature
on/off and writes before/after pairs to `harness_out/<run>/REPORT.md`.

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

## Principles

- Behavioral compatibility over approximation. Unknown original behavior is
  measured, not invented (`research/UNKNOWNS.md`, `research/LOG.md`).
- Gameplay simulation lives in portable, fixed-step Rust. Bevy is presentation.
- Binary-level findings are tagged with the `Dishonored.exe` SHA-256 they apply to.
