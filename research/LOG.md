# Research Log

Each entry: Question / Evidence / Experiment / Result / Confidence / Implementation consequence.
Build: GOG Definitive Edition, `Dishonored.exe` SHA-256 `70da9613…c6fb` unless noted.

---

## 2026-10-04 — Installation fingerprint
- **Question:** What exactly is the canonical target?
- **Evidence:** GOG registry key `GOG.com\Games\1701063787`, `goggame-1701063787.info`, file hashes.
- **Experiment:** Hash and version query of the install.
- **Result:** buildId 56034905455963308; exe 18,041,856 B, SHA-256 `70da9613…c6fb`;
  FileVersion is the uninformative `1,0,0,0`. Details are in `dishonored/GAME_FILES.md`.
- **Confidence:** CONFIRMED
- **Implementation consequence:** Every binary-level finding is tagged with this hash.

## 2026-10-04 — Package container format
- **Question:** Can cooked packages be read without the engine?
- **Evidence:** The header of `L_Streets1_P.upk` shows tag `9E2A83C1`, version `0x321`/`0x1E`
  and compression flag 2, followed by a chunk table.
- **Experiment:** Wrote `ue3_package`, which reads the summary, inflates the LZO chunks
  and parses the tables. The reader has a strict check that the export table ends
  exactly at `DependsOffset`. Ran `package_scan` over all 470 `.upk`.
- **Result:** 470/470 packages open and the strict check holds in every one.
  Exports are 68+4N bytes.
- **Confidence:** CONFIRMED
- **Implementation consequence:** Package reading is no longer a blocker (U-001, U-002).

## 2026-10-04 — Object flag dword order and script state frames
- **Question:** Why do some placed StaticMeshActors fail to parse?
- **Evidence:** Data for `StaticMeshActor_1737` starts with two import references, then
  `FFFFFFFF 00000000`, then `0000`, then `FFFFFFFF`, then NetIndex. The export's flag
  dwords read `00000000 02070001`.
- **Experiment:** Treated the first flag dword as the high half. That makes
  `RF_HasStack` (`0x02000000`) visible. Then read a state frame of
  `Node, StateNode, u64 ProbeMask, u16 LatentAction, Offset`.
- **Result:** All 74,408 objects in the `L_Boyle*` packages parse. Class default objects
  show `0x200` in the high dword, consistent with `RF_ClassDefaultObject = 0x200_0000_0000`.
- **Confidence:** CONFIRMED (level content)
- **Implementation consequence:** `Export::object_flags` is recombined as hi:lo. The state
  frame has no StateStack array at this version.

## 2026-10-04 — Component and class-specific prefixes
- **Question:** What precedes NetIndex for components?
- **Evidence:** StaticMeshComponent data starts with `00000000 00000000` before the first tag.
  DistributionFloatUniform has the same prefix. DominantDirectionalLightComponent starts
  with a u16 array of 30,624 entries.
- **Experiment:** For component classes, read `TemplateOwnerClass` (plus `TemplateName`
  when the component is owned by a CDO). For dominant lights, read `TArray<u16>` first.
- **Result:** All Boyle objects parse. Across the full install 1,260,662 objects parse
  and 1,079 fail (before the CDO-template rule). The failures are concentrated in
  script packages.
- **Confidence:** CONFIRMED for level content, STRONG for script packages
- **Implementation consequence:** These rules live in `Package::read_object`.

## 2026-10-04 — StaticMesh layout and the kDOP quirk
- **Question:** How is `UStaticMesh` serialized?
- **Evidence:** Hex of `air_duct_01.air_duct_09`. The kDOP nodes have element size 32,
  not the compact 6-byte format expected at v>=770. UEViewer has an explicit
  `GAME_Dishonored → old_kdop` branch.
- **Experiment:** Implemented the decoder and imported every mesh in Boyle Ext.
- **Result:** 405/405 meshes decode. Positions, packed tangents, half-float UVs and
  u16 indices are all consistent with the stated counts.
- **Confidence:** STRONG
- **Implementation consequence:** kDOP triangles (u16×3 + material) give per-poly
  collision for free. They are the starting point for M5.

## 2026-10-04 — Gameplay trace channels on primitives
- **Question:** Do primitives carry Dishonored-specific collision semantics?
- **Evidence:** Every StaticMeshComponent has `m_CollisionTraceTypes: DisPrimTraceMask`
  with booleans for Crosshair, Projectile, Melee and VisionLOS traces.
- **Experiment:** None yet (property decode only).
- **Result:** Per-primitive opt-outs exist for the four gameplay trace types.
- **Confidence:** CONFIRMED (existence); semantics UNKNOWN (U-012)
- **Implementation consequence:** `WorldQuery` needs trace channels that mirror these
  flags, or Blink, projectile, melee and AI LOS results will not match the original.

