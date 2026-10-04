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

## Layout

```
crates/
  ue3_package        minimal UE3 v801/licensee-30 reader (summary, LZO chunks, tables, properties)
  dishonored_ir      engine-agnostic map representation (Dishonored world space: Z-up, Unreal units)
  dishonored_assets  Dishonored decoders: StaticMesh, Texture2D (+ .tfc streaming), materials, levels
  dishonored_runtime Bevy presentation: renders an imported map with a free camera
tools/
  package_dump       inspect a package: names / imports / exports / properties / hex
                     (package_scan: sweep a whole directory)
  map_dump           run the full map import and report statistics
research/            evidence log, unknowns register, format notes, prior art
```

## Usage

Requires a GOG install of Dishonored: Definitive Edition. The default path is
`C:\Program Files\GOG Galaxy\Games\Dishonored`; override it with `DISHONORED_DIR`.

```bash
cargo run --release -p dishonored_runtime -- --map L_Boyle_Ext_P
```

Controls: hold RMB (or Tab to lock the cursor) to look; WASD to move; Q/E down/up;
Shift fast; Alt slow. Options: `--pos x,y,z` and `--yaw`/`--pitch` (Dishonored units),
`--show-hidden` (blockers/invisible collision), `--show-fx` (fog/smoke cards),
`--max-tex N`, `--screenshot out.png --frames N`.

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
