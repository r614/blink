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

## 2026-10-04 — Sony Edge animation decoded (U-008)
- **Question:** Can Dishonored's `ACF_EdgeAnim` clips (all 113,232 sequences) be decoded without Sony's SDK?
- **Evidence:** Static analysis of Dishonored.exe (hash above) with `exe_probe`. The only reference to tag `0x45413035` is in the blend-tree evaluator `0x009DBC10`. From there I followed the frame-set decompressor `0x009DADA0`, the constant-channel decoder `0x009CD3A0` (reproduced instruction by instruction: packing-spec bit fields, fixed/float component decode, smallest-three recomposition, big-endian bit streams) and the animated-channel decoder `0x009CE380` (argument layout).
- **Experiment:** I implemented the decoder in Rust and checked it against the data. Initial-frame section sizes equal the summed spec bit widths, and key-section sizes equal popcount(mask) x spec bits exactly in every set tested. Set boundaries line up byte-exactly. Channel-major key order gives smooth trajectories (1.3–7 deg/key) where frame-major gives noise (49–91 deg). I also rendered stick figures of Idle/WalkN/RunN on the elite-guard skeleton.
- **Result:** Walk and run cycles decode correctly: alternating legs, counter-swinging arms, knee drive. Format documented in `research/dishonored/animation/EDGE_ANIM.md`.
- **Confidence:** STRONG (structure CONFIRMED by exact size accounting; motion validated visually; interpolation nlerp vs the binary's polynomial slerp is LIKELY close)
- **Implementation consequence:** Characters play decoded locomotion (idle / walk / run in 4 directions, rotation-only retargeting by bone name). Phase II NPC and player animation is unblocked. The fixed-format channel paths (flag bits clear) remain to be ported if the survey shows they are used.

## 2026-10-04 — Combat timings from animation notifies (U-021)
- **Question:** What are the sword swing, parry, equip and reload timings that the original drives from animation?
- **Evidence:** `map_dump anim_notifies` lists AnimSequence lengths, rate scales and AnimNotify events for the player AnimSets in Startup.upk (Edge decoder now available).
  - `Ply_Sword_Attack_as`, "Small" attacks: `DishonoredNotify_AttackZone` starts at 0.104–0.158 s and lasts 0.186–0.243 s; `AttackInterruptable` ≈ 0.58–0.63 s; `AnimStateExit` 0.78–0.95 s.
  - `Ply_Sword_Versus_as`: `DisNotify_BlockParryZone` from t=0.001 with duration 0.430 (0.422 sneaking).
  - `Ply_Sword_Assassination_as`: `Sword_Ready_Assassination_Back` exits at 1.886 s and the drop variants at 1.90–1.97 s.
  - `Ply_Pistol_as`: Fire exits at 0.30 s, Reload_In takes 0.433 s and Reload_Out 0.333 s. `CorvoGun_Mag_as`: Mag_0to1 takes 0.433 s.
  - `Ply_Crossbow_as`: Fire exits at 0.398 s, Reload_In 0.233 s, Reload_Out 0.333 s. `Crossbows_as`: RapidReload takes 0.4 s.
  - `Ply_Guns_as`: Equip exits at 0.43 s.
- **Experiment:** Replaced the speculative `CombatTuning` values with the following:
  - swing: startup 0.13, active 0.21, recovery 0.26
  - parry window 0.43, assassination 1.886
  - pistol: equip 0.43, reload 1.2
  - crossbow: equip 0.43, reload 0.97

  Re-ran the sim and multiplayer tests.
- **Result:** All mechanics and contracts tests pass with the measured values.
- **Confidence:** LIKELY. Notify times are CONFIRMED data. Mapping them to gameplay windows (for example, reload = sum of the In/Mag/Out sequences) is an inference. Stagger and blocked recoil are still SPECULATIVE.
- **Implementation consequence:** U-021 is mostly resolved. Swings are faster and parries more forgiving than the earlier guesses.

## 2026-10-04 — First-person rig and Edge joint order
- **Question:** Can the first-person view use Corvo's real arms, weapons and animations?
- **Evidence:**
  - `Engine.upk` contains `Ply_Player.Skm_Player`, a 79-bone full-body rig. It has `camera_jnt`, `handAttachment_R/L_jnt` and legs.
  - `Startup.upk` contains the weapons (`Wpn_PlySword01`, `Wpn_PlyGunElite`, `crossbow_01`, each with a `Root_Attachment_jnt`) and the player AnimSets (`Ply_Sword_*`, `Ply_Guns_as`, `Ply_Powers_as`, `Ply_Pistol_as`, `Ply_Crossbow_as`).
  - The sword sets animate 69 tracks (the whole rig minus the legs). The gun and power sets animate 30 (root plus the left arm).
  - The `fp_probe` tool poses the rig and prints bones relative to the camera bone.
- **Experiment:**
  1. Track mapping. Under `TrackBoneNames` order, `head_jnt` received exactly camera_jnt's bind translation. Re-mapping Edge joints to the set's tracks sorted by skeleton bone index made every animated translation match its bind length.
  2. Camera frame. The rig is placed so the animated camera bone coincides with the game camera (forward = −Z, right = +Y, up = +X of camera_jnt).
  3. Sockets. I compared candidate socket conventions numerically (blade axis against the fist's pinky→index axis) and visually with scripted screenshots.
  4. FOV. `DefaultCamera.ini` `m_fDefaultFOV=75` is applied as a horizontal FOV. It had been fed to Bevy as a vertical FOV, which made the world view much too wide.
- **Result:** The first-person view shows the following:
  - the left hand in the power idle, lower left
  - the sword tip in the lower right in Ready idle
  - full slash arcs synchronised to the simulation's swing phases
  - block/parry poses
  - pistol and crossbow equip, fire and reload in the left hand
  - Blink aim

  The NPC locomotion set was also reordered (its third-person walk still reads correctly).
- **Confidence:**
  - Joint order: CONFIRMED.
  - Camera-bone placement: STRONG (the left-arm poses frame naturally).
  - Socket conventions: LIKELY (U-025).
  - FOV: CONFIRMED value; the horizontal interpretation is LIKELY (UE3 convention).
- **Implementation consequence:** `crates/dishonored_runtime/src/viewmodel.rs` adds an overlay camera on render layer 1 with its own depth. Its clip state machine is presentation-only, driven by sword/left-hand phase, movement mode, speed and crouch.

## 2026-10-04 — Contracts bots navigate the original nav points
- **Question:** Why did the 10-minute 8-bot Contracts harness produce 0 kills?
- **Evidence:** Per-bot traces showed every bot frozen against geometry within 10 s. Bots steered in a straight line toward their target.
- **Experiment:** Built `dishonored_bots::nav::NavGraph` over the map's original AI points and PlayerStarts:
  - 235 nodes and 788 edges on L_Boyle_Ext_P
  - edges are segments with clearance at knee and chest height and ground under the path
  - A* pathing with waypoint skipping
  - stuck recovery (hop, sidestep, replan)
- **Result:** The harness passes 15/15: 29 contract kills, 34 chase transitions, guard and civilian responses, determinism, and the networked checks.
- **Confidence:** CONFIRMED (harness).
- **Implementation consequence:** Contracts bots in the harness use the nav graph. Test bots remain clients that act only through `PlayerInput`.

## 2026-10-04 — Third-person weapons and animation states
- **Question:** Do remote players, guards and civilians hold weapons and animate according to what they are doing?
- **Evidence:**
  - NPC skeletons (elite guard, male and female aristocrats) have the same `handAttachment_R/L_jnt` sockets as the player rig.
  - `L_Boyle_Ext_Script` has `Npc_EliteGuard_Sword_as` (sword idle/walk/run in 4 directions, patrol, attacks, parry, stun), `_Generic_as` (deaths, Windblast reactions, assassination victims), `_Gun_as` (draw, hold, fire, reload) and `_empty_as` (civilian locomotion, panic run).
- **Experiment:** Added a state machine (`choose_tp_clip`) driven only by simulation state, and per-kind weapon spawning:
  - guards: sword
  - players: sword, pistol, crossbow
  - civilians: nothing

  The socket convention is the same as first person (U-025). Checked with orbit-camera screenshots (`DISH_TP_ORBIT=<deg>`) in FFA and Contracts.
- **Result:** In FFA, players run with the sword in the right hand; swings, blocks, gun fire and reloads use NPC clips. In Contracts, players hide weapons until they draw (attack, block, aim, fire) and use the civilian gait, so they blend into the crowd. Deaths play a death clip instead of standing. Offline bots now use the nav graph too.
- **Confidence:** STRONG (visual) for clip choice. Socket convention LIKELY (U-025). There is no crouch or jump locomotion in the NPC sets; crouched armed idle uses `Sword_Cover_Loop`.
- **Implementation consequence:** `crates/dishonored_runtime/src/characters.rs` (`WeaponLibrary`, `shown_weapons`, `choose_tp_clip`). The grey placeholder sword is gone whenever character models load.
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
  basis intensities), see U-026.
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

## 2026-10-04 — Reflection cubemap and FX cards
- **Question:** Where do `C_` cubemap reflections come from, and can the translucent cards be shown?
- **Evidence:** `WorldInfo_4.mSceneReflection -> Level_Cubemap.BOYLE.boyle_ext_cubemap` (TextureCube,
  `FacePosX..FaceNegZ` -> six 256x256 DXT1 `CubemapFace*` Texture2D, no mips) in `L_Boyle_Ext_P`. 59
  materials set `C_Cubemap_texture`. FX materials now resolve their base material (`DefaultTranslu_PMAT`,
  `generic_translucent_PMAT`, unlit fog sheets with `D_DiffuseColorTweak` rgb + a).
- **Experiment:** Cube image -> `GeneratedEnvironmentMapLight` on the camera (GPU prefiltered, specular only).
  Translucent/additive/modulate materials render with Bevy Blend/Add/Multiply; unlit fog sheets use the tweak
  colour with alpha = tweak.a x opacity mask (remapped by `O_Minimum/Maximum_Opacity`). FX cards are no longer
  hidden by default (`--no-fx` hides them).
- **Result:** Works; reflections are subtle at night. Screenshot set `harness_out/step2_materials/*`.
- **Confidence:** CONFIRMED for the data path; APPROXIMATION for face orientation (UE3 faces reassigned to
  Bevy's Y-up slots without rotation) and for applying reflections to all materials via Fresnel instead of the
  per-material cubemap mask. Depth-biased alpha, distortion and UV panning of FX materials are not reproduced.
- **Implementation consequence:** A custom material would be needed for Arkane's per-material cubemap mask
  and the scrolling fog/smoke parameters (`Mo_Motion_Speed`, `UV_Multiply_*`).

## 2026-10-04 — Dynamic lights
- **Question:** Which lights must be rendered in real time, and with what parameters?
- **Evidence:** `L_Boyle_Ext_Light`: 1 `DominantDirectionalLight` (moon: Brightness 0.25, LightColor (164,209,255),
  pitch ~-18 deg), 2 `DominantSpotLight`, 1 `DominantPointLight`; 3 `StaticLightCollectionActor`s with 216 spot +
  83 point static components (baked; their GUIDs are in the lightmaps' LightGuids). `L_Boyle_Ext_Env`: 4
  `SpotLightToggleable` + 58 static collection lights; `*_Script`: 2 `DisPointLightToggleable` (dim red).
  `DominantPointLightComponent` has **no** `DominantLightShadowMap` array (first tag at offset 8, export 7),
  unlike the directional/spot variants: the old rule mis-parsed it (garbage `AdjustSaturation_135` tag).
  Tagged `Matrix` properties (`CachedParentToWorld`) store each row as (W, X, Y, Z); spot light actors leave
  their (class default, pointing down) `Rotation` unserialized, so the component matrix is the reliable source.
- **Experiment:** Import dominant + toggleable/movable lights (10 on Boyle Ext), convert units (see
  `crates/dishonored_runtime/src/lighting.rs` header), shadows only from dominant lights, cap 64 local lights,
  drop the placeholder sun and reduce the placeholder ambient to fill only non-lightmapped meshes.
- **Result:** Moonlight now lights the scene directionally with cascaded shadows; lamp spots light the garden
  and balloons. `harness_out/step2_materials/*` (before) vs `harness_out/step3_lights/*` (after).
- **Confidence:** CONFIRMED (which lights, parse fix, matrix layout); APPROXIMATION for point/spot falloff
  (Bevy inverse-square matched to UE3's radial falloff at R/4) and for dominant shadows (Bevy real-time
  cascades instead of the cooked `DominantLightShadowMap` / per-component distance-field `ShadowMap2D`).
- **Implementation consequence:** Light functions (`LightFunction`, 3 in Light) and lighting channels are ignored.

## 2026-10-04 — Atmosphere and post
- **Question:** Where are fog and post-process settings, and how do they map to Bevy?
- **Evidence:** Boyle Ext has no HeightFog/ExponentialHeightFog/PostProcessVolume actors. Fog is Arkane's
  `Engine.DisFog` actor + `DisFogComponent` (3 placed in `L_Boyle_Ext_Env`): `Opacity`, `LightColor`,
  `Origin`, `Height`, `NearPlane`, `FarPlane`, `NoFogPlane`, `SunPower`, `bCustomTransition`,
  `fCustomTransitionHeight`, `FogLUT`/`m_RawLut` (128-byte LUT); class defaults (`Engine.Default__DisFogComponent`)
  Opacity 0.8, white, Height 20, Near 2000, Far 10000. Layers: DisFog_0 (z -2075..3425, near 3000, opacity 0.4,
  dark blue), DisFog_2 (z -2075..13925, near 20000, far 100000, opacity 0.5, lighter blue), DisFog_3 (ground
  layer z -2075..-1275, near 800, far 7500, opacity 0.65). Post is `WorldInfo.m_ArkDefaultPpSettings`
  (`ArkPpConfig`): DOF focus 5500 uu / far blur 0.2; colour balance CrMgYb shadows (0, .03, .04) and
  midtones (0, .3, .4); HDR exposure 0.3, gamma 0.95, film grain 3.0, "GimpContrast" 0.05; bloom tint
  (1, .95, .72) scale 0.4. Only overridden fields are serialized; the ArkPpConfig struct defaults are not
  decoded yet.
- **Experiment:** `atmosphere.rs`: per-frame layer selection by camera height -> Bevy linear `DistanceFog`
  (colour alpha = opacity); TonyMcMapface tonemapping, `ColorGrading` exposure = log2(0.3 / 0.18) EV,
  temperature/tint from the midtone colour balance, `Bloom` intensity 0.4 x scale. Calibration sweep of
  tonemappers and EV offsets in `harness_out/cal/` (sheet.png): Bevy section gamma/contrast with Arkane's
  values crushed the shadows to black, so those two are left unmapped.
- **Result:** Distant geometry fades into blue-teal haze, foreground keeps the warm lamp pools.
  `harness_out/step3_lights/*` (before) vs `harness_out/step4_post/*` (after; `sheet.png`).
- **Confidence:** CONFIRMED for the data; APPROXIMATION for every mapping (single fog layer, exposure key
  interpretation, colour balance -> white balance). DOF is opt-in (`--dof`) because Bevy also blurs the near
  field while Arkane's is far-only; film grain and bloom tint have no Bevy equivalent.
- **Implementation consequence:** Matching brightness needs original-game captures (U-011); the exposure can
  be tuned with `DISHONORED_EV_BIAS` / `DISHONORED_TONEMAP` without rebuilding.

## 2026-10-04 — Skydome, moon and backdrop cards
- **Question:** What forms the sky and the distant city, and why did it look wrong?
- **Evidence:** `StaticMeshCollectionActor_32[99]` = `Skies.SkyDome_Multi_UV` (bounds +-205,852 uu, height 123,511,
  centred at (35379, -6871, 1793)) with `Skies02.BOYLE.Boyle_2_inst` (base `Skies.Skies_PMAT`, MLM_Unlit:
  gradient top/bottom, two scrolling cloud layers, horizon glow; `D_Diffuse_color` 0.07). `StaticMeshCollectionActor_33[0]`
  = `Skies.Mesh.Stm_SunDome` scaled ~4000x at (27976, -127954, 73119) with `Moon_Boyle_INST` (base
  `Vfx_Materials.main_materials.Moon_PMAT`, translucent, `Moon_Color` (0.95, 0.95, 1) x 2, texture only as a base
  default). Backdrop: `Boyle_vista_buildings_*`/`Boyle_vista_far_*` cards (up to ~83,000 uu out, masked, lit windows
  in the G channel of the `_o` mask, see Cross-package materials) plus `vista_buildings_01.*` meshes. The far edge of
  the dome is ~244,000 uu (4.9 km Bevy) from the estate, close to the old 6 km far plane.
- **Experiment:** Sky materials (base `Skies.*`, `Moon_PMAT`, `Sun_PMAT`) render unlit, unfogged, double-sided,
  without lightmaps or shadow casting; the moon is additive with `Moon_Color` x intensity; sky dome colour =
  texture x (D_Diffuse_color + mean gradient). Far plane 12 km. Translucent FX no longer cast shadows.
- **Result:** The moon appears as the large pale disc behind the estate and the cloud dome is visible above
  the fog. `harness_out/step5_nosky/{moon,skyline}.png` (before) vs `harness_out/step5_sky/{moon,skyline}.png`
  (after, side by side in `harness_out/step5_sky/sky_sheet.png`); full views `harness_out/step5_sky/sheet.png`.
- **Confidence:** CONFIRMED for which meshes/materials form the sky; APPROXIMATION for their shading (no cloud
  scrolling, no horizon glow, moon uses the material's default cloud texture instead of the real moon shader).
- **Implementation consequence:** A custom sky material is needed for the gradient/cloud animation. The clear
  colour stays the old dark blue: the sky gradient's bottom colour rendered as saturated cyan in gaps below the
  dome rim.

## 2026-10-04 — Fidelity harness
- **Question:** How do we keep before/after evidence for every visual feature reproducible?
- **Experiment:** `harness fidelity` (`tools/harness/src/fidelity.rs`): one `dishonored_runtime --inspect --script`
  run per feature config (full, baseline, no-lightmaps, no-materials, no-fx, no-dynamic-lights, no-fog, no-post,
  no-sky, legacy-geometry), 8 fixed viewpoints each (aerial, street, garden, spawn1, mansion, sewer, moon,
  skyline), `feature-off | full` pairs composed with ffmpeg, REPORT.md with an image table. `--reference DIR`
  computes SSIM against original captures named `<viewpoint>.png` (hook for U-011; none exist yet).
- **Result:** `harness_out/fidelity_run/REPORT.md`: 11/11 checks pass, 80 renders, 72 pairs.
- **Confidence:** CONFIRMED (tooling).
- **Implementation consequence:** Once original-game captures exist at the same poses, the SSIM table turns the
  approximations above (lightmap formula, light falloff, exposure) into measurable targets.

## 2026-10-04 — Frame time
- **Question:** What do the fidelity features cost?
- **Evidence:** HUD fps after 35 s of the offline game (960x540, 1 bot, same spawn), `harness_out/perf/*/t35.png`:
  baseline 160, full renderer with 208 separate lightmap atlases 18, `--no-lightmaps` 81. After shelf-packing all
  atlases into one 4096x4096 RGB9E5 page (`Importer::pack_lightmap_pages`): full 31-34, `--no-dynamic-lights` 69,
  `--no-materials` 33. Disabling all shadows (`DISHONORED_NO_SHADOWS=1`) did not measurably help (27); readings
  vary by roughly +-10 fps between runs (other GPU work on the machine).
- **Result:** Packing gave ~1.8x. The dominant point light's cube shadow is disabled and the moon uses 3 cascades.
  The remaining gap to baseline is not yet attributed (candidates: per-light clustered shading over ~4,400 entities,
  lightmapped + normal-mapped pipeline variants, env-map prefiltering, bloom).
- **Confidence:** LIKELY (noisy measurements, no GPU profiler run).
- **Implementation consequence:** Needs a proper GPU profile before shipping; merging static meshes per material
  would cut draw calls the most.

## 2026-10-04 — HUD rebuilt from the game's Scaleform movie
- **Question:** Can the in-game UI match the original without hand-drawing it?
- **Evidence:**
  - `UI_HUD_SF.upk` holds `UI_HUD.HUD`, an uncompressed GFx v8 movie (313 KB; stage 1280x720), plus 11 `Texture2D` atlas pages.
  - Tag 1009 (external image) uses 32-bit ids. Atlas pages are `0x0009_xxxx`, and tag 1008 (sub-image) names its page by the low 16 bits; truncating ids to 16 bits made the standalone images collide with the pages.
  - The root timeline places `masterHUD_mc` at (95.5, 54). Inside it, `_healthMeter_mc` is rotated 15° and `_manaMeter_mc` 30°, both drawn through sliding `mask_mc` layers with `upgd2/4/6` labels, beside `_equipment_mc`. It also places `_dot_mc` at centre (with exported `crosshair_gun/_crossbow/_power`), `playerStatesIcon_mc` bottom-left, and `objectives_mc`/`targetNotification_mc` top-right.
  - `DisFonts_SF` `fonts_efigs` embeds "Emerge BF" and "ChaletComprime-CologneEighty" as `DefineFont3` (188 glyphs each).
- **Experiment:**
  - `dishonored_assets::swf` parses shapes (bitmap fills), sprites, display lists, exports and external images, and flattens a sprite into textured quads.
  - `swf_render` CPU-rasterises the quads over a screenshot for checking.
  - `swf_font` converts DefineFont3 outlines (quadratic, 20480-unit EM) into an in-memory TrueType font. I validated it with FreeType (ffmpeg drawtext).
- **Result:** The runtime draws the real gauges and their glow, the equipment emblem with the left-hand item icon (`UI_ItemIcons_Large`, `UI_Powers_Large`) and ammo, the item-specific crosshairs, the crouch icon, and the objectives banner and target paper, all in the game's fonts.
  - Health and mana fills are cropped along the shard like `mask_mc`.
  - The objectives banner carries mode, clock and score.
  - The target paper carries the Contracts clues. It is offset under the banner; in the game the two never show together.
  - The debug text moved behind F1.
- **Confidence:** CONFIRMED (layout, textures and fonts come straight from the movie). Script-driven animation (refill flashes, upgrade lengths, the stealth indicator) is not reproduced.
- **Implementation consequence:** `crates/dishonored_runtime/src/gfx_hud.rs`. Nothing from the movie is written to disk or committed; research renders go to `harness_out/` only.

## 2026-10-04 — Audio containers and codecs
- **Question:** How is the game's audio stored, and which codecs must be decoded?
- **Evidence:** 503 `.pck` files (base + DLC) start with `AKPK`; each holds one soundbank (BKHD version 65)
  and a stream table. `audio_dump survey`: 3,265 embedded + 1,908 streamed media files, all RIFF/WAVE with
  format tag `0xFFFF` (Wwise Vorbis), 0x42-byte `fmt ` with the `vorb` block at +0x18; no PCM/ADPCM/XMA.
- **Experiment:** AKPK/bank/WEM readers in `crates/dishonored_audio`; layouts in `dishonored/AUDIO_FORMAT.md`.
- **Result:** Every package and bank parses; codec survey as above.
- **Confidence:** CONFIRMED
- **Implementation consequence:** Only Wwise Vorbis needs a decoder.

## 2026-10-04 — Wwise Vorbis codebooks come from the game executable
- **Question:** The setup headers reference external codebooks by 10-bit index (44 codebooks in a 228-byte
  setup). Where can the codebook library come from without third-party files?
- **Evidence:** The Wwise runtime is statically linked into `Dishonored.exe`. `exescan` finds a run of 599
  ascending pointers at file offset 0xFAEE78 (`.data`) into `.rdata` (VA 0x011D89F8..0x011EA32F); expanding
  each target with the packed-codebook grammar consumes exactly the pointer stride for all 598 codebooks.
- **Experiment:** `CodebookLibrary::from_exe_bytes` locates the table by content at runtime; the stripped setup
  and modified audio packets are rebuilt to standard Vorbis and decoded with `lewton`.
- **Result:** `audio_dump decodeall --streams`: 5,173 / 5,173 files decode, 0 silent or non-finite,
  1,083.7 min of audio; decoded lengths equal the `vorb` sample counts. Spectrograms of sword sounds show
  structured metallic partials (`harness_out/audio/sword_spec.png`).
- **Confidence:** CONFIRMED
- **Implementation consequence:** Audio decodes from the install alone; nothing is downloaded or shipped.

## 2026-10-04 — Event ids and names
- **Question:** How are events named, and how do game objects reference them?
- **Evidence:** `STID` only names banks. 9,821 `AkEvent` exports (1,553 distinct names) in the base packages,
  each with `RequiredBank`. `ak_survey`: FNV-1 32-bit of the lowercased name matches a bank event id for
  1,553 / 1,553 names.
- **Experiment:** Name -> id by hashing; event index over all banks (HIRC only).
- **Result:** Player/combat events (`Snd_Weapon_Sword_Whooshe_ak`, `Snd_Power_P_Blink_Start`, ...),
  per-surface footsteps (`FS_P_<Surface>_{Sn,R,Sp}`, `_Fall_{Small,High}`), impacts
  (`Imp_<Projectile>_on_<Surface>`) and level ambience (`Play_Amb_LVL_Boyle_Estate`, `Play_Music_Boyle`) are
  addressed by name in `dishonored_audio::cues`. L_Boyle_Ext places 89 `AkAmbientSound` emitters
  (`PlayEvent` + `Location`), all of which resolve.
- **Confidence:** CONFIRMED (hash, names); the SimEvent -> event choice is a design mapping (see AUDIO_FORMAT.md).
- **Implementation consequence:** No name table has to be stored; ids are computed from names.

## 2026-10-04 — HIRC v65 resolution
- **Question:** Can events be resolved to playable media without a complete v65 HIRC parser?
- **Evidence:** Play actions target sounds/containers; every node stores its direct parent after the FX block;
  Sound sources give media id and stream type; SetState actions carry (group, state); switch containers
  store group id and default switch before the children; music switches end with (state, child) pairs.
- **Experiment:** `dishonored_audio::hirc` decodes those fields; children come from inverting the parent links;
  `audio_dump event` prints the resolved tree and writes picks to `harness_out/audio/`.
- **Result:** All 29 mapped player/combat cues resolve to decodable sounds (147 of 149 postable events including surface variants; the two missing footstep variants fall back to stone); `Play_Amb_LVL_Boyle_Estate` with
  `Set_State_Amb_LVL_Boyle_Ext_Streets` selects the 78 s exterior street bed; `Play_Music_Boyle` with
  `Music_State_Boyle_Explor` selects the exploration branch. Volumes (dB, summed along the parent chain),
  pitch offsets and attenuation curves (e.g. footsteps 3,000 uu, sword whoosh 2,000 uu) are read.
- **Confidence:** LIKELY for the layouts (validated by consistency across all banks, not by a reference parser);
  container play modes, randomizer ranges, RTPCs and loop counts are not decoded (U-029, U-030).
- **Implementation consequence:** Random containers are played as weighted random picks, sequence containers
  as random, switches by state when known else by default switch; ambience re-picks when a pick ends.

## 2026-10-04 — Runtime audio
- **Question:** How should simulation events become sound, presentation-only?
- **Experiment:** `crates/dishonored_runtime/src/audio.rs`: worker-thread load + decode, `SimEvent` mapping,
  movement-generated footsteps (surface from the colliding mesh name), local player 2D, others 3D with
  bevy_audio panning and the Wwise dry-volume curve, level bed/score and placed emitters (culled beyond their
  attenuation range). `--no-audio`, `--volume V`.
- **Result:** `harness audio` passes (decode sanity per codec class, every cue resolves, Boyle ambience).
- **Confidence:** APPROXIMATION for footstep surfaces, stride lengths, ambience looping and emitter selection.
- **Implementation consequence:** Original-game captures (U-011) are needed to tune footstep cadence/levels.

## 2026-10-05 — Frame time: lightmap slots, render views, batching
- **Question:** Why did the full renderer drop to about 30 fps (baseline 165)?
- **Evidence:**
  - Bevy `RenderDiagnosticsPlugin` GPU timings reported under 1 ms for the instrumented passes, so the time was elsewhere.
  - A `trace_chrome` CPU trace, summarised by `trace_summary`, showed per frame:
    - `prepare_mesh_bind_groups`: 15.7 ms unmerged, 0.4 ms with `--no-lightmaps`. Bevy allocates a lightmap slot per entity (binding arrays of 4) and rebuilds those bind groups every frame.
    - Three render views per frame: the world camera, the first-person overlay camera (about 3 ms, including its own directional shadow cascades) and the environment-map view.
  - fps readings from unfocused harness windows vary a lot between runs (window/driver throttling); like-for-like traces were used for attribution.
- **Experiment:**
  1. Merged static opaque/masked geometry into world-space batches (material x lightmap page x 4096 uu cell; 3,754 instances -> 1,232 batches). Lightmap regions are folded into UV1 and mirrored instances have their winding flipped.
  2. Replaced Bevy's per-entity `Lightmap` with a material extension (`BakedLightmap`, `lightmap_ext.wgsl`): the standard PBR forward fragment plus one shared atlas sampled on UV1 into `pbr_input.lightmap_light`.
  3. Drew the first-person rig in the world camera, scaled by 0.2 about the eye. Uniform scaling about the projection centre leaves the image unchanged. Near plane 0.005.
  4. Built batch meshes and tangents in parallel.
- **Result:** About 30 -> 82-90 fps in the same (unfocused) conditions; startup 12.4 s (unmerged 11.5 s). The renders compare equal to Bevy's own lightmap path, the arms are lit by the level lights, and translucent, sky and debug geometry stay per instance.
- **Confidence:** STRONG for attribution (traces). Absolute fps needs a focused-window measurement.
- **Implementation consequence:** `DISHONORED_NO_MERGE=1` keeps the per-instance path for comparison, and `--profile` logs fps and render-pass timings. The extension path does not compile Bevy's `LIGHTMAP` define, so global ambient also reaches baked meshes. Ambient is 60 with level lights, so the effect is small.

## 2026-10-05 — How Dishonored builds missions; custom mission framework
- **Question:** How are the original missions authored, and what should a custom mission framework look like?
- **Evidence:** `kismet_dump` / `mission original` on the Boyle packages:
  - `DishonoredObjective` "Assassinate Lady Boyle" has 27 `DishonoredTask_Custom` tasks (initial state, hidden flag). They include the costume identification step, lethal and non-lethal variants per sister and dress, optional clues and the escape.
  - Kismet (976 nodes in 13 sequences in `L_Boyle_Ext_Script`) drives them with `DisSeqAct_SetTaskState` / `SetTaskHidden` / `UpdateTaskTarget`, story-flag sets (`SF_LadyBoyle`, GUID flags), spawners, AI direction actions with Success/Interrupted outputs, touch triggers on named actors, remote events, delays and gates.
  - Details in `research/dishonored/MISSIONS.md`.
- **Experiment:**
  - Built `dishonored_mission`: RON missions with objectives/tasks, flags, counters, markers (original actor names, sublevel-qualified when ambiguous), mission NPCs and `on/when/then` rules, plus a deterministic interpreter (rules cascade within a step like Kismet within a frame).
  - Added a mission game mode, HUD and authoring tool, and the sample mission `missions/boyle_masked_host.ron`.
  - `harness mission` plays the sample mission with a scripted agent.
- **Result:**
  - Interpreter tests pass (5).
  - `harness mission` passes 7/7: markers resolve (8 markers, 6 NPCs, crowd of 21); informant at 1.6 s, host killed at 94.9 s, mission complete at 108 s; two runs identical (hash and tick); a dedicated server hosts the mission and the client receives objectives and messages.
  - Level actor names repeat across sublevels (e.g. `TargetPoint_56` in two packages), so references must be package-qualified; `mission check` reports these.
  - Packages could appear twice in a map's package list, which duplicated NPC data; extraction now reads each package once.
- **Confidence:** CONFIRMED (structure read from data; harness).
- **Implementation consequence:** Conversations, pickups, possession and cutscene matinees from the original model are not implemented yet; chaos is a kill-ratio approximation.

## 2026-10-05 — Menus, settings and the launcher
- **Question:** How are the original main, pause and options menus built, where do their texts
  and settings come from, and can the port reproduce them from the install?
- **Evidence:**
  - `UI_MainMenu.MainMenu` (in `Dishonored_MainMenu.upk`) imports `o_OptionsScreen`,
    `o_videoSettings`, the gamma and pad screens from `UI_OptionsMenu.OptionsMenu` (`Startup.upk`).
    `SwfMovie.ReferencedSwfs` and the GFx `ImportAssets2` tags name them.
  - Widgets carry state labels (`default`, `over`, `selected`, `unselected`, `bind_in`, ...).
  - Every text field holds a placeholder. The labels are in `Settings.int`, `Twk_InGameUI.int` and
    `DishonoredGame.int`.
  - Settings are `Engine.ArkProfileSettings`. Its class default object holds 113 defaults and
    124 value mappings (types, ranges, value-label keys).
  - The menu level `Dishonored_MainMenu` imports with the map importer. Its matinee cameras
    share one main pose.
  - The UI sound theme is `Twk_InGameUI.UISoundTheme_Common` -> `Bank_UI` events. The menu
    music is `Musics.Music_Menu`.
  - Details: `dishonored/MENUS.md`.
- **Experiment:**
  - Extended the SWF reader additively: `DefineEditText`, ImportAssets, DefineButton2, font ids,
    solid-fill shapes, label listing. `swf_dump --texts` prints them.
  - Added a UE3 localization reader (`dishonored_assets::loc`).
  - Built the `dishonored_settings` crate (every original option with its type, range and
    default; RON file).
  - Built a launcher and pause menu in the runtime that draw the movies' sprites at their
    state frames and fill their fields.
  - `harness launcher` drives it through a control socket.
- **Result:**
  - Main menu over the 3D menu level, with the menu music and UI sounds.
  - Screens: Play, Missions, Multiplayer (browser and server creator), Options (4 tabs,
    7 lists, video / brightness / pad screens), and the in-game pause menu.
  - Ids 29-133 map to `PSI_*` by order. Each group's types and ranges match its labels.
  - `harness launcher`: 14/14 checks. These cover screenshots of 15 screens, settings
    persisted, a launched game reading the new FOV / invert / rebound key, the pause menu, and
    hosting, browsing and joining a local server through the launcher.
- **Confidence:** CONFIRMED for structure, texts, sounds and defaults (read from data); LIKELY for
  the id -> `PSI_*` correspondence; APPROXIMATION for layout computed by ActionScript (list
  rows, button bar spacing), the static menu camera (matinee keys not decoded) and colour
  transforms with additive terms.
- **Implementation consequence:** `dishonored_runtime` without arguments opens the launcher.
  Games run as child processes with `--from-launcher`. Unimplemented options are saved and
  marked "not yet effective". The subtitle mode is available to the dialogue HUD through the
  `GameSettings` resource.
