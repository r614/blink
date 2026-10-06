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

## 2026-10-05: Level travel between persistent maps (Boyle Ext <-> Int)

- **Question:** How does the original move the player between persistent maps, and can the port do the same from the data?
- **Method:** `mission travel <map>` (new `dishonored_assets::travel`) walks each `DisSeqEvent_Used` in the map's Kismet (output links, remote events to their listeners, sub-sequence activation) to the `SetPlayerTravelDestination` tag and the `ChangeLvl_*` remote event, then follows that event in the global `DishonoredGameFull_P` Kismet to `SeqAct_PrepareMapChange(MainLevelName)`. Door prompts are the localized `m_InteractText` of each door's `m_pUsableObjTweaks` (`Localization/INT/<Package>.int`, read at runtime).
- **Findings:**
  - Boyle Ext has 4 travel doors (`DishonoredUsableObject_5..8`), Int has 4 (`_10..13`); Ext has 5 tagged `PlayerStart`s, Int 4. Example: `_7` (`LTD_Garden_to_Ballroom`, "Press `k to go to Boyle Mansion Ballroom") -> `PlayerStart_Ballroom` via `ChangeLvl_fromBoyleExterior_toBoyleInterior` -> `L_Boyle_Int_P`.
  - Ext and Int share one coordinate frame, so a renderer must hide the level being left.
  - Actor names repeat across the two maps (`DisGuardWatchPoint_5`), so each map is its own actor namespace; unqualified names resolve on the start map first.
- **Implementation:** `MissionMatch` saves the level state of the map being left (NPC states and brains, as `DisSeqAct_SaveLevelState`), restores or freshly populates the destination, places players at the tagged start, and emits `Arrived`. The host swaps the active collision map (`MapSet`), `MatchView.map` tells clients, and the runtime shows a loading overlay, spawns the level on its first visit, and re-shows it on later visits.
- **Validation:** `harness travel` 7/7: Ext -> `PlayerStart_Ballroom` on Int (0 uu), standing on the Int floor, Int doors usable, back through `_13` -> `PlayerStart_FrontDoors` on Ext, and an NPC killed before leaving is still dead on return (26/26 NPCs kept). A rendered client run through `_7` shows the loading overlay, then the interior with the original return-door prompt.
- **Confidence:** CONFIRMED for the travel table and the arrival points. The story-flag gates on doors (for example "Mansion Second Floor (Forbidden!)") are not evaluated yet (U-032).

### Addendum: travel doors confirm first (original `DisSeqAct_DialogScriptedChoice`)
- The Kismet chain behind each Boyle door goes `DisSeqEvent_Used` -> `DisSeqAct_DialogScriptedChoice`. Its first option posts `Snd_FixInt_AP_Door.Door_Metal_Level_Transition` and calls `SetPlayerTravelDestination`. The chain then runs "Turn off the Fireworks" -> "bulletproofkillinvite" -> `DisSeqCond_CheckStoryFlag(SF_Global)`. True leads to `ChangeLvl_*`; False leads to a `SeqAct_ConsoleCommand` (the standalone-map path; the same flag gates "Add Boyle Party Objective" and the autosave, so it reads as "running inside the full game").
- Option text is localized per level package: `L_Boyle_Ext_P.int` `[TheWorld:PersistentLevel.Main_Sequence.DisSeqAct_DialogScriptedChoice_2 ...] m_Choices[0]=(m_ChoiceText="Go to Boyle Mansion Ballroom")`, `m_Choices[1]=(... "Stay in the Garden")`.
- Port: `mission travel <map> --chain` prints the chain. Doors now open the choice (`MissionView.choices`, picked with the `CHOICE_n` buttons, number keys on PC); option 1 posts the door sound and travels. `harness travel` checks both "stay" and "go".

## 2026-10-05: Conversation runtime (original DisConversation graphs, VO, choices)