## 2026-10-04 — Data-driven gameplay constants in INIs
- **Question:** Where do power and movement tuning values live?
- **Evidence:** `DefaultPlayer.ini` has `m_fMaxFallSpeed=2000.0`, and `m_Powers=` entries
  that define Agility levels as attribute modifiers (`Attribute_JumpZ_PowerJump +1300`,
  `Attribute_MaxSpeedBeforeFallingDamage 2550`, `...FallingDeath 3500`,
  `Attribute_GroundSpeedSprint +30%`). There are also `DefaultPower.ini`, `DefaultWeapon.ini`
  and `DefaultPawn.ini`.
- **Experiment:** Read so far; not parsed yet.
- **Result:** Much of the movement and power tuning is plain-text config layered on
  script defaults.
- **Confidence:** CONFIRMED (values exist); runtime effect still to be measured
- **Implementation consequence:** Add an INI reader to `dishonored_assets`. The
  simulation should take constants from INI + class defaults, not hard-coded numbers.

## 2026-10-04 — First render of an original map (M4)
- **Question:** Does the decoded geometry come out in the right place, at the right scale, with usable textures?
- **Evidence:** Bevy screenshots of `L_Boyle_Ext_P` and its sublevels: an aerial shot from (-6000,-6000,6000) and a street-level shot from PlayerStart_1 (-2663,-1879,98).
- **Experiment:** Converted to Bevy space with a Y/Z swap and 0.02 m/uu. Collection matrices are applied as `ParentToWorld * component local`. BC textures are uploaded directly with their mip chains.
- **Result:** The Boyle estate is recognizable: mansion, walled garden, street perimeter, surrounding terraces. Transforms and scale are consistent at player eye height. Material texture params follow a consistent convention (`D_` diffuse, `N_` normal, `S_` specular, `O_` opacity, `E_` emissive, `D2_`/`N2_` variation). Ranking by that convention fixed the false-colour surfaces.
- **Confidence:** STRONG (visual). The transform convention still needs a numeric check against the original game (U-011).
- **Implementation consequence:** M4 is reached. Remaining visual gaps are unresolved cross-package materials (U-005, e.g. street cobbles), translucent FX cards (hidden by default) and lightmaps (U-013). None of these block collision work.

## 2026-10-04 — Baked lighting: component lightmaps (U-013)
- **Question:** Where and how are lightmaps stored on Boyle Ext static geometry, and can they drive Bevy's lightmap path?
- **Evidence:** Hex of `L_Boyle_Ext_Env` `StaticMeshActor_1737.StaticMeshComponent0` after its properties
  (offset 0x218): LOD count 1, one ShadowMap2D ref, zero shadow vertex buffers, `LightMapType = 2`,
  6 light GUIDs, then 3 × (object ref + 3 floats), UV scale/bias, then a single zero byte that ends
  the export. The two non-null references are `NormalizedAverageColor0_139` (scale 1,1,1) and
  `DirectionalMaxComponent0_139` (HDR scale). Env holds 185 textures of each name plus 11 more in
  other sublevels, all DXT1 in `Lighting.tfc`. `StaticMeshCollectionActor` components use the same layout.
- **Experiment:** `lightmap::read_static_mesh_component_lods` with a strict end-of-export check over the
  full Boyle Ext import; bake each component's rect into an RGB9E5 irradiance atlas (same layout as the
  source atlas), attach `bevy::pbr::Lightmap` on UV1, ambient and placeholder sun no longer affect
  lightmapped meshes. Screenshots: `harness_out/step0_baseline/{aerial,street,garden}.png` (before) vs
  `harness_out/step1_lightmaps/*.png` (after, lightmaps only) and `harness_out/step1_debug/*.png`
  (`--debug-lighting`: white albedo, lighting only). Irradiance luminance over all baked texels
  (`map_dump`): p10 0.003, p50 0.033, p90 0.21, p99 1.84, max 7.3.
- **Result:** 3,387 components with texture lightmaps, 14 without, 0 vertex lightmaps, 0 parse failures,
  196 atlases; import still takes ~3.5 s. The render shows the expected night look: blue moonlit
  surfaces, warm pools under the street lamps, dark interiors and occluded corners.
- **Confidence:** STRONG for the layout (strict end check on every component). LIKELY for the meaning of
  the two coefficients; the reconstruction formula is an APPROXIMATION (flat-normal average of the three
  basis intensities), see U-016.
- **Implementation consequence:** Static lights are already in the lightmaps, so step 3 must only add
  lights that UE3 also renders dynamically (dominant + movable). Normal maps cannot modulate the
  baked light until a custom material samples both coefficient textures.

