# Existing Work: Dishonored (2012) / UE3 Asset and Runtime Prior Art

Context: a Rust/Bevy reimplementation of Dishonored 1 gameplay that loads assets from a local GOG
install. Known package facts: UE3 file version 801, licensee version 30, engine version 9098
(UELib's table lists 9099 for some builds), cooker version 133. Packages use the standard UE3
compressed-chunk table with `CompressionFlags = 2` (LZO). Textures stream from `Textures.tfc`,
`CharTextures.tfc`, `Lighting.tfc`.

Research date: 2026-10-04. "Unknown" means no license file or statement could be found.

---

## Summary table

| Project | Type | License | Runtime model | Relevance |
|---|---|---|---|---|
| UE Viewer / UModel (gildor2/UEViewer) | C++ asset viewer/exporter | MIT | Standalone | **Highest**. Reference parser for 801/30, meshes, textures, TFC, LZO |
| UE Explorer / UELib (EliotVU/Unreal-Library) | C# package library + decompiler | MIT | Standalone | High for UnrealScript/UClass/defaultproperties. Cannot read compressed packages |
| Gildor's Unreal Package Decompressor (decompress.exe) | C++ CLI (same codebase as UModel) | Source is part of UEViewer (MIT). Binary is freeware | Standalone | Medium. Good for producing uncompressed reference files to diff against |
| deadYokai/dishonored-toolkit | Python UPK tools (unpack/patch/fonts/subtitles/textures) | Unknown (none found). Archived 2025-06-08 | Standalone | Medium. Dishonored-specific localization and texture handling |
| deadYokai/ue3-tools | Rust rewrite of the above | AGPL-3.0 | Standalone | Medium-high. Only known Rust UE3 tool aimed at Dishonored. License is viral |
| deadYokai/u3t (CU3ML) | C++ UE3 mod loader (dinput8 proxy) | MIT | Injected | Low-medium. Runtime-hook reference |
| renatoehzproyectos/upk-viewer | Rust to WASM UE3 header viewer | Unknown (none found) | Standalone (browser) | Low-medium. Header/tables only, no decompression |
| UPKUtils (wghost/UPKUtils) | C++ UPK inspect/patch utilities | GPL-2.0 | Standalone | Low-medium. Generic UE3 table knowledge (XCOM/Batman) |
| UPK Explorer / TFC Installer (Nexus) | Windows texture export/import tools | Unknown (closed source freeware) | Standalone | Low. TFC texture-pack workflow |
| Dishonored Vortex Extension (Nexus site mod 924) | Mod-manager plugin | Unknown | N/A | Low. Confirms install layout (`CookedPCConsole`) and .upk/.pck mod targets |
| Dishonored VR (GingasVRFO, now VR-Stereo-Hub) | d3d9.dll proxy (forked DXVK) + OpenXR | zlib/libpng | Injected (Steam exe only) | Low for assets, some for camera/body knowledge |
| UEVR (praydog) | UE4/UE5 VR injector | MIT | Injected | None. Supports UE 4.8 to 5.x, not UE3 |
| adm244/Dishonored-FullRegen, Dishonored-MissionStats | Native C/C++ mods (dinput8.dll) | Unlicense | Injected | Low-medium. Shows regen is driven by tunable max/amount/delay/step |
| ProfessorFrog/DishonoredCuhrayzeeMod, MinusGix/DishonoredMods | .ini + UPK edit mods | Unknown | Asset/ini edits | Medium for finding gameplay tunables |
| ItsBranK/UE3SDKGenerator (TheFeckless-derived) | Internal UE3 SDK generator | See repo (deprecated) | Injected | Medium. Could dump Dishonored's full class/property layout from the running game |
| Blender PSK/PSA importers | Blender add-ons | DarklightGames: GPL-3.0-or-later. Befzz: unknown | Standalone | Medium for the art pipeline (UModel to PSK to Blender) |
| Dishonored Wiki (fandom), Steam power guides, speedrun.com | Documentation | CC BY-SA (fandom text). Others unknown | N/A | Gameplay numbers and movement-tech reference |

---

## 1. UE Viewer / UModel (gildor2/UEViewer)

- **Repository:** https://github.com/gildor2/UEViewer (checked commit a0bfb468, 2024-02-23). Site: https://www.gildor.org/en/projects/umodel
- **Purpose:** Viewer and exporter for UE1 to UE4 assets: meshes, textures, animations, sounds.
- **License:** MIT (LICENSE.txt, "Copyright 2022, Konstantin Nosov"). Third-party libs it bundles (LZO, zlib, etc.) keep their own licenses. LZO (miniLZO/lzo1x) is GPL-2.0, so do **not** port that part. Use a pure-Rust LZO decoder instead.
- **Dishonored support:** added 2012-10-10. The readme says "implemented support for Dishonored (except animations)". Detection: `if (ArVer == 801 && ArLicenseeVer == 30) SET(GAME_Dishonored);`. Game tag `-game=dis`.
- **Dishonored-specific code (complete list):**
  - `USkeletalMesh3::Serialize`: extra `FUserBounds m_UserBounds` (FName, FVector [ver>=761], float) before `Bounds`, and `TArray<byte> EdgeSkeleton` after `MeshOrigin/RotOrigin` (ver>=775).
  - `UStaticMesh3::Serialize`: forces the **old** kDOP tree format (`FkDOPNode3` array) even though ver 801 >= 770.
  - `ACF_EdgeAnim` added to the `AnimationCompressionFormat` enum. There is no decoder.
  - Renderer material heuristic only: `cubemap_tex` is treated as a cubemap and `cubemap_mask` as emissive.
  - Everything else (summary, export/import tables, names, property tags, bulk data, UTexture2D, LZO chunk reader) uses the generic UE3 code paths at version 801.
- **Reusable implementation:** the C++ is a direct reference to port to Rust (MIT allows it). Main files: `Unreal/UnrealPackage/UnPackage3.cpp`, `UnPackage.cpp`, `UnPackageUE3Reader.h`, `UnPackageReader.cpp`, `Unreal/UnCoreSerialize.cpp` (bulk data, compressed chunks), `Unreal/UnObject.cpp` (property tags), `Unreal/UnrealMesh/UnMesh3.cpp`, `Unreal/UnrealMaterial/UnTexture3.cpp`.
- **Reusable knowledge:** confirms which version gates apply at 801. Shows how TFC lookup works (`TextureFileCacheName` plus `.tfc`, with bulk offset/size in the TFC). Shows that the TFC payload is itself an LZO compressed-chunk block.
- **Limitations:**
  - **No Dishonored animation support.** Gildor confirmed on his forum (topic 1593) that Dishonored uses Sony's **Edge animation ("edgeanim")** compression on all platforms, and said it "looks like never will be supported".
  - Does not parse levels or actors (only meshes, textures, materials, sounds and anims). Audio is Wwise (`.pck`), not USoundNodeWave.
  - Drops trailing data in several objects (`DROP_REMAINING_DATA`).
- **Runtime model:** standalone.
- **Relevance:** highest. It is the primary reference for the Rust package/mesh/texture loaders. Exporting via UModel to glTF/PSK is also a quick way to validate the Rust parsers against known-good output.

## 2. UE Explorer / UELib (EliotVU/Unreal-Library, UE-Explorer/UE-Explorer)

- **Repositories:** https://github.com/EliotVU/Unreal-Library (library, checked commit 3207a17e, 2026-08-15). https://github.com/UE-Explorer/UE-Explorer (GUI).
- **Purpose:** C# library and decompiler for UE1 to UE3 packages. It decompiles UnrealScript bytecode and shows defaultproperties.
- **License:** MIT.
- **Dishonored support:** README table lists `Dishonored | 9099 | 801/030 | ✅` (script column). Build detection: `[Build(801, 30)] Dishonored`. Dishonored-specific code exists only in `UClass` deserialization (`src/Core/Classes/UClass.cs`, `#if DISHONORED`):
  1. After the generic `ForceScriptOrder` bool: an extra **FName** (`"Unknown:Dishonored"`).
  2. Then `NativeClassName = ReadString()`, followed by `goto skipClassGroups`. This skips the generic ClassGroups and NativeClassName reads.
  3. After `DLLBindName` (FName): `ClassGroups = DeserializeGroup("ClassGroups")`. In Dishonored, ClassGroups is moved to after DLLBindName.
- **Reusable implementation:** UnrealScript bytecode token tables and decompiler logic, plus UStruct/UFunction/UState/UClass layouts. This is the route to reading Dishonored's gameplay logic (`DishonoredGame.u`-style script packages) and **defaultproperties**. The real Blink range, mana costs and movement speeds live there.
- **Limitations:** it **does not decompress** packages. `UnrealPackage.cs` returns early when `CompressedChunks` is non-empty and `CompressionFlags != 0`. You must run Gildor's decompressor first. No mesh/texture export of note.
- **Runtime model:** standalone.
- **Relevance:** high for gameplay data extraction (script and defaultproperties). The three UClass quirks above must be replicated in any Rust UClass parser.

## 3. Gildor's Unreal Package Decompressor (decompress.exe) and UModel's `-uncook`/export modes

- **Source:** built from the UEViewer tree (`Tools/` / `UmodelTool`). Downloads at https://www.gildor.org/downloads. Forum: https://www.gildor.org/smf/index.php?topic=300.0
- **Purpose:** rewrites compressed UE3 packages to uncompressed form (`decompress.exe [-lzo|-lzx|-zlib] file.upk`). Output goes to `unpacked/`.
- **License:** code is MIT as part of UEViewer. The prebuilt binary is freeware.
- **Reusable:** a reference oracle. Decompress a Dishonored .upk with it and byte-compare against the Rust LZO chunk reader's output.
- **Limitations:** no Dishonored-specific logic needed. It just applies the generic chunk reader.
- **Runtime model:** standalone. **Relevance:** medium (testing).

## 4. Dishonored modding community tools

### 4a. deadYokai/dishonored-toolkit
- https://github.com/deadYokai/dishonored-toolkit. Python 3. **Archived 2025-06-08.** License: none found (unknown).
- Scripts: `unpack.py` (splits a decompressed UPK into `ObjectName.ObjectId.ObjectType` files), `patch.py` (repack), `texture2d.py` (DDS in/out), `fontEdit.py` (font replace via TTF/OTF), `subedit.py` (subtitles to/from YAML, 14 language codes: INT, CHN, DEU, FRA, JPN, RUS, ...), `upkreader.py`, `upkElements.py`, `upkCompressor.py`.
- Requires packages to be **pre-decompressed** with Gildor's tool.
- Dishonored-specific knowledge: localization arrays such as `m_Choices_Static` with per-language entries. It also implements a property-tag reader, which matches the generic UE3 tag format.
- Relevance: medium (localization and subtitles, texture repack). Standalone.

### 4b. deadYokai/ue3-tools (Rust)
- https://github.com/deadYokai/ue3-tools (mirror; primary at git.yokai.digital, docs at docs.yokai.digital/ue3-tools/). **AGPL-3.0.**
- A Rust rewrite of the toolkit. `src/` has `upkreader.rs`, `upkprops.rs`, `upkpacker.rs`, `schema.rs`/`schemadb.rs`, `versions.rs`, `pseudo*.rs`, and `native/`, `types/`, `utils/`. Uses `byteorder`, `bitflags`, `serde`. Version constants follow Epic names (e.g. `VER_MOVED_EXPORTIMPORTMAPS_ADDED_TOTALHEADERSIZE`).
- Relevance: medium-high as the only Dishonored-focused Rust UE3 code found. **The AGPL license means do not copy code into this project unless the project accepts AGPL.** Read it for knowledge only, or contact the author.
- Standalone.

### 4c. deadYokai/u3t (CU3ML, "Crappy Unreal Engine 3 Mod Loader")
- https://github.com/deadYokai/u3t (mirror of git.yokai.digital/deadYokai/cu3ml). C++. MIT. A `dinput8.dll` proxy that loads mods without replacing game files. README says "Unstable and maybe not working".
- Injected runtime. Low relevance, except as a reference for runtime inspection of the original game to measure values.

### 4d. Nexus Mods tools
- **UPK Explorer (OBSOLETE)**, https://www.nexusmods.com/dishonored/mods/49, and its successor **UPK Explorer for UE2-UE3**, https://www.nexusmods.com/site/mods/587. Texture export and texture-pack creation. Packs are installed with **TFC Installer**. Closed-source freeware, license unknown.
- **Dishonored Vortex Extension**, https://www.nexusmods.com/site/mods/924. Installs .upk/.pck mods into `CookedPCConsole` and auto-installs TFC Installer. License unknown.
- **Enhanced Powers Mod** (dishonored/mods/97) and **Infinite Bend Time Duration** (dishonored/mods/166). These show that power tunables are editable in packages/ini. Their pages were not retrievable (403), so the exact properties edited are unknown.
- Relevance: low to medium. Standalone tools.

### 4e. Gameplay mods on GitHub
- **ProfessorFrog/DishonoredCuhrayzeeMod**, https://github.com/ProfessorFrog/DishonoredCuhrayzeeMod. Has `.ini` edits and an `upk edits` directory. License unknown. **Worth reading:** the ini/UPK edits name the gameplay variables (combat and AI tunables).
- **MinusGix/DishonoredMods**, https://github.com/MinusGix/DishonoredMods. License unknown.
- **Dishonored Ultimate Difficulty Mod** (TTLG forum thread 141188) and **Field Editor 0.5.1 "Tautologist tool"** (ModDB). A UPK field editor that requires pre-decompressed packages.
- **adm244/Dishonored-FullRegen**, https://github.com/adm244/Dishonored-FullRegen, and **adm244/Dishonored-MissionStats**, https://github.com/adm244/Dishonored-MissionStats. C/C++, Unlicense, `dinput8.dll` injection, built for the post-DLC (Knife of Dunwall / Brigmore Witches) exe; GOG users must not launch through Galaxy. FullRegen exposes health/mana regen as *max value, regen amount, regen delay (s), regen step (s)*. This confirms the vanilla game uses a capped, stepped regeneration model.

## 5. Runtime hooking / VR projects

- **Dishonored VR**, https://github.com/VR-Stereo-Hub/Dishonored-VR (fork; the original by Gingas, https://github.com/GingasVRFO/Dishonored-VR, was discontinued after alpha 38.92). License: zlib/libpng. Implementation: a `d3d9.dll` proxy built on a forked DXVK, with an OpenXR backend (SteamVR or Virtual Desktop). **Steam exe (app 205100) only. GOG and Epic exes are not supported.** Code is split into `src/core` (generic VR) and `src/game/dishonored`. The game-specific part may contain camera and player-body hooks, but the README does not publish offsets. Low relevance to asset loading. Possibly useful for understanding view/camera behavior.
- **UEVR (praydog)**: supports UE 4.8 to 5.x only. **UE3 is not supported.** Not relevant.
- **vorpX**: commercial generic VR injector with Dishonored profiles. Not relevant.
- **UE3 SDK generators**, e.g. https://github.com/ItsBranK/UE3SDKGenerator (C++17, derived from TheFeckless's generator, deprecated/archival). Injected into a running UE3 game, it dumps every UClass/UStruct/UProperty with offsets via GObjects/GNames. Running it (or a similar dumper) against the GOG exe would give the authoritative native class layout and default values. Note the UE3 GObjects/GNames signatures differ per exe build.
- **Cheat tables:** csimbi's Dishonored CT (gamepressure mirror), and the FearLess "Dishonored - UnrealEngine3 Cheats" thread (t=3857). These include a move-speed mod and hooks on the pawn's `EPhysics` handling. Source pages were not retrievable (403). Treat as pointers only.

## 6. UPK tools (generic UE3)

- **UPKUtils**, https://github.com/wghost/UPKUtils. GPL-2.0. C++. `ExtractNameLists`, `FindObjectEntry`, `PatchUPK`, `DecompressLZO`, etc. Built for XCOM EU/EW and later Batman AA/AC. Good documentation of export/import table fields and hex-patching practice. Relevance low-medium. GPL, so knowledge only.
- **Gildor decompress.exe**: see section 3.
- **"Unreal Package Extractor"** (also Gildor; extracts raw objects from packages). Same forum thread family (topic 296).
- **stricq/UPKManager** (C#, Blade & Soul) and **GoneUp/GPK_RePack** (C#, Tera). Fork-specific, not Dishonored. Useful only as additional examples of LZO chunk readers and TFC handling.

## 7. Rust UE3 crates

There is **no general-purpose UE3 package crate on crates.io**. Findings:
- **deadYokai/ue3-tools** (AGPL-3.0). See 4b. The only Dishonored-targeted Rust code.
- **renatoehzproyectos/upk-viewer**, https://github.com/renatoehzproyectos/upk-viewer. Rust crate `upk-wasm` compiled to WASM with a JS frontend. Parses summary, names, imports, exports for "vanilla" UE3. **No decompression.** License not found. Low-medium relevance.
- UE4/UE5-only crates (not applicable to UE3 formats): `unpak`, `repak` (trumank), `uasset`, `rust-unreal-unpak`, `unreal_asset`.
- LZO in Rust: pure-Rust ports exist (e.g. `lzokay-native` / lzokay ports, and `minilzo-rs`, which wraps C). Prefer a pure-Rust, permissively licensed decoder (lzokay is MIT) over GPL miniLZO bindings. Verify each crate's license before adopting. UE3 uses plain **LZO1X** (UEViewer calls `lzo1x_decompress_safe`).

## 8. Blender UE3 import path

- **DarklightGames/io_scene_psk_psa**, https://github.com/DarklightGames/io_scene_psk_psa. Also on extensions.blender.org. GPL-3.0-or-later with "Game Engine Import-Export" provisions. Imports and exports PSK/PSKX/PSA. Current releases target recent Blender.
- **Befzz/blender3d_import_psk_psa**, https://github.com/Befzz/blender3d_import_psk_psa. Import-only for psk/pskx/psa and can use UModel's `.config` for PSA. A heavily modified continuation of the original `io_import_scene_unreal_psa_psk.py` (Darknet / Optimus_P-Fat / Active_Trash / Sinsoft / flufy3d). License not stated in the README. The maintainer plans to archive it.
- The legacy bundled Blender add-on `io_import_scene_unreal_psa_psk` (Darknet, Optimus_P-Fat) was historically GPL like other bundled Blender add-ons.
- Workflow: UModel exports Dishonored static/skeletal meshes to PSK/glTF. **Animations cannot be exported** (EdgeAnim), so PSA export is unavailable for Dishonored.
- Relevance: medium for an offline art and validation pipeline. Not for runtime.

## 9. Gameplay numbers (Dishonored 1)

Sources: Dishonored Wiki (fandom; direct page fetch was blocked, values below come from search-result excerpts of the Blink, Dark Vision, Windblast, Bend Time and Supernatural Abilities pages), dishonored.wiki, and the Steam guide "Everything you want to know about Dishonored's powers" (id 960545901). **These are player-facing values. The authoritative numbers are the script defaultproperties in the game's .u/.upk packages. Extract them with UELib (after decompression) and treat this table as a cross-check.**

| Power / mechanic | Value(s) | Notes / confidence |
|---|---|---|
| Blink I | range **10 m**, cost **20%** mana | Wiki. Given at game start |
| Blink II | range **15 m**, cost **20%**, **3 runes** | dishonored.wiki says Tier II also has "moderately extended" vertical reach. Reach shape is a "squashed sphere" (farther horizontally than vertically) |
| Blink behavior | does **not** cancel momentum; one Blink per airborne state, but "Double Blink" is possible if the first Blink happens very early (during the second jump with Agility); aiming at a ledge triggers an automatic climb/mantle that negates fall damage | Speedrun community / guides |
| Dark Vision | cost **20%** mana, Tier I **1 rune** | Wiki |
| Windblast | cost **50%**, Tier I **3 runes**, Tier II **4 runes** | Wiki (search excerpt) |
| Bend Time | Tier I slows time ~**12 s**; Tier II stops time ~**8 s**; cost **60%**; Tier I listed as **2 runes** in one excerpt | Rune count needs verification |
| Possession | **60%** mana; duration **20 s** (I) / **30 s** (II); the "Welcoming Host" bone charm extends rat possession to 30 s | Wiki / Steam discussion |
| Mana regen | only a small portion (~**20%** of the bar) regenerates automatically after a short delay | Wiki. adm244 FullRegen confirms capped regen with delay/step parameters |
| Unlimited-use powers | Blink, Dark Vision (also Void Gaze, Far Reach in D2) can be used indefinitely because their cost is within the regen cap | Wiki |

UE units: UE3 uses roughly 1 uu = 2 cm by Epic convention (UT scale), but Arkane may have used a different scale. Confirm with known geometry (door heights) or with the Blink range defaultproperty, e.g. 10 m vs its value in uu.

**Speedrun community (speedrun.com/dishonored, /dishonoredce):**
- Categories include Any%, "Minimum Blink" (record cited at 44:22 with 7 blinks vs 42:43 standard), "4 Blinks", "7 Blinks", "Only Blink%".
- Documented tech: **Double Blink** (forum thread z912k); **Slip Clipping** (sprint and lean into a door/wall, rotate the camera into it until the Blink marker shows through, release to pass through, which shows the Blink target trace can start from the leaned camera position); **Damage cancel** (a fall height that deals damage but skips the hard-landing animation); **Vault speed** (mantling over obstacles temporarily increases speed); the **Elevator glitch**; the **Outsider jump glitch** (forum 0cp7y).
- The Steam guide "Speed-Run Setup" (id 1876540431) covers FPS caps and binds. Some glitches are frame-rate dependent, which suggests movement/physics integration is tied to the variable timestep.
- The detailed rules docs are external Google Docs and were not retrieved.

## 10. Recommendations for this project

1. Port UEViewer's generic UE3 path (MIT) for summary, names, imports, exports, the LZO chunk reader, bulk data, UTexture2D plus TFC, UStaticMesh and USkeletalMesh. Add the three Dishonored quirks.
2. Use a pure-Rust LZO1X decoder. Validate against `decompress.exe` output.
3. Replicate the UELib UClass quirks (extra FName, NativeClassName moved, ClassGroups after DLLBindName) to parse script classes and defaultproperties. Pull the real power and movement constants from there.
4. Animations: budget for reverse-engineering Sony **Edge Animation** compressed data. No public decoder for Dishonored is known. Alternatives: procedural or hand-made animation, or capturing poses from the running game via an injected dumper.
5. Avoid copying AGPL (ue3-tools) or GPL (UPKUtils, miniLZO, Blender add-ons) code into the runtime.