- **Question:** Can the original dialog graphs run in the port as data, with their subtitles, VO and choices, and how do their nodes behave?
- **Method:** New crate `dishonored_conversation`: a loader for `DisDialogOneShot` / `DisDialogTree(_OneShot)` / `DisConversation` / `DisConv_*` / `DisDialogVoiceData*` in cooked level packages (story-flag GUIDs named from `Startup.upk` `DisStoryFlagSet`, soiree matinees linked by `m_MatineeGUID`), VO lengths from the per-voice `Bk_<GUID>.INT` banks (`dishonored_audio::vo`, header-only `media_length`), and a deterministic runner. Research CLI `conv stats|list|flags|play|findevent`.
- **Defaults:** cooked objects omit values equal to the class default object (`Default__DisConv_*` in `DishonoredGame.upk`): `DisConversation.m_bIsBlocking = true`, `m_Priority = HIGH_PRIORITY_CONV`, `m_fPlayerNearRadius = 500`; `DisConv_Blurb.m_bMainSubtitle = true`, `m_fVoiceRadius = 3000`, `m_SpokenTo = -1`; `DisConv_SetStoryFlag.m_bValue = true`; `DisConv_TimeLimit` 30 s, global; `DisConv_RandomBranch.m_bTryKeepUnique = true`; `DisConv_Hook_Distance` 200/500/1000.
- **Findings:**
  - Output link names are not in the data (every `m_Description` is empty; the node classes are native) and are not among the exe strings. Condition polarity was taken from the data: output 0 = false / not seen / not possessed, output 1 = true. Evidence: Miss White's `Hits_On_Player` sends `SeenDialogLabel("youreAscandal")` output 0 into the line that carries that label and output 1 to the follow-up line; `Idle_With_Jack` only runs through output 0 of `CheckStoryFlag(BLoyle_JackWhiteDone)`, the flag `Hits_On_Player` sets right before "Jack Goes Packing"; the doorman's `IsPossessed(player)` output 0 leads to "InvitationPlease"; his use hook reaches "doing my job" only once `GaveInvitationtoGuard` is set. LIKELY (U-034).
  - `DisConversation` outputs are `Start`, `Interrupted`, `Player Walked Away` (exe strings; `Idle_With_Jack` wires output 2, with `m_bPlayerMustBeNear` / `m_fPlayerNearRadius`).
  - `DisConv_Soiree`: output 1 continues while the matinee plays, output 0 runs when it finishes ("Stair Warnings": output 1 = the warning line, output 0 = `backstair_goes_walking`). LIKELY (U-037).
  - `DisConv_Hook_KismetActivated.m_KismetDialogHooks` names map 1:1 onto its output links (doorman: 9 names, 9 outputs; "Come In" -> `ConversationRef_1` -> `Come In`), confirming `DisSeqAct_DialogInputs` input i -> hook output i. `DisDialogTree.m_DialogHooks` is indexed by `m_iHookIndex` (`DDH_PLAYER_USED` = 1, `DDH_PLAYER_APPROACHES` = 2, `DDH_KISMET_ACTIVATED` = 10). `RandomSequentialBranch` exe strings include "Depleted"; `SpeakerInStoryGroup` and siblings end with "Everyone Else".
  - Brisby: `Lord_Brisby_Quest_Start` is started by his `DDH_PLAYER_APPROACHES` hook (close distance 250, `NOT_INTERRUPTABLE_CONV`) -> `IsPossessed(initiator)` output 0; `Brisby_Boyle_Invite` by his use hook once `Boyle_BrisbyInPosition` is set (`CheckStoryFlag` -> `SequentialBranch` [invite, nothing]). This resolves the open start conditions noted in BOYLE_MISSION.md section 8.
  - Unreachable nodes exist: Esma_KO's `KismetActivateRemoteEvent("Player_Takes_Brisby_Quest")` has no incoming link (the quest is driven by the soiree matinee's Kismet), and Samuel_Intro's two blurbs whose events exist in no `Bk_*` bank (all six languages) are not linked into its flow (cut lines, U-039).
  - Coverage, lines inside conversations / with a decodable VO clip: L_Boyle_Int_Script 415/408 (116 conversations, 28 one-shots), L_Boyle_Ext_Script 87/86, L_BoyleStreet_Ext_Script 163/157, L_Boyle_Ext_P 19/18, L_Boyle_Int_P 16/15. Every blurb carries a voice-table event; the unresolved ones are events absent from their bank.
  - HUD movie: the `Subtitles` class (`SetSubtitles(NPC_NameStr, subtitleStr)`) references `_NPC_Name_txt`, but `subtitles_mc` only places `_txt` ($NormalFont 24, #e8f7db, centred, letter spacing 0.5), so the shipped HUD shows no speaker name. `PlayerChoice` is a vertical, bottom/left-aligned `GenericMenu` of `hud_plC_button`s; the button spacing is set by script (34 px used).
  - No non-English blurb text: `DisConv_Blurb.m_PerLanguageData` / `DisPlayerChoiceLocalized` exist in the classes, but the cooked packages carry English only and there are no per-language script packages (U-040).
- **Implementation:** `MissionState` owns a `Runner`. Actions `Conversation(ref, [npc..])` / `StopConversation(ref)`, triggers `ConversationOutput(name)` / `ConversationEnded(ref)`, `NpcDef.talk_conversation` (use hook or a conversation), `NpcDef.speaker` (original spawner -> speaker binding), approach hooks, story flags shared with the mission flags by `SF_*` name. The current lines and choice are replicated in `MissionView.dialogue` / `.choice`; the choice also joins `MissionView.choices` (the shared `CHOICE_n` input; the chooser stands still). The host loads the graphs (`dishonored_content::load_mission_conversations`, cached under `cache/`) into `MissionSetup` (stored in replays). Runtime: `hud_dialogue.rs` draws subtitles in `subtitles_mc` and choices in `playerChoice_mc`; `dialogue_vo.rs` plays each line's VO 3D on the speaker's pawn (Wwise attenuation curve) or 2D.
- **Validation:** `harness conversation` 17/17: 81 runs over every conversation of 8 key one-shots (both choice policies), 114/114 reached lines with text and decodable VO, 21 choices, required outputs and flags, flag-check polarity; the mission end to end (doorman refuses, Lydia's choices through the number keys, Miss White's output and flag reach the rules, Brisby's approach then use hook, "Come In" completes the mission), view replication and determinism; a rendered client capture with subtitle and choice screenshots and 3D VO.
- **Confidence:** CONFIRMED for the data model, hook/name mapping and VO resolution; LIKELY for condition and soiree output order; timing details and concurrency are APPROXIMATIONS (U-035..U-038).
## 2026-10-05 — Original navigation is a UE3 navmesh (replaces the invented waypoint graph)
- **Question:** What is in the `L_*_Nav` packages, and can bots/NPCs path on it instead of the invented AI-point graph?
- **Evidence:** `L_Boyle_Ext_Nav` holds one `DisPylon` (Arkane `Pylon` subclass) with two `NavigationMeshBase` exports (walkable mesh and obstacle mesh), `NavMeshBoundsVolume`s and the empty `DisPatrolManager`/`DisFleePointManager`. No PathNode/ReachSpec network anywhere. `ENavMeshEdgeType` values read from `Engine.upk`. Path-object GUIDs match `DisDoor.NavGuid` and `DishonoredBreakableNavBlock.m_NavGuid`.
- **Experiment:** Decoded the native serialization step by step (`nav_dump`, `enum_dump`); format in `research/dishonored/NAVIGATION.md`. Reader `dishonored_assets::navigation`, IR `dishonored_ir::nav`, pathfinding `dishonored_world::nav` (poly A* + funnel, door/blocker gating). Survey of the whole game; real-map test `crates/dishonored_content/tests/real_nav.rs`.
- **Result:**
  - 40/40 pylon packages decode with exact byte accounting (navmesh version 28). Edge classes: base, basic one-way, cross-pylon, path-object; types used: Normal, Removed, PathObject only. No mantle/drop/jump edges exist in any shipped mesh.
  - Boyle Ext: 2120 verts, 984 polys, 1345 edges, 22 path objects (8 doors, 14 plank blockers). Polys sit on the collision floor (median +3 uu). 8 islands: the estate (353 polys) and the street/approach (554 polys) share no edge.
  - Same-island paths: 100% ignoring path objects, 90% with level-start lock states; 38 ms for ~2000 queries.
  - `nextNavigationPoint` on `DishonoredNavPoint`s is the engine's flat point list, not patrol links.
- **Confidence:** CONFIRMED (layout, sizes, cross-references); UE3 field names LIKELY; two cross-pylon fields UNKNOWN (U-046).
- **Implementation consequence:** `CollisionWorld` carries the map's `NavWorld` (cached `cache/<map>.nav.bin`); Contracts/FFA bots, offline session bots, the harness mission agent and NPC brains path on it through `WorldQuery::nav()`. The previous `NavGraph` (AI points + raycast and drop edges) is gone; bots no longer take drops the original data does not have.

## 2026-10-05 — NPC perception and behaviour from Arkane's AI tweaks
- **Question:** Which original data drives NPC perception and reactions, and how close can `dishonored_ai` follow it?
- **Evidence:** pawn tweaks (`Twk_Pawn_Boyle_Party.*`, level script packages) -> brain tweaks (`AI_BrainTweaks_Elite.BrainTweaks_Guard_Elite_Party`, `AI_BrainTweaks_Civilian.BrainTweaks_Civilian`) -> attention process -> `DisTweaks_PawnAttention` profiles (`Twk_Attn_Guard_Unsuspecting/Suspecting`, `Twk_Attn_Default_Neutral`, `Twk_Attn_Noncombatant_*`), `Twk_Vision_Default`, behaviour/sub-state tweaks (Search, Panic, Combat, TriggerAlarm, Guard), `DefaultAI.ini` (noise loudness table, reaction delay, search crumbs, skirmish caps), `DefaultNPC.ini`. Enums via `enum_dump`. Arkane tweak inheritance (`m_FallbackChainCooked`, `m_FallbackSkip`) now resolved by `TweakDb`.
- **Experiment:** `AiConfig` loader (`dishonored_assets::ai_config`, retail test checks it against the documented values); brains rewritten around per-stimulus attention with the original thresholds/leaks/increases, vision cones, noise contexts, reaction delay, Notice/Investigate/Search/Combat/Panic behaviours, skirmish caps, flee points and help barks (`research/dishonored/AI.md`). Harness runs before (`harness_out/baseline_*`, binaries built at b23dec7) and after.
- **Result:**
  - Party civilians panic on AlarmBell/Help/DeathRattle noises, not on gunshots by ear (raw `m_ScaryNoises` arrays), plus seen enemies, deaths, corpses, fighting guards and fleeing guests.
  - Players are neutral guests until seen attacking (neutral profile only head-tracks).
  - Harness (3-minute Contracts, same seeds): contract kills 10 -> 7, pursuits 14 -> 9, guard-combat ticks 4,565 -> 13,685, civilian-flee ticks 13,792 -> 206,173 (panics last until attention leaks below the Investigate stop: 0.015-0.025/s), NPC deaths 1 -> 5. Mission: completes (host at 13 s, done at 23 s) but the assassination is now witnessed (wider original outer cone 160 deg / 1700 uu): detected 7, alarms 2, one agent death.
- **Confidence:** data CONFIRMED; vision-rate combination SPECULATIVE (U-041); no-amount increase semantics LIKELY (U-042); patrol topology, attack patterns, alarms OPEN (U-043, U-044).
- **Implementation consequence:** `SimConfig.gameplay.ai` (cache `gameplay_config.v2.json`) feeds every crowd at match start; mission `Alert`/`Calm` commands go through the attention model.
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

## 2026-10-05: Decompiling the original executable (Ghidra) for travel, choices and conditions

- **Method:** Ghidra 12.1.4 headless analysis of `Binaries/Win32/Dishonored.exe`. Scripts are in `research/ghidra/`; the project and decompiled output stay outside the repo.
  - `NativeTable.java` labels the 2,560 native `exec*` functions from the engine's `FNativeFunctionLookup` tables. Names carry their class prefix, e.g. `UObjectexecGetEngineVersion`; 626 belong to Dishonored classes.
  - `ClassDecomp.java` finds a class's static registration record (it points at the UTF-16 class name), its InternalConstructor and vtable, and decompiles the slots that differ from the parent class.
  - Vtable slot 93 (+0x174) is `Activated()` for sequence ops, slot 92 (+0x170) is the latent `Update()`, and slot 80 (+0x140) is a conversation condition's `Evaluate()`.
- **Findings:**
  - `DisSeqCond_CheckStoryFlag::Activated` fires output 0 when the flag is set and output 1 when it is not. Output links live at +0xAC with a 0x38 stride and the impulse bit at +0x18. This matches the data's link names ("True", then "False").
  - `DisConv_CheckStoryFlag::Evaluate` returns 1 when the flag is set. Which output the conversation runner then fires is still open (U-034): the caller of +0x140 was not found in the conversation code range.
  - `DisSeqAct_SetPlayerTravelDestination` stores `m_Tag` **and the name of the outermost package (the map being left)** on the game object.
  - `DisSeqAct_GotoPlayerTravelDestination` teleports only when the current map differs from that stored map and a tag is set. It iterates actors for the one whose `Tag` matches, moves the pawn there, and takes that actor's rotation when `m_bUseDestinationTargetRotation` is set (class default **true**). It then clears the tag. The arriving map's own Kismet runs it from its "Loaded and Visible" / "State Restored" outputs.
  - `DisSeqAct_SaveLevelState` saves the current level, with a full (non-`m_bPartial`) save on the Boyle Ext <-> Int paths of the global Kismet (PrepareMapChange -> SaveLevelState -> CommitMapChange). `DisSeqAct_DiscardAllLevelStates` wipes all saved levels; the Kismet uses it on the path back to the main menu / leaving the mission.
  - `DisSeqAct_DialogScriptedChoice`:
    - Output 0 is "Player Busy" and output i+1 is "[choice i]"; the link names are built natively from `m_ChoiceText`.
    - `Activated` tries to open the choice UI. If that fails it sets `m_bFailed` and fires "Player Busy".
    - `Update` waits while the UI is open and fires the chosen option's output (no pick means it keeps waiting). A choice with `m_bDisablePlayerInputUntilMapChange` disables player input first.
    - The player sits in `StatePlayerMasterInScriptedChoice` meanwhile.
    - Boyle's doors set that flag on "Go to ..." and not on "Stay ...".
- **Port changes:** arrival now takes the PlayerStart's rotation (pitch reset) per the class default. The rest of the travel and choice behaviour already matched: travel commits within one tick, so no input window remains.

## 2026-10-05: Level population from the original spawners (Boyle Int)

- **Question:** What does the original put in a map on arrival, and how do guards follow the player between maps?
- **Method:** `mission spawners <map>` lists each `DishonoredSpawner` / `DisTravelSpawner` with its settings, read from the level packages. Class defaults come from `Default__DishonoredSpawner` (`m_bSpawnOnBeginPlay` true, `m_bCapableOfFleeing` true).
- **Findings (Boyle Int, 54 spawners):**
  - 33 spawn on begin play: party guests and servants (faction `Neutral_Civilian` / `Neutral_Guard`), guards (`UpstairsGuards`, `MainStair_Guard`, `overseer`, ...), the Boyle ladies (`LadiesBoyle`) and Brisby, Mrs White and Mr Ramsey, each with a squad name and `m_bPatrolUponStartup`.
  - 5 do not spawn on begin play: 4 `alarm_guards` with `m_SpawnAtSuspicionLevel = DAISL_Suspecting` (alarm reinforcements), plus a Kismet-spawned Lydia variant.
  - 16 `DisTravelSpawner`s carry `m_PlayerTravelLocationTags` (8 for `PlayerStart_Ballroom`, 8 for `PlayerStart_Foyer`), `m_SpawnAtSuspicionLevel = DAISL_Suspecting` and `m_bAwareOfPlayerUponStartup`. When the player arrives at that tag while the AI is suspecting, guards come through after them.
- **Implementation:** `MissionDef::original_npcs` lists the maps to populate this way.
  - First visit spawns the begin-play spawners. Corpse and knocked-out spawners lie on the floor. Patrol-on-startup decides whether the brain patrols.
  - Raising the alarm on a map spawns its alarm squads, already hunting.
  - Arriving through a door while a guard is investigating, searching or fighting a player spawns that tag's travel spawners, hunting.
  - Each spawner spawns once.
  - Characters still use the four presentation archetypes chosen by role; pawn tweak -> mesh resolution is open.
- **Validation:** `harness travel` 10/10. First visit: 33 NPCs, equal to the 33 begin-play spawners. Pursuit through the ballroom door: 8/8 pursuers present, all targeting the player.
- **Confidence:** CONFIRMED for the data. LIKELY for the trigger rules: "suspecting" is approximated from guard brain states, and how often a travel spawner fires per travel is not decoded (U-047).

### Addendum: UnrealScript decompilation (UE Explorer's UELib) and the travel spawner defaults
- `package_inflate` writes a package with its LZO chunks inflated and the compression flag cleared, to scratch space. `research/ghidra/uc_decompile.ps1` then drives UE Explorer's `Eliot.UELib.dll` to decompile its UnrealScript classes; all 2,284 classes of `DishonoredGame.upk` decompile. Most Dishonored logic is native, so the script mainly gives class layouts, defaults, script functions and the list of natives (`// Export UClass::execX`), which match the names `NativeTable.java` labels in Ghidra.
- `DisTravelSpawner` defaultproperties: `m_bSpawnOnBeginPlay=false`, `m_bCanSpawnWhenVisible=true`, `m_bCanSpawnWhenPlayerNear=true`; pursuers may appear in view and next to the player. The extraction now applies the class's begin-play default. `DisTravelSpawner` has no native overrides of its own (Ghidra: only the destructor), and its `DisTravelSpawnerComponent` keeps the resolved `m_PlayerTravelLocations`; the trigger is in native `DishonoredSpawner` / AI manager code (U-047).

## 2026-10-05: NPC looks from their pawn tweaks

- **Question:** What does an NPC look like in the original, given its spawner?
- **Method:** Resolve each spawner's `m_pPawnTweaks` (`DisTweaks_NPCPawn`) through Arkane's fallback chain (`TweakDb::resolve`, with the map's packages loaded, since `Twk_Pawn_*` level tweaks are cooked into them). Read `m_pSkeletalMesh` (body) and `m_RandomHeadMeshes` (head options, `m_pMesh` and `m_fRandomChance`), then find which map package cooks each `SkeletalMesh`.
- **Findings:** Boyle Int's 54 spawners resolve to bodies and heads. Examples: elite guards get `Skm_EliteGuard_Body` with one of `Skm_EliteGuard_Head`/`Head02`/`Head03`; party aristocrats get `skm_MaleAristo01_Body` with a mask face (`Npc_Masks.Primitive.skm_primitive_face`); Lady Esma Boyle gets `Npc_Masks.BOYLE.Skm_Boyle_Body` with `skm_Boylemask`.
- **Implementation:**
  - Appearances are cached per map in `cache/<map>.appearance1.json`.
  - Original NPCs get archetype `APPEARANCE_BASE + i`, indexing a look table that the host publishes in `MatchView::appearances`. The head is picked per pawn, deterministically and weighted by `m_fRandomChance`.
  - The table rides on every 60th snapshot; clients keep the last one.
  - The runtime loads looks on demand and binds the third-person Edge sets to them by joint name.
  - Per-material variations (`m_MaterialVariation`) are not applied yet.
- **Validation:** `harness travel` 11/11, with 33/33 interior NPCs on 19 tweak-resolved looks. A rendered capture shows an elite guard with his tweak's own head variant. `harness net` 8/8, mission 7/7, conversation 17/17.

## 2026-10-05: The original loading screen (`DisBinkOverlayManager`)

- **Question:** What does the original show while a level loads?
- **Method:** Read `DishonoredEngine` / `DisBinkOverlayManager` (decompiled UnrealScript class layouts) together with their config and localization.
- **Findings:**
  - Not a Scaleform movie. `DefaultEngine.ini [DishonoredGame.DishonoredEngine] m_MapConfig` gives per map a Bink movie (`m_LoadingMovieName`, `Movies/<name>.bik`; Boyle Ext/Int: `LoadingBoyle`) and `m_bShowLoadingTexts`.
  - `DefaultUI.ini [DishonoredGame.DisBinkOverlayManager]` lays out the texts as `DisBinkTextOverlayInfo`: position, wrap width, rotation in degrees, scale, colour. For example the map name sits at (105,109), rotated -3.42 deg, scale (0.8,0.96), dark, with a light shadow; hints at (106,550), wrapped to 1080, rotated -3 deg; "LOADING" right-aligned at (1180,625). It also sets `m_fHintDuration` (7 s) and `m_MapHintBindings` (Boyle -> hint set `Boyle`).
  - Text comes from `DishonoredGame.int`: `[MapNames]`, `[DisBinkOverlayManager]` (font names and `m_LoadingText`) and `[DisBinkOverlayManager_HintSet_<set>] Hint=`.
  - The fonts are UE3 bitmap `Font` objects in `UI_Loading_SF_LOC_INT`: `Emerge_BF` for titles and `ChaletComprime-CologneEighty` for text.
- **Implementation:**
  - `dishonored_assets::font` reads UE3 bitmap fonts: `Characters` (21-byte native `FontCharacter`), glyph pages, and the native `CharRemap` map.
  - `loading_screen.rs` plays the Bink through an ffmpeg pipe (RGBA frames at the movie rate) and draws the texts as glyph quads with the configured layout. Hints rotate every `m_fHintDuration`.
  - First visits import on a worker thread behind the screen.
  - An offline match does not tick while the screen is up.
- **Validation:** a recorded travel into Boyle Int shows `LoadingBoyle` with the map name on the painted banner and a Boyle hint in the lower band. `harness visual` 6/6, launcher 14/14.
- **Open:** the shadow offset of the map name (2 px assumed), and the save-notification and "press any key" variants (not used in travel).

## 2026-10-05: NPC material swaps (`DisSeqAct_NPCSetMaterials`): the Boyle dresses

- **Finding:** `DisSeqAct_NPCSetMaterials` has `m_NewBodyMaterials` and `m_NewHeadMaterials`, arrays of `DisMaterialSetting {m_MaterialIndex, m_pMaterial}`, with "Set" and "Clear" inputs. Boyle's interior setup applies to each sister body slot 0 `Npc_Boyle.Materials.Boyle_{red_inst, inst, black_inst}` and mask slot 0 `Npc_Masks.BOYLE.BoyleMask_{red,white,black}_inst`, all cooked in `L_Boyle_Int_Script`.
- **Implementation:**
  - Mission actions `SetMaterials { npc, body, head }` and `ClearMaterials(npc)`. The NPC is a mission NPC id or an original spawner name such as `DishonoredSpawner_11`.
  - A swap derives a new `Look` (parts plus material replacements) in `MatchView::appearances`, so it replicates like any look. Pending looks apply when an unspawned NPC spawns; Clear restores the NPC's own look.
  - The renderer resolves each material's package through an index over the session's map packages, loads the model with the overrides (`load_model_with`), and rebuilds the body when the look changes.
- **Validation:** `harness travel` 12/12. The new check: a Boyle lady goes to red and back. A rendered capture shows the sisters in red and black inside the mansion. Mission 7/7, conversation 17/17.
## 2026-10-05: Conversation runtime decompiled; runner follows the original

- **Method:** Ghidra decompile of every `DisConv_*` class (vtable overrides), the runtime classes (`DisConversationComponent`, `DisConversation_InGameData`, `DisDialogTree`, `DisConvGlobalMan`) and the whole dialog code range 0x00c86000-0x00cc0000 (`DecompRange.java`); the pawn's `DisConvSpeakerInterface` through `Vtables.java`; class layouts and defaults from the UnrealScript decompile. Full notes with addresses: `research/dishonored/CONVERSATIONS.md`.
- **Findings:**
  - Every node's `Evaluate` (+0x140) returns the index of the output to follow (or waits); the walker is `FUN_00ca3c10`. Conditions return 1 when true, so the runner's "false, true" order was right (U-034 resolved). `IsPossessed` has three outputs, `SpeakerCombatStatus` four, the suspicion check returns the level, group branches return the first matching entry or "Everyone Else". An index past the links ends the conversation as an error.
  - `m_Label` is recorded on any node that runs, per speaker and per conversation; `SeenDialogLabel` / `HadConversation` ask the memory of their `m_iSpeaker`, `ConversationFired` the global memory.
  - Random branches use percentage weights (`rand()*100/32767`); keep-unique moves a repeat to the output with the closest weight, and the stored last pick starts at 0. Sequential and random-sequential counters, random last picks and time limits written before a conversation has started stay pending and only commit when one starts. Random-sequential branches have `m_bLooping` (reshuffled on a committed wrap), "apply globally" only in trees no one-shot owns; time limits block while `now - last <= limit`.
  - Starting: `m_bPlayerMustBeNear` is a start condition (strictly within the radius of a speaker); speakers refuse while in an equal or higher priority conversation (the priority check applies to fresh starts from hooks) or in one started by an uninterruptible hook; hooks flagged "interrupts" override. A speaker is in one conversation at a time; the old one ends as "new conversation".
  - Ending for any reason but "finished" walks the `Interrupted` output: instant nodes run, the first line starts and plays on its own. "Player Walked Away" is checked each tick, once, for conversations that do not block the player; without that link the conversation ends (and runs `Interrupted`).
  - Lines hold until the speaker's Wwise voice stops, then `m_fBlurbEndDelay`; without a VO event nothing plays and no subtitle shows (U-036). Choices have no timeout; optional choices follow their story flag and map to outputs after the static ones. Soirees are skipped in non-blocking conversations; output 1 is the preview-blurb chain (played only with `m_UsePreviewBlurbsOnly`, else the matinee plays), output 0 always follows.
  - Hook table (`FUN_00c8d820`): fire group, refire delays and flags for all 126 hooks; refire is re-armed as `now + rand*(max-min)`; `DisTweaks_ConvSettings` defaults give the search/combat/patrol overrides and the fire-group caps. Approach hooks fire on inclusive band edges from the last distance seen inside the far edge.
  - Text: INT reads `m_Text`, other languages the single `m_PerLanguageData` entry of their cooked packages (U-040 resolved).
- **Port changes (`dishonored_conversation`):** new runner semantics above; `hooks.rs` (hook table, `HookClock`, `DistanceHook`); `Runner::fire_hook` for AI barks; `ConvEnv` gains possession, combat status, player combat, item counts, NPC group/spawner queries; the loader reads labels on every node, random weights, looping, optional choices, group entries with exact flags, `m_pConversation` targets, soiree preview flag, optional speakers and per-language text. Mission: approach hooks use `DistanceHook` (radial variant too), the view hides subtitle-less lines and shows lines started by an `Interrupted` branch, Kismet-style conversation starts no longer force their way in.
- **For the AI side:** call `Runner::fire_hook(HookRequest { npc, trees, hook: "DDH_...", output, initiator })` on brain transitions (e.g. search start -> `DDH_SEARCH`, combat threat -> `DDH_COMBAT_THREAT`, pain -> `DDH_COMBAT_OUCH_SMALL`); the runner applies refire delays and group caps. `ConvEnv::npc_matches` should answer story group / tweaks / faction membership of an NPC's pawn.
- **Tests:** 25 runner tests pin the decoded rules; `harness conversation` 17/17.
- **Confidence:** CONFIRMED for node outputs, walking, start/end/interrupt, timing, choices; LIKELY for the bark queue (fire-group gating is simplified) and soirees (matinee not played).
## 2026-10-05: Player mechanics from the original executable (movement, Blink, Windblast, parry, vitals)

- **Question:** How does the original compute player speeds, Blink targets and travel, Windblast, parries and regeneration, beyond the tweak constants already extracted?
- **Method:** Ghidra on `Dishonored.exe` with new helpers:
  - `ClassDecomp2.java` picks the primary vtable (Actor subclasses store native-interface vtables too);
  - `Decomp.java` decompiles by address or by global reference and prints the constants read;
  - `FNameMap.java` maps 1,111 hard-coded `FName` globals to their strings (attribute reads become legible);
  - `prop_layout` (tools/package_dump) recomputes UE3 property offsets (`UStruct::Link` rules), checked against interface thunks and known field reads.
- **Findings** (details and addresses in `research/dishonored/PLAYER.md`):
  - Speed is `GroundSpeed x MaxSpeedModifier` (0x00B56C40).
    - The controller (0x00AAE610) sets discrete flags by stick angle: strafing below 10 deg (forward) or 50 deg (back) from the side axis, backwards within 40 deg.
    - The strafe/back multipliers replace the base rather than blend; diagonal forward runs at full speed.
    - Sprint works in any direction above stick 0.8. The sword factor 0.9 applies to sprint too.
    - Speed drops blend at 8/s.
    - Walk and slow walk come from stick deflection (0.95 / 0.45).
  - Blink:
    - targeting is a range envelope, an extent sweep, close-obstacle offset retries, a crawl-box back-off and a ground lift;
    - travel moves 100 uu per frame without collision and restores the pre-Blink velocity and physics, then tries the Blink mantle (min edge height 0).
  - Windblast:
    - starts 0.1 s after the cast, and its front expands at 5000 uu/s;
    - factor `(1 - d/r)^2 x ((dot - cos)/(1 - cos))^2`, with a horizontal cone of half 67.5 deg;
    - push along the aim plus a distance-scaled up force; damage `round(30 k)`.
  - Parry: block age < 0.2 s (tap) or inside the block anim's parry zone, but not during the 0.3 s forced block after a tap parry. Block cone 50 deg.
  - Mana:
    - after a spend, a 20-point pool with a 3 s delay, then 1 point per 0.1 s (timer set, not added);
    - mana always refills to 20.
  - Health: below 10 only, 1 point every 0.03 s after a 10 s delay. Health and damage are integers.
  - Falling damage: `(int)(HealthMax x frac)`.
  - Slide: cosine ease from the sprint velocity to sneak speed over 1.1 s, ending on a 25 deg turn.
  - Crossbow bolt speed 20000.
- **Port changes:**
  - `dishonored_sim` movement, blink, left_hand (Windblast state in `WorldState`), combat (parry), vitals, slide.
  - New config fields with provenance (`slow_walk_speed`, `speed_blend_down_speed`, input thresholds, mantle face angles, Blink step time, slide).
  - Cache bumped to `gameplay_config.v3.json` and the fixture regenerated.
  - Tests for each decoded rule.
  - Harness expectations changed only where the original demands it (sprint 540, delayed Windblast).
- **Confidence:** CONFIRMED for the formulas read in code. Mapping onto player-vs-player (Windblast momentum on players, parry stagger) is LIKELY or SPECULATIVE, as listed in UNKNOWNS U-048 to U-054.
- **Addendum (lean):** `DishonoredCamera_Lean` (0x00AC5420, 0x00AC5950) swings the eye about a pivot `m_fLeanHeightPct x CollisionHeight x 2` (350 uu) below it, up to 15 deg (90.6 uu sideways, 11.9 down), on a spring (80 / 12) from DefaultCamera.ini. This replaces the speculative 40 uu slide. A ray stop against walls stands in for the lean camera collision.
- **Harness:** sim 12/12, net 8/8, contracts (3 min) 16/16, mission 7/7, travel 8/8, conversation 17/17.
## 2026-10-05: NPC attention, vision, patrols and combat decoded from the executable

- **Question:** How does the original turn vision, hearing and stimuli into attention and
  behaviour (U-041..U-045)?
- **Evidence / method:** Ghidra on `Dishonored.exe` (70da9613…). New helpers in
  `research/ghidra/`: `FuncDecomp.java` (functions with callees; seeds by address, name,
  `str:` string users, `ref:` data users, `vt:` whole vtable; lists the constants each function
  reads), `OffsetScan.java` (functions using given field offsets). `ClassDecomp.java` now picks
  the longest vtable a constructor stores (the UObject one, not an interface's).
  `package_dump --classes` prints the class hierarchy; `class_layout` computes native field
  offsets from the cooked reflection data (reproduces `DisAttentionInfo_Complex` = 0x66c).
  The UnrealScript decompile of `DishonoredGame.upk` gave enums and defaults.
- **Result** (details and addresses in `dishonored/AI.md`, "Decompiled attention code"):
  - Vision (`ArkComponentVisionNPC`): normalized distance = d / range of the first cone,
    inner then outer, that contains the target (0x00bf1580); the cones are elliptical
    (0x00bec910, vertical half angle = scale * horizontal). Rate = scalar * clamp01(distance
    curve(1 - norm) + target `Attribute_Visibility`) (0x00b4fa50). The curve is zero beyond 45 %
    of the range, so far targets are seen only through their visibility.
  - Player visibility: 0.1 * light01 + 0.25 * speed (0 while sneaking) + stance bonus (only
    auto-crouch and swimming, -1) (0x00ad1590, `Twk_PlayerVisibility`).
  - Increases add their amount, then floor at the threshold (0x00b60ca0) (U-042). The value
    leaks only on ticks without any change (0x00b61040). Stops are rolled per brain with
    `m_fStoppingVariance` (0x00b30270). Busted needs a reason with `m_bCanIncreaseToBusted`
    (hearing cannot bust) and is refused for an unsighted target at a stale proxy (0x00b4fce0).
  - Patrol: `DishonoredRoute`s adopted by distance and capacity (0x00c41590), with guard
    waits from `m_fGuardDuration`. 48 routes in Boyle Ext, mostly single-point posts.
  - Attack patterns are per-context cooldowns plus a group inhibition per attack class.
  - Panic contagion: `AllyBusted` group stimulus (0x00b61430), range `m_fGroupDistance` 1000.
- **Confidence:** CONFIRMED for the formulas, cones, increase/leak/level rules and route
  adoption. LIKELY: target visibility = cached `Attribute_Visibility` (0x00c86bf0), the group
  range, the inhibition semantics. SPECULATIVE: light level 1.0 (no light probe), speed
  normalisation 600 uu/s.
- **Implementation consequence:** `dishonored_ai` uses these rules (13 unit tests). Guards
  walk the original routes, combat ends through attention (no 5 s timer), melee follows the
  BerserkerElite cooldowns. Harness, before -> after (same seeds; replay hashes stable within each
  version):
  - contracts 3 min: pursuits 9 -> 11; guard-combat ticks 13685 -> 14661; civilian-flee
    ticks 206173 -> 207540 (68.2 % -> 68.6 % of civilian time: the help-bark chain, which is
    original behaviour, keeps panics going).
  - mission (Masked Host): detected 7 -> 3, alarms 2 -> 1, completion 23 s -> 21 s.
  - sim 12/12, travel 16/16 and conversation 8/8 pass.

## 2026-10-05: Running the original Kismet (Lady Boyle's Last Party end to end)

- **Question:** Can the original level scripting run as data, so a mission executes its own logic instead of a hand-written mission file?
- **Method:**
  - Loader `dishonored_kismet::load`: every `SequenceObject` under `TheWorld` of the global `DishonoredGameFull_P` and the map packages, with class defaults merged from the `Default__*` chain of `Core` / `Engine` / `GameFramework` / `DishonoredGame` (links included), typed properties, `SeqVar_Named` resolved as at level start, referenced actors with transforms and trigger shapes, Dishonored's `MatineeData` (length, event keys), story-flag names, localized strings, objectives. CLI `kismet` (`stats`, `op`, `find`, `fwd`, `bwd`, `actors`, `obj`, `fields`, `run`).
  - Engine semantics decoded in Ghidra (new helpers `DecompAt.java`, `KismetSlots.java`, `FindFieldWrites.java`; `ClassDecomp.java` fixed to compare vtables slot by slot): `Sequence::UpdateOp`, `ExecuteActiveOps`, `QueueSequenceOp`, `CheckActivate`, `ActivateEvent`, populate/publish, touch, remote events, attach-to-event, gate, switch, random switch, delay, toggle, finish sequence, comparisons, set-variable ops, matinee play/update/finish, `DeActivated` of every Kismet class, and the Dishonored ops on the Boyle path. Details: `research/dishonored/KISMET.md`.
  - Interpreter `dishonored_kismet::runtime` with those semantics; host trait; host on the mission match (`kismet_host.rs`); `MissionDef::original_kismet`; `missions/boyle_original.ron`; `harness kismet`.
- **Findings:**
  - `SequenceOp` flags at +0x8c: bit 2 `bActive`, 3 `bLatentExecution`, 4 `bAutoActivateOutputLinks`, 10 `m_bAlwaysOutOfBendTime` (declaration order of the fields in `Engine.upk`, matching the code). CONFIRMED.
  - Each sequence has its own op stack; a tick runs a sequence's ops, then its nested sequences; map levels are nested under the global sequence. Linked ops are pushed on top in reverse link order (depth first, first link first); a repeated impulse on an input is a queued activation. `FinishSequence` targets run on the next tick. CONFIRMED.
  - Switches, random switches, delays and spawn requests override `DeActivated` with an empty function: their class default `bAutoActivateOutputLinks` is true, so the override is what keeps them from firing every link. The previous `ClassDecomp.java` could not see such overrides. CONFIRMED.
  - Dishonored's `InterpData` is a stub pointing at a `MatineeData` (`m_RunData`: length, groups); Samuel's boat intro is 29.81 s. CONFIRMED.
  - Remote events reach every enabled listener in every loaded level (the global sequence listens for the maps' `ChangeLvl_*`). Named variables resolve globally first. CONFIRMED.
  - The original mission enters Boyle through the global `ChangeLvl_fromHub_toBoyle` and leaves through `ChangeLvl_fromBoyle_toHub` (PrepareMapChange `L_Pub_Morning_P`). CONFIRMED (data).
- **Result:** `harness kismet` 14/14. From `ChangeLvl_fromHub_toBoyle`: Samuel's intro conversation and its 29.8 s matinee add the objectives with their localized text (33.9 s); the estate entrance trigger; the garden door's own Kismet (choice, travel destination, global PrepareMapChange / SaveLevelState / CommitMapChange) into the ballroom; the sisters' positions, dresses and the guilty sister from the original random switches (all three sisters come out over 6 seeds); the guilty sister's note picked up shows her dress task; killing her fires the death event the Kismet attached at runtime, completing the eliminations and activating the escape task; back through the ballroom door; the canal trigger; Samuel's "Lets_Go" choice; the final exit runs the original end-of-mission Kismet to `ChangeLvl_fromBoyle_toHub`, which ends the mission at 58.7 s of play. Same seed, same Kismet state hash. Coverage on that run: 78 op classes executed of 153 in the data, 885 activations of 771 ops; 43 presentation-only activations (fades, HUD, chapters, statistics, achievements); 14 activations of 6 unsupported classes (`AISetSenses`, `Door`, `AttachToActor`, `SetDisposition` on factions, `ActivateUsable`, `ActorFactory`). The agent is placed next to what it uses; it does not path across the maps. Unit tests: 17 interpreter semantics tests. `harness travel` 12/12 (its material check now starts from the lady's own look, since the mission dresses her on arrival), `mission` 7/7, `conversation` 17/17, `sim` 12/12.
- **Confidence:** CONFIRMED for the decoded engine semantics and the data; LIKELY for level-loaded outputs on a return visit, camera fade timing and event instigators (U-080, U-081, U-084); APPROXIMATION for matinees (timed, not animated), AI go-to results and NPC vanishing (U-085..U-087).
- **Implementation consequence:** a mission can run its maps' original Kismet with `original_kismet: OriginalKismet(start_event: ...)`; the hand-written rule interpreter stays for custom missions.

## 2026-10-05: Wwise playback semantics, interactive music, music manager, footstep surfaces, notify sounds, particle effects

- **Question:** How does the original play its banks (loops, containers, transitions, states, RTPCs), which game states drive its music, where do footstep surfaces and combat sounds come from, and what do the levels' particle systems contain?
- **Method:** a strict v65 HIRC parser checked against every object of the 503 banks (`audio_dump hirc`: each object must end exactly at its body end; stats of properties, container modes, music rules); Init.bnk STMG; Ghidra on `Dishonored.exe` for `DisGlobalMusicManager` and its states (ClassDecomp, DecompRange 0x00c51000-0x00c54000, FuncDecomp on `FUN_00c52a40`, `FUN_00c486d0`, `FUN_00c48620`, `FUN_00c483c0`), `DishonoredNotify_Footfall` (`FUN_00a8d5f0` -> `FUN_00b66ed0` -> `FUN_00b66a80`), `AnimNotify_AkEvent` (`FUN_00716bd0`); class layouts (`class_layout`) of `DisTweaks_MusicEvents`, `DisGlobalMusicManager`, `AnimNotify_AkEvent`; `obj_dump` of `Dis_ContactSystem` and the Boyle particle systems; `audio_dump setters` (which events write a state group).
- **Findings** (details in AUDIO_FORMAT.md and EFFECTS.md):
  - All 20,946 HIRC objects decode exactly. v65 property ids have no bus volume: Loop is id 6 (967 looping sounds), Delay 13, TransitionTime 14, Probability 15 (U-029). RanSeq transition time is in ms. Switch group type is a u32. RTPC entries have no FX id and an u8 scaling. Music transition rules carry one source and one destination id and an always-present transition object (which explains the earlier "10 declared, 9 pairs" puzzle).
  - Boyle music: `Play_Music_Boyle` is a switch on the music state group with a nested switch on an interior/exterior group (0x0956DE29) set by the ambience room events; exterior is never set by a Boyle event (U-161).
  - Music manager: Exploration / Suspense / RatAttack / Combat / Chase with the selection rule, tick rules, combat levels from intensity thresholds and chase levels from hostile distance; events from the map's `DisTweaks_MusicEvents` (Boyle: suspense 800, chase 15 s at 2,000 / 4,500). The intensity writer is not found (U-160).
  - Footsteps are notify-driven contacts: footfall type x the hit triangle's physical material contact type through the 646-entry contact table (U-030); 391 `AnimNotify_AkEvent` in the Startup AnimSets post sword whooshes, equips, reloads at their clip time (U-031).
  - Particles: cooked distributions keep only their lookup tables ([time scale, start, entries]); Boyle has 190 systems and 259 emitters; gameplay effects live in DishonoredGame (`Vfx_GamePlay`, `Vfx_PhysMat`, `Vfx_Weapon`).
- **Port changes:** `dishonored_audio::{hirc (complete), init, engine, music, music_manager, contact, anim_notify}`; runtime audio rewritten on the engine (game objects, virtual voices, travel reloads banks, Kismet ambient start/stop/toggle and mission sounds); `MatchView::ai` (host-filled NPC awareness near humans); collision cache `collision2.bin` with per-triangle contact types; `dishonored_assets::particles` + IR particle systems/emitters; runtime `effects` (CPU particles, sprites, gameplay effects); `AnimSounds::advance` called by the first- and third-person animation code.
- **Confidence:** CONFIRMED for the HIRC layouts (exact decode), the music manager selection/tick/level rules, the footfall notify path and the contact data; LIKELY for the intensity sum, the contact lookup order and burst timing; APPROXIMATION for the LPF mapping (U-163), buses/FX (U-164) and effect sockets (U-165).
## 2026-10-05: NPC animation selection from the original (AnimSets, locomotion configs, animation definitions)

- **Question:** Which animations does the original play on an NPC, and how does it pick them?
- **Method:** pawn tweaks through the fallback chain (`m_AnimSets`, `m_pAnimDefClass`,
  `m_pComponentLocomotionConfig`, `m_RandomHeadMeshes[*].m_MaterialVariation`,
  `m_BodyVariationMaterials`); the cooked AnimTree `Npc_Common_at.NPC_Human_at`; the
  `DisNPCAnimDefinitions_*` default objects; Ghidra on `ArkAnimNodeLocomotion` (0x0091B280 and
  callees), `DisAISubProcessAmbientAnims` (0x00B39620) and the pawn spawn setup (0x00B7F8A0,
  0x00B76390, 0x00B7B0D0). New tool `npc_anim_dump`. Details: `research/dishonored/ANIMATION.md`.
- **Findings:**
  - Sequences are found from the last AnimSet to the first: the aristocrats' `Npc_Aristo_Empty_as`
    replaces the guard walk. Women use the 14 `Npc_CivFemale_*` sets and `FemaleLocomotionConfig`.
  - Locomotion: modifier (stance) x speed -> speed mode; each mode lists S, W, N, E loops with
    authored ground speeds. Directions blend by angle, the two speed modes around the current
    speed blend linearly (0.98 snap), and **S and W play with rate -1** (`_Revert` loops are
    authored reversed; the old renderer played them forward).
  - One-shots: `DisAnimDefinitions.m_Animations[eDisNPCAnimations]` chains (groups of steps,
    item prefixes, blend times, directional end-after-one groups).
  - Ambient idles: random timer in [2, 6] s, a random one of the leading steps that exist.
  - Material variations: one roll per listed slot, running sum of unnormalized chances.
  - Every Boyle tweak has `m_bRagdollOnDeath` false; deaths animate, then a
    `DishonoredNotify_Ragdoll` (2.3 s into `Generic_DeathFrontA`) hands over to physics.
  - NPC swing timing is in `DishonoredNotify_AttackZone` (0.49-0.59 s into the 1.767 s `Small`
    attacks), documented for the simulation.
- **Implementation:** looks carry an animation profile index (`MatchView::anim_profiles`) and
  their material variations; the host publishes `MatchView::npc_poses` (brain state -> stance).
  `dishonored_runtime::npc_anim` evaluates locomotion, full-body chains (attacks, parries, stuns,
  hit reactions, deaths, assassinations, panic, cower, ambient idles, modifier transitions) and
  upper-body talk gestures, with root translations for falls. Cache bumped to
  `<map>.appearance2.json`. Harness scripts gained a `view` step (third person, follow a pawn,
  orbit, distance, height); state dumps list kind, archetype, stance and look.
- **Validation:** captures in `harness_out/C_cap/{A,B,C,E}` (Boyle Ext with the original
  spawners: idle aristocrat, guard, woman; guards walking while searching; Lydia gesturing while
  she talks; an elite guard assassinated and lying as a corpse). Looks on Int: 33/33 NPCs on 24
  looks (19 before: head material variations now split looks). `harness` travel 12/12, visual
  6/6, launcher 14/14, kismet 14/14, net 8/8, mission 7/7, conversation 17/17.
- **Open:** U-140..U-147.
## 2026-10-06: NPC factions, alarm bells, barks, perception details and route squads from the executable

- **Question:** How do Dishonored's factions, forbidden zones, alarm bells, AI barks, light
  and hearing work, and why did travel pursuers lose their target (U-055)?
- **Method:** Ghidra (shared pool): `ClassDecomp` of the combat, alarm, faction, spawner,
  visibility, noise and behaviour classes; `DecompRange` over the faction manager
  (0x00c62800-0x00c64100) and `DisForbiddenZone` (0x00c95300-0x00c95800); `FuncDecomp` from
  the native exec table (`natives.tsv`) and with `ref:` seeds (all callers of the bark function
  0x00af00f0); `OffsetScan` with `class_layout` offsets (tweak fields of
  `DisTweaks_AIBehavior_TriggerAlarm`, `DishonoredRoute`); the FName table for attribute names;
  `tweaks` / `obj_dump` / `package_dump --exports` for the data.
- **Findings** (addresses in `dishonored/AI.md`):
  - Factions: relationship = individual first, then trespass (forbidden zones), then
    `SetDisposition` overrides, then the from-faction's enemy then ally lists (same faction =
    ally; not symmetric). Boyle: party guards `Neutral_Guard` do not list Corvo (the disguise),
    street guards `Neutral_Guard_Street` and the default guards (alarm squads, pursuers) do;
    upstairs forbidden zones on Int make the player an enemy of the party guards.
  - Alarm bells: TriggerAlarm chances per cause (party tweak: 1.0, help 0), alone/group by
    companions within 2000, fight instead within 500; ring after 0.25 s at 150 uu; after 2.5 s
    the bell's 6000 uu alarm noise and its triggered spawners (far from the enemy first, capped);
    rings while the enemy is hunted. `m_SpawnAtSuspicionLevel` is the spawned pawn's suspicion,
    not a trigger.
  - U-055: aware upon startup is MaxOutAttention -> a Psychic change to max with the proxy at
    the player; the reported drop to Search was the harness agent dying.
  - Light = light environment brightness ^(1/2.2) (scale global unknown); speed = speed /
    `Attribute_GroundSpeedSprint` unclamped; target visibility = `Attribute_Visibility` with
    three named modifiers; hearing = strict radius plus a trace outside audio cells, cell graph
    between cells.
  - Barks: 40+ call sites of 0x00af00f0 mapped to hooks and behaviours.
  - Routes: squads, capacity, adoption range, active flag; neglected routes (30 s) preferred.
- **Port changes:** `dishonored_ir::ai` (factions, zones, bells, probe lights, `AiLevel`, alarm
  tweak, route fields); `dishonored_assets::ai_level` (cached `<map>.ailevel1.json`; routes
  re-cached as `npcs6`); `dishonored_ai` (relationships via `Social`, faction per brain, forbidden
  zones, `RingAlarm` behaviour and bell state, `AiEvent` barks and alarms, light probe, hearing
  trace, deaf/blind/numb, route adoption, pistol start delay); mission host (`[ai]` marked):
  factions for spawned NPCs, aware-on-startup, AlarmRaised from the bell, bell spawns, barks to
  `MissionState::ai_hooks` -> `Runner::fire_hook`; Kismet host: `SetDisposition`,
  `AISetSenses`.
- **Harness (before -> after, same seeds):** contracts 3 min: pursuits 10 -> 16, guard-combat
  ticks 16320 -> 10050, civilian flee 205525 -> 197366 ticks (68.0 % -> 65.3 % of civilian
  time), NPC deaths 2 -> 1 (crowd guards now take their posts' factions, civilians the party
  faction). Mission (Masked Host): detected 3 -> 3, alarms 1 -> 1 (now raised by a guard ringing
  `DisAlarmBell_1`). travel 13/13 (new check: all 8 pursuers fight the player 0.33 s after
  arrival), sim 12/12, conversation 17/17, kismet 14/14, net 8/8, contracts 16/16 (one run had
  the timing-dependent "scoreboards agree" fail, rerun passed); determinism and replay hashes
  stable.
- **Confidence:** CONFIRMED for the relationship order, faction lists, zone semantics, alarm
  sequence, aware-on-startup, speed normalisation and the hearing branch; LIKELY for the bark
  attachment to port states and alarm decision details; SPECULATIVE for the light probe's
  absolute scale (U-120..U-125).
## 2026-10-05: Inventory, takedowns, powers and gadgets decoded from the executable

- **Question:** How does the original handle pickups, elixirs, ammo, chokes, bodies, Possession, Bend Time, Devouring Swarm, Shadow Kill, passive powers, bone charms, grenades and Spring Razors; and the open U-049 / U-050 / U-052 / U-054?
- **Method:** Ghidra (`ClassDecomp2` on the state, power, gadget and pickup classes; `FuncDecomp` with `ref:` seeds on the attribute `FName` globals such as `Attribute_HealthPotionPotencyBonus`, `Attribute_RangedDamage`, `Attribute_ChokeTime`; `DumpMem` on a StaticClass record); `prop_layout` for the tweak and controller field offsets; the UnrealScript decompile for enums and defaults; `tweaks` for the cooked values (with level packages for pickup tweaks); `anim_notifies` for the cast, choke, carry and gadget timings; DefaultPlayer.ini (`m_Powers`), DefaultWeapon.ini (damage types), DefaultGadget.ini.
- **Findings** (addresses and values in `dishonored/INVENTORY.md`):
  - Ammo add/remove (0x00C052E0 / 0x00C05260), elixir counts (0x00C04E10), drinking (0x00C04FC0: only below the maximum, `(int)((1 + potency) x 40 / 50)`), stat pickups (rolled ammo ranges, partial takes, potency-scaled food).
  - Choke (0x00A7AA00 / 0x00A71100): `ChokeTime` + the choke animation (6.53 s), the choke button must stay held through the loop (released = the victim breaks free), cancel when the victim leaves the being-choked state, holding Use after the win lifts the body after 0.2 s.
  - Carrying replaces `GroundSpeed` with `Attribute_GroundSpeedCarryingCorpse` (350).
  - Possession stages, duration `m_fPossessDuration x (1 + SpellStrength)` (people: level 2, 15 s; rats: 20 / 30 s), `m_fCancelCooldown`, host speed 300.
  - Bend Time dilations, real-time duration with `SpellStrength`, linear blend back over the cool-down (0x009FA0D0), projectiles waking 0.25 s early, re-use ends it.
  - Spring Razor arming / detection (NPCs only) / reaction / damage delay / NPC kill or 5 non-NPC damage; grenades armed on impact and exploding 0.5 s later with radial falloff.
  - U-049: bolt damage = `(int)Attribute_RangedDamage` (20) x `m_fDamageMultiplier` (0x00C257A0, 0x00C4A310). U-050: the regen-pausing class is the DLC's `DisDLC06ActivePowerComponent_Transversal`. U-052: reach speed factor (0x00AB2050). U-054: slide cancellable after 0.55 s (0x00A71450).
- **Port changes:**
  - `dishonored_ir::items` + `dishonored_assets::items_config` (config cache `gameplay_config.v5.json`), placed pickups (`dishonored_assets::pickups`, `pickup_dump`, cache `<map>.pickups1.json`; Boyle Ext 68 pickups, 6 carried by NPCs).
  - `dishonored_sim`: `inventory`, `takedown`, `powers`, `gadgets`; new left-hand items (Possession, Bend Time, Devouring Swarm, sleep darts, incendiary bolts, grenades, sticky grenades, Spring Razor) and `PlayerInput::action` (elixirs); per-pawn time scale; new world state replicated in snapshots (`extras`, protocol 2) and hashed.
  - Missions: pickups per map (saved with the level), pickpocket pickups ride on their spawner's pawn, `m_bTreatAsKnockedOut` spawners start unconscious, `PickedUp` / `NpcKnockedOut` events, triggers and statistics. Kismet: sim pickups fire `DisSeqEvent_PickupPickedUp`, knock-outs fire `DisSeqEvent_KnockedOut` / `PutToSleep` / `NPCIncapacitated` (Asleep); pickups the Kismet scripts stay Kismet usables.
  - Runtime: elixir keys (`GBA_HealthElixir` / `GBA_ManaElixir`), shortcuts 6-0 for the new items.
- **Behaviour changes (before -> after):** crossbow bolt 35 -> 20 damage; sword reach 210 -> up to 283.5 when sprinting forward; slides cancellable after 0.55 s by releasing the stick. Brawl hash changed (state hash covers the new fields), stable between runs.
- **Tests:** 18 new mechanics tests (`dishonored_sim/tests/items.rs`) plus the existing 25 mechanics and unit tests; `harness sim` 18/18 (new: placed elixir picked up and drunk, choke to knock-out in 6.55 s then carry at 350 uu/s and drop, possess and release, stop time freezes a guard for 8 s, a guard walks into a Spring Razor), `mission` 7/7, `travel` 12/12, `kismet` 14/14, `conversation` 17/17, `net` 8/8.
- **Workshop upgrades:** `DisTweaks_Upgrade` `m_Attributes` add to attributes (Grenade Belt +4 grenades, Instant Sleep removes the dart delay, Spring Razor radius x2, ...); the sim applies capacities, sleep delay and razor radius.
- **Nonlethal Brisby route (`harness kismet` table, not a check yet):** knock-out and carrying work on the original pawns (the guilty sister choked unconscious in the ballroom and carried to the cellar boat trigger), but the route stops earlier: Brisby's `DisDialogOneShot_6` approach hook ends without starting `Lord_Brisby_Quest_Start` in the conversation runner, so `GetBrisbyQuest` never attaches the sister KO events and the boat trigger stays inactive. Needs the conversation-side hook conditions (owner: conversation/Kismet work).
- **All suites on a clean per-agent build (`--cfg agent_a_build`):** sim 18/18, mission 7/7, travel 12/12, kismet 14/14, conversation 17/17, net 8/8, audio 53/53, contracts (3 min) 16/16, visual 6/6, launcher 14/14. (An earlier contracts failure, 2 contract kills, came from stale shared-cache artifacts.)
- **Confidence:** CONFIRMED for the formulas and data read in code; LIKELY / SPECULATIVE items are U-100..U-113 (choke anim mapping, grenade fuse, choke range, swarm movement, unaware sleep, body throw, immolation, possession targeting, Bend Time warm-up, Shadow Kill level 1, pickup details, use target, PvP mapping).
## 2026-10-06: Saving and loading like the original (slots, autosaves, level states)

- **Question:** How does Dishonored save, autosave and load, and can the port save a mission and continue bit-identically?
- **Method:** class layouts and enums (`DishonoredEngine`, `DisSeqAct_AutoSave`, `*LevelState`, menu classes); `DefaultInput.ini`, `DefaultEngine.ini`, `DefaultDebugMenu.ini`, `Twk_InGameUI.int`, `DishonoredGame.int`; Ghidra: `FuncDecomp` on the engine natives (`Dis_Save` 0x009ecd10, `Dis_Load` 0x009ecd70, push/pop disable / ignore), `ClassDecomp` of `DishonoredEngine` (vtable 0x010db390) and the four Kismet actions, the autosave code (0x00a0c530, 0x00a0d7f0, 0x00a09640, 0x00a0b050, 0x00a070b0, 0x009e41c0), `StrUsers` (save file names, 0x00b75ed0), `F_FuncCallers` (the map-load autosave 0x00a152f0), `VCallers` / `OffsetScan` for the push callers (none found); `kismet_dump` for Boyle's AutoSave ops. Details: `dishonored/SAVES.md`.
- **Findings:**
  - Slots (`ESaveGameSlot`): 0 last save, 4..12 mission starts, 13/14 autosaves in turn (the one not newest), 15 quicksave (F5 `Dis_Save 15`, F9 `Dis_Load 15`), 16..55 manual; files `DisMission<n>.SAV`, `DisAutoSave0/1.SAV`, `DisQuickSave.SAV`, `DISHONORED<slot-16>.SAV`.
  - `Dis_Save` only accepts slots above 14 (plus the DLC transition slots) and refuses while saving is disallowed or a mission-start save is pending; `Dis_Load` clamps to [0, 55], 0 = newest.
  - `DisSeqAct_AutoSave`: mission start (`m_MissionNumber` < 9) -> pending save in slot 4 + n; otherwise joins the pending autosave (timer `m_fAutosaveWaitTime`, 0 in the shipped config; `m_bSkipAutosaveDelay` zeroes it). The engine drops a pending autosave (all requesters "Denied") when the player is in combat and no requester has `m_bEvenInCombat`, or the game is over; it takes it once saving is allowed; the requesters go Saving -> "Accepted" when the save is written. Every map load that is not a reload queues an autosave from code (no combat check). Boyle's mission-start save (Ext op 3852, `DisMission4`) gates Samuel's intro matinee on "Accepted".
  - `DisSeqAct_SaveLevelState` only sets engine flags (save the level at the map change; `m_bPartial` = not complete); discards go through 0x00a14df0 (by name, or all). Level contents serialize natively (`DisSerializableObject` on AI blackboard records and stimuli; conversation and task-target save structs); the property list is open (U-343).
- **Port changes:** new crate `dishonored_save` (slots, names, RON + deflate container, save list, last save, autosave rotation, exact `state_hash`); `MissionMatch` and everything under it serialize through serde derives (added to `MissionState`, conversation `Runner`, `Kismet`, Kismet `HostState` / `KismetRun`, `LevelState`; databases and the setup are skipped and re-attached on load); `mission::save` (save / restore / setup fingerprint, autosave bookkeeping and `DisSeqAct_AutoSave` Activated / Update in the Kismet host instead of an immediate "Accepted"); runtime: F5 / F9, autosave writing, pause menu Save Game / Load Game lists, launcher Continue / Load (`--load FILE`).
- **Validation:** `harness save` 9/9: two save points (inside the mansion with the street's level state in the save; in the street before going in), each written to its slot file, loaded into a fresh match built from the mission setup: bit-identical state (516 / 448 KB of state, 112 / 83 KB on disk, encode ~0.1-0.2 s, load ~0.5-0.8 s) and every tick's world hash plus the full match hash (every second) identical to the uninterrupted run through two level travels (806 and 1156 ticks compared); a save from another setup is refused; autosaves on mission start, into the mansion and back out, alternating slots 13 / 14, plus the mission-start save in slot 8 and two Kismet autosaves. `kismet` 14/14 (objectives still at 33.9 s), `mission` 7/7.
- **Behaviour change:** the Kismet's `DisSeqAct_AutoSave` now finishes one to two ticks after activation ("Accepted" once the match took the save) instead of on the same tick; same-seed hashes stay stable.
- **Open:** U-340..U-344.
## 2026-10-06: Engine physics, swimming, Agility, Blood Thirsty, Dark Vision, armour and rats decoded

- **Question:** Are `physWalking` / `physFalling` / `CalcVelocity` stock UE3 (U-053)? Pawn shape (U-016), crouch height (U-017), lean (U-048), Windblast on players (U-051), reach multiplier (U-052); the missing mechanics (swimming, climbing, Dark Vision, Agility, Blood Thirsty, charms, upgrades, armour, slide impact) and rats (U-104/105/106/108/110).
- **Method:** Ghidra: `G_OffsetGroups.java` (new: functions touching groups of Pawn fields, with their vtable slots) found physFalling (Pawn slot 140), physWalking (141), the startNewPhysics dispatch (283) and from it physSwimming (280); `Decomp` on those, CalcVelocity (326), braking (327), NewFallVelocity (287), stepUp (148), the swim state, breath, drowning, adrenaline, Dark Vision, armour/TakeDamage, Windblast hit, hit reactions, possession targeting, Shadow Kill; `G_Callers.java` (new) for the adrenaline sources and finisher test; `Disasm` to confirm the falling time step. `obj_dump` on `Engine.upk` (`Default__Pawn`, `Default__PhysicsVolume`), the rat archetype and helmet tweak; `prop_layout` for DisTweaks_Pawn / PlayerPawn / damage-type flags; `tweaks` for Corvo, Twk_Rat, the swarm.
- **Findings** (PLAYER.md "Engine physics" and following, INVENTORY.md "Rats"):
  - Engine constants are UE3's x1.27 (step fudge 2.54, floor band 2.413..3.048, MAXSTEPSIDEZ 0.1016, min step 232.26, braking stop 12.7); `GroundFriction` 8 / `TerminalVelocity` 4000 confirmed from Engine.upk; `WalkableFloorZ = cos(m_fMaxWalkableFloorInDegrees 50)` = 0.643 (was 0.7).
  - CalcVelocity limits the acceleration to `AccelRate x modifier` (1800 running with the sword, 900 walking); braking in 0.03 s sub-steps.
  - physFalling: `bLimitFallAccel` air control with a wall test; velocity `2 x measured - v0`: everything in the air accelerates twice as fast. Jump apex 107 uu (was 213), falling damage from 844 uu (was ~1700).
  - Pawn collides as an axis-aligned box (extent traces); sneak 65 / crawl 33 confirmed; lean spring is a 3D point spring, 0x00AC5540 is the angle limiter.
  - Windblast momentum never moves the player pawn (damage path clears it); NPCs are thrown.
  - Swimming (physSwimming, startSwimming, swim state accelerations, breath 30 s / 5 s, drowning 1 hp/s direct), Agility's power jump (hold jump through the apex: +1300), Blood Thirsty (gains, burn, finisher), Dark Vision (fade-in 0.05, 30 s, fade-out 0.2, 1 s cooldown, no drain), armour (`DamageReduction`, min 1, `m_bApplyDamageReduction`), pistol magazine / reload speed, helmet knock-off list, rat archetype (350 uu/s, health 1, reach 120; player 200 / 60) and attack groups (int damage, no delay on the player, 40 % slowdown), Shadow Kill level 1 (awareness < 1 or out, not bites).
- **Port changes:** `dishonored_world`: `PawnBox` sweeps / contacts (SAT box-triangle) and water volumes (`water_at`); `dishonored_assets::water` + `<map>.water1.json`; `MovementConfig.water`, `power_jump_z`, `GameplayConfig.adrenaline` (cache `gameplay_config.v6.json`); `dishonored_sim`: movement rewritten as the engine port, `swim`, `adrenaline`, `rats` (rat pawns, attack groups, swarm rat possession), Dark Vision timers, armour, magazine bonus, `ChanceUseNoManaForSpell` (per-pawn deterministic roll), AI hooks `ai_unaware` (sleep darts, Shadow Kill) and `helmet`; new events (Swim, PowerJump, AdrenalineFull/Attack, RatSpawned, RatBite); state hash covers the new fields; protocol 3.
- **Behaviour changes (before -> after):** jump apex 213 -> 107 uu, airtime 1.07 -> 0.53 s; fall damage from ~1690 -> 844 uu, death ~2800 -> 1400 uu; walkable slope 45.6 -> 50 deg; ground acceleration 2000 -> 1800 running / 900 walking; braking stops below 12.7 (was 1) uu/s; walking pawn hovers 2.73 above the floor; Windblast no longer pushes players; Shadow Kill level 1 needs the AI's unaware flag; swarm reach 100 -> 120 (player 200 / 60), first bite one interval after the delay, no delay on players, bites slow by 40 %.
- **Confidence:** CONFIRMED for the decoded formulas and data; LIKELY / SPECULATIVE items are U-240..U-251 (climbing, level physics volumes, drowning vs regen, underwater strokes, underwater test, finisher reset, helmet absorption, dispersion, slide impact, random charms, swarm rat representation, reload speed).

## 2026-10-06: Matinees and soirees played (cutscenes)

- **Question:** How does the original play `SeqAct_Interp` matinees and conversation soirees (U-085, U-142), and can Samuel's intro on Boyle play like the original?
- **Method:** `obj_dump` / new `kismet matinee` on the Boyle matinee data; the UnrealScript layouts of `Engine.upk` (Arkane's soiree track classes); Ghidra `ClassDecomp` on the interp group / track / track-instance classes and `Decomp` / `FuncDecomp` on the stepping and stretched-key functions.
- **Findings** (details and addresses in `dishonored/CUTSCENES.md`):
  - `InterpData.m_Data` -> `MatineeData.m_RunData` (length, groups, soiree conversation node GUID); groups bind by name to the `SeqAct_Interp` variable links; the player group binds to the player.
  - Stretched anim keys (0x00905740): piecewise mapping through the sequence's soiree markers with per-marker offsets; the End segment loops `round(span / original)` times.
  - Soiree control: loop keys (0x009005e0 enter, 0x00900480 loop count / pin, wrap in 0x006340f0), break immediately (0x00900a40), pause keys, pins = `SCT_<pin>` inputs (0x00906b30); dialog keys start their blurb when crossed (0x00c73750) and `WaitForEndOfBlurb` holds the position (0x00c735c0).
  - Attachments name skeletal mesh sockets (`Player_Attach`, `Npc_attach` on `SmallBoat`), not bones.
  - Boyle's "Fade (Start)" matinee holds black in its `BlackLoop` until Samuel's intro starts (`TerminateFadeLoop` -> `SCT_BlackLoop`).
- **Port changes:** `dishonored_kismet::interp` (model, loader, evaluation, playback control, scenes), interpreter hooks for soiree control, `Actor::bones` (reference bones + sockets of skeletal mesh actors), server `kismet_host/cutscene.rs` (bindings, move / attachment poses on actors and pawns, player held, AI held, locomotion / face-to / visibility keys, dialog keys through `Runner::speak_blurb`, soirees wait for their matinee), `MissionView::cutscenes`, runtime `cutscene.rs` (InterpActor meshes, skeletal props with their anims, pawn anim tracks, AkEvent tracks, director fade and cuts).
- **Behaviour changes (before -> after):** Samuel's intro: the player stood at the drop-off for 29.8 s -> held in the boat and carried from the stage mark (-9269, 7890) to (-8159, 3243), 178 uu from the matinee's Shore point; Samuel's three lines at 5.7 / 16.9 / 22.6 s of the matinee instead of back to back; the conversation continues when the matinee ends, so the objectives appear at 33.9 s of play instead of earlier. Mission end time unchanged (58.7 s, agent placement).
- **Tests:** 12 interp unit tests; `harness kismet` 15/15 (new check for Samuel's intro).
- **Confidence:** CONFIRMED for the data and decoded functions; APPROXIMATION / LIKELY items are U-200..U-209.
## 2026-10-06: Character presentation: locomotion node, ragdoll, accessories, bodies, look-at, first person, Edge fixed format

- **Question:** What of `NPC_Human_at` and the character setup was still missing (U-140..U-147,
  U-022..U-025), and how do knocked-out, choked and carried pawns look?
- **Method:** Ghidra (`FuncDecomp` of the locomotion node steps from its TickAnim 0x0091B280,
  the Edge fixed decoders 0x009D4D90 / 0x009D5390 / 0x009D6CA0 / 0x009D6D80 and the frame-set
  dispatcher 0x009DADA0; `ClassDecomp` of the cower and strong-hit-react states; `StrUsers` for
  the blink node); `class_layout` for the node and config fields; `obj_dump` of `NPC_Human_at`,
  the Engine class defaults, `DisLookatConfig.DefaultLookatConfig`, the elite guard physics
  asset and sockets; `tweaks` for pawn, armour and behaviour tweaks; `npc_anim_dump` for the
  chains and player sets; the 0x00B7F8A0 decompile from the previous round for the accessories.
- **Findings** (details in `dishonored/ANIMATION.md` §2-§11, `animation/EDGE_ANIM.md`):
  - Locomotion node: polar velocity smoothing (2000 / 100 / 0.1 class defaults), diagonal ratio
    `cos + sin` of the angle between loops, sync-group rate `len * speed / sum(w * speed * len)`
    times the modifier's speed multiplier, modifier crossfade 0.5 s, transitions only below
    `m_fVelocityToStopMoving`. Start / stop / turn sequences are requested by the locomotion
    component, not the node.
  - NPC walk loops have no footfall notifies; the node's `m_bFootOnGround` (feet at sync phases
    0.44 / 0.94, planted for `m_fFootOnGroundPct` of the cycle) is the footstep source.
  - Cower = body stance `PanicRecover` (idle loop `Empty_Idle_Panic_Recover`), no chain (U-141).
  - Directional steps follow `ECardinalDirection` / `eDisDeathDirection` (U-143).
  - Ambient idles are per behaviour: patrol / guard 30-60 s upper body, stationary search 2-6 s,
    relaxed none (U-145).
  - Accessories: roll, weighted pick, static mesh on socket (U-144); Boyle helmets come from
    `m_PotentialNPCArmor` (`Stm_EliteHelmet` on `Helmet`).
  - Physics assets: `RB_BodySetup` / `RB_ConstraintSetup` decoded (elite guard 18 bodies, 17
    joints).
  - Look-at modes and blink timing from `ArkComponentLookatConfig` (0.5-6 s, rate 1-2).
  - Weapon sockets `RightHandWpn` / `LeftHandWpn` explain the first-person attach (U-025).
  - Edge fixed format: 48-bit smallest-three quaternions, raw float vectors (U-022); partial
    clips omit their last Edge joints (U-024).
  - No player set has an elixir drinking sequence.
- **Port changes:** `runtime/npc_anim.rs` (node smoothing, diagonal, rate, start / stop, feet,
  choke / carry / knock-out states, cower stance, behaviour ambient idles, blink, face slot,
  look-at grids, direction enums), new `runtime/ragdoll.rs` (presentation ragdoll, fixed steps),
  `characters.rs` (accessories, model sockets for weapons and accessories, foot-down reports,
  `DISH_ANIM_DEBUG`), `players.rs` (skinned bodies not tipped over, carried bodies at the
  carrier), `viewmodel.rs` (choke, carry, powers, gadgets; socket attach), `audio.rs`
  (`AnimSounds::foot_down`), assets (`read_physics_asset`, `load_lookat_config`,
  `load_static_model`, `mesh_sockets`, armour and accessory resolution, Edge fixed format, face
  names, feet). Cache: `<map>.appearance3.json`. `Look::accessories` added.
- **Validation:** captures in `harness_out/I_cap/` (worktree): `bodies/`, `bodies2/`, `final/`
  (a Helm1 elite guard choked from behind on Boyle Ext: In / Loop slave poses, Win, carried at
  the carrier, dropped, the ragdoll at rest on the floor; first-person choke and carry), `crowd/`
  (Contracts crowd walking). `edge_survey`: 113,232 / 113,232 sequences bake (410 more than
  before), fixed-format worst frame step 5.8 degrees. Tests: `dishonored_assets/tests/anim_retail.rs`
  (5), runtime unit tests (ragdoll settling, diagonal, smoothing, feet, directions, sockets).
  Harness: visual 6/6, travel 13/13, launcher 14/14, audio 64/64, net 8/8 (replay bit-exact).
- **Open:** U-280 (foot placement, turns, start / stop trigger, small additive hits), U-281
  (look-at target), U-282 (ragdoll vs PhysX), U-143 thresholds, U-147 dead poses.

## 2026-10-06: Kismet interpreter gaps, the CRT random stream, and the Boyle routes (Brisby, Miss White, Lord Shaw)

- **Question:** Which Kismet behaviour in Boyle was still guessed or unsupported (U-080..U-087), can the original `rand()` stream be reproduced, and do the other routes through the mission play under the original Kismet?
- **Method:** Ghidra (`ClassDecomp2` on the remaining op classes and `DisBehaviorGoTo(_Base)`; `Decomp` / `Vtables` / `DumpMem` on the pawn and controller handlers; `VCallers`; a new `F_FuncCallers.java` listing callers and raw pointers of a function, used on `srand` / `rand`, the level-loaded and level-restore functions and the latent result callbacks; `OffsetScan` for the vanish timer); the UnrealScript decompile for enums and properties; `kismet bwd` (now also lists the toggles / attaches that reference an event) and `conv play` to trace the route graphs.
- **Findings** (details and addresses in `dishonored/KISMET.md`):
  - Random numbers: the CRT `rand()` (MSVC LCG 214013 / 2531011), `appFrand` = rand x 1/32767 (0x00fb9318); seeded once from `QueryPerformanceCounter` (0x005fe910, `srand(0)` with `-FIXEDSEED`) and shared by 258 call sites in 181 functions, so outcomes per run cannot be reproduced; the port now runs the same generator on its own per-match stream.
  - `SeqEvent_LevelLoaded`: one engine call per output, each only for events whose output is linked (0x00700120, 0x007004b0); "State Restored" from the level-state restore (0x00a02760 <- 0x00a13070 <- 0x00a13db0), which resets `TriggerCount` first.
  - `SeqAct_CameraFade` (0x006e6960 / 0x006e6de0) matched the port's timing.
  - `DisSeqAct_AIGoToActor` results come from `DisBehaviorGoTo` through the pawn (Failed on a failed path request 0x00ae8600, Success at completion, Interrupted otherwise 0x00ae4d60; Success only for the last pawn; Abort fires nothing).
  - `DisSeqAct_NPCMarkForVanish`, `DisSeqAct_Door`, `DisSeqAct_ActivateUsable`, `SeqAct_ActorFactory`, the usable stage outputs of `DisSeqEvent_Used` (0x00a57640), `DisConv_KismetOutput`'s instigator (0x00cb1080), the `SeqEvent_Touch` toucher filter (0x006da210), and the NPC controller handlers behind the AI ops (search, suspicion, attention, ring alarm, shoot, distraction).
- **Port changes:** `runtime::CrtRand`; LevelLoaded per output; host: GoTo per pawn, doors with lock state driving an AI navigation overlay, usable stages, attach, actor factory, vanish with a view test, touch filters, `SeqEvent_LOS`, `SeqEvent_TakeDamage`, engine `SeqEvent_Used`, abstract items shared with the conversations (`MissionState::items`, `DisConv_HasAbstractItem`), `GivePickup` (tweak resolved at load), the AI and player ops; `NOT_MODELLED` for 14 decoded classes without a port system; hook outputs outside conversations find their one-shot. Every op class in the Boyle data is now handled (0 unsupported on all routes).
- **Routes (`harness kismet`):** lethal unchanged (58.7 s, same hash on replay: 06a4bba65efa6fd3); nonlethal Brisby end to end (quest, window, invite, KO conversation, choke and carry, cellar boat, `Thanks`, `CompleteMercyPath`, exit at 136.9 s); Miss White's drink up to the three colours (her walk away does not arrive, U-220); Lord Shaw's duel up to the pistol (the countdown on the mark is open, U-221).
- **Harness:** kismet 31/31, mission 7/7, travel 13/13, conversation 17/17; `dishonored_kismet` tests 19 + 5 (two new: CRT sequence, LevelLoaded outputs).
- **Confidence:** CONFIRMED for the decoded ops and the RNG; LIKELY / APPROXIMATION items are U-080, U-084, U-087, U-220..U-224.
## 2026-10-06: NPC combat moves, hearing through audio cells, light scale, sleep darts, rat swarms

- **Combat moves (U-124):** the elite guard's ranges come from its sword's pawn-specific item contexts (Short 50-235, StepBack 0-170, Medium 235-375, Long 375-570, Jump 730-850, BackStep 300 uu, SideStep 250 uu) and the pistol's (250-3000). `DisAISubProcessManageAttacks` refresh 0x00b47880 (flurry of 2-3 stamina-draining attacks, then 4-5 s recovery), elite gun rules 0x00b0e220 (start: timer expired, standing still, in (min, max); continue: in max; unreachable target: shoot at once), FirePistol enter 0x00b32c50. `dishonored_ai::combat` picks contexts in pattern order by range, height, cooldown and group inhibition, dodges by incoming-attack count and ranged-threat time; the move kind is exposed in `PlayerInput::npc_move` (`dishonored_sim::input::npc_moves`) and `Brain::attack_kind()` for animation.
- **Hearing through audio cells (U-120):** `DisAudioPropagationInfo.m_PathData` is a sorted key table plus path records (0x00b92320, 0x00b86d50); the hearing test 0x00bb50d0 / 0x00ba3700 / 0x00b9d5e0 string-pulls the portal path and divides by the portals' open fraction. Extracted per level (`AiLevel::audio`; Boyle Int 41 cells, 55 portals, 2704 paths); `dishonored_ai::hearing`.
- **Light scale (U-121):** `DAT_01435230` = 2 sqrt(pi) (static initializer 0x00f67460), cancelling the SH band-0 basis: the probe's scale 1 is right. Luminance weights 0.3/0.59/0.11 (0x004da330) replace Rec. 709 in the probe; sky lights added.
- **Sleep darts (U-105):** 0x00b83680 makes a sleep dart instant unless the NPC's awareness is `EAIAwareness_InCombat` (pawn virtual 0x6c0 = 0x00b6e340 -> 0x00b0c1e0). Hook `Crowd::unaware_of`.
- **Rat swarms:** `dishonored_ai::rats::RatSwarm` from `DisTweaks_RatSwarm` (`Twk_RandomRatSwarm`, `Twk_DevouringSwarm_Lvl1`); the native swarm tick is not decoded (U-260, U-261). Each rat pawn gets a brain (`Crowd::think_rats`); NPC awareness goes to `PlayerState::ai_unaware` every tick (`Crowd::write_awareness`, called by the mission and contracts hosts). Combat moves are visible to presentation as `PlayerState::last_input.npc_move`.
- **Suites (per-worktree build, after merging main 09faa6f):** mission 7/7, travel 13/13, kismet 14/14, save 9/9, sim 19/19, contracts (3 min) 16/16 (6 contract kills). Before the merge, contracts failed with 4 kills (5 needed); with the long and jump attacks disabled it gave 6 on the same code.
- **Generic voices (U-122):** `m_GenericDialogData` per pawn tweak recorded in `AiLevel::generic_trees`; binding to the barks still open.
- Caches: `gameplay_config.v7.json`, `<map>.ailevel2.json`. `ue3_package::Package::name` now wraps the name number like release builds do (a debug-build overflow on a misread light component).

## 2026-10-06: Init.bnk buses and environments, contact lookup, music intensity writer, gameplay effect sockets, particle modules

- **Question:** U-160..U-165: the Wwise bus hierarchy and effects, the LPF mapping, the writer of the music intensity, the interior/exterior music state, the contact lookup order, and the owners/sockets of the gameplay effects; plus the particle modules the effects runtime skipped.
- **Method:** Init.bnk HIRC dumped and decoded field by field (`audio_dump buses`, `fxsurvey`, `hirc`); names from the executable's `UDishonoredAudioSystem` strings (`FUN_00ba3120`) hashed with FNV-1; `DishonoredAudioVolume` layout (`prop_layout`) and instances (`obj_dump`, `audio_dump volumes`); Ghidra: `OffsetScan` on `m_CombatIntensityType` (+0x2D2) -> `FUN_00c5b190`, `F_FuncCallers` up to `FUN_00c5b610`; `FuncDecomp` of the contact trigger `FUN_00be61a0` / `FUN_00be4b10`, `ClassDecomp` of `DisContactSub_SoundCue` (0x00BC6290) and its poster 0x00BA95C0; `tweaks` for `DisTweaks_FirePistol`, `DisTweaks_NPCFireGun`, `Twk_WindBlast`, `Twk_Blink`, `Twk_Pawn_DefaultNPC`; `obj_dump` of `SkeletalMeshSocket`s and of `Volume_Light_PMAT`'s cooked expressions; a float-table scan of the image for an LPF table.
- **Findings** (details in AUDIO_FORMAT.md "Buses and environments", "Music manager", "Contact system" and EFFECTS.md "Gameplay effects"):
  - 27 buses (layout decoded; master -4 dB; `Global_Volume` / `Music_Volume` / `SFX_Volume` / `Voices_Volume` curves; `TimeDilation` pitch curve on the music and environmental buses; the dialogue bus ducks SFX / music -8 dB and the second voice bus -7 dB, fade out 899 ms, in 1,399 ms; 15 SFX sub-buses with playback limits). HIRC effect types are 18 (shareset) and 19 (custom), not 16/17: 55 sharesets in Init (RoomVerb, Matrix Reverb, Delay...) are the environments named by `DishonoredAudioVolume.m_Environment`; 583 Parametric EQs on sounds.
  - Music intensity writer `FUN_00c5b610` (U-160); contact lookup by smallest striker + struck depth per slot, event-less sound slots silence, speed scaling clamp and `Scale_RTPC` (U-162, U-304).
  - Boyle Ext: no event sets the music interior/exterior group; its 45 audio volumes post the ambience states (U-161 stays LIKELY).
  - No LPF cutoff table in the image (U-163 stays APPROXIMATION).
  - Gameplay effect systems and sockets from the tweaks (U-165); cooked particle materials keep only parameter expressions.
- **Port changes:** `dishonored_audio::{bus, mixer, volumes}`, bus / effect decoding in `hirc` / `init`; the engine applies bus volumes, states, RTPCs, playback limits, ducking, per-object environment reverbs and Parametric EQ; the runtime imports the audio volumes (entry events, RTPCs, environments), sets `TimeDilation` from Bend Time and `Scale_RTPC` on contacts, consumes direct footfall notifies (`AnimSounds::footfall`) with the stride fallback only for pawns without footfalls, posts the Shadow Kill sound. Effects: original muzzle / Blink / Windblast / Shadow Kill systems at their sockets (`EffectSockets` from the animated pawns), impacts and blood from the contact system, orbit, point / line attractors, collision, velocity over life, trails / ribbons and beams, and a glow material for the untextured mesh emitters.
- **Behaviour changes (before -> after):** every sound now goes through its bus chain (master -4 dB); dialogue ducks music and SFX by 8 dB; sounds in audio volumes get their environment's reverb tail; footstep volume scales with speed and steps at or below the contact minimum speed are silent; the pistol flash is the player's / pawn's own muzzle system at the gun's `Spitfire` socket instead of `Ps_Pistol_Blast` in front of the pawn; volumetric light cones and glow spheres are drawn.
- **Unknowns:** U-160, U-162, U-164, U-165, U-304 resolved; U-161, U-163 updated; new U-300 (options -> volume parameters), U-301 (attacker query filter), U-302 (environment assignment), U-303 (DSP algorithms).

## 2026-10-06: Level rendering from the original shaders (lightmaps, static switches, DisFog, ArkPp)

- **Question:** What do the original's shaders and parameter setters compute for directional
  lightmaps, material permutations, DisFog and the ArkPp post chain (U-026, U-027, U-028)?
- **Method:** `tools/package_dump/src/bin/shader_dump.rs` carves the D3D9 SM3 blobs out of
  `RefShaderCache-PC-D3D-SM3.upk` (39,747 shaders after LZO inflation; the package holds one
  `ShaderCache` export of 112 MB) and `GlobalShaderCache-PC-D3D-SM3.bin` (263) and lists their
  constant tables; `fxc /dumpbin` (Windows SDK) disassembles them. Parameter setters found in
  Ghidra from the shader-type registrations (static initialisers pushing the type name, e.g.
  `FDisFogPixelShader30Layer` at 0x010786F4, constructor 0x0081C8D0, pixel shader vtable
  0x01079F7C). Engine.upk UnrealScript (`ArkPpSettings`, `DisFogComponent`) via UELib.
- **Directional lightmaps (U-026):** `TBasePassPixelShaderFDirectionalLightMapTexturePolicy*`
  samples `LightMapTextures[0]` (NormalizedAverageColor) x `LightMapScale[0]` = colour and
  `LightMapTextures[1]` (DirectionalMaxComponent) x `LightMapScale[1]` = three intensities along
  the HL2 basis B0 = (0, 0.8165, 0.5774), B1 = (-0.7071, -0.4082, 0.5774), B2 = (0.7071, -0.4082,
  0.5774) in tangent space (constants in every such shader). With N the normalised material
  normal (`tex * 2 - 1`) and R = 2N(N.V) - V:
  diffuse = DiffuseColor x colour x sum_i dir_i x max(saturate(N.B_i)^2, 1e-6);
  specular = SpecularColor x colour x sum_i dir_i x max(saturate(R.B_i), 1e-6)^(SpecularPower + 1)
  (shader 15459: `pow(x, 1 + UniformPixelScalars_0)`; a material with constant power 15 folds it
  into four squarings, shader 1063). For the flat normal every diffuse weight is 1/3, which is the
  old flat average. `SpecularOverrideParameter` (`x * w + rgb`) and `DiffuseOverrideParameter`
  wrap the material colours; their runtime values were not traced (U-320).
- **Static permutations (U-027):** a MIC with `bHasStaticPermutationResource` ends its export with
  `FStaticParameterSet`: BaseMaterialId, switches (FName, UBOOL value, UBOOL override, GUID: 32 B),
  component masks (FName, UBOOL R/G/B/A, override, GUID: 44 B), normal parameters (FName, BYTE
  compression, UBOOL override, GUID: 29 B), terrain layer weights (32 B). 400/400 (Boyle Ext Env),
  228/228 (Int Env), 136/136 (DishonoredGame) decode with an exact end check. Boyle Ext Env: 312
  instances with `S_Enable_Specular` on and 57 off, 332/43 normal maps, 31 emissive, 39 cubemap,
  25 bloom; specular masks RGB 307, single channel 58; emissive single channel 9; opacity R 51, G 10,
  B 6. The cooked `generic_PMAT` (349 of Boyle Ext's 514 materials) keeps only its parameter and
  switch expressions; the math between them is stripped, so it is read from the compiled shaders
  (e.g. 15459: specular colour = the masked specular channel alone when `S_Enable_Specular_color`
  is off, cubemap x mask added to the diffuse colour).
- **DisFog (U-028):** `FDisFogPixelShader<N><M>Layer` (N layers <= 4, M with a LUT) is a
  full-screen pass over the scene colour (depth in its alpha). Per layer i, with D = surface -
  camera and dz its height component: t = saturate((h - camZ) / dz x HeightDensityFactor) for the
  layer bottom and top, in-slab length L = |t_top - t_bottom| x |D|, fog f_i = saturate((L - Near) /
  (Far - Near)) (or the 128-entry LUT at that coordinate) x Opacity, zero where the scene depth
  exceeds `NoFogPlane`. Layer 0, if `SunPower > 0.1`, is weighted by max(cos(D, sun), 0)^SunPower.
  out = colour x prod(1 - f_i) + sum f_i C_i + bloom parts. The setter (0x0081C940) passes Min =
  Origin - camZ, Max = Origin + Height - camZ, Near, 1 / (Far - Near), NoFogPlane, the density
  factor, colours, opacity and the negated sun direction (w = SunPower for a sun layer); with the
  custom-transition flag, opacity x= x / fCustomTransitionHeight when the layer top is x < T above
  the camera (not clamped). Boyle Ext layer 0 uses its LUT (`m_RawLut`, 128 bytes).
- **ArkPp (U-028):** `ArkPpConfig` struct defaults (Engine.upk): exposure 0, gamma 1, focus 1000,
  in-focus radius 300, far blur 0, colour balance 0, opacity 1, desaturation 0, grain / brightness /
  contrast 0, bloom on, tint white, threshold 0, scale 1. `FArkPpDofLutBlenderPS` bakes a 16^3 LUT
  indexed by sqrt(colour) (so the scene clamps at 1): x 2^m_Exposure (`pow(2.0, m_Exposure)`,
  setter 0x0090ED00), pre-desaturation (Rec.709 luma), CrMgYb colour balance (shadow / highlight
  curves `(1.075 - 1 / (c x 15.9375 + 1)) x 0.392`, midtone parabola x 0.2616), opacity, overlay,
  post-desaturation, `^(m_GammaAdjustment / display gamma)` (display gamma from the options, default
  1 / 0.4545), brightness, contrast `tan((m_GimpContrast + 1) pi / 4)` about 0.5.
  `FArkPpDofUber_*PS`: blend towards a blurred quarter-size copy by `saturate((depth - Focus) /
  InFocusRadius) x FarBlurAmount` (0x00914D40), then the LUT and film grain `+- m_FilmGrainNoise /
  255`. Bloom is material driven (`B_Enable_Bloom` bloom parts, `TBloomPartMesh*`), tinted by
  `gBloomTint` and added after fog.
- **Port:** lightmap atlases carry both planes (`LightmapAtlas::directional`); `lightmap_ext.wgsl`
  reconstructs diffuse and specular per pixel (tangent frame dP/du, dP/dv from screen derivatives,
  so the UE3 -> Bevy mirroring cannot flip it); static switches gate the S_/E_/N_/DN_ slots and pick
  mask channels (`MaterialChain::switch` / `mask_channel`, `MaterialParams::static_switches`);
  `ark_post.rs` + `ark_post.wgsl` run DisFog, far DOF, the LUT maths and grain as one pass on the
  main depth buffer, with Bevy tonemapping, distance fog and bloom off. `--no-lightmap-specular`
  and a fidelity config compare the lightmap specular term.
- **Result:** `harness fidelity` before `scratchpad/k_out/fid_before/fidelity/full/*.png` (main) vs
  after `scratchpad/k_out/fid_final/fidelity/*` (pairs per feature in its REPORT.md). Haze is much
  weaker than the old linear fog (Boyle's street level is inside layer 0 only, ~2-10 % fog at
  3-6 km of in-slab distance), the teal white-balance cast is gone, wet/glossy highlights appear
  where the lightmap specular lines up with a basis direction (strongest on flat roofs seen at
  about 35 deg, the basis elevation).
- **Confidence:** CONFIRMED for the shader maths and the static parameter layout; LIKELY for the
  parameter mapping where it came from the setters' field order (DisFog flag bits, sun layer);
  open: runtime override parameters (U-320), DisFog layer selection and the negative transition
  opacity (U-321), lightmap/texture sRGB sampling (U-322), bloom parts (U-323), dominant-light
  distance-field shadows (U-324), DOF downsample and blur kernel (U-325).

## 2026-10-06: Level data: Ext/Int frame, BSP, the last property-parse failures (U-006, U-007, U-014)

- **U-006:** the travel player starts pair up at the same height on both sides of the mansion
  wall, facing away from it: Ext `PlayerStart_1` (-2663, -1879, 98, yaw 180) / Int `PlayerStart_3`
  (-2013, -1848, 113, yaw 0); Ext `PlayerStart_3` (1355, 1445, 93) / Int `PlayerStart_4` (1715, 1295,
  113); Ext `PlayerStart_4` (-1775, -1940, 1113) / Int `PlayerStart_5` (-1385, -1895, 1113). No
  static mesh is placed identically in both maps (81 shared meshes, nearest pair 271 uu apart), so
  the maps do not duplicate geometry; together with the doors (BOYLE_MISSION.md) the frames are
  the same (offset 0). STRONG.
- **U-007:** `L_Boyle_Int_*` like Ext: every level `Model` belongs to the level or to volumes
  (blocking, audio, nav bounds, triggers, water, stealth, hideout, tether, possession); no walkable
  BSP in either map, so nothing to add to rendering or collision.
- **U-014:** the 88 remaining failures over all 470 packages were class-default subobjects whose
  component-ness the name rule got wrong: `DisActivePowerComponent_*`, `ArkComponentContainer`,
  `RB_Handle`, `DisDLC*` powers are components without the `Component` suffix (TemplateOwnerClass,
  TemplateName, NetIndex, then tags), while `DisTweaks_GrenadeComponent` and
  `DisAnimStateComponent` are plain objects (NetIndex, then tags); plus Engine's
  `Default__DominantDirectionalLightComponent` / `Default__DominantSpotLightComponent`, which have
  no `DominantLightShadowMap` array. `is_component_class` now walks the super chain of classes the
  package defines (imports: name rule plus Engine's `ApexComponentBase`, `ArkComponentContainer`,
  `RB_Handle`, `RB_Spring`). `package_scan --props`: 1,261,653 ok / 88 failed -> 1,261,741 ok / 0.

## 2026-10-06: Mover collision (the intro boat), the Boyle routes re-checked, kDOP section collision

- **Question:** Why does the player drop into the water when Samuel's intro releases them, why does Lord Shaw's duel stop before the countdown, and is Miss White's walk still open (U-220 / U-221)?
- **Method:** `harness kismet` with the new intro check and route diagnostics; `kismet fwd/bwd` on the duel graph (`On Mark Trigger` -> `Mark Touched` -> `Jump To Check License` -> `Check Licence Elegibility` -> `Duel Logic Loop`); `conv list/play` on `DisDialogOneShot_7`; `obj_dump` of the boat's `SkeletalMeshSocket`s, the intro's `InterpGroupPlayer` and stretch-anim keys; new `anim_root` tool (samples a sequence's tracks); Ghidra: `stepUp` 0x006ADC10, `processHitWall` 0x006AE2E0, `execSetPhysics` 0x006A1500 (setPhysics = vtable +0x218), `InterpGroupInstPlayer` (init 0x009027F0, term 0x008FE910), the player pawn's soiree slots (+0x4AC / +0x4B0 / +0x4C4), `DishonoredNPCController` / `DishonoredNPCPawn` overrides; `class_layout` for `Actor.Physics` (+0x104), `Pawn.MaxStepHeight` (+0x250), `WalkableFloorZ` (+0x258), the `Pawn` bit flags (+0x280: bit5 `bCanCrouch`, bit8 `bJumpCapable`, bit9 `bCanJump`); an exe scan of literal setPhysics modes.
- **Findings:**
  - The intro: `Player_Attach` is a socket of the boat's `Boat_Jnt` at (19.4, 144.3, 258.4): mid-boat, so at the detach key (29.64 s) the original leaves the player standing in the boat beside the jetty. `SC_boat` is a 2.67 s clip stretched over the matinee with no root translation; the player group has no snap flags (`InterpGroupInstPlayer` term only snaps to the root bone with them) and the Kismet does not teleport after `Completed`. The boat's collision is `InterpActor_1`, attached at `Boat_Attach`: InterpActors (`bCollideActors` / `bBlockActors` default true) block pawns where they are, and the port had no collision for them at all, so the player fell through.
  - Miss White's walk (U-220) arrives on main now (`WhiteOKtoPatrol` at 104.8 s).
  - The duel (U-221): the loop waits for "Shaw On Mark", set by his touch trigger once AIGoToActor_0 succeeds (`PositionGuysForDuel` key of `SoireeDuelExplained` -> `enableshawtrig`). Shaw stops 84 uu short against a 4.4 uu lip of the garden ground mesh (normal Z 0.456). In the binary a walking pawn neither steps nor slides up such a face (`stepUp` takes the slope slide only when `Physics != PHYS_Walking`, CONFIRMED with the field offset; the slide normal is flattened), `processHitWall` only tries crouching, the NPC controller does not override `AdjustFromWall`, and no code sets `PHYS_NavMeshWalking` (12) literally. How the original gets him onto his mark is open (the locomotion request's end threshold, `DisLocoRequest.m_fEndLocationThreshold` -1 = component default, is the lead).
  - UE3's static mesh collision provider skips kDOP triangles of sections without `EnableCollision` (`ShouldCheckMaterial`); the port now does too (no Boyle section has collision off, so the Boyle collision is unchanged).
  - NPC pawns have their own capsule in the original (`DishonoredNPCPawn`: radius 37, `MaxStepHeight` 30, `bJumpCapable` false, `cos(60 deg)` walkable from `DisTweaks_Pawn`); the port keeps them on the player's movement on purpose (players and NPCs must be indistinguishable in the multiplayer modes, U-020).
- **Port changes:** `MeshInstance.mover` (InterpActor components: actor and placed transform); `CollisionWorld.movers` (cache `collision4`); `WorldQuery::movers`; `Layered` query (static world + placed movers, nearest hit); `MissionMatch` places movers each step from the Kismet's actor transforms (`KismetRun::mover_delta`), rebuilding only moved ones; the sim and the AI step on the layered world. Kismet untouch events are logged.
- **Harness:** kismet 16/16 on the lethal route with the new check "after the intro the player stands in the boat" (before: swimming 2 s after release); routes: Miss White 6/6 (arrives), duel 6/6 (countdown still open, now reported with Shaw's walk).
- **Confidence:** CONFIRMED for the socket, the attachment and the stepUp / processHitWall reading; the InterpActor collision defaults are UE3's (LIKELY that no Boyle InterpActor overrides them beyond the flags read).

## 2026-10-06: Level physics volumes (zone velocity and pain)

- **Question:** What do Boyle's 24 non-water `PhysicsVolume`s do (U-241)?
- **Method:** `obj_dump` of the volumes (L_Boyle_Ext packages); UELib decompile of `Engine.PhysicsVolume` (`PostBeginPlay` spawns a `VolumeTimer`, `TimerPop`, `CausePainTo`, defaults); the zone-velocity terms of UE3 `physWalking` / `physFalling` as recorded in U-241.
- **Findings:** fence volumes with `bPainCausing`, `DamagePerSec` 1, `DisDamageType_SpringRazor` (and `_Impact` on two), zone velocity (40, 90, 25) / (-40, -70, 25) / (0, 300, 300). Pain: every `PainInterval` (1) from level start, `TakeDamage(int(DamagePerSec x PainInterval))` to each touching damageable, non-static actor; entry pain by default.
- **Port changes:** `dishonored_world::PhysicsVolume` (hulls, priority, zone velocity, pain), `WorldQuery::physics_volumes`, cache `physvol1`; sim: zone velocity in walking and falling, `volumes::step_pain` (timer pops and entry pain, per-pawn touching bits).
- **Tests:** `dishonored_sim` player tests `pain_volume_hurts_on_entry_and_every_interval_and_pushes`, `outside_pain_volume_no_damage`.
- **Confidence:** CONFIRMED for the volume data and the pain script; LIKELY for the timer on unscaled time (Bend Time slows the world, the port's timer does not slow with it).

## 2026-10-06: Muzzle flash capture and distortion materials

- **Question:** Do pistol shots show the original muzzle flash in the rendered client (last round could not capture it)?
- **Method:** new runtime control command `effects` (the last gameplay effects asked for, spawned or not decoded); the visual suite fires the pistol, screenshots it and checks the effect log; `obj_dump` / `mic_static` of the muzzle system's emitters and materials.
- **Findings:** the shot spawns `Ps_PlayerPistol_StdMuzzle_01` (the last round's capture ran a stale runtime: the harness starts the runtime and server from its own folder, and `.bin/` held binaries from 03:20; copy every binary when refreshing `.bin`). The screenshot showed a half-screen purple band: emitter `ParticleSpriteEmitter_20` uses `WindWave_Disto_01_inst` (parent `DefaultAdditive_PMAT`, `bHasDistortion`), whose static switches are `Dist_Enable_Distortion` on and `D_Disable_Diffuse` on; the port ignored the switch and drew its `D_Diffuse` (`Teleport_Ink_m`) additively.
- **Port changes:** distortion-only emitters are skipped in the colour pass; the distortion pass itself is U-360.
- **Harness:** visual 7/7 with the new check "pistol shots spawn the original muzzle flash systems"; screenshot `screens/muzzle_flash.png` (no band).

## 2026-10-06: Lamp glow planes drawn as boxes (fog texture switch)

- **Question:** What is the translucent green box at the Boyle Ext spawn (visual capture `smoke/01_spawn.png`)?
- **Method:** new `map_dump --near X Y R` (instances and placed emitters around a point, with material blend modes and decoded size / colour / alpha); `obj_dump` / `mic_static` of `Boyle_FX.Lights.Light_small_Yellow_01` and `Vfx_Lights.Materials.wall_lamp_Inst`.
- **Findings:** the box is the lamp glow emitter at (-2662, -2126, 237): a mesh emitter (`light_plane`, size 20, colour (0.996, 0.946, 0.727), alpha 0.12, additive) with `wall_lamp_Inst` (`G_GlowPower` 8, `D_Distance_Fade_Factor` 1024, switches `F_Use_With_Fog` off, `D_Enable_Distance_Fade` on). The port took the instance's `F_Fog_Texture` (`Cloud_64_d`) as its colour texture; with `F_Use_With_Fog` off the original never samples it.
- **Port changes:** the fog texture is not a colour source while `F_Use_With_Fog` is off, so the emitter takes the procedural glow path; `G_GlowPower` feeds the glow falloff exponent when the material has no `Fresnel_exponent`. APPROXIMATION: the glow graph is stripped in the cooked data (U-305); the distance fade is not applied.

## 2026-10-06: Ragdoll on the level floor

- **Change:** the presentation ragdoll (U-282) collides with the level's floor instead of a plane at the pawn's feet: at the hand-over a 17 x 17 grid (12 uu cells, pawn space, turned by the pawn's yaw) is sampled from the collision by down-raycasts, and each body is kept above the bilinear height under it. Walls are still not collided.
- **Note:** the point-mass solver is sensitive to the last bits of the floor height (a flat floor through `a (1 - t) + b t` settled a different way than the exact constant); the grid interpolates as `a + (b - a) t`, exact where the samples agree.
- **Tests:** `ragdoll::tests::floor_grid_follows_a_step`, `elite_guard_settles_on_a_step` (retail), `elite_guard_settles_on_the_floor` unchanged.

## 2026-10-06: Generic per-voice barks (U-122)

- **Method:** `class_layout` of `DisConversationComponent` (`m_pGenericDialogTree` +0x54, `m_GenericDialogData` +0x64, `m_pGenericDialogData_Cached` +0x7C); `OffsetScan` over the conversation code; decompile of 0x00C8D2C0; `obj_dump` of `Dlg_EliteGuard.VoiceData.Voice_EliteGuard_A`.
- **Findings:** the component caches one voice of its tweak's list at random (`m_GenericDialogData[FUN_00401940(n)]`) and binds that voice's tree. The voice data names its tree in `m_pDialogTree` and per blurb list (`m_VoiceData[].m_pTree`); the trees are cooked into the level script packages (`Dlg_EliteGuard.DialogTree.Dlg_EliteGuard_Shared`, 78 hooks). Their hook graphs speak blurbs directly (random branch -> line) without a `DisConversation`.
- **Port changes:** `ConversationDb::voice_trees` (dialog cache key versioned, `v2`), bark requests carry the pawn's voice (`MissionMatch::request_bark`), the runner counts lines spoken from the hook as content (before: every generic bark was "NoContent"). Bug fixed on the way: the AI level table is keyed by the lowercase map name.
- **Harness:** `kismet --route barks` (an elite guard's hooks: "You look like you're up to no good." from `Dlg_EliteGuard_Shared`); `dishonored_conversation` test `generic_voice`.

## 2026-10-06: Distortion pass (U-360)

- **Question:** What does UE3's distortion (heat-haze) pass compute for Dishonored's `Dist_Enable_Distortion` materials (pistol muzzle shockwave, Blink / Windblast waves)?
- **Method:** `shader_dump` of `RefShaderCache-PC-D3D-SM3.upk` (filter `SceneColorTexture`, 1,770 pixel shaders) and `GlobalShaderCache-PC-D3D-SM3.bin` (filter `AccumulatedDistortion`), `fxc /dumpbin`; the accumulate shaders are the 133 whose output writes `oC0.zw` from a negated min. `obj_dump` / `mic_static` of `WindWave_Disto_01_inst` and `DefaultAdditive_PMAT` (parameter expressions, `bUsesDistortion`).
- **Findings:**
  - Accumulate (same tail in all 133): D = the material `Distortion` input; `dp2add D, D, -0.1; texkill` drops |D|^2 < 0.1; the scene colour is sampled at `uv + D x (1/255, -1/255)` and D is zeroed where `SceneColor.a x 256 - pixel depth < 0` (scene in front); E = D x 4 / 255 clamped to [-1, 1]; `oC0.xy = max(E, 0)`, `oC0.zw = max(-E, 0)` (additive into the offset target). So D is in 1/255 of the screen.
  - Apply (global shader 237: `AccumulatedDistortionTexture` s1, `SceneColorTexture` s0, `SceneColorRect` c0): offset = (RG - BA) x (0.25, -0.25); the new coordinate is used only if it lies inside `SceneColorRect`, else the pixel's own.
  - `DefaultAdditive_PMAT` family (e.g. 36064 / 36065, sprite and mesh vertex factories): D is a scalar (both components equal) = `Dist_Distortion` sample (R in most permutations, B in some) at the UV-distorted coordinate x uniform x vertex colour alpha (sprite `TEXCOORD1.w` / mesh `COLOR.w`) x `O_Opacity.r` at a panned coordinate x another uniform; `Enable_UV_Distortion` adds `UV_Distortion.rg x UV_Distortion_Intensity` to the coordinate.
  - The uniforms come from each shader map's `FUniformExpressionSet`, serialized in the cache as arrays of (type FName, payload) expressions; the type names are FNames 512-528 of the cache's name table (`FMaterialUniformExpressionAbs` .. `VectorParameter`). A research script (scratch `R_out/uexpr.ps1`) finds `ScalarParameter(Dist_Distortion_Intensity)` (41 shader maps) and parses the vector / scalar arrays before it. The mesh-emitter permutation that matches shader 36094: vectors `SelectionColor`, `D_DiffuseColorTweak` (c0), `MeshEmitterVertexColor` (c6), Append(frac(Time x `Dist_Distortion_Speed`) x 2) (c7); scalars Time, `U_Scrolling_Speed`, U x Time (c8.z), `V_Scrolling_Speed`, V x Time (c9.x), `Db_Depth_Biased_Alpha_Intensity`, the pan terms, `Dist_Distortion_Intensity` (c10.z); textures `O_Opacity`, `Dist_Distortion`. Hence D = `O_Opacity.r`(uv + scroll) x `D_DiffuseColorTweak.a` x `MeshEmitterVertexColor.a` x vertex colour x `Dist_Distortion.r`(uv + pan) x intensity (per-permutation details: U-400).
  - `WindWave_Disto_01_inst`: intensity -100, `Assassins_Teleport_01`, opacity `Wind_Swirl_01`, scrolling (0.1, -0.5), vertex colour on; the decoded level set has 12 distortion emitters (muzzle shockwaves, Blink ground / mantle bodies, Windblast twists, Teleport vanish, water splash).
  - Port bug found on the way: a particle burst at emitter time 0 never fired on the first cycle (`EmitterSim::last_burst_t` started at 0), so one-shot effects lost every time-0 burst (the muzzle shockwave had no particle at all). Fixed (starts at -1).
- **Port changes:** `distortion.rs` (`DistortionMaterial::decode` from the static switches and parameters; `DistortionFrame` of distorting triangles rebuilt by `effects::simulate` from the same sprite / mesh particles; accumulate + apply passes in Core3d `EarlyPostProcess`, ArkPp now explicitly in `PostProcess`); `distortion.wgsl`, `distortion_apply.wgsl`; the import keeps the raw `Dist_Distortion` / `O_Opacity` / `UV_Distortion` textures of distortion materials (`MaterialAsset::texture_params`). `--no-distortion` and `DISH_DISTORTION_DEBUG` (offset view) for comparisons.
- **Tests:** `distortion::tests::decodes_the_pistol_wind_wave`, `accumulate_and_apply_round_trip`; `effects::tests::burst_at_time_zero_fires_on_the_first_cycle`. Captures (scratch, `fixtures/scripts/effects_gameplay.json`): the shockwave and Windblast twist now displace the scene (debug view: offsets up to ~0.4 of the clamp near the muzzle).
- **Confidence:** CONFIRMED for both shader tails and the encoding; LIKELY for the Arkane material terms (U-400).

## 2026-10-06: Distance-field shadows of the dominant directional light (U-324)

- **Question:** How does the original shadow the dominant directional light (the moon) on static geometry, and can the cooked `ShadowMap2D`s replace Bevy's cascades there?
- **Method:** `obj_dump` of `ShadowMap2D` / `ShadowMapTexture2D` (L_Boyle_Ext_Env) and `DominantDirectionalLightComponent` (L_Boyle_Ext_Light, L_Boyle_Int_Light); `shader_dump` (filter `DistanceFieldParameters`, 3,676 shaders) + `fxc`; Engine `BaseLightmass.ini`; Ghidra: the shader type registration for `TLightPixelShaderFDirectionalLightPolicyFSignedDistanceFieldShadowTexturePolicy` (string 0x00FC7478, static initialiser at file offset 0xB6E7A9: constructor 0x004F6F70, type global 0x0144012C), its constructor 0x004F4480 (vtable 0x00FCE630) and `Serialize` 0x004F4520 (shadow policy parameters at +0x14C .. +0x164).
- **Findings:**
  - Every lightmapped component carries one `ShadowMap2D` per unbaked light: `Texture` (PF_G8 atlas, 1024 x 1024, `Lighting.tfc`), `CoordinateScale` / `CoordinateBias` on UV1, `LightGuid`, `bIsShadowFactorTexture` false = signed distance field. Boyle Ext: 462 of 3,503 lightmapped components have one for the dominant directional light (`LightGuid` (549869335, 1241035487, 3786277786, 3976827150)).
  - The RefShaderCache has no base-pass permutation with the dominant light (`FDistanceFieldShadowedDynamicLight*` exist in the exe but were not cooked); the light is an additive pass, `TLightPixelShader<FDirectionalLightPolicy, FSignedDistanceFieldShadowTexturePolicy>` (e.g. shader 1445): s = saturate((d + P.x) x P.y), 0 below 1e-6, else s^P.z, with P = `DistanceFieldParameters`; the light term (N.L diffuse, specular) is multiplied by s.
  - Lightmass stores d = 0.5 + distance / (2 x `MaxTransitionDistanceWorldSpace`), 50 uu in `BaseLightmass.ini` (`bAllowSignedDistanceFieldShadows=True`). The setter of P was not reached (U-401); the material's `DistanceFieldPenumbraScale` (default 1, render command `UpdateDistanceFieldPenumbraScaleCommand`) and `LightmassSettings.ShadowExponent` (default 2) are its inputs.
  - `DominantLightShadowMap` (u16, 132 x 232) is a light-space depth map for the engine's CPU dominant-shadow queries, not a shader input (U-402). Dynamic receivers keep whole-scene dynamic shadows (`bAllowWholeSceneDominantShadows=True`), i.e. the Bevy cascades stay for them.
- **Port changes:** import: `ShadowMapRef` per component, the dominant directional light's distance field resampled (bilinear, texel centres) into the lightmap atlas layout (`LightmapAtlas::shadow`, `InstanceLightmap::df_shadow`, `lightmap::sample_r8_bilinear`); render: lightmap batches split by `df`, `BakedLightmap` bindings 104 / 105, `lightmap_ext.wgsl` swaps the cascade factor of directional light 0 for the distance-field one (computing Bevy's own directional term and the cascade lookup again, so other lights keep their shadows). P = (-0.5 + 0.5/4, 4, 2) (`map_render::DF_SHADOW`). `--no-df-shadows` restores cascades; `DISHONORED_LIGHTMAP_DEBUG=shadow` shows (s, d, cascade). Also fixed: `map_render::to_image` built mipmapped images with `Image::new`, whose debug assert panicked in every debug build.
- **Result:** Boyle Ext street / aerial captures (scratch `R_out/sh_df` vs `sh_casc`): 3 % of pixels change, mostly darker facades beyond the cascades' 200 m; near the player the two agree (garden view: 110 of 160,200 sampled pixels differ), which also confirms the field's polarity.
- **Tests:** `lightmap::tests::r8_bilinear_resample`.
- **Confidence:** CONFIRMED for the data layout and the shader form; LIKELY for the constants (U-401).