## 2026-10-04 — Cross-package materials (U-005) and Arkane material slots
- **Question:** Why do ~145 resource references stay unresolved, and what do the material parameters mean?
- **Evidence:** `find_export` (new, `tools/package_dump/src/bin/find_export.rs`) over all 470 packages: every
  unresolved Boyle Ext texture import (`grounds.street_cobbles_01_d`, `Materials_ref.Textures.normalmap_N`, ...)
  is exported by `DishonoredGame.upk`; `NPC_Body_Eaten.Materials.Body_eaten_inst` by `Startup.upk`. MIC
  parents such as `grounds.MASTER_street_cobbles_01_WET_INST` are imports that the *same* level package also
  exports (the cooker duplicates them), and base materials (`Materials_ref.main_materials.generic_PMAT`,
  `Vfx_Materials.main_materials.DefaultTranslu_PMAT`) live in `DishonoredGame.upk`. The old resolver stopped at
  the first import in a parent chain, so BlendMode/TwoSided and inherited textures were never seen.
- **Experiment:** Index `DishonoredGame` + `Startup` (resources only) after the level packages; walk MIC chains
  through the index (`material_chain.rs`), collecting texture/scalar/vector parameters (child wins) and the
  base material's `BlendMode`, `TwoSided`, `LightingModel`, `OpacityMaskClipValue` and expression defaults.
- **Result:** unresolved references 145 -> 5 (2 distinct: `Npc_EliteGuard.Textures.EliteHead_nblur`,
  `Vfx_Materials.Textures.Water_foam`); every material reaches its base material (blend known for 374/374:
  302 opaque, 52 masked, 19 translucent, 1 additive). Materials whose diffuse resolves to nothing: 2
  (`Vfx_Materials.Material.Sun_PMAT`, `Skies02.BOYLE.Moon_Boyle_INST`, both sky, step 5), down from 11
  (blockout greybox materials now use their `Blockout_Texture`). Parameter conventions observed:
  colour vectors are rgb * a intensity (`E_Emissive_Color`, `S_Specular_color`), per-slot tiling scalars
  (`D_Diffuse_Tilling`, almost always 1), Phong `SP_Specular_Power` (10..128), opacity remap
  `O_Minimum_Opacity`/`O_Maximum_Opacity`. Base-material texture defaults are placeholders (a red/green
  "D" checker) behind static switches, so only explicitly set `S_/E_/O_` textures are used. Vista cards pack
  opacity in R and the lit-window emissive mask in G of the same `_o` texture.
  Screenshots: `harness_out/step1_lightmaps/*` (before) vs `harness_out/step2_materials/*` (after).
- **Confidence:** CONFIRMED for the package locations; STRONG for slot semantics; the Bevy mapping is an
  APPROXIMATION (see `map_render::build_material` doc comment: Phong -> GGX roughness, S_ as specular tint,
  no detail normals, one UV transform per material, static switches not decoded).
- **Implementation consequence:** `LIBRARY_PACKAGES` adds ~15 MB of packages to the import (import now ~5.5 s).
  Decoding the MIC `StaticParameters` (static switch values) would remove the explicit-only heuristic.

## 2026-10-04 — Map coverage: streaming levels, BSP, non-static actors (lead request)
- **Question:** Where do the reported geometry gaps come from?
- **Evidence:** `L_Boyle_Ext_P` `WorldInfo_4.StreamingLevels` = 7 `LevelStreamingAlwaysLoaded` with
  `PackageName` Audio, Env, Fx, Light, Nav, Script and **`L_BoyleStreet_Ext_Script`** (missed by the
  `L_Boyle_Ext_*` prefix match). `map_dump --survey`: every level `Model` is 156-192 bytes and every level
  `Polys` 24 bytes, i.e. empty BSP; all other `Model`/`BrushComponent`s belong to volumes (blocking, physics,
  audio, trigger, nav bounds). Placed StaticMeshComponents outside StaticMeshActor/Collection: InterpActor
  (29), DishonoredMovable (31, party balloons and bottles), DishonoredBreakableNavBlock (14), DisDoor (8),
  pickups/usable objects/keys/elixirs (~80), DisWaterSource, KActor, SceneCaptureReflectActor; plus 33
  SplineMeshComponents (cables) on SplineLoftActors.
- **Experiment:** `map_packages` now reads `StreamingLevels`; the importer adds components of any other
  placed actor at the actor's initial transform (render-only, `collides = false`) and bakes spline meshes
  (`spline_mesh.rs`, Hermite along mesh X). `--legacy-geometry` reproduces the old coverage.
- **Result:** instances 3,514 -> 3,754 (+30 from `L_BoyleStreet_Ext_Script` collections, +210 other-actor
  components incl. 33 deformed cables). No BSP to decode (U-007 resolved: no). Before/after:
  `harness_out/gaps_before/sheet.png` vs `harness_out/gaps_after/sheet.png` (8 spawn-point views; the mansion
  door opening in view 3 and the garden balloons are filled in), aerial difference image
  `harness_out/gaps_diff_aerial.png`.
- **Confidence:** CONFIRMED (package list, BSP emptiness); spline frame convention LIKELY.
- **Implementation consequence:** `dishonored_content::import_geometry` uses `map_packages` too, so the
  collision world now includes the 30 static `L_BoyleStreet_Ext_Script` collection components. Other-actor
  instances are not collidable, so doors/movables/pickups do not block the simulation.
