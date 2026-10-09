# Research Log

Each entry: Question / Evidence / Experiment / Result / Confidence / Implementation consequence.
Build: GOG Definitive Edition, `Dishonored.exe` SHA-256 `70da9613â€¦c6fb` unless noted.

---

## 2026-10-04 â€” Installation fingerprint
- **Question:** What exactly is the canonical target?
- **Evidence:** GOG registry key `GOG.com\Games\1701063787`, `goggame-1701063787.info`, file hashes.
- **Experiment:** Hash and version query of the install.
- **Result:** buildId 56034905455963308; exe 18,041,856 B, SHA-256 `70da9613â€¦c6fb`;
  FileVersion is the uninformative `1,0,0,0`. Details are in `dishonored/GAME_FILES.md`.
- **Confidence:** CONFIRMED
- **Implementation consequence:** Every binary-level finding is tagged with this hash.

## 2026-10-04 â€” Package container format
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

## 2026-10-04 â€” Object flag dword order and script state frames
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

## 2026-10-04 â€” Component and class-specific prefixes
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

## 2026-10-04 â€” StaticMesh layout and the kDOP quirk
- **Question:** How is `UStaticMesh` serialized?
- **Evidence:** Hex of `air_duct_01.air_duct_09`. The kDOP nodes have element size 32,
  not the compact 6-byte format expected at v>=770. UEViewer has an explicit
  `GAME_Dishonored â†’ old_kdop` branch.
- **Experiment:** Implemented the decoder and imported every mesh in Boyle Ext.
- **Result:** 405/405 meshes decode. Positions, packed tangents, half-float UVs and
  u16 indices are all consistent with the stated counts.
- **Confidence:** STRONG
- **Implementation consequence:** kDOP triangles (u16Ã—3 + material) give per-poly
  collision for free. They are the starting point for M5.

## 2026-10-04 â€” Gameplay trace channels on primitives
- **Question:** Do primitives carry Dishonored-specific collision semantics?
- **Evidence:** Every StaticMeshComponent has `m_CollisionTraceTypes: DisPrimTraceMask`
  with booleans for Crosshair, Projectile, Melee and VisionLOS traces.
- **Experiment:** None yet (property decode only).
- **Result:** Per-primitive opt-outs exist for the four gameplay trace types.
- **Confidence:** CONFIRMED (existence); semantics UNKNOWN (U-012)
- **Implementation consequence:** `WorldQuery` needs trace channels that mirror these
  flags, or Blink, projectile, melee and AI LOS results will not match the original.

## 2026-10-04 â€” Data-driven gameplay constants in INIs
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

## 2026-10-04 â€” First render of an original map (M4)
- **Question:** Does the decoded geometry come out in the right place, at the right scale, with usable textures?
- **Evidence:** Bevy screenshots of `L_Boyle_Ext_P` and its sublevels: an aerial shot from (-6000,-6000,6000) and a street-level shot from PlayerStart_1 (-2663,-1879,98).
- **Experiment:** Converted to Bevy space with a Y/Z swap and 0.02 m/uu. Collection matrices are applied as `ParentToWorld * component local`. BC textures are uploaded directly with their mip chains.
- **Result:** The Boyle estate is recognizable: mansion, walled garden, street perimeter, surrounding terraces. Transforms and scale are consistent at player eye height. Material texture params follow a consistent convention (`D_` diffuse, `N_` normal, `S_` specular, `O_` opacity, `E_` emissive, `D2_`/`N2_` variation). Ranking by that convention fixed the false-colour surfaces.
- **Confidence:** STRONG (visual). The transform convention still needs a numeric check against the original game (U-011).
- **Implementation consequence:** M4 is reached. Remaining visual gaps are unresolved cross-package materials (U-005, e.g. street cobbles), translucent FX cards (hidden by default) and lightmaps (U-013). None of these block collision work.

## 2026-10-04 â€” Sony Edge animation decoded (U-008)
- **Question:** Can Dishonored's `ACF_EdgeAnim` clips (all 113,232 sequences) be decoded without Sony's SDK?
- **Evidence:** Static analysis of Dishonored.exe (hash above) with `exe_probe`. The only reference to tag `0x45413035` is in the blend-tree evaluator `0x009DBC10`. From there I followed the frame-set decompressor `0x009DADA0`, the constant-channel decoder `0x009CD3A0` (reproduced instruction by instruction: packing-spec bit fields, fixed/float component decode, smallest-three recomposition, big-endian bit streams) and the animated-channel decoder `0x009CE380` (argument layout).
- **Experiment:** I implemented the decoder in Rust and checked it against the data. Initial-frame section sizes equal the summed spec bit widths, and key-section sizes equal popcount(mask) x spec bits exactly in every set tested. Set boundaries line up byte-exactly. Channel-major key order gives smooth trajectories (1.3â€“7 deg/key) where frame-major gives noise (49â€“91 deg). I also rendered stick figures of Idle/WalkN/RunN on the elite-guard skeleton.
- **Result:** Walk and run cycles decode correctly: alternating legs, counter-swinging arms, knee drive. Format documented in `research/dishonored/animation/EDGE_ANIM.md`.
- **Confidence:** STRONG (structure CONFIRMED by exact size accounting; motion validated visually; interpolation nlerp vs the binary's polynomial slerp is LIKELY close)
- **Implementation consequence:** Characters play decoded locomotion (idle / walk / run in 4 directions, rotation-only retargeting by bone name). Phase II NPC and player animation is unblocked. The fixed-format channel paths (flag bits clear) remain to be ported if the survey shows they are used.

## 2026-10-04 â€” Combat timings from animation notifies (U-021)
- **Question:** What are the sword swing, parry, equip and reload timings that the original drives from animation?
- **Evidence:** `map_dump anim_notifies` lists AnimSequence lengths, rate scales and AnimNotify events for the player AnimSets in Startup.upk (Edge decoder now available).
  - `Ply_Sword_Attack_as`, "Small" attacks: `DishonoredNotify_AttackZone` starts at 0.104â€“0.158 s and lasts 0.186â€“0.243 s; `AttackInterruptable` â‰ˆ 0.58â€“0.63 s; `AnimStateExit` 0.78â€“0.95 s.
  - `Ply_Sword_Versus_as`: `DisNotify_BlockParryZone` from t=0.001 with duration 0.430 (0.422 sneaking).
  - `Ply_Sword_Assassination_as`: `Sword_Ready_Assassination_Back` exits at 1.886 s and the drop variants at 1.90â€“1.97 s.
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

## 2026-10-04 â€” First-person rig and Edge joint order
- **Question:** Can the first-person view use Corvo's real arms, weapons and animations?
- **Evidence:**
  - `Engine.upk` contains `Ply_Player.Skm_Player`, a 79-bone full-body rig. It has `camera_jnt`, `handAttachment_R/L_jnt` and legs.
  - `Startup.upk` contains the weapons (`Wpn_PlySword01`, `Wpn_PlyGunElite`, `crossbow_01`, each with a `Root_Attachment_jnt`) and the player AnimSets (`Ply_Sword_*`, `Ply_Guns_as`, `Ply_Powers_as`, `Ply_Pistol_as`, `Ply_Crossbow_as`).
  - The sword sets animate 69 tracks (the whole rig minus the legs). The gun and power sets animate 30 (root plus the left arm).
  - The `fp_probe` tool poses the rig and prints bones relative to the camera bone.
- **Experiment:**
  1. Track mapping. Under `TrackBoneNames` order, `head_jnt` received exactly camera_jnt's bind translation. Re-mapping Edge joints to the set's tracks sorted by skeleton bone index made every animated translation match its bind length.
  2. Camera frame. The rig is placed so the animated camera bone coincides with the game camera (forward = âˆ’Z, right = +Y, up = +X of camera_jnt).
  3. Sockets. I compared candidate socket conventions numerically (blade axis against the fist's pinkyâ†’index axis) and visually with scripted screenshots.
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

## 2026-10-04 â€” Contracts bots navigate the original nav points
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

## 2026-10-04 â€” Third-person weapons and animation states
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
## 2026-10-04 â€” Baked lighting: component lightmaps (U-013)
- **Question:** Where and how are lightmaps stored on Boyle Ext static geometry, and can they drive Bevy's lightmap path?
- **Evidence:** Hex of `L_Boyle_Ext_Env` `StaticMeshActor_1737.StaticMeshComponent0` after its properties
  (offset 0x218): LOD count 1, one ShadowMap2D ref, zero shadow vertex buffers, `LightMapType = 2`,
  6 light GUIDs, then 3 Ã— (object ref + 3 floats), UV scale/bias, then a single zero byte that ends
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

## 2026-10-04 â€” Cross-package materials (U-005) and Arkane material slots
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

## 2026-10-04 â€” Map coverage: streaming levels, BSP, non-static actors (lead request)
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

## 2026-10-04 â€” Reflection cubemap and FX cards
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

## 2026-10-04 â€” Dynamic lights
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

## 2026-10-04 â€” Atmosphere and post
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

## 2026-10-04 â€” Skydome, moon and backdrop cards
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

## 2026-10-04 â€” Fidelity harness
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

## 2026-10-04 â€” Frame time
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

## 2026-10-04 â€” HUD rebuilt from the game's Scaleform movie
- **Question:** Can the in-game UI match the original without hand-drawing it?
- **Evidence:**
  - `UI_HUD_SF.upk` holds `UI_HUD.HUD`, an uncompressed GFx v8 movie (313 KB; stage 1280x720), plus 11 `Texture2D` atlas pages.
  - Tag 1009 (external image) uses 32-bit ids. Atlas pages are `0x0009_xxxx`, and tag 1008 (sub-image) names its page by the low 16 bits; truncating ids to 16 bits made the standalone images collide with the pages.
  - The root timeline places `masterHUD_mc` at (95.5, 54). Inside it, `_healthMeter_mc` is rotated 15Â° and `_manaMeter_mc` 30Â°, both drawn through sliding `mask_mc` layers with `upgd2/4/6` labels, beside `_equipment_mc`. It also places `_dot_mc` at centre (with exported `crosshair_gun/_crossbow/_power`), `playerStatesIcon_mc` bottom-left, and `objectives_mc`/`targetNotification_mc` top-right.
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

## 2026-10-04 â€” Audio containers and codecs
- **Question:** How is the game's audio stored, and which codecs must be decoded?
- **Evidence:** 503 `.pck` files (base + DLC) start with `AKPK`; each holds one soundbank (BKHD version 65)
  and a stream table. `audio_dump survey`: 3,265 embedded + 1,908 streamed media files, all RIFF/WAVE with
  format tag `0xFFFF` (Wwise Vorbis), 0x42-byte `fmt ` with the `vorb` block at +0x18; no PCM/ADPCM/XMA.
- **Experiment:** AKPK/bank/WEM readers in `crates/dishonored_audio`; layouts in `dishonored/AUDIO_FORMAT.md`.
- **Result:** Every package and bank parses; codec survey as above.
- **Confidence:** CONFIRMED
- **Implementation consequence:** Only Wwise Vorbis needs a decoder.

## 2026-10-04 â€” Wwise Vorbis codebooks come from the game executable
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

## 2026-10-04 â€” Event ids and names
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

## 2026-10-04 â€” HIRC v65 resolution
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

## 2026-10-04 â€” Runtime audio
- **Question:** How should simulation events become sound, presentation-only?
- **Experiment:** `crates/dishonored_runtime/src/audio.rs`: worker-thread load + decode, `SimEvent` mapping,
  movement-generated footsteps (surface from the colliding mesh name), local player 2D, others 3D with
  bevy_audio panning and the Wwise dry-volume curve, level bed/score and placed emitters (culled beyond their
  attenuation range). `--no-audio`, `--volume V`.
- **Result:** `harness audio` passes (decode sanity per codec class, every cue resolves, Boyle ambience).
- **Confidence:** APPROXIMATION for footstep surfaces, stride lengths, ambience looping and emitter selection.
- **Implementation consequence:** Original-game captures (U-011) are needed to tune footstep cadence/levels.

## 2026-10-05 â€” Frame time: lightmap slots, render views, batching
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

## 2026-10-05 â€” How Dishonored builds missions; custom mission framework
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
## 2026-10-05 â€” Original navigation is a UE3 navmesh (replaces the invented waypoint graph)
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

## 2026-10-05 â€” NPC perception and behaviour from Arkane's AI tweaks
- **Question:** Which original data drives NPC perception and reactions, and how close can `dishonored_ai` follow it?
- **Evidence:** pawn tweaks (`Twk_Pawn_Boyle_Party.*`, level script packages) -> brain tweaks (`AI_BrainTweaks_Elite.BrainTweaks_Guard_Elite_Party`, `AI_BrainTweaks_Civilian.BrainTweaks_Civilian`) -> attention process -> `DisTweaks_PawnAttention` profiles (`Twk_Attn_Guard_Unsuspecting/Suspecting`, `Twk_Attn_Default_Neutral`, `Twk_Attn_Noncombatant_*`), `Twk_Vision_Default`, behaviour/sub-state tweaks (Search, Panic, Combat, TriggerAlarm, Guard), `DefaultAI.ini` (noise loudness table, reaction delay, search crumbs, skirmish caps), `DefaultNPC.ini`. Enums via `enum_dump`. Arkane tweak inheritance (`m_FallbackChainCooked`, `m_FallbackSkip`) now resolved by `TweakDb`.
- **Experiment:** `AiConfig` loader (`dishonored_assets::ai_config`, retail test checks it against the documented values); brains rewritten around per-stimulus attention with the original thresholds/leaks/increases, vision cones, noise contexts, reaction delay, Notice/Investigate/Search/Combat/Panic behaviours, skirmish caps, flee points and help barks (`research/dishonored/AI.md`). Harness runs before (`harness_out/baseline_*`, binaries built at b23dec7) and after.
- **Result:**
  - Party civilians panic on AlarmBell/Help/DeathRattle noises, not on gunshots by ear (raw `m_ScaryNoises` arrays), plus seen enemies, deaths, corpses, fighting guards and fleeing guests.
  - Players are neutral guests until seen attacking (neutral profile only head-tracks).
  - Harness (3-minute Contracts, same seeds): contract kills 10 -> 7, pursuits 14 -> 9, guard-combat ticks 4,565 -> 13,685, civilian-flee ticks 13,792 -> 206,173 (panics last until attention leaks below the Investigate stop: 0.015-0.025/s), NPC deaths 1 -> 5. Mission: completes (host at 13 s, done at 23 s) but the assassination is now witnessed (wider original outer cone 160 deg / 1700 uu): detected 7, alarms 2, one agent death.
- **Confidence:** data CONFIRMED; vision-rate combination SPECULATIVE (U-041); no-amount increase semantics LIKELY (U-042); patrol topology, attack patterns, alarms OPEN (U-043, U-044).
- **Implementation consequence:** `SimConfig.gameplay.ai` (cache `gameplay_config.v2.json`) feeds every crowd at match start; mission `Alert`/`Calm` commands go through the attention model.
## 2026-10-05 â€” Menus, settings and the launcher
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
- **Evidence / method:** Ghidra on `Dishonored.exe` (70da9613â€¦). New helpers in
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
- **Findings** (details in `dishonored/ANIMATION.md` Â§2-Â§11, `animation/EDGE_ANIM.md`):
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
  - The uniforms come from each shader map's `FUniformExpressionSet`, serialized in the cache as arrays of (type FName, payload) expressions; the type names are FNames 512-528 of the cache's name table (`FMaterialUniformExpressionAbs` .. `VectorParameter`). A research script (`research/ghidra/uexpr_dump.ps1` (on the cache decompressed with `SHADER_DUMP_RAW`)) finds `ScalarParameter(Dist_Distortion_Intensity)` (41 shader maps) and parses the vector / scalar arrays before it. The mesh-emitter permutation that matches shader 36094: vectors `SelectionColor`, `D_DiffuseColorTweak` (c0), `MeshEmitterVertexColor` (c6), Append(frac(Time x `Dist_Distortion_Speed`) x 2) (c7); scalars Time, `U_Scrolling_Speed`, U x Time (c8.z), `V_Scrolling_Speed`, V x Time (c9.x), `Db_Depth_Biased_Alpha_Intensity`, the pan terms, `Dist_Distortion_Intensity` (c10.z); textures `O_Opacity`, `Dist_Distortion`. Hence D = `O_Opacity.r`(uv + scroll) x `D_DiffuseColorTweak.a` x `MeshEmitterVertexColor.a` x vertex colour x `Dist_Distortion.r`(uv + pan) x intensity (per-permutation details: U-400).
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
## 2026-10-06: FaceFX lip sync (U-204)

- **Question:** What do the 44 Boyle `InterpTrackFaceFX` tracks hold, where is the lip sync data, and how does it move the head?
- **Method:** new `export_hex` tool on the cooked tracks, `FaceFXAnimSet` and `FaceFXAsset` exports; the archive layout read from the bytes (string tables, counts, record sizes checked against the next record), then a reader decoding every Boyle archive to its last byte (`facefx_dump`); `obj_dump` / `skel_dump` to tie actors to head meshes and to compare the reference pose with the mesh bind pose.
- **Findings** (`dishonored/ANIMATION.md` Â§12): the cooked FaceFX tracks are empty (12 bytes each, 18 + 16 + 10). Lip sync is the voice data's `FaceFXAnimSet`: one animation per blurb GUID, Hermite curves on face graph nodes, blend 0.16 / 0.22 s. The head mesh's `FaceFXAsset` holds the master bone list (reference, inverse reference rotation, bone pose links with position delta and absolute rotation) and the compiled face graph (combiners and bone poses, linear and type-2 links; ranges with their reciprocals). FaceFX is right-handed: Y mirrored against the mesh.
- **Port changes:** `dishonored_assets::facefx` (reader, curve / graph / bone evaluation); `DialogueLineView.blurb`; `runtime::facefx` (anim index over the map's packages, actor per model from its parts' `FaceFXAsset`, line start clock); `npc_anim::animate` applies the bone offsets after the face slot, before look-at.
- **Tests:** `dishonored_assets` `facefx::hermite_and_links`, `facefx::boyle_facefx` (every Boyle actor and set decodes; servant graph wiring, jaw open about 20 degrees, a servant line opens the mouth; skipped without the install).
- **Confidence:** CONFIRMED for the layout, the empty tracks and the actor / set links; LIKELY for the blending rules and for playback being tied to the blurb (U-380).

## 2026-10-06: Go-to completion in the locomotion component (U-221)

- **Question:** Shaw's `AIGoToActor_0` stops 84 uu short of `TargetPoint_5` against a 4.4 uu lip. Does the original end the go-to farther out (`DisLocoRequest.m_fEndLocationThreshold` -1, `m_bAccurateStop`)?
- **Method:** Ghidra: `ClassDecomp2` on `DisBehaviorGoTo(_Base)`, `Decomp` on the go-to callbacks and the request setters (0x00cba540 / 0x00cba670 and callees), `StrUsers` on `ArkCpntType_Locomotion`, `F_FuncCallers` (0x0093dd50), `DecompRange` over the locomotion component (0x00930000-0x0094d000) and the go-to code (0x00ae4000-0x00af1800), `OffsetScan` for `m_bDisableTeleportOnNavmesh` (+0x966); `obj_dump` of the locomotion configs, the duel ops, `TargetPoint_5`, `TriggerVolume_2` and the garden meshes; new `nav_dump path`.
- **Findings** (`dishonored/AI.md` "Go-to completion"):
  - An actor destination always requests an accurate stop (0x00ae4de0 -> MoveToActor with accurate 1); `AIGoToActor_0` keeps the actor (`m_bTrackMovingTarget` default).
  - The component's destination is the last point of the found path (0x00949890). Arrival (0x00944390): within 0.1 uu for accurate requests; within `m_fUnaccuracyRadius` (120) at speed below 20 only for non-accurate requests whose speed mode has stop animations. Event 0x18 then marks the behaviour reached (0x00ae4ea0).
  - Off-mesh pawns return to the nearest navmesh point at `m_fReturnToNavMeshSpeed` unless `m_bDisableTeleportOnNavmesh` (0x00b7cd10, 0x0093ec80).
  - The target lies on navmesh poly 594, and the garden mesh with the lip has no `BodySetup`. So the original must physically reach the target.
  - NPC movement over the lip: NPCs walk on the navmesh. `DishonoredGameInfo.m_bNewNPCPhysWalking` (default true) sends the NPC pawn's physics (0x00b4e9f0) to the locomotion component (0x00943d00 -> 0x00940d80). That move adds `Velocity x dt` and takes the height from the navmesh plane. It sets `Location` directly, with no world sweep, whenever the end point is on the navmesh (0x0068f660). Lips below the step height never block an NPC.
  - The duel's go-tos: `AIGoToActor_0` (`Shaw_Elite` -> `TargetPoint_5`), `AIGoToActor_1` (`Lord Shaw` -> `TargetPoint_9`, inside `TriggerVolume_2`, the "Shaw On Mark" trigger), `AIGoToActor_2` (`Shaw_Elite_Pistols` -> `TargetPoint_55`).
- **Port changes:** `movement::nav_walk_move` with the `PlayerState::nav_walk` flag. The mission host sets the flag for NPCs that move along a held navmesh path. `nav_dump path`.
- **Harness:** `kismet --route duel` (the one targeted debug run): 6/6 checks.
  - Before: `AIGoToActor_0` never completed (stuck 84 uu short).
  - Now: pawn 2007 arrives for `AIGoToActor_1` (79.2 s), then `_0` and `_2` (86.2 s). The loop sends `Go_to_Mark` / `Stop_stand_there` at 86-87 s.
  - The countdown is still not reached. All three go-tos move the same pawn, so Lord Shaw is taken off `TargetPoint_9` / `TriggerVolume_2` to `TargetPoint_5`, and "Shaw On Mark" is never set (next lead, U-221).
- **Confidence:** CONFIRMED for the request fields, the arrival test, the constants (0.01 squared at 0x010a5590, radius 120) and the navmesh-walking path and flag. LIKELY for the record layout linking `m_bAccurateStop` to record +0x24 (from the builder 0x0093ddd0 and the record copy in 0x00945010 / 0x009450c0). APPROXIMATION for the port's navmesh walk (poly by position, no actor box check).

## 2026-10-06: FaceFX bone blending from the binary (U-380)

- **Question:** How does the original blend FaceFX bone poses and combine them with the animation (U-380)?
- **Method:** exe string scan (FaceFX RTTI, the link function formula strings, `SHOWFACEFXBONES`); Ghidra `StrUsers` (console command 0x006C75A0 -> flag 0x01448FD4), `Decomp` on its users, `OffsetScan` for `SkeletalMeshComponent.FaceFXActorInstance` (+0x390), `Decomp` of the 0x390 users; `prop_layout` / `obj_dump` for `FaceFXBlendMode` and its default.
- **Findings** (`dishonored/ANIMATION.md` Â§12): 0x00725EA0 is the FaceFX update. Position = reference + sum of value x delta. Rotation = deltas `slerp(ref, pose, w) x ref^-1` multiplied onto the reference (true slerp when the dot is at most 0.55, else lerp), then normalized. Additive blend mode (the engine default, not overridden): position += FaceFX - mesh reference; rotation = (FaceFX x mesh reference^-1) x animated, after the whole AnimTree. Link function types follow the registration order of their classes; one-clamp is `1 if x <= 1 else 1 / x` (the reader had it wrong; unused in Boyle).
- **Port changes:** `facefx::bone_poses` (replaces `bone_deltas`) with the original slerp; the runtime builds additive offsets against the bind pose and applies them after the AnimTree layers, pre-multiplied.
- **Tests:** `dishonored_assets` facefx tests (slerp branches, one-clamp, jaw open about 20 degrees through the new blend).
- **Confidence:** CONFIRMED for the blending and the combine; graph evaluation and blend in / out remain LIKELY.
- **Follow-up (same day):** the face graph update (0x00EFBB00) and the tick's curve evaluation (0x00EF5640 -> 0x00F00B50, Hermite 0x00F01070, linear 0x00F01130) decoded. Curves are written raw (the port's blend in / out ramp removed). Node input operations are sum / multiply / max / min with the identities at 0x0120B91C; corrective links scale the node by `1 - c`; the clamp comes last. The reader keeps the stored 1/min and 1/max for corrective links.
- **Follow-up 2:** the blurb start (0x00C9FBF0) plays the blurb's FaceFX anim through `PlayFaceFXAnim` (0x00756EF0) on the speaker's FaceFX component, with the conversation component as the audio handler; the VO is posted when the anim starts (0x00CA0120), so lip sync and VO start together (the port's line-start clock). FaceFX registers have no script callers.
## 2026-10-06: Climbing on DisClimbable chains (U-240)

- **Question:** How does `StatePlayerMasterClimb` move the player on a chain, how is it entered
  and left, and where do the chain's bones come from?
- **Evidence:** `ClassDecomp2` of `StatePlayerMasterClimb` (enter 0x00A72B00, exit 0x00A6E2C0,
  transition test 0x00A6E570, HUD contexts 0x00A67830), `DisClimbable` (collision notify
  0x00A3FA70, physics change 0x00A1E800, tick 0x00A2BFD0) and its interface vtables (`Interact`
  0x00A23130, visibility 0x00A1E6F0); `Decomp` of `physLadder` 0x00AA82C0 with its callees
  (`FindClosestBone` 0x00713130, `GetBoneLocation` 0x00732CD0, `VInterpTo` 0x0041E860,
  `TwoWallAdjust` 0x0065FC60), the Use handler 0x00AD5BA0, let-go 0x00AAA030, the Jump state's
  enter / exit 0x00A75EE0 / 0x00A5FFA0 (found by an `OffsetScan` for `m_pClimbable` +0xAA0);
  `prop_layout` offsets (pawn +0x8A0 bits 6/16/17, +0xAA0, +0x1238 `m_ClimbingAimThreshold`;
  camera +0x33C = `CameraCache.POV.Rotation.Pitch`; component +0xE4 = `Bounds.SphereRadius`);
  class records 0x01300048 / 0x01300570 / 0x01300A98 / 0x01300200 for the climb, falling, jump
  states and `DisSeqEvent_Climb`; the two chains' actors, meshes and physics assets (`obj_dump`,
  `skel_dump`).
- **Result:** see PLAYER.md "Climbing". In short: grab by Use, by touch while not walking, or by
  Blinking into the chain; `physLadder` zeroes the velocity and, with any input, moves the height
  target by `0.0005 x 16383.5 = 8.19` uu (up when the input is within `acos 0.9` of the facing,
  down otherwise; `m_bCameraRelativeClimbing` reverses it when looking down past
  `m_ClimbingAimThreshold` 1000), clamped to `[anchor.z - 2 x SphereRadius, anchor.z -
  CollisionHeight]`, and follows the bone nearest 2/3 of the height above the centre, 30 uu
  behind and 10 uu to the right, by `VInterpTo` at speed 20: 163.8 uu/s. Every exit sets
  `Velocity = facing x 525` (x2 under something within 200 uu); jumping adds `JumpZ` 800 and keeps
  the chain held until the jump ends, then its collision returns after 0.3 s; Use lets go
  (`m_bLettingGoOfClimbable`). Both are refused for `m_fClimbReuseDelay` (0.5 s,
  `Twk_Pawn_PlayerInput`) after the grab. No automatic exit at either end. Chains: Int
  `DisClimbable_1` (-3153, -420, -627), `chain_400` (6 bones, sphere 198.04, centre range
  -1023.1 .. -714.5); Ext `DisClimbable_3` (-2085, 580, 1884), `chain_1400` (16 bones, sphere
  697.57, 488.9 .. 1796.5). No Boyle Kismet uses `DisSeqEvent_Climb`.
- **Note:** the property reader returns a cooked `Matrix` (`KBoxElem.TM`) as 64 raw bytes starting
  one dword early (rows at +4, the last 1.0 cut off); `dishonored_assets::climbable` reads it that
  way.
- **Confidence:** CONFIRMED for the movement, limits, exits and data; LIKELY for the Use target
  rule (the pickups' cone), the ceiling-trace reading (U-443); the camera-relative flag's setter
  is unknown (U-440: no AND/OR write of +0x8A0 bit 16 or byte +0x8A2 bit 0); chains are rest poses
  (U-442). `m_bLettingGoOfClimbable` is cleared only by `StatePlayerMasterFalling`'s exit
  (0x00A63110, `FindFieldWrites` +0x8A0 / 0x40; U-441 resolved); the Jump state's +0x54 bit 0 is
  `m_bJumpingFromChain`.
- **Implementation consequence:** `MoveMode::Climbing`, `dishonored_sim::climb` (tests
  `crates/dishonored_sim/tests/climb.rs`: grab, up / down speeds and limits, camera-relative
  hysteresis, jump off with and without a ceiling, let go, touch grab, top exit over a ledge,
  determinism); `WorldQuery::climbables` with `CollisionWorld.climbables` loaded by
  `dishonored_content::load_climbables` (cache `<map>.climb1.json`); config
  `movement.climb` (cache `gameplay_config.v8.json`); `SimEvent::Climb` / `ClimbSound` (the
  runtime posts `Snd_P_Chain_Climbing`); the mission view lists the chains' bones as "Grab"
  usables and the HUD shows "Jump off" while climbing; net protocol 4. The state hash mixes the
  climb state only once a pawn has touched a chain, so existing replay hashes are unchanged
  (`harness sim`, debug build: 19/19, brawl hashes identical). Real-map test
  (`dishonored_content` `climb_boyle_chains`): dropped beside either chain, the pawn grabs it
  and climbs to z -722.0 (Int; the limit -714.5 is not reached because the beam the chain hangs
  from blocks the pawn's top) and 1786.0 (Ext; limit 1796.5), and down to the lower limits.

## 2026-10-06: Slide impacts (U-248)

- **Method:** `class_layout` of `DisTweaks_PlayerPawn` (slide fields +0x23C..+0x250), `OffsetScan`, decompiles of 0x00A79BF0 and its caller 0x00A7B220; Corvo's tweak values.
- **Findings:** each slide tick, until bit 0 of the state's +0x70 is set, a line trace of 75 uu along the velocity; the first actor hit (not immune, not asleep) takes its class's `m_SlideDamageInfo` damage (`DishonoredNPCPawn` 0, rats 0, `Actor` 5) with momentum velocity x 0.15 (`m_fSlideImpactDamageVelRatio`), damage type `DisDamageType_Slide`; the loop breaks after that actor. Who sets the flag is not traced (LIKELY: the hit, so one impact per slide).
- **Port:** `MovementConfig.slide_*` (cache `gameplay_config.v10`), `SlideState.hit`, `movement::slide_impacts` (segment vs pawn cylinders). In multiplayer a slide into another player costs 5 health; NPCs are shoved, not hurt.
- **Tests:** `mechanics::slide_impact_hurts_another_player_once`, `slide_impact_pushes_npcs_without_damage`.
## 2026-10-06: Ranged weapon dispersion: the shot's screen offset (U-247)

- **Question:** How does `DishonoredWeapon_Ranged.m_fDispersion` turn into a projectile direction, and in what unit?
- **Method:** Ghidra on the player's fire path: `ClassDecomp` of `DisItemContext_ProjectileAttack`, `_FirePistol` and `_FireCrossbow`; `Decomp` of the pistol fire (slot 108, 0x00C2A3E0), the aim helpers 0x00C14640 / 0x00AA2B10 and the offset generator 0x00BFFBA0; `OffsetScan` for the weapon's +0x11C / +0x12C and the pawn tweak's +0x2AC / +0x2B0; `VCallers` / `G_Callers` for slot 98; `tweaks` for the cooked attributes and upgrades; `prop_layout` for the `m_bMaxDispersionOn*` bits.
- **Findings:**
  - The projectile context fires queued shots on its camera update (slot 103, 0x00C03A60): for each, the item's fire function, then the weapon's slot 109 (dispersion = max). A shot uses the dispersion it had, then it blooms.
  - The pistol fire takes the aim point (screen centre, or the aim-assist point from 0x00C14460) and draws an offset (0x00BFFBA0: radius `rand() x d / 32767`, angle `rand() x 2 pi / 32767 - pi`, `d` = slot 91 = `m_fDispersion`). It deprojects aim point + offset x `height / 720` through the view (0x00AA2B10 -> 0x006C5180). The unit is pixels of a 720-line screen: 45 px is 3.09 degrees at the default 75 degree FOV on 16:9.
  - `m_bMaxDispersionOnSprint / Jump / Block / Melee` are bits 0..3 of the ranged tweak's +0x118; the falling state's tick (0x00A79150) tests bit 1. Corvo's pistol and crossbow set Sprint, Jump and Block.
  - Values: pistol 45 / 15 / 70 (max / min / speed), crossbow 30 / 0 / 70. Accuracy upgrades apply to their host weapon only (`m_pHostItemType`).
  - No player code reads `m_fMovementDispersionIncrease` / `m_fCameraMovementDispersionIncrease` (U-422). The crosshair's `SetDispersion` script is not decoded (U-420). Slot 98's caller is not found (U-421).
- **Port:** `LeftHandState.dispersion`. `left_hand::update_dispersion`: equip and shot set max; the held states keep max; `FInterpConstantTo` toward min while loaded. `dispersion_offset` takes two rolls of the pawn's stream. `offset_direction` uses a fixed reference view (75 degree FOV, 16:9). `fire_direction` serves the pistol, crossbow, sleep darts and incendiary bolts. `CombatTuning.pistol_dispersion` / `crossbow_dispersion` come from the `disp.*` weapon values (cache `gameplay_config.v9.json`). `powers::attribute_for` filters upgrades by host. Net protocol 5 (`LeftHandState` grew).
- **Tests:** `crates/dishonored_sim/tests/player.rs`:
  - narrowing to 15 in 0.43 s, a shot inside the 15 px cone, then 45; no narrowing while empty;
  - held at max falling and sprinting; upgrades per weapon;
  - offsets inside the radius; 45 px = 3.09 degrees; screen axes.

  `cargo test -p dishonored_sim`: all pass.
- **Confidence:** CONFIRMED for the formulas, the order and the data; the reference view is an APPROXIMATION of the player's real view.

## 2026-10-06: Lord Shaw's duel reaches the countdown (U-221)

- **Question:** Why do the duel's three go-tos all move Lord Shaw (pawn 2007), leaving him off his mark?
- **Method:** `kismet find` on the Spawned events, the go-tos and their variables. `harness kismet --route duel`, extended to report the three variables and each spawner's pawn. Reading the interpreter's populate / publish.
- **Findings:**
  - The spawner -> pawn mapping and the Spawned events were right: `_45` -> 2007, `_11` -> 2004, `_46` -> 2008. Yet all three variables held 2007.
  - Many duel ops (`AISetSuspicionLevel`, `AISetPatrol`, ...) link their `Targets` array to two or three of Shaw's variables. `Kismet::publish` (`PublishLinkedVariableValues`) wrote the whole array into each variable, and `write_var` keeps entry 0. So every variable became the first one, Lord Shaw. UE3's `USeqVar_Object::PublishValue` copies element i into variable i.
  - Fixed: `runtime::publish_values`. When the counts differ (the port's populate skips empty object variables), nothing is written back.
  - Run after the fix: the elites go to TargetPoint_5 / _55 and Shaw to TargetPoint_9. Shaw touches TriggerVolume_2 at 90.1 s ("Shaw On Mark"). `Stop_stand_there` and `Go_to_Mark` play (`Go_to_Mark` is refused once while `Stop_stand_there` plays: equal priority, the decoded rule; the loop retries), then `Three` (92.8 s), `Two`, `One` (97.8 s). "turn off mark trigger" completes PN T2.
  - Next blocker: at 98.5 s Shaw is in `BrainState::Flee` and leaves his mark; `Turn_and_Fire` never plays. `Twk_Pawn_LordShaw` falls back to the elite guard pawns, with a sword and `Twk_Inv_PistolShaw` in `m_ContentInventoryLoadout`. The port classes him Civilian from the tweak name ("party").
  - Navmesh walking (`movement::nav_walk_move`, decoded by the FaceFX agent, commit c05d9e1) is in this branch with a unit test.
- **Harness:** `kismet --route duel` (debug build) 6/6. The open stage now reports the countdown up to `One` (before: only `Duel_Explained`).
- **Confidence:** CONFIRMED for the publish semantics (UE3 engine behaviour) and the run; the panic's cause is SPECULATIVE.

## 2026-10-06: Bloom parts, DOF downsample, sky dome, local distance-field shadows, ragdoll walls (U-323, U-325, U-403, U-282)

- **Question:** What do the original's bloom, far-DOF source and sky dome compute, can the
  dominant point / spot lights use their distance-field shadow maps like the moon, and can the
  presentation ragdoll stop at walls?
- **Method:** `shader_dump` of `GlobalShaderCache-PC-D3D-SM3.bin` (every shader is preceded by its
  type string: 52 - 55 `FKuwa*`, 56 - 63 ArkPp DOF, 64 - 71 ArkPp blurs, 82 - 87 bloom); new
  `tools/package_dump/src/bin/shader_maps.rs`, which walks the RefShaderCache's material shader
  maps (friendly name, static parameter set, `(type, GUID)` entries resolved to shader numbers,
  uniform expressions; layout in RENDERING.md); `fxc /dumpbin`; Ghidra `StrUsers` on the shader type
  strings and `Decomp` of their users (0x009251A0, 0x0091FE50, 0x0090DE60, 0x00922210,
  0x0090EAB0, 0x00924F00); `obj_dump` / `mic_static` of the Boyle sky MICs and dominant lights.
- **Bloom (U-323):** 0x009251A0 draws the primitives with bloom relevance with their
  `TBloomPartMeshPixelShader` into a scene-size target (LogLuv RGBA8: the 64 bloom shaders all end
  in the LogLuv encode, matrix 0.2209 / 0.3390 / 0.4184 ...), then at quarter size
  `FBloomDownSamplePixelShader` (four taps at +-1/4 of a quarter pixel, decoded, averaged, x
  `gBloomTint.rgb` + `gBloomTint.w`, max 0) with `gBloomTint` = (m_Tint.rgb x m_Scale, -m_Threshold
  x m_Scale) of `ArkPpBloomParameters` (0x0090DE60 reads +0x6C..+0x80 of the settings), then
  "ArkGaussian" (0x0091FE50): `TBloomBlurPixelShader` once along X and once along Y (taps 0,
  +-1.3973, +-3.3973 texels, weights 0.2416, 0.2643, 0.1149). The result is added after the fog
  by the DisFog shaders' `sBloomParts` (or `FBloomComposePixelShader` = scene + bloom). Bloom-part
  colour of the generic bases (shader maps of `generic_PMAT` / `generic_masked_SingleSided_PMAT`
  with `B_Enable_Bloom`): `E_Emissive` (masked) [x `E_Emissive_Color.rgb x a` with
  `E_Enable_Emissive_color`] x `B_Bloom_Intensity` + `SelectionColor`; without `E_Enable_Emissive`
  `0 x B_Bloom_Intensity`; maps without bloom use one shader that outputs `SelectionColor` (0).
  Boyle Ext: m_Scale 0.4, m_Tint (1, 0.953, 0.723).
- **DOF source (U-325):** 0x00922210 runs `FArkPpDofDownsamplePS` twice (full -> half -> quarter);
  the pixel shader is a single bilinear tap at uv + `gResolutionParams.xy` (= 1 / size, 0x0090EAB0),
  the corner of a 2x2 block; the uber shader samples the quarter copy at the pixel's own uv. The
  box-blur pixel shader (68) is a stub writing magenta.
- **Sky dome:** the Boyle Ext sky (`Boyle_2_inst`: storm, clouds, clouds 2, gradient, UV channel 2)
  and Boyle Int sky (`Boyle_01_inst`: clouds, UV channel 2) shader maps decode to the formula in
  RENDERING.md (cloud layers rotated by `Speed x Time x 0.1`, gradient over `|v + clouds.r x
  distortion|^power` or `D_Diffuse` on UV1, cloud lerps, `^sky_contrast`, + storm). The old
  approximation was texture x (D_Diffuse_color + mean gradient).
- **Local distance-field shadows (U-403):** Boyle Ext has `DominantPointLight_1` (radius 900) and
  `DominantSpotLight_0/2`, Boyle Int `DominantSpotLight_0`; their `LightGuid`s match components'
  `ShadowMap2D`s like the moon's.
- **Port:** `bloom.rs` / `bloom.wgsl` (parts collected per material at level spawn as world-space
  triangles, depth-tested against the scene depth into RGBA16F; downsample, blur; `--no-bloom`);
  `ark_post` split into DisFog (+ bloom), two 2x2 downsamples and the uber pass
  (`ark_downsample.wgsl`); `sky.rs` / `sky.wgsl` (`SkyMaterial` swapped in for Skies_PMAT domes;
  the import keeps their four textures); import of `Light::guid`, `LightmapAtlas::shadow_local`,
  `InstanceLightmap::df_light`; lightmap batches split by local light, `lightmap_ext.wgsl` finds the
  light in the fragment's cluster and swaps Bevy's shadow factor for the field
  (`DISHONORED_LIGHTMAP_DEBUG=shadow_local`); ragdoll walls: the steep (|n.z| < 0.7) collision
  triangles within 128 uu of the corpse are copied into pawn space at the hand-over and each body
  is pushed out of them as a sphere (two-sided, losing its velocity into the wall).
- **Tests:** `bloom::tests::bloom_colour_follows_the_switches`, `sky::tests::*`,
  `ark_post::tests::*`, `ragdoll::tests::chain_falls_against_a_wall` (without the wall the chain
  crosses the plane; with it every body stays a radius in front; two runs bit-identical),
  `ragdoll::tests::walls_keep_only_steep_triangles_and_push_out`.
- **Confidence:** CONFIRMED for the shader maths and the setters read; LIKELY for the bloom parts'
  depth state and target filtering (assumed depth-tested, 4x4 box), which local lights use the measured
  DF scale 3.125 vs 100 (U-461; the moon now uses the measured 3.75, U-401). Open: U-520 (sky terms not used by Boyle), U-521 (other bloom bases), U-522 (Kuwahara
  node).
## 2026-10-06: Fast reload, random bone charms, helmets, the rat swarm tick, body drops (U-246, U-247, U-249, U-251, U-260, U-261, U-106)

- **Questions:** how `Attribute_FastReload` changes reloads; what Twist of Fortune and Scavenger do; what NPC helmets absorb; the order of the native rat swarm's decisions and how many limbs rats eat; whether bodies can be thrown.
- **Method:** Ghidra pool: `FuncDecomp` with `ref:` seeds on the attribute FName globals (`FastReload` 0x0145DF10, `PotionManaBonusChanceRatio` 0x0145E0F8, `StatPickupCapacityBonusChance` 0x0145E188, `FoodHealBonus` 0x0145DF28, `ThrowStrength` 0x0145E1B8); `ClassDecomp` of `DisItemContext_ReloadGunPlayer`, `DisItemContext_FireCrossbow` / `FirePistol`, `DisWepCrossbow`, `DishonoredWepPistol`, `StatePlayerCarryCorpseIdle`; `OffsetScan` on `DisTweaks_Armor` (+0x91 / +0xA0) and `DisRatSwarm` (+0x290..+0x310) offsets; `DisasmAround` on the data references of the swarm functions (a static initializer filling the state table at 0x01471B30); `Vtables` of `DishonoredPawn` / `DishonoredNPCPawn` (rat-target interface vtables 0x011197C0 / 0x011414D8); `DumpMem` on class records; `prop_layout` for the tweak layouts; `tweaks` / `obj_dump` for values; `anim_notifies` on `Ply_Pistol_as`, `Ply_Crossbow_as`, `CorvoGun_ReloadSpeed_as`; `find_export` over all 470 base and 434 DLC packages for `Twk_Inv_HelmetBase`.
- **Findings** (PLAYER.md "Reloading" and "Armour", INVENTORY.md "Bodies" / "Devouring Swarm" / "Passive powers and bone charms", AI.md "The swarm tick"):
  - Reload context: ReloadIn action, then `m_fReloadTimeRemaining = m_fReloadTime / ReloadSpeed` (tweak +0x134: 0.75 pistol, 1.0 crossbow class default), the magazine loads when it runs out, then ReloadOut. With `Attribute_FastReload` the `m_ReloadFast` action plays, its timer is the attribute (pistol Reload 2: 1.0, crossbow upgrade: 0.3) and the context ends when it runs out. A crossbow shot that leaves a bolt plays `m_FireReloadFast` (0.625 s at rate 1.5) and loads at its end.
  - Twist of Fortune: after either elixir, `rand() / 32768 < chance` adds `(int)(ManaMax x 2)` mana (capped). Scavenger: each ammo slot of a stat pickup holding more than one round offers one more on `rand() / 32768 < 0.5`. Food adds `(int)Attribute_FoodHealBonus` health.
  - Armour filter 0x00B78610: region, side (facing . hit direction) and type list -> damage 0. `Twk_Inv_HelmetBase` is not cooked anywhere, so the shipped helmets absorb nothing; only the Boyle Int overseer's mask and music box absorb.
  - Swarm: state machine 0x00C65380 (first true condition of Hidden, Flee, Attack, Move, Idle; exit / enter; update). Attack condition 0x00C87EA0 (groups per detected target, drops on escape / unreachable / too few rats, corpse -> eating with `m_EatRequiredRatCount`), update 0x00C89EE0 (chase / bite / eat), eat loop 0x00C6D760 (startup with the mesh swap, one limb per interval from the NPC's 6-entry `DisSeveredLimbInfo`, then one more interval; the clock starts at `-m_fEatStartupDelayAfterKill` after a kill).
  - Bodies: the corpse-carry state has no throw; `ThrowStrength` is only read by the carried-object release 0x00C45340.
- **Port changes:** `tuning::ReloadTiming`, `left_hand::gun_reload` (per-weapon FastReload / ReloadSpeed), crossbow fire recovery and fast fire; `inventory::twist_of_fortune` / `scavenger_amount` and the food bonus on the pawn's own random stream (drawn only while a charm is worn); `vitals::armor_absorbs`; `rats::{CORPSE_LIMBS, eat_duration, eat_clock_start}`; `powers::step_swarms` keeps its target within `m_fPawnEscapeRadius`, needs `m_EatRequiredRatCount` rats to eat and restarts the bite counter while out of reach (`SwarmLevel.escape_radius` / `eat_required_rats`, config cache `gameplay_config.v12u`); `dishonored_ai::rats::LIMBS` 5 -> 6 plus the last interval; carried bodies are dropped on attack instead of thrown; spawner armour rolls set `PlayerState::helmet` (`AccessorySlot::helmet`, `Appearance::helmet`, cache `appearance4`; `OriginalNpc::helmet` in the mission layer).
- **Behaviour changes (before -> after):** pistol reload 1.20 -> 1.478 s (round in at 1.176 s), with Reload 1 0.60 -> 1.103 s, with Reload 2 0.006 -> 1.0 s; crossbow reload 0.97 -> 1.303 s and fire recovery 0.2 -> 0.398 s, with its upgrade a shot reloads in 0.417 s; Devouring Swarm level 1 eats a corpse in 12 s (+0.5 after its own kill) instead of 8.0 s, level 2 in 6 s instead of 3.75 s, and no longer switches to a nearer pawn; ambient swarm brains eat in startup + 7 x per limb (was 5 x); attack while carrying drops the body (was a 500 uu/s throw); spawners with helmet armour (elite / city guards, thugs) now wear `helmet` (knocked off by head hits, Windblast, flare bolts).
- **Tests:** `cargo test -p dishonored_sim` (new `tests/gaps.rs`, 12 tests), `-p dishonored_ir`, `-p dishonored_ai` pass. No harness suite run (this round's rules).
- **Unknowns:** U-106, U-246, U-249, U-251, U-261 resolved; U-247 resolved (fast reload); U-260 partly resolved (the order; rat lights and `CombatRatSwarm` open); U-104 / U-111 updated; new U-500 (overseer armour), U-501 (crossbow reload time from the class default), U-502 (rats eating the living), U-503 (the swarm as one group), U-504 (attack while carrying).
## 2026-10-06: Matinee AI groups, root motion, animated attachment bones, the soiree camera, toggles (U-085, U-200, U-202, U-203, U-208, U-142)

- **Question:** How do matinee AI groups move (stage mark, root motion), do attachments follow the animated bone, how does the player group's animation reach the camera, what do toggle and visibility tracks drive, and where do conversation body gestures come from?
- **Method:** Ghidra: earlier `ClassDecomp` of the interp classes (`InterpGroupInstAI` vtable 0x01028c50, `InterpGroupInstPlayer` 0x01096fc0, `InterpTrackInstStretchAnimControl` 0x01096e80); `Decomp` of 0x0062bbc0, 0x00628460, 0x009082c0, 0x0062fed0, the key accessors 0x00901310..0x009014e0, 0x00612e20 / 0x00612e40, 0x00b4d440, 0x00b4d520, 0x0062aa60, 0x0061fcb0; `Vtables` of `DishonoredNPCPawn` (slots +0x2fc, +0x30c, +0x314, +0x4b4..+0x4cc, +0x508); `ClassDecomp` of `DishonoredPlayerPawn` and `StatePlayerMasterSoiree`; `class_layout` of the group instance classes. Data: `kismet matinees` (new: root-motion keys, toggle and visibility tracks with their bound objects over the three Boyle maps), `kismet matinee` (now with the baked tracks), `anim_root` (new `ANIM_ROOT_SAMPLE` / `ANIM_ROOT_SUMMARY` / `ANIM_ROOT_SKEL`), `fp_probe` (new `FP_PKG`), `obj_dump` of the boat, hull and emitters, `facefx_dump` of the seven Boyle Street actors.
- **Findings:**
  - `InterpGroupInstAI` in game never reads its stage mark (only the editor update path spawns a preview pawn there); init locks the pawn's `Gesture` / `Posture` / `Distraction` / `Face` requests, adds `GroupAnimSets`, and with a move track switches the pawn to `PHYS_Interpolating`. Root motion is per stretched key: the track instance (0x00905c70) hands the key's `m_MatineeRootMotionMode` to the group (+0x52), the group update gives it to the pawn (vfunc +0x30c, 0x00b4d440: translate -> `RMM_Translate` or the locomotion component, accel -> `RMM_Accel`). Key properties sit at key +0x34 (+0x38 root motion, +0x39 mesh translation, +0x3c bit 0 disable collision). Term restores physics and applies `SnapToRootBoneLocationWhenFinished`. CONFIRMED.
  - Boyle uses root motion in the distraction soirees' In / Out keys and the campfire guard; e.g. `Empty_Distraction_Piss_In` carries `root0_jnt` 72 uu forward (mesh +Z), `_Out` 72 back.
  - Samuel's boat: `Boat_Jnt` rocks 1.26 degrees with `SC_boat_Idle`; the hull `InterpActor_1` is hard-attached to that bone (`BaseBoneName`), the attachment sockets ride it (up to 5 uu). CONFIRMED.
  - The player camera is the mesh's camera bone (`m_CameraBoneName`, `camera_jnt`); `SC_boat` is a seated still pose: `camera_jnt` 50 uu back, 11 right, 59 down from `Sword_Ready_Idle`, looking 13 degrees right. LIKELY for the camera combination (U-542).
  - Distraction soirees show / hide hand props (glasses, cigarettes, lighters, feather dusters) attached to `RightHandWpn` / `LeftHandWpn`, and toggle emitters (cigarette smoke and puffs on `Puke_Spit`, lighter, piss, puke). Emitters default to `bHardAttach`; `FX_SmallBoat`'s `Emitter_179` keeps a cooked relative location 11.7 km from the hull (U-541).
  - No conversation gestures outside soirees: blurbs carry no animation, `DisBehaviorBlockingDialog` only faces the listener, no `eDisNPCAnimations` entry is a gesture, FaceFX bone poses are on `head_jnt` and the face. CONFIRMED (U-142).
- **Port:**
  - `dishonored_kismet::interp_bake` (Kismet load): attachment bones / sockets of animated skeletal mesh actors baked over the matinee (30 Hz, the presentation's pose) into `Matinee::bone_tracks`; root bone tracks of AI-group sequences (every set holding them) into `Matinee::root_tracks`. `Scene::xform` uses the baked bone; `Scene::root_motion` / `root_offset`; `AnimKey::root_motion` / `mesh_translation`. The KismetSetup grows by the baked tracks (no cache format: it is rebuilt at load; old replays read them as empty).
  - Server (`kismet_host/cutscene.rs`): AI pawns not placed by a move track or attachment move by their root motion (horizontal, pawn yaw and the NPC `RotOrigin`); the snap at the end.
  - Client (`runtime/cutscene.rs`, `npc_anim.rs`, `characters.rs`, `effects.rs`, `main.rs`): root-motion keys keep the mesh root at bind; props follow visibility keys and pawn bones (`PawnPoints`); toggle keys drive placed emitters (`effects::start_placed_emitter`, `FxInstance::stop`); the player group's full-body keys offset the first-person eye and the look keys clamp the view; the invented talk gestures are removed.
  - The soiree state's enter (0x00a66ff0) blends both hands' items to 1 (0x00aa1cf0, back to 0 at the exit 0x00a628f0); the client hides the first-person rig while the local player is held (LIKELY).
- **Tests:** `cargo test -p dishonored_kismet` (new: `attachment_follows_the_baked_bone`, `root_motion_of_stretched_keys`); `harness kismet --route lethal` (debug build, muted): 16/16, Samuel's intro releases the player at (-8147, 3486, -1953) standing on the rocking hull 2 s later, same-seed Kismet state hash 0f8de0da42af5f90 twice.
- **Confidence:** CONFIRMED for the group instance, root motion and attachment findings and the data; LIKELY for the camera offset and the root joint / mesh rotation conventions (U-543); APPROXIMATION for collision-free root motion (U-540) and server-side pawn-bone attachments (U-544).
## 2026-10-06: Runtime probe of the original game; first measurements (U-011)

- **Question:** Can the port's movement and lighting constants be measured on the running original instead of inferred from the decompiled code? In particular, is the halved jump apex (213 -> 107 uu) right?
- **Method:** New tools (research/dishonored/INSTRUMENTATION.md):
  - `probe_launch` starts `Dishonored.exe` suspended with a session token and injects `dishonored_probe_dll` (i686).
  - The DLL detours `UObject::ProcessEvent` (0x00470640) and patches d3d9's `CreateDevice`, then the device's Present / draw / pixel-shader slots.
  - It serves a current-user, local-only named pipe. `measure boot` loads Boyle Ext by console command; `measure jump|fall|run|sdf|views` drive Corvo by keys through `FViewportClient::InputKey` and sample the pawn every frame.
- **Findings:**
  - No ASLR (DllCharacteristics 0x8100), so the Ghidra addresses hold at runtime.
  - `GObjObjects` 0x01423630, `GNames` 0x01435674, `GEngine` 0x0144721C (`DishonoredEngine_0`).
  - There is no script `PlayerTick`. Rendering is threaded.
  - The keyboard comes from deferred window messages that are processed only with focus; DirectInput is for the mouse (0x009CA1C0).
  - Skipping the start screen leaves `LocalPlayer.ControllerId` at -1, and then no key reaches the player.
  - `bPauseOnLossOfFocus` pauses the world (`WorldInfo.Paused`) while the window is in the background.
  - `-nosound` crashes the game. The loading movie needs `-nomovie`.
  - Jump: launch 800 uu/s; apex 106.45-106.67 uu over 4 jumps (parabola fit 106.66, gravity 3000 = 2 x 1500); airtime 0.533 s. **Confirms the halved apex** (107, not 213).
  - Falling damage: an 800 uu drop does none and 868 uu does 1. Then 900: 3, 1000: 19, 1100: 32, 1200: 51, 1300: 58 of 70. This is consistent with damage above 2250 uu/s and `v ~ sqrt(4 g h)` (onset at about 844 uu), with a few points of frame-rate scatter (U-462).
  - Run 400, sneak 275, sprint 600 uu/s (sword sheathed, factor 1.0).
  - Slide: from 600 to 275 in 1.10 s on the cosine ease (434.7 uu/s at 0.555 s against 434.9 predicted), covering 483 uu (port 481); physics `PHYS_Custom`.
  - Distance-field shadows: the dominant directional light uses `DistanceFieldParameters` = (-0.366667, 3.75, 2.0) (port: 4). Spot lights use 3.125 and 100. Bias is always -0.5 + 0.5 / scale (U-401, U-460, U-461).
  - The light pass's `DiffuseOverrideParameter` / `SpecularOverrideParameter` are (0, 0, 0, 1).
  - `measure views` captured all 8 `harness fidelity` viewpoints (camera within 0.5 uu, FOV 75, HUD hidden) as reference images (kept in scratch, not committed).
- **Port:** No sim change. The measured movement matches the port. The SDF scale (3.75) is left to the presentation area (U-460).
- **Tests:** `cargo test -p dishonored_probe` (frame and sample round trips). The measurements above were run on a live game session.
- **Confidence:** CONFIRMED for the jump, run, sprint and slide numbers and the SDF constants (read from the game). LIKELY for the falling-damage onset (frame sampling).
## 2026-10-06: Lord Shaw's duel resolved, view tracking, accurate go-to stop, crosshair spread (U-221, U-420, U-421)

- **Questions:** Why does Shaw flee after `One`, how do `Turn_and_Fire` and his `AIShoot` play, what does `DisSeqAct_PlayerTrackTarget` do, can the go-to stop be the decoded 0.1 uu, and how does the crosshair spread?
- **Method:** `tweaks` on `Twk_Pawn_LordShaw`, the elite and civilian tweaks, the brain and `AI_BehaviorTweaks_Shoot`; `find_export` for the loadout classes; `kismet op/fwd/bwd` around `DialogInputs_5/6`, "Shaw Shoots", "Go Time", the outcome and the tracking ops; Ghidra: `ClassDecomp` of `DisSeqAct_PlayerTrackTarget`, `DisSeqAct_AIShoot`, `DisBehaviorShoot`, `DisAISubStateFirePistol`, `Decomp` of the camera tracking (0x00aaf4c0, 0x00aa0cb0, 0x00aa0ec0, 0x00ab4e60, 0x00aa0f10, 0x00aaf3a0, 0x00aa0e60, 0x00bbb620), `OnAIShoot` 0x00b63d00 and the FirePistol states; `OffsetScan` for the camera channel fields; `VCallers` +0x188 over all code and `G_Callers` for slot 98; new `swf_dump --flatten EXPORT` and `--actions SPRITE` (AS2 disassembler) on the HUD movie.
- **Findings:**
  - Shaw's tweak falls back to `Pwn_EliteGuard_MTall_2` / `Pwn_Guard_Base`, carries `Twk_Inv_SwordEliteGuard` (`DisTweaks_WepSword`) and `Twk_Inv_PistolShaw` (`DisTweaks_WepPistol`) and uses `BrainTweaks_Guard_Elite_Party`; only his faction is civilian. Roles now come from the chain and loadout (AI.md "Pawn roles from the tweak data").
  - `Turn_and_Fire`: "Go Time" reaches it only through input 5 of `DisSeqAct_DialogInputs_6`, which is **disabled** in the data. It never plays in the original. Shaw starts shooting at `One` ("Shaw Shoots" from the same condition as `One`), so he turns early (KISMET.md "The shot").
  - `AIShoot` posts stimulus 0x5c to the brain's `DisBehaviorShoot` (Comply slot); it does not make the target an enemy. The Shoot behaviour stands, turns and runs `DisAISubStateFirePistol` in Kismet mode; the Shoot tweak's FirePistol sets no timers (AI.md "Scripted shoot").
  - `DisSeqAct_PlayerTrackTarget` (0x00b8f9c0) asks the camera for a yaw and a pitch channel (flags 0xC3, 0xC7 when holding): eased `RLerp` from the start view to the target over `m_fBlendTime`, plus the player's offset within the freedom; the channel ends by itself once blended in unless `m_bHoldUntilStopped`. In the duel it turns the player toward `Trigger_LOS_2` (looking away from Shaw), 3 s, 20 degrees.
  - Crosshair: the crossbow's internal parts are part0 `_y = -d`, part1 (`d`, `-d`), part2 `_y = d`, part3 (`-d`, `-d`); `_rotation` keeps the clip scale (the gun's internal clip is authored at -45 degrees, scale 0.586).
  - Slot 98: no caller with its signature in the whole code (PLAYER.md "Slot 98's callers"), LIKELY unused.
- **Port:** `npcs::role_from_chain` / `role_from_tweak_data` (cache `npcs7`); `Brain::shoot` / `order_shoot` / `kismet_shoot`; `kismet_host` AIShoot (no hostility, Abort clears), `ViewTrack` + `track_views` (server inputs), `PlayerTrackTarget` handler (no longer "not modelled"); `Brain::accurate_stop` / `arrived` and `PlayerState::nav_end` (host-only, `serde(skip)`, no protocol change): a scripted go-to lands on its path's end and arrives within 0.1 uu (before: 60 uu of the target); `gfx_hud::cross_layout` (crosshair spread). Harness: the duel waits for "Go Time" (Duel_Started) instead of `Turn_and_Fire`, the agent looks at `Trigger_LOS_2` on the mark, and the final stage checks Shaw's death, PN T2 / T1 and `PistolWinHonor`; death notes name the killer.
- **Tests:** `npcs::tests::roles_come_from_the_fallback_chain_and_loadout`, `kismet_host::tests::view_tracking_blends_eased_and_keeps_the_freedom`, `gfx_hud::tests` (gun corners and clip rotation, crossbow axes); `cargo test -p dishonored_assets -p dishonored_ai -p dishonored_sim -p dishonored_multiplayer` all pass.
- **Harness:** `kismet --route duel` (debug, three runs). Before: 6/6 with two open stages (the countdown stopped after `One`, Shaw fled). Run 1: the countdown never started (the agent faced yaw 0, 90 degrees off the LOS trigger, once the 3 s tracking ended). Runs 2 and 3 (agent looking at the LOS trigger): 8/8, identical: `Three` 93.4 s, `Two` 96.4 s, `One` and "Shaw Shoots" 98.4 s, Shaw killed by the player 101.4 s, `PistolWinHonor`, PN T1 active 102.4 s, the player alive with 2 health. Shaw now stands on TargetPoint_9 exactly (accurate stop).
- **Open:** U-480 (the second 2004 kills the pistol second 2008 at 101.1 s; `AIProtectNeutralsOverride` not modelled), U-481 (tracking is server-side only), U-482 (NPC accuracy override).
- **Confidence:** CONFIRMED for the data (disabled input, tweaks, loadout classes), the tracking blend and its end, the AS2 layout; LIKELY for the FirePistol state roles; APPROXIMATION for the port's tracking on inputs and the NPC shot timing (aim time 0.75 s).
## 2026-10-06: Doors and usable objects rendered with their door state; translucent decals shaded like generic_translucent_PMAT (U-560 .. U-563)

- **Questions:** Why does the `street` viewpoint see through the closed double door, and which placed actors does the level loader skip? Why do Boyle's ground puddles (`ext_boyle_ground_01.ext_boyle_puddles_decals_01`) show as pale opaque patches (`garden`, `mansion`)?
- **Method:** `package_dump --exports` / `obj_dump` of every Boyle Ext package (actor classes, components, locations near the viewpoint); `tweaks` (with `TWEAK_PKGS`) on the door tweaks; `map_dump --near --material-params`, `mic_static`, `shader_maps ... generic_translucent` and `fxc /dumpbin` of the base and light passes (9353, 9352, 9362, 9366, 9461, 9488, 9542, 10321); texture dumps of the puddle maps; `probe_launch measure psconst` (new) on the running game.
- **Findings:**
  - The level loader imports the `StaticMeshComponent`s of every placed actor, but doors and usables are `DisSkeletalBreakable`s whose visible part is `SkeletalMeshComponent0` (the `StaticUsablePart` is empty). The `street` view looks at `L_Boyle_Ext_P:DishonoredUsableObject_7` (the Garden-to-Ballroom transition door, `double_doors.DBdoor_big_rich_ext_metal_sk`, 154 uu ahead). Boyle Ext has 39 such props on 19 skeletal meshes: 8 `DisDoor`s (all in `L_Boyle_Ext_Audio`), 4 transition doors, lockers, bins, hatches, sinks (`DisWaterSource`), a chest, alarm panels and bells (`DisAlarmBell`).
  - A door's stages are in its `DisTweaks_Door` (`m_DoorOpenCW` .. `m_DoorCloseCCW`, `m_TransitionAnimName` in `Doors.Doors_as` / `double_doors.double_doors_as`); the double doors' tweak names the opposite direction's animation (`OpenCW` plays `Player_Open_Ccw`). No Boyle door sets `m_InitialDoorState` (all closed).
  - `generic_translucent_PMAT` (puddles, splotches, graffiti, confetti): for static meshes only `FNoLightMapPolicy` / light-environment base passes and `FNoStaticShadowingPolicy` light passes are compiled, so translucency has no lightmap and no shadows. diffuse = D [x D colour] + SceneReflection(R) x `C_Cubemap_Mask[ch]` [x cube colour]; the inverted Fresnel lerps to `F_Fresnel_Color` and adds to alpha; colour = diffuse x (`AmbientColorAndSkyFactor` + sum saturate(N.L) x light); alpha = clamp(O[ch], min, max). The puddles: opacity in G and reflection mask in B of `puddles_decals_01_s`, no diffuse colour, no specular (RENDERING.md "Translucent decals").
  - `AmbientColorAndSkyFactor` = (0, 0, 0, 1) in all ~30 base-pass shaders drawn in the sampled frames of the running game (U-562).
  - The pale look came from Bevy: environment specular (Fresnel-bright at the ground's grazing angles) and the placeholder ambient on a lit `StandardMaterial`.
- **Port:**
  - `dishonored_assets::usable_props` (the map's `skeletal_props`: mesh, transform, AnimSets, door stages; called from `import_level`); `MaterialParams::mask_channels`; the cubemap mask kept for translucent materials.
  - `level_props.rs`: props spawned skinned, placed with the Kismet's delta, doors posed for their state (transition animation, then rest on the open stage's last frame); `follow_movers` keeps `InterpActor` meshes where the Kismet has them outside matinees.
  - `MissionView::doors` / `movers` filled by `kismet_host::view` (**net protocol 5 -> 6**).
  - `decals.rs` + `ark_translucent.wgsl`: an extended material with the decoded maths; no lightmap, not a shadow receiver.
  - `probe_launch measure psconst`.
- **Tests:** `usable_props::tests` (door states, classes), `level_props::tests` (stage per transition, rest pose, mover matching), `decals::tests` (puddle decode, colour / Fresnel switches), `kismet_host::tests::mover_delta_maps_placed_onto_current`; `cargo test -p dishonored_assets -p dishonored_multiplayer -p dishonored_mission -p dishonored_net --lib` and `-p dishonored_runtime` pass.
- **Harness:** `fidelity --configs full` (debug), SSIM against the original captures, before (the main-branch renders in the session's `caps/port2`) -> after: aerial 0.358 -> 0.361, street 0.611 -> 0.730 (the door), garden 0.293 -> 0.294, spawn1 0.299 -> 0.302, mansion 0.352 -> 0.360, sewer 0.640 -> 0.638, moon 0.376 -> 0.376, skyline 0.473 -> 0.487.
- **Confidence:** CONFIRMED for the skipped components, the prop list and the decal shader maths (read from the shaders); LIKELY for the state-to-stage mapping (U-560) and the zero ambient (U-562: the puddle shader itself was not drawn in the sample). APPROXIMATION: static lights do not light translucency in the port (U-561), so the confetti by the mansion lamps is darker than in the original.

## 2026-10-06: Foliage, the diffuse colour's alpha, skeletal scenery (U-570 - U-573)

- **Question:** Why are the hedges along the mansion steps and the garden fence missing in the
  `mansion` / `garden` / `moon` pairs, and what other visible geometry does the importer skip?
- **Method:** export class counts of all eight Boyle Ext packages (`package_dump --exports`);
  `map_dump --survey`, `--near` (now with each instance's lightmap luminance), `--mesh` (new:
  vertex / triangle counts, bounds, UV ranges); `obj_dump` of the shrub components and of every
  placed `SkeletalMeshComponent`; `mic_static` of `boyle_shrubs_01_inst`; `shader_maps` (now
  printing the vertex uniform expressions) and `fxc /dumpbin` of `vertexAnim_PMAT`'s base pass
  (PS 1743, VS 7327 / 7358) and of one base pass per base material exposing `D_Diffuse_color`;
  close-up renders of the hedges (normal and `--debug-lighting`).
- **Findings:**
  - No foliage-specific storage: no `InstancedFoliageActor`, `InstancedStaticMeshComponent`,
    SpeedTree, terrain or `FoliageFactory` in any package. The hedges (`square_shrubs_02/03`,
    `boyle_shrub_01/02`), cone shrubs, flowers, grass, ivy and yews are `StaticMeshCollectionActor`
    components: 197 shrub / flower and 192 grass instances, all imported and lightmapped, with
    `CollideActors` false, no cull distance, none hidden. The importer drops none of them.
  - They were drawn, but as dark low mounds: (1) the base pass multiplies `D_Diffuse_color.rgb` by
    its alpha (`mul r.xyz, cN, cN.w`) in all 14 base materials that expose it (`FogSheet_PMAT`
    alone reads the vector whole); the port ignored the alpha (shrubs 1.5, grass and flowers 2);
    (2) the port's viewpoints see the scene smaller than the reference captures (U-572).
  - `vertexAnim_PMAT` wind (simple permutation): world += `Va_Radius_spreading x sin(2 pi frac(z x
    Va_Distorsion_strenght + Time x Va_Vertex_Anim_Speed + 0.5) - pi)` on every axis, at most 1 uu
    on Boyle (U-570). Its pixel shader clips at `O_Opacity_mask.r - 0.3333` (as the port).
  - Skipped visible geometry: placed `SkeletalMeshComponent`s. Of those, 42 are scenery
    (8 `DisDoor`, 25 `DishonoredUsableObject`, 4 `DisWaterSource`, 2 `DisAlarmBell`, the
    `DisClimbable` chain, and the two `SkeletalMeshActor`s no Kismet variable names: the sewer gate
    and the bridge); 19 `DisFish`; the rest matinee / hand props (`SkeletalMeshActor[MAT]`). The
    `street` viewpoint looks straight at `DishonoredUsableObject_7`'s double door, which the port
    did not draw. BSP: the level `Model`s have no components. Everything else with a primitive
    (static meshes of every owner class, spline meshes, emitters) was already imported; hidden
    instances are only the 115 `Collision_model` blockers, blockout planes / cubes and one sheet.
- **Port changes:** `map_render::diffuse_color_terms` (rgb x a for the decoded bases);
  `level.rs` `import_skeletal_props` (reference-pose, render-only instances after every static
  component, so existing instance indices are unchanged; `ImportStats::skeletal_props`);
  `shader_maps` prints cube textures and vertex vectors / scalars; `map_dump --mesh` and lightmap
  stats in `--near`.
- **Tests:** `dishonored_assets` (retail `boyle_ext_import`: 533 meshes, 3796 instances,
  42 skeletal props). `harness fidelity --configs full` (debug), SSIM before -> after: aerial
  0.3614 -> 0.3513, street 0.6176 -> 0.6708, garden 0.2957 -> 0.3010, spawn1 0.3019 -> 0.3078,
  mansion 0.3525 -> 0.3490, sewer 0.6380 -> 0.6381, moon 0.3727 -> 0.3755, skyline 0.4705 -> 0.4842.
- **Confidence:** CONFIRMED for the storage, the counts, the diffuse alpha and the wind formula;
  LIKELY for the viewpoint explanation (U-572); APPROXIMATION for the skeletal scenery's lighting
  and pose (U-571). Open: U-570 (wind not drawn, complex permutations), U-573 (base-pass ambient).

- Merge reconcile (foliage survey Ã— usable props): static reference-pose skeletal instancing is limited to `DisClimbable` and unscripted `SkeletalMeshActor`s; doors, usables, water sources and alarm bells come only from `usable_props` (animated). Boyle Ext: 514 meshes, 3757 instances, 3 static skeletal props.

## 2026-10-06: Camera, exposure and the moon (U-320, U-322, U-573 resolved; U-580..U-584)

- **Question:** Why is every fidelity pair wider and differently framed than the original, why is the port darker and flatter (`mansion`), and why is the moon disc far too large?
- **Method:** the 8 reference captures (`U_hook/views`) against port renders at fixed viewpoints, per-patch RGB means; the engine hook on a running game (passive reads only): `measure consts` (new) on the base-pass and light-pass pixel shaders, `peek` of `LocalPlayer.m_CurrentArkPpSettings`; `shader_maps` + `fxc /dumpbin` on `Moon_PMAT`, `Sun_PMAT`, generic_PMAT (D2 permutations), the light-pass shaders and the ArkPp LUT bake / uber (global 57, 59); `export_hex` of `Moon_PMAT`'s native data; Ghidra `ClassDecomp` / `Decomp` of `LightMapTexture2D` (constructor 0x00657DC0); `obj_dump` of the lights' `FalloffExponent`, the moon textures and materials.
- **Findings:**
  - FOV: horizontal 75 at 16:9 (no aspect constraint in this UE3's `LocalPlayer`). The port's game camera had it; the `--inspect` camera used Bevy's 75 degrees vertical (107 horizontal). With it fixed every viewpoint frames like the capture. Both the probe and the port place the eye (not the pawn) on the viewpoint.
  - Base pass: `DiffuseOverrideParameter` = `SpecularOverrideParameter` = `AmbientColorAndSkyFactor` = (0, 0, 0, 1) in every sampled draw; `LightMapScale[0..1]` = the cooked scales (w = 1). Lightmap textures are sRGB (class default; the constructor sets no `SRGB`).
  - The lightmap specular term had been turned off as "blowing out"; the cause was a missing view exposure on that term (~1000x). Fixed and on by default.
  - Light pass: falloff `max(1 - d^2/R^2, 0)^FalloffExponent`, no inverse square, no 1/pi; spot cone as Bevy's; specular a normalised Phong (U-581). Boyle's lights use exponents 2 to 6. `LightColor` linearises as `(c/255)^2.2` (measured on the moon light).
  - generic_PMAT `D2_Enable_Diffuse_Variation`: `lerp(D_Diffuse_color x D, D2(UV0 x tiling), mask(uv).ch x power)`, the mask on UV0 or (with `D2_Use_UV_coordinate_Index_1`) on the lightmap channel. The garden gravel's dirt variation was missing (near-black ground).
  - ArkPp: the live settings equal the cooked ones; shaders 57 / 59 confirm the port's LUT maths. They are drawn with `DrawPrimitiveUP`, now hooked (slots 83 / 84); their constants were not read (U-582).
  - `Moon_PMAT`: emissive `Tex#0.r x Moon_Color.rgb x a`, opacity `saturate(Cloud(u x 0.15 + Cloud_Speed x t, v).r) x Tex#0.g`; `Tex#0` = `Vfx_Materials.Textures.Moon` from the material's `UniformExpressionTextures` (native data). The port drew `MoonClouds` over the whole mesh. `Sun_PMAT`: `max(1 - 2|uv - 0.5|, 0)^SphereMask_Power x SunIntensity x Color.rgb` (unused on Boyle).
- **Port:** `viewmodel::apply_fov` in inspect mode; `sky_disc.rs` / `.wgsl` (moon, sun); `material_chain::uniform_expression_textures` (`Tex<i>` texture params for sky materials); `lightmap_ext.wgsl` specular x exposure and the D2 variation (`map_render::D2Variation`, components with a lightmap-UV mask batched alone); `lighting::Ue3FalloffPlugin` rewrites `bevy_pbr`'s `pbr_lighting.wgsl` (embedded source patched and reloaded before the first frame) with UE3's falloff, the exponent carried in the light radius; `lightmap::color_to_linear` for light and fog colours; `--no-lightmap-specular` replaces `--lightmap-specular` (fidelity config renamed). Probe: `measure consts`, UP draw hooks.
- **Level load (coordinator request):** the lightmap bake is cached as `<map>.lightmaps1.bin` under `DISHONORED_CACHE` (`lightmap_cache.rs`, signature of the inputs). Boyle Ext in this worktree: import = packages 0.8 s + actors 4.6-4.9 s + lightmaps 2.1 s baked, 5.3 s on the first run (writing 168 MB), 0.2 s warm. Renders with the bake and with the cache differ exactly as much as two baked runs (PSNR 36.1547 vs 36.1547 dB with post / FX / bloom off; run-to-run animation), and a unit test round-trips the file. The 33 s seen in a viewer log was not reproduced here (the whole import takes about 8 s warm); the import now logs its stage times.
- **SSIM (8 viewpoints, `harness fidelity --configs full`):** before (8414366): aerial 0.3615, street 0.6176, garden 0.2956, spawn1 0.3016, mansion 0.3523, sewer 0.6381, moon 0.3718, skyline 0.4706. After (this branch with main a831027 merged): aerial 0.5838, street 0.7651, garden 0.4952, spawn1 0.5276, mansion 0.5312, sewer 0.7124, moon 0.6833, skyline 0.6288. The FOV fix alone (before the main merge) gave 0.578 / 0.616 / 0.486 / 0.534 / 0.533 / 0.712 / 0.693 / 0.586; street's further rise is main's doors.
- **Tests:** `cargo test -p dishonored_runtime -p dishonored_assets` pass (new: `sky_disc` moon / sun decode, `lightmap_cache` round trip).
- **Confidence:** CONFIRMED for the FOV, the measured constants, the moon / sun / D2 / falloff shader maths; LIKELY for lightmap sRGB (no sampler-state read) and the D2 mask's lightmap channel assumption. Open: U-580 (moon glow), U-581 (light-pass specular), U-582 (display gamma at the LUT), U-583 (other variation terms), U-584 (remaining dark ground spots).

## 2026-10-06: The engine hook as an oracle for open approximations (U-100, U-121, U-340, U-341, U-421, U-422, U-462, U-501, U-590, U-591)

- **Question:** Which open approximations can a live session of the original settle, and do the port's combat, power, weapon and stealth numbers match?
- **Method:** Triage of the 95 LIKELY / APPROXIMATION / SPECULATIVE / OPEN rows (research/dishonored/HOOK_TRIAGE.md: 31 measurable, 64 need reverse engineering). The probe DLL gained native instruments (protocol 2: entry hooks with memory reads, native calls, scratch memory, ProcessEvent traces, absolute watches); `probe_launch lab` sets up scenarios (items and powers through the Kismet handlers `OnAddInventoryItem` / `OnAddPower` with the action class defaults, tutorials off, the player kept alive by `m_MinimumScriptedHealth`, a fixed 1/60 s time step through `GUseFixedTimeStep` 0x0141B27C / `GFixedDeltaTime` 0x012696B0); `measure gun|blink|mana|windblast|sword|detect|vis` record the native calls the decompilation named. Three game sessions, Boyle Ext.
- **Findings:**
  - Pistol dispersion: 150 settled shots: radius / dispersion uniform (mean 0.486, variance 0.0861, KS 0.051); traced directions match the port's 720-line-pixel cone through 75 deg on 16:9 within 0.085 deg. Reload stages 0.283 / 0.716 / 1.483 s after the shot at 60 Hz (port 0.30 / 0.726 / 1.476); crossbow timed stage 1.02 s (U-501: the class default 1.0). Slot 98 is off the fire path (U-421); moving and turning do not widen the spread (U-422).
  - Blink: 1100 / 1600 uu for levels 1 / 2 (1-based `m_CurrentLevel`), the 500 uu vertical clamp, 100 uu per tick, cost 20 refunded after 3.00 s at one point per 7 ticks. The flight lifts the pawn centre 71 uu (U-591).
  - Windblast: cost 50; a hit at 182.8 uu took 26 and momentum (2362, -227, 1317): the momentum is `impulse x k`, not `(impulse.xy k, 0.15 |impulse| k)`. Fixed in the port.
  - Sword: 10 per hit on a 50-health guard (0.2 x the victim's HealthMax; the port's NPCs have the player's 70 by rule); the hit lands 0.30-0.35 s after the press, near the end of the attack zone (port 0.13 s, U-590). Guards' `NPCSlash` 15 (8 on some first hits), bullets 20.
  - Detection: the seen value equals `DistanceTuning(1 - norm) + 0.1 light01 + 0.25 speed` (13 cases within 0.006), norm = d / 3500 inner, d / 1700 outer. The light is normalized by the map's `Boyle_Twk.Pvs_BoyleExterior` (0.2 / 0.5, `DishonoredMapInfo.m_pMapPlayerVisSettings`, chosen in 0x00BC8E60), not the global 0.05 / 1.0. The raw light at 14 points is 0.168-0.313; the port's probe gives 0.225 everywhere (U-121).
  - Falling damage with the exact landing speed (hook on 0x00B4DCC0, 60 Hz): exactly `trunc(70 x (v - 2250) / 650)` (U-462). Vitality levels do not stack (U-100). `m_fAutosaveWaitTime` is 0 (U-341); engine +0x770 is 1 while guards fight the player (U-340).
- **Port:** `left_hand::windblast_momentum` returns `impulse * k`; `AiLevel::player_light_range` (extracted from the map info, cache `ailevel3u`) replaces the global light range in `Crowd::set_level`.
- **Tests:** `cargo test -p dishonored_sim` (updated `windblast_factor_and_momentum_rules` with the measured hit, new `fall_damage_matches_the_original_at_60_hz`), `cargo test -p dishonored_ai` (new `seen_value_matches_the_original_on_boyle_ext`), `dishonored_assets` retail `boyle_ext_player_light_range`, `dishonored_content` real-map `light_probe_against_the_original` (printed comparison), `cargo test -p dishonored_probe`.
- **Confidence:** CONFIRMED for the measured values (read from the running game; timings exact to a 1/60 s tick); LIKELY for the Blink cylinder reading (U-591).

## 2026-10-06: Engine hook, second round: sword timing, moon glow, post constants, lean, drowning, Bend Time, grenades (U-590, U-580, U-582, U-048, U-242, U-244, U-109, U-102; U-600..U-602)

- **Question:** Why does the port's sword hit land at 0.13 s instead of 0.30-0.35 s; what lights the clouds around the moon; does the display gamma reaching the LUT differ; and how many of the planned measurable triage rows match?
- **Method:** Ghidra (ClassDecomp of `DishonoredNotify_AttackZone` and `DisItemContext_MeleeAttackPlayer`; Decomp of the drowning, health, breath and lean camera functions); the engine hook with new instruments: per-shader draw census, draw skipping, the shader hash of `shader_maps` blobs, and `measure lean / bendtime / grenade`; port renders of the `moon` view.
- **Findings:**
  - Sword: the notify's tick handler (0x00A8E760) opens the versus zone at `end - DamageMax - VersusMax` and the damage zone at `end - DamageMax`; entering it (context slot 115, 0x00C13140) sets `m_bDamageIsPosssible` and clears `m_AlreadyHitActors`; the context tick (0x00C13640) sweeps every tick while 0x00BFF7B0 holds, until the notify end (0x00A8E710). Right_A: 0.138 + 0.207 - 0.05 = 0.295 s.
  - Post chain: the LUT bake (PS 1906, every frame) reads exposure 1.231144, gamma 0.431818 (= 0.95 / 2.2), contrast 1.081794, as the port computes them. UE3's gamma pass (1875, `InverseGamma` 1/2.2) runs but skipping its draws changes nothing; skipping the uber shader blacks the screen.
  - Moon: one capture per drawn shader with that shader skipped showed the glow belongs to the sky dome (`309cfebf79696f7c` = shader map 6052 of `Skies_PMAT`); `SetScalarParameterValue(Storm_Intensity, 0)` on the live `Boyle_2_inst` removes it. The dome samples its `Storm` texture (`Sky_Boyle02_d`) at texture coordinate 1 (VS 1215 `o1.zw = v5.xyyx`); the port used the lightmap set, which is 0 on the dome.
  - Lean: 20 degrees held = `m_fMaxLeanAngle` 15 + `m_fMaxLeanSoftenAngle` 5 (0x00AC5540: `+0xA0` grows toward 5 at `m_fMaxLeanSoftenSpeed` while the head is clamped at the limit), roll = `m_fCameraTiltPercent` 0.5 x angle; 119.7 uu sideways, 21.4 down.
  - Drowning: air 30 s, then one point per 61 ticks down to death; regeneration does not run while drowning (countdown frozen) though it does under water with air. The underwater flag flips with the camera at the volume top.
  - Bend Time level 1 matches the port (absolute caster rate 0.85, world clock 0.15 x real, linear 0.15 / 0.25 s ramps, 12 s real).
  - Grenade: armed at the throw, 3.0 s fuse. The cooked `m_fDetonationDelay` is 3.0; the property parser took the imported `DisTweaks_GrenadeComponent` (an Object) for a component and lost its first property.
  - Light probe: adding the 358 static-collection lights at full brightness gives the right pattern but 35 times too much light (not kept, U-121).
- **Port:** `tuning::SwingTiming::PLAYER` and `combat::swing_phases` (per-clip phases; `SwordState::swings` / `swing_clip`, the viewmodel plays the sim's clip; net protocol 7); `StaticMeshAsset::uv_set1` and the sky dome mesh's UV1; `vitals::tick_with` (regen held while drowning); lean soften / roll (`movement::lean_angle`, `lean_roll`, config `soften_angle`, `soften_speed`, `camera_tilt_percent`, cache `gameplay_config.v12w`); grenades armed at the throw; `ue3_package` component detection excludes the grenade tweak components; fixture fuse 3.0.
- **Tests:** `cargo test -p dishonored_sim` (new `sword_hit_lands_in_the_damage_zone`, `drowning_stops_health_regeneration`, `grenade_explodes_three_seconds_after_the_throw`; `lean_swings_the_eye_about_the_pivot` on the measured values), `-p dishonored_runtime` (`ark_post` live constants), `-p dishonored_assets` (retail `boyle_ext_import`: the dome's UV1), `-p ue3_package`, `-p dishonored_ai -p dishonored_multiplayer -p dishonored_net`.
- **Confidence:** CONFIRMED for the measured values and the decoded zone rule; LIKELY for the lean soften trigger in the port (clamp of the normalized angle) and for holding regeneration (the mechanism in the original is not found). Open: U-600 (NPC swing timing), U-601 (clip choice), U-602 (sticky grenades), the lean onset, U-121.

## 2026-10-07: Lightmaps on the merged batches, lit decals, light environments, light-pass Phong, N2 / S2 (U-561, U-571, U-581, U-583, U-584; U-620, U-621)

- **Questions:** Why is the port's ground still darker than the original under the lamps (`spawn1` path, `mansion` courtyard), why is the street's double door dark and blue with a bluish band at its seam, what do the generic_PMAT variation layers and the light passes' specular compute, why is the confetti dark?
- **Method:** per-patch RGB means of the port renders against the reference captures (`ffmpeg` crops), renders with feature flags and `DISHONORED_LIGHTMAP_DEBUG=diffuse`, `map_dump --near` / `--material-params` / `--materials`, texture dumps, `mic_static`, `shader_maps` + `fxc /dumpbin` (28116 gravel lightmap base pass, 9347 puddle lightmap base pass, 15153 / 15166 door base and light passes, 1743 hedges, 1815 / 1831 balloons), Ghidra on the shared pool: `ClassDecomp` of `DynamicLightEnvironmentComponent` and the light component classes, `FuncDecomp` of the light environment state (0x005035D0 and callees), new `W_RangeWriters` / `W_ForceDisasm` / `W_DumpFloats` (the SH tables' static initialisers and constant tables), `obj_dump` of the light archetypes in Engine.upk, new `level_volume` tool on the `Level` exports.
- **Findings:**
  - The merged static batches carry their lightmap in the material, so Bevy never defines `LIGHTMAP` for them and `apply_pbr_lighting` never added `lightmap_light`: since the batching (19b4d96) the level's diffuse was Bevy's placeholder ambient + the environment cube's diffuse + the dynamic lights; only the lightmap specular (added outside) came from the bakes. Feature toggles showed it (`--no-dynamic-lights`, whose 900 ambient lit the "lightmapped" batches, was brighter than the full render).
  - A mesh shader map writes its vertex factory name after its shaders: `shader_maps` labelled each list with the previous name, so the `FGPUSkinVertexFactory` list was read as the static one and "translucent static meshes have no lightmap" (U-561) was wrong: `generic_translucent_PMAT` has `FDirectionalLightMapTexturePolicy` base passes (puddles 9347: diffuse x directional lightmap, no specular since it is multiplied by `SpecularOverrideParameter.xyz`).
  - `DynamicLightEnvironmentComponent` (doors: class defaults, `bDynamic` false) decoded (RENDERING.md "Light environments"): static lights whose channels with CompositeDynamic as Dynamic overlap the owner's (static light archetypes keep CompositeDynamic), traced for visibility, projected as `Y(L) x intensity`, plus the floor level's precomputed light volume (Level +0x2B0; 33-byte samples with RGBE radiances, weight (1 - d/R)/R); then a directional light along the luminance SH's linear band and upper / lower sky lights by projection; the skinned base pass (15153) draws `(N.L x diffuse + S x Phong) x LightColor + diffuse x (Upper ((1 + N.Z)/2)^2 + Lower ((1 - N.Z)/2)^2)`. SH tables: normalisation constants 0x0126B100, Ambient / Upper / Lower sky functions from the static initialisers at 0x00F67490..0x00F674FE.
  - Light passes (15166): Lambert without 1/pi in UE3 units, normalised Phong without N.L; the generic base-pass variation (28116): the D2 mask also lerps N -> N2 (normalised) and S -> S2.
  - `O_Enable_full_opacity` masked bases have no `texkill` (balloons).
- **Port:** `lightmap_ext.wgsl` adds the lightmap diffuse and zeroes the batches' ambient / environment diffuse; `decals.rs` / `ark_translucent.wgsl` light decals with their directional lightmap and the dominant light's distance field; `dishonored_assets::light_env` (volumes, static lights, composite, synthesis; `SkeletalProp::light_env`) and `prop_light.rs` / `.wgsl` for the props; `lighting::ue3_light` replaces Bevy's BRDF for the map's lights (reflectance 0.5 = S / 2, SP from roughness); `map_render` N2 / S2 (`BakedLightmap` v2 bindings); full-opacity masked materials opaque; `shader_maps` VF labels fixed; tools `level_volume`, `map_dump --props` / `--unlit`.
- **Tests:** `cargo test -p dishonored_assets` (new `light_env` SH basis / RGBE / synthesis / channel tests) and `-p dishonored_runtime` (new `prop_light` test).
- **SSIM (8 viewpoints, same script as `harness fidelity --configs full`, debug build):** before (session's `caps/port3`): aerial 0.579, street 0.756, garden 0.492, spawn1 0.524, mansion 0.531, sewer 0.716, moon 0.689, skyline 0.630. After: aerial 0.603, street 0.814, garden 0.513, spawn1 0.571, mansion 0.608, sewer 0.830, moon 0.669, skyline 0.747. Ground patches: see U-584.
- **Confidence:** CONFIRMED for the batch lightmap bug, the vertex factory labelling, the decal / door / light pass / variation shader maths and the light environment algorithm's structure and constants; APPROXIMATION for parts of the light environment (U-620). Open: U-621 (hedge contrast), U-580 (moon glow), pickups / movers / pawns still on Bevy's placeholder lighting.

## 2026-10-07: World deaths on Boyle Ext, hunting test bots (U-640, U-641, U-643)

- **Question:** A 5-minute playtest (dedicated server, three rendered `--autopilot` clients on L_Boyle_Ext_P) ended with five `died (World)`, one drowning and no player kill. Which of these deaths does the original's level data allow, and why do the bots never meet?
- **Method:** `harness ffa` (new: a headless free-for-all of test bots on the real map, every death logged with position, mode, the volumes it touched, where the pawn last stood and where it went into the water); `obj_dump` / `package_dump` / `map_dump --near` / `--survey` on the Boyle Ext packages (BlockingVolumes, PlayerStarts and their `Base`, water and physics volumes, usable objects, the door physics asset); raycasts and box sweeps against the port's collision (`crates/dishonored_content/tests/world_deaths.rs`); the navmesh's connected components; Engine.upk and DishonoredGame.upk decompiled with `uc_decompile.ps1` (`Pawn`, `BlockingVolume`, `PhysicsVolume`, `KillZDamageType`, the usable object and door classes).
- **Findings:**
  - Every `World` death was `KillZ` (`WorldInfo.KillZ` -20000 on `L_Boyle_Ext_P`, read from the package), none a pain volume (Boyle's 24 pain volumes deal 1 point a second, 22 of them `SpringRazor`). Headless with the old bots (3 bots, 5 minutes): 10 `KillZ`, 1 drowning, 0 kills.
  - Port bug: Boyle's front door (`DishonoredUsableObject_6`, `LTD_Courtyard_to_Foyer`) had no collision. Usable objects collide through their `SkeletalMeshComponent`'s physics asset (`bCollideActors`, `bBlockActors`, component `BlockActors` by class default); the port drew the door and let pawns through the 304 uu doorway onto a strip of floor ending at x 1711, 2000 uu over the sea. Five of the ten deaths went that way: through the door, into the sea, out of the sea volume's east side, down to `KillZ`. The four `LTD_*` doors (box bodies on their hinge bones) are now static collision.
  - Port bug: the level's eight `BlockingVolume`s were not collision. Two are the `Base` of PlayerStarts (`PlayerStart_Garden` on `BlockingVolume_5`, `PlayerStart_RiverGrate` on `BlockingVolume_2`), which also confirms the brush hull transform. They now block pawn movement (U-640 for traces). The two `DynamicBlockingVolume`s are placed with collision off; `DisStaticCollisionActor_15` is NPC-only (U-643).
  - Legitimate: the sea (`DishonoredWaterVolume_0`) is one convex prism with no floor under its eastern half and nothing past its sides; a swimmer who leaves it falls to `KillZ`, in the original as in the port. One death walked off a roof edge (z 1100) into the sea and out of its south-east side. Kept; the bots keep out of the water.
  - Spawn choice: four deaths and the drowning came from `PlayerStart_SamuelDropOff`, the mission's scripted start, which stands off the navmesh on the canal floor under the visual water plane but outside the water volume; its floor ends at y ~13100. The roof patio and river grate starts stand on navmesh islands of their own. Free-for-all now spawns only at PlayerStarts on the navmesh in the largest walkable group (Boyle: the garden and front-door starts).
  - Bots never met: they only chased opponents in sight and otherwise walked straight in random directions; every client receives every player's replicated position (no relevancy culling), so they now hunt along the navmesh.
- **Port:** `dishonored_assets::water::extract_blocking_volumes`, `dishonored_assets::prop_collision` (physics asset box / capsule / sphere elements at the reference pose), `dishonored_world::{hull_triangles, box_triangles, CollisionWorld::with_extra}`, `dishonored_content::add_level_blockers` (cache `collision5.bin`) and `ffa_spawns` (server, offline session, harness); `dishonored_bots`: hunting `think` (nearest reachable opponent by replicated position, `nav::Route` with corners pushed for the player's wider box, fight in the clear, roam), `hazard` (`safe_ahead`: floor within 260 uu, no deep water, no pain volume), swimming to the nearest navmesh shore; `harness ffa`; `harness net` checks player kills. Docs: PLAYER.md "Level collision and the kill plane", multiplayer/FFA.md, README.
- **Results:** headless (`harness ffa`, 3 bots, 5 min): before 0 player kills, 10 `KillZ`, 1 drowning; after 62 / 62 / 63 player kills and no environmental death (seeds 1 / 2 / 3); from every PlayerStart (islands and the drop-off included) no environmental death in 3 minutes. `harness net` (8 clients, 75 s): 70 player kills and 0 self / world deaths seen by one client. Rendered (release server and three 960x540 `--autopilot` clients, duelist / gunner / duelist, 5 minutes, scores read over the control socket): 57 player kills (38 / 12 / 7), 57 deaths, no world death; the three clients' scoreboards agree.
- **Tests:** `cargo test -p dishonored_world` (new `hull_faces_close_a_box`, `blockers_added_to_a_world_stop_pawns_only`), `-p dishonored_assets` (retail `boyle_ext_blocking_volumes`, `boyle_front_door_bodies_fill_the_doorway`), `-p dishonored_content --test world_deaths` (retail: the four doors collide and the front doorway is shut, a pawn pushing east from the front-door start stays in the courtyard, the two starts stand on their blocking volumes, the FFA spawns), `-p dishonored_bots --test hunt` (hazard probes; a bot walks round a wall after an unseen opponent and waits at a ledge instead of following it into the sea).
- **Confidence:** CONFIRMED for the kill plane, the blocking volumes and their PlayerStarts, the door's physics asset and collision flags, the sea volume's shape; LIKELY for blocking volumes letting traces through (U-640). Open: `DisDoor`s and other usables without collision (U-641).

## 2026-10-07: Collision audit against the original on both Boyle maps (U-640, U-641, U-643; U-650..U-656)

- **Question:** Does the port's level collision answer the player's movement, the pistol shot and the vision line of sight as the original does, source by source, on `L_Boyle_Ext_P` and `L_Boyle_Int_P`?
- **Method:** `harness collision gen` (checks derived from the original navmesh: grid drops, box drops, foot / waist / head lines on the three trace types, box sweeps out of polygons and along links); `probe_launch collide` runs them in the original through the hook's `UWorld::SingleLineCheck` and records the door states; `harness collision check` clusters and classifies the disagreements; Ghidra `ClassDecomp` of the KActor, pickup, battery and usable classes (`ShouldTrace` overrides, `USkeletalMeshComponent::LineCheck` 0x0076F270); `obj_dump`, `tweaks`, `skel_dump` and the `collision_audit` tool on the map packages; live `Pawn` fields.
- **Findings:** research/dishonored/COLLISION.md. Trace flags, the per-component trace types, simplified vs kDOP collision, the back-off and the separating-axis extent checks (earlier in the day); usable objects rest on the first frame of their first stage (bins' lids closed); `DisSkeletalBreakable` / `DisDoor` vision answers follow their tweaks; `AKActor::ShouldTrace` (0x00782D60) lets the vision line meet non-blocking KActors (pickups); a battery in a receptacle is held and blocks; `BlockingVolume`s stop the player's movement lines and boxes only, `DisStaticCollisionActor_15` NPC movement only; `MaxStepHeight` 44, `WalkableFloorZ` 0.6428 live, as in the port. Port bug: box sweeps found convex elements only through their faces' tree leaves.
- **Port:** `dishonored_assets::{level, collision_audit (KACTOR_CLASSES), prop_collision (usable_start_bodies)}`, `dishonored_content::add_level_blockers` (cache `collision16`), `dishonored_world::CollisionWorld::sweep_box`; `harness collision` (cause classification, stored oracle results in `fixtures/collision/`).
- **Results:** agreement before (first rounds) / after: Ext 95.55 % / 99.94 % (76,826 checks), Int 98.37 % / 99.90 % (44,798). Remaining: Ext 14 edge, 21 coincident surfaces, 13 unknown; Int 6 edge, 13 coincident, 1 inside, 16 dynamic, 9 unknown.
- **Tests:** `cargo test -p dishonored_world -p dishonored_assets -p dishonored_content`, `harness collision` (stored oracle), sim, travel, ffa, net, visual.
- **Confidence:** CONFIRMED for the trace flags, the blocking volume and NPC collision answers, the KActor vision rule, the step and slope values; LIKELY for the usable start pose (U-655). Open: U-650..U-656.

## 2026-10-07: Regressions after the collision audit: hover height, Miss White's drink, the mission agent

- **Question:** After the collision audit (exact box sweeps with the engine's back-off, solid usables and doors), four `dishonored_sim` mechanics tests stood the pawn 4 uu higher than expected, `harness kismet --route white` no longer got the party drink, and `harness mission`'s agent never reached the host. Which side is wrong?
- **Method:** engine hook on L_Boyle_Ext_P (fixed 1/60 s step): the pawn's `Location` and `CylinderComponent` (+0x3E0, height +0x1C4, radius +0x1C8) read live, then physWalking's floor check (extent 35 x 35 x height, 46.54 uu down, flags 0x20DF) run under it through `probe_launch collide`, with and without `TRACE_Accurate`; standing, crouched, after a Blink and after a mantle (a 119 uu ledge found by a grid of accurate down lines); the same checks with `harness collision probe`; physWalking (0x006BD150) re-read for the floor check (`MultiLineCheck` 0x0064A860, nearest hit, `Time x (MaxStepHeight + 2.54)`). New `probe_launch place X Y Z [YAW]`.
- **Findings:**
  - The original's walking pawn rests 6.73 uu above per-polygon floors (runway, standing / crouched / after a Blink: not accurate 2.730, accurate 6.730) and 3.73 uu above simplified collision (mantle ledge: 2.731 / 3.731): the floor distance that physWalking holds at 2.7305 is the backed-off time. The port's sweeps give the same times at both spots (0.05866, 0.05868). The tests assumed the real gap was 2.73; their expectation was wrong, the movement is right.
  - Short moves are backed off by a tenth of their length: the pawn that walked into the ledge stopped 0.04 uu from its face.
  - Miss White's drink: the cider fountain (`DisWaterSource_7`, a usable) now collides with its static part, a box up to z 153.6 over the drinks table; the harness stood the player 90 uu from the fountain on top of it, out of the use reach (231 uu from the eye). Harness bug, not collision: the player now stands on a floor no higher than the usable, box clear, the usable within reach.
  - The mission agent: after its first death the navmesh path led to a corner at (-88, 1710) beside a balcony post (`rich_stn_balcony_02` in `StaticMeshCollectionActor_13`, simplified box x -90..-30, y 1700..1760). The original blocks there too: the box check from (-88, 1795, -300) southwards hits the post at time 0, and the hooked pawn walking south at x -88 stops at y 1795.14 while at x -140 it walks past. The funnel's corners are not offset by the pawn's radius, and the agent's unstuck sidestep always went the same way; it now alternates sides.
- **Port:** `crates/dishonored_sim/tests/mechanics.rs` (`HOVER` = `FLOOR_DIST + 4`, new `walking_hover_follows_the_floor_back_off`), `tools/harness/src/kismet.rs` (`place_to_use`), `tools/harness/src/mission.rs` (alternating sidesteps, `MISSION_AGENT_DEBUG`), `tools/probe_launch` (`place`). Docs: COLLISION.md "Movement and the back-off", PLAYER.md, INSTRUMENTATION.md.
- **Results:** `cargo test -p dishonored_sim --test mechanics` 30 passed (4 failed before); `harness collision` unchanged (Ext 99.94 %, Int 99.90 %); every harness suite and route passes (release build, muted; `visual`'s "player moves" failed once, walked 160 uu, and passed on the rerun); `cargo test --workspace --no-fail-fast` 112 test binaries, 0 failures.
- **Confidence:** CONFIRMED (measured and code read) for the hover heights and the post blocking.

## 2026-10-07: Contracts playtest: NPC "assassinations", NPC names, a stranded player, guards hunting respawns (U-660, U-661)

- **Question:** In a rendered 3-client autopilot Contracts match on main (13eb647), NPCs #1029-#1033 caused all but one death, killed each other by "Assassination", the feed named them "#1030", and Bob (gunner) did nothing for 5 minutes with black screenshots. Who are these NPCs, why do they kill, and what should each class do?
- **Method:** the playtest logs; `harness contracts` with a new all-deaths-by-source table (killer class, victim class, cause); the crowd layout code; `uc_all` class hierarchy of the item contexts; Ghidra (pool) `ClassDecomp` of `DisItemContext_MeleeAttack` and `DisItemContext_MeleeAttackNPCBase`, `Decomp` of 0x00C13640, 0x00C131C0, 0x00C338C0, 0x00C0CB80, 0x00C06440, 0x00C102C0; the pawn tweaks' localization (`Localization/INT/Pwn_*.int`, `Twk_Pawn_Boyle_Party.int`); the Boyle faction table (`ailevel3u`).
- **Findings:**
  - #1000-#1027 are the crowd's 28 party guests and #1028-#1033 its 6 guards (`default_crowd`, ids from `NPC_ID_BASE` civilians first): every killer was a guard.
  - The "Assassinations" were a sim bug: `find_assassination` ran for any pawn starting a swing, so a guard swinging at a player from behind (or standing behind another guard swinging at the same player) instantly killed whoever stood in the back-assassination geometry. In the original assassinations are player item contexts (`DisItemContext_Assassinate` < `DisItemContext_Finisher` < `DisItemContext_MeleeAttackPlayer`; `DisItemContext_DropAssassinate`); NPC melee uses `DisItemContext_NPCAttack*` (< `DisItemContext_MeleeAttackNPCBase`) and deals damage. The victim's `_Slave` assassination animations are synced to the player's (ANIMATION.md).
  - Guard-on-ally damage that remains is friendly fire: the NPC contexts' hit gathering (slot 117, 0x00C338C0) keeps every actor the weapon trace (0x00C0CB80 -> 0x00C06440, callback 0x00C102C0, flags 0x4002092) meets, and the shared hit handling (slot 119, 0x00C131C0) applies damage through the hit actor's vtable +0x148 with no relationship test found (U-660).
  - Guards hunted respawned players: their individual hostility (`Brain::hostile`, from a seen attack or witnessed murder) and attention were keyed by player id and never cleared, so every respawn under the same id was attacked on sight. In the original relationships and attention belong to a pawn actor; a respawn is a new pawn.
  - Party guests never kill: the deaths "by" civilians were none; guests are `Boyle_Factions.Neutral_Civilian`, guards take the faction of the guard spawner at their post (`Neutral_Guard`, neutral to `Faction_Corvo_Default`).
  - NPC names: the original names a pawn by its tweak's interactable subobject `m_Name` (`DisTweaks_InteractableInterface`, localized): Boyle's party guests "Party Guest", party guards "Watch Officer", named characters their own ("Lord Brisby", "Miss White"); street elite guards "Watch Officer", city guards "City Watch Guard".
  - Bob: Contracts drew spawns from all five Boyle PlayerStarts and every crowd point, so a player could start on the river grate or roof patio navmesh islands (or the off-mesh canal drop-off), which FFA already excludes. Bob ended at (-2950, 941, -1205), far below the garden (z about -315), never in reach of anyone and never seen by a guard: hence 0 kills, 0 deaths and dark frames. `think_contract` does move (it walked on its island).
  - The autopilot bots swung every 20 ticks within 160 uu whatever the geometry and whoever watched, so guards saw attacks constantly; with the two bugs above that meant repeated deaths.
- **Port:** `dishonored_sim::combat::resolve_sword_hits` (assassination for `PawnKind::Human` attackers only); `dishonored_ai::Crowd::forget`; `ContractsMatch` spawn candidates limited to the largest walkable navmesh group (`walkable_candidates`, the FFA rule extended to crowd points) and `forget` on respawn; `dishonored_assets::characters::ARCHETYPE_TWEAKS`, `dishonored_multiplayer::Look::tweak` (net protocol 8), `dishonored_content::pawn_name`, the runtime kill feed; `dishonored_bots::Bot::think_contract` (back assassination only when lined up and nothing else is nearer, never in front of a watching guard, run from a charging guard, after 8 s close without an opening the sword, or the pistol for gunners); `harness contracts` (all-deaths table, "only players assassinate"). Docs: multiplayer/CONTRACTS.md.
- **Results:** headless `harness contracts --minutes 3` (8 bots, seed 9) before / after: contract kills 12 / 13; guard kills of players 26 (22 "Assassination", 2 pistol headshot, 2 sword) / 10 (9 sword, 1 pistol headshot); NPC-on-NPC 9 (all "Assassination") / 8 (friendly-fire sword hits: 6 guard, 2 guest); NPC assassinations 31 / 0. Rendered (release server plus three 960x540 `--autopilot` clients, Alice duelist, Bob gunner at 80 ms, Cara duelist at 120 ms and 3 % loss, 5 minutes) before / after: player kills 1 / 15 (all assassinations; 13 contract kills: Alice 6, Cara 5, Bob 2), guard kills of players about 27 / 6 (5 sword, 1 pistol headshot), NPC-on-NPC kills by "Assassination" several / 0 (5 friendly-fire sword deaths), Bob 0 kills 0 deaths / 2 kills 8 deaths; the feed reads "Watch Officer killed Bob (Sword)" and the scoreboards agree.
- **Tests:** `dishonored_sim` `npcs_never_assassinate`; `dishonored_ai` `a_new_pawn_starts_without_the_old_grudges`; `dishonored_content` (retail) `npc_names_come_from_the_pawn_tweaks`, `contracts_spawns_stay_in_the_walkable_estate`.
- **Confidence:** CONFIRMED for the assassination contexts being player-only (class hierarchy) and the names (localization); LIKELY for melee friendly fire (U-660). Forgetting grudges on respawn is a multiplayer rule (the original has no respawn), argued from per-actor relationships.
## 2026-10-07: Square particle backgrounds by the party lanterns; the objectives banner as a popup

- **Question:** (1) Why do the steam / smoke sprites by the front-door lanterns on L_Boyle_Ext_P draw dark grey squares? (2) When does the original show the top-right objectives panel?
- **Method:** (1) `DISH_FX_LIST` / `DISH_FX_DUMP` (now also printing static switches, masks and textures) over the placed emitters; captures at the front door; `find_export`, `obj_dump` and `mic_static` of `M_TB_CrouchSteam_INST`, `cigarette_smoke_inst` and `DefaultTranslu_PMAT`'s texture expressions; `shader_maps` / `shader_dump` of RefShaderCache and `fxc /dumpbin` of base pass pixel shaders 2815, 35485, 35475. (2) `swf_dump UI_HUD_SF HUD - --actions` on `ObjectivesNotification` (798), `ObjectivesWindow` (775), `TargetNotification` (796) and the export sprites' class registration; `tweaks Twk_InGameUI.Twk_GFxMoviePlayerHUD`; `DisGFxMoviePlayerHUD.uc`; `DefaultInput.ini`.
- **Findings:** (1) `Ps_SmallLights_01`'s steam and smoke emitters are on `DefaultTranslu_PMAT`, whose opacity is `O_Opacity`'s channel or, with `Mo_Enable_Opacity_Motion`, the red of a distorted `Motion_Mask` sample, times `D_DiffuseColorTweak.a` and vertex alpha; colour is `D_Diffuse` / white (`D_DisableDiffuse`) / `Motion_Mask` times the tweak. The port used the opaque `D_Diffuse` alpha and no tweak: dark squares. EFFECTS.md "Particle materials". (2) The objectives panel and the target paper are timed popups (0.2 s slide-in, 0.7 s + 3.5 s per task, 0.25 s fade, hidden 0.5 s later; target 1 s + 5 s, 0.25 s fade), the first one 2 s after game start; no key shows them (objectives live in the journal, J). MENUS.md "Objective popups".
- **Port:** `dishonored_assets::level` (DefaultTranslu permutation into the sprite's diffuse slot), `effects::material_for` (tweak tint and opacity), new `hud_popups.rs` (banner / target popups: match start, score or leader change, objective change, J), `gfx_hud.rs` (alpha and slide), `hud_settings.rs`, scoreboard header with the clock (`hud.rs`).
- **Confidence:** CONFIRMED for the shader terms and the popup timings; APPROXIMATION for the undistorted motion mask, the popup queue (re-open restarts) and the multiplayer banner's triggers (no original counterpart).
## 2026-10-07: Frame rate in a free-for-all on Boyle Ext (per-frame work, first pass)

- **Question:** A player reported very low frame rates in an offline free-for-all on `L_Boyle_Ext_P` (release 13eb647, from the launcher). What costs the frame time, and what can go without changing the image?
- **Method:** code reading of every `Update` system and the custom render passes, and of Bevy 0.19's environment-map generator and UI layout; new `--profile` statistics (per viewpoint in `--inspect` scripts) and an optional Chrome trace build (`--features trace`) for the measurements. The measurements, before and after, are taken on the integrated build (RENDERING.md "Performance").
- **Findings (per-frame work that does not change the output):**
  - Bevy's `GeneratedEnvironmentMapLight` re-filters the level's static reflection cubemap every frame (copy, downsample, radiance and irradiance convolutions); the filter is deterministic.
  - Every particle emitter in range (9000 uu) rebuilt and re-uploaded its sprite mesh every frame: empty emitters, emitters that draw nothing and emitters behind the camera included.
  - The HUD, dialogue HUD and menu layout wrote every UI node, image and text every frame, so Bevy re-ran the UI layout and re-shaped the texts every frame; the HUD options were applied after the HUD and flipped the same components back each frame.
  - Each door and usable cloned its skeletal model every frame; lip sync re-read the map's package list from the install every frame a line played; `DISH_ANIM_DEBUG` was read from the environment per pawn per frame.
- **Port:** `env_map.rs` (filter once), `effects.rs` (sprite meshes rebuilt only with particles in this frame's view, emptied once when they leave it), `gfx_hud.rs` (`place`, `set_text`, `set_font_size`), `hud_settings.rs` (`HudOverrides`, decided before the HUD), `hud.rs`, `hud_dialogue.rs`, `menu_art.rs`, `level_props.rs`, `characters.rs`, `profile.rs`. Docs: RENDERING.md "Performance".
- **Results:** pending the integrated build's measurements (frame time at the 8 fidelity viewpoints and in a 3-bot match, 1280x720 and 1920x1080, before and after; `harness fidelity --configs full` SSIM before and after).
- **Confidence:** CONFIRMED (code read) that each removed piece of work left the output unchanged; the size of each saving is not measured yet.


## 2026-10-07: Playground failures after integration (9 of 31)

- **Question:** Which of the first `harness playground` failures are port bugs, and which come from the layout or the harness?
- **Method:** the integrated release harness run on the layout as it was and after the layout changes; the sim's Blink, stepUp, mantle and breath code read against PLAYER.md, COLLISION.md and HOOK_TRIAGE.md.
- **Findings:**
  - Blink level 1 travelled 1600: **port bug**. The range used a fixed level index (`BLINK_LEVEL`), not the pawn's `m_Powers` level. The measured value is 1100 at level 1 (HOOK_TRIAGE.md).
  - Blink "straight up" read 0: **harness**. It measured the pawn after it had fallen back. It now reads the target while aiming at 19.84 degrees up, the measured case (1385.7 ahead, +500).
  - Steps 45 and 50 climbed: **expectation**. `stepUp` rises `MaxStepHeight` + 2.54 from where the box rests, 6.73 above a per-polygon floor, so it clears up to about 53. With the new steps (30 / 44 / 50 / 56 / 60), 50 is climbed and 56 is not.
  - Mantle 260 and 300 reached: **expectation**. The finder also runs every airborne tick, and the jump lifts the feet by up to 113. With the new ledges (up to 360), 330 is reached and 360 is not.
  - These two limits, and the launch speed below, are LIKELY, not measured (U-763).
  - Jump launch 750 against 800: **sampling**. The port's trace is taken after the jump tick's physFalling. The apex agrees. The launch check is dropped from the Playground scenario (U-763).
  - No drowning: **harness**. The idle swimmer floats with its head out, and drowning takes health directly, with no damage event. The harness now dives and counts the health drops.
  - Door not passable: **layout**. With yaw 0 the copied doors stood across the doorway. They now use yaw -90 with the hinge at the gap's edge. The gaps are 152 wide, as the door bodies are.
  - Locker unused: **harness**. Its use point was out of the 220 uu reach.
- **Port:** `dishonored_sim::blink` (the level per pawn; new test `blink_range_follows_the_power_level`), `maps/playground.ron`, `missions/playground.ron`, `fixtures/playground/scenarios`, `tools/harness/src/playground.rs`, PLAYGROUND.md, U-763.
- **Results:** with the old binary on the new layout, steps, mantle and both doors pass (26 / 31). The Blink, pool and locker fixes need the next build.
- **Confidence:** CONFIRMED (code read) for the Blink level bug; LIKELY for the step and mantle limits and the launch sampling (U-763).

## 2026-10-08: Square blood, Blink orbs, Dark Vision, power and gadget effects, the target paper, item icons

- **Question:** From the playground playtest: why do blood hits draw squares, why does a glowing orb stay at every Blink destination, what do Dark Vision and the other powers and gadgets show in the original, and why does the target paper copy the task line?
- **Method:** `obj_dump` of every `ParticleModuleRequired` and `MaterialInstanceConstant` in DishonoredGame (systems to materials to bases, 118 materials on 22 bases) and of the contact system's `DisContactSub_Particle` objects; `mic_static` of every instance; `shader_maps` of RefShaderCache and `fxc /dumpbin` of each base's particle base pass pixel shaders; `tweaks` of `Twk_Blink`, `Twk_DarkVision`, `Twk_BendTime`; `find_export` for the Startup systems; `obj_dump` of UI_Powers_Large / UI_ItemIcons_Large; `swf_dump --flatten hud_tgt`. Findings from the oracle scenario runner (phase 4) folded in.
- **Findings:** EFFECTS.md "Particle materials" (the per-base colour and opacity terms; the blood is SubUV_HeightMap with an opaque DXT1 diffuse, alpha-tested by the port), "Gameplay effects" (the Blink systems are the targeting indicator with endless, kill-on-deactivate emitters), "Dark Vision"; muzzle sprites without a Lifetime module live until their emitter completes; the contact table's Sword x Body names `Pfx_BloodMotion`, which was not decoded.
- **Port:** `dishonored_assets::particle_material` (recipes, gameplay particle list incl. the whole contact table and the Startup power / gadget systems), `level::particle_sprite`, `ParticleShading`, `sub_random`; `effects.rs` (shading, erosion, random sub-images, emitter completion, 12 s one-shot cap, Blink indicator, attached trails / cones, Possession / Swarm / grenade / razor / crossbow effects, no muzzle on throws); `dark_vision.rs` + `dark_vision.wgsl`, `ark_post` power grade; `debug.rs` gizmos behind F3; `hud_popups` / `gfx_hud` target paper as its own popup, item icons; `PlayerState::new` keeps the spawn yaw as last input; harness playground power checks, visual Blink indicator check.
- **Confidence:** CONFIRMED for the shader terms, the tweak values and the system paths; APPROXIMATION for the erosion, the Look_Up_Fire colour, the Dark Vision materials, the Bend Time screen effect (not done), cast effects at the eye.
- **Tests:** not run (no-build rule); new: `dishonored_assets` `particle_material`, retail `gameplay_particle_sprites_have_an_opacity`, `dishonored_audio` retail `contact_particles_are_all_decoded`, runtime `effects` `particles_without_lifetime_go_with_their_completed_emitter`, `hud_popups` `mission_paper_opens_for_a_notification_only`.

## 2026-10-08: First-person layering: the player's AnimTree filters, the states' sequences, weapon sequences

- **Question:** Why does the held gun rotate while running forward, why does a hand pass through the pistol / crossbow when firing, and how does the original select and blend the first-person sequences?
- **Evidence (Startup, `Ply_Player_at` and the `Twk_Pawn_Corvo` animation tweaks, dumped with `obj_dump` / `export_hex`):**
  - The tree: `LeftArmFilter` (`DisAnimNodeBlendPerBone`, branch `Collarbone_L_Jnt`) over `UpperBodyFilter` (branches `spine_3_jnt`, `Collarbone_R_Jnt`, `Collarbone_L_Jnt`) over the `FullBodyStatePicker`; the left filter's target is the `LeftArmStatePicker` under `DisAnimNodeBlendItemAimAdditive` (`DishonoredSeqBlendByAim`, `ADD_Guns_Aim_*`) and `DisAnimNodeBlendHeartAdditive`.
  - Both filters cook `LocalToCompReqBones` (0, 1, 48 for the left arm; 0 to 5, 20, 48 for the upper body), so they blend in component space. `Child2PerBoneWeight` is 1 over the branches and 0 elsewhere (bones 10 to 19, the legs, are outside the upper body). `Collarbone_L_Jnt` hangs off bone 1.
  - The gun and power sets drive bones 0, 1 and the left arm only; the sword sets drive everything but the legs.
  - The states (`m_AnimStates_*`): full body `Nav_Default_Ready` (Ready_Idle / Walk / Run / Jump), `Nav_Sprinting_Ready`, `Nav_Crouched` (Sneak_Idle / Walk / Jump), `Mantle` (low 20 / medium 50 / high 80 travel Z, blocks children), `AnimStunned`, `Choke` (blend 0), the assassinations; upper body the melee attacks (0.1), `AnimMeleeBlock`, `PistolReaction` / `CrossbowReaction` (Ready_GunshotReaction), jump in / small land, crouch in / out, equip; left hand `Nav_Default` (`Ply_Nav_LeftHand_at`: idle, walk for walking and running, sprint, air; `MasterIdleSync` / `MasterRunSync` / `MasterSprintSync`, `bNoNotifies`), equip / switch, `SwordSlash_Left/Right`, jump reactions, fire, reload in / out, fast reload (blend 0), casts. Blend times as listed in `viewmodel.rs`.
  - Head bob is an additive (`ADD_Head_Walk/Run/Sprint/Sneak{N,S,E,W}`, `Ply_Head_Locomotion_as`, `DisAnimNodeBlendByHeadBob`) over walk and run cycles whose own camera is steady.
  - The weapon meshes have their own sets and trees: `CorvoGun_as` (and `_Accuracy`, `_Mag`, `_ReloadSpeed`), `Crossbow_as` / `Crossbows_as`, `PlySword_Anim_as`; `CorvoGun_at`, `Crossbow_at`, `CorvoSword_at`.
- **Result:**
  - Rotation while running: the port blended the left arm in local space, so the gun arm inherited the pelvis twist of `Sword_Ready_Run` through bone 1. In component space it keeps the gun set's own pose relative to the root.
  - Firing: the original plays the gunshot reaction on the upper body (the sword arm moves away) and the weapon's own sequences; the port played neither.
- **Port:** `crates/dishonored_runtime/src/viewmodel.rs` (three pickers, component-space filters, states and blend times from the tweaks, one-shot transitions, left-hand sync groups, the weapon sets matched by sequence name), `dishonored_assets::characters::anim_set_path`, `fixtures/scripts/viewmodel_states.json` (Playground screenshots of every weapon and power state).
- **Not done:** the head-bob and aim additives (the additive encoding of Edge clips is not verified), the weapon trees (`CorvoGun_at`, `Crossbow_at`: the weapon sequences are matched by name), zoom, the ready / non-ready stance switch, the big landing, parries.
- **Confidence:** CONFIRMED for the tree, the masks and the state tables (data); LIKELY for the weapon sequence names; the port's result is not yet checked against the game.
## 2026-10-08: Differential oracle phase 4: the scenario library and the full port runner (U-860..U-866)

- **Question:** Can the oracle play every system of the port the way the game does, headless, and say per scenario whether it matches the original's measurements, including what the presentation would show (the playground reports: powers showing nothing, Blink orbs never despawning, blood squares)?
- **Method:** `tools/oracle` runner on the game's own match (`dishonored_content::mission_setup` on `MapContents::load`, `Game::step` at 60 Hz); a presentation shim compiled from the runtime's own handlers (`build.rs` extracts `effects::gameplay_effects`, `effects::step` and the one-shot despawn rule, `audio::drive`'s simulation-event part, `characters::collect_hits`); gameplay ParticleSystems decoded headless; the contact system loaded. 46 RON scenarios with checkpoints from INSTRUMENTATION.md / HOOK_TRIAGE.md (Measured), the tweaks and decoded rules (Static), or capture requests (Unmeasured / Unverified). Expected effect systems read from the tweaks dump (`m_pGroundPS`, `m_EffectTemplate`, `m_VisionConePatrolTemplate`, `m_pHandCastParticle`, `m_pSpawnFX`, `m_pExplosionEffect`, `m_pDetonationEffect`, `m_pBulletTracerFX`, `m_pTracerFX`, `m_pTrailEffect`).
- **Findings:** 46 scenarios in 6-10 s: 28 pass, 13 fail, 5 unmeasured; 84 / 24 / 36 / 10 checks pass / fail / unmeasured / unverified. The measured mechanics of Blink, Windblast, Bend Time, grenade fuse, pistol reload, dispersion and spread, sword timing, run / sprint / jump, drowning, lean angle and the step / slope / mantle limits match. Presentation divergences: the Blink arrival always plays `Blink_Fall_01` and it never ends (U-860); the player muzzle never ends (U-861) and also shows on grenade throws (`SimEvent::Fired` from the throw); sword hits draw no blood (`Pfx_BloodMotion` from the contact system is not decoded); Dark Vision, Possession, Devouring Swarm, grenade explosion / trail, Spring Razor, pistol tracer, crossbow muzzle / trail spawn nothing. Mechanics: crossbow shot to load 1.583 s (1.555), hearing a shot raises attention to the maximum (U-862), passive NPCs turn to yaw 0 (U-863), lean onset 10.7 degrees at 0.2 s (14.2), detection placement unknown in the original capture (U-864).
- **Port:** `tools/oracle/{build.rs, src/present.rs, src/runner.rs, src/scen.rs}`, `dishonored_oracle::scenario` (spec additions, event metrics, statuses, `spec_hash` over the run-defining fields only), `fixtures/oracle/scenarios/**`, `fixtures/oracle/causes.ron` (U-113 sword damage rule), ORACLE.md phase 4.
- **Tests:** `cargo test --release -p dishonored_oracle -p oracle` (new `event_metrics_and_statuses`, `extracted_tables_present`); `oracle scenario --all`.
- **Confidence:** CONFIRMED for the port's values (deterministic runs) and the tweak references; the Unverified rows wait for comparable captures.

## 2026-10-08: Mantle motion from the sequences' root motion; synced bodies drawn at the master's root; gadget meshes

- **Question:** How does the original move the pawn through a mantle, and where are choked and carried bodies drawn?
- **Evidence:**
  - `Sword_Ready_Mantle{Low,Medium,High}` (Startup `Ply_Sword_Locomotion_as`, Edge; `anim_root`): the root bone `root0_jnt` rises 20 / 50 / 80 uu (mesh -Y) and moves 40 uu forward (mesh +Z) over 0.633 / 0.700 / 1.033 s (19 / 21 / 31 frames). The rises equal the `m_fMantleTravelDistanceZ` of the `DishonoredAnimNodeSeqMantle` nodes of `Ply_Player_Mantle_at`. The `Sword_Sneak_*` variants carry the same root motion.
  - `StatePlayerMasterMantle` (class registration 0x01300DF8, size 0xC0; `prop_layout`): `m_fCurrentMantleHeight` +0x7C, the intro start / target location and yaw +0x84..+0xA0, `m_fIntroTransitionTime` +0xA4, `m_bFaceEdge`.
  - 0x00A8F9B0 (`DishonoredAnimNodeSeqMantle`): if `m_fMantleTravelDistanceZ` is not 0, it builds a transform (component rotation, scale (1, 1, `m_fCurrentMantleHeight` / travel)) and gives it to the sequence's root motion (`m_pLocomotionState` +0x2C, flag 0x4000). The vertical root motion is scaled to the height; the forward part is not.
  - The state's tick (0x00A6C590) runs the timeout and the player cancel and does not move the pawn. The intro (0x00A636C0) slides the location and yaw linearly. Its time is the previous state's `m_fTimeToGo` when that state is a `StateSharedActionBase`, else -1 (0x00A6EC20 -> 0x00A60270). With no intro the pawn is set at the intro target at once (`SetLocation` / `SetRotation`).
  - The request (0x00AD78D0) reads the finder result: location +0x60, yaw +0x6C, height +0x70, crouch / sneak flags +0x74. An edge no higher than `m_LowMantleMaxEdgeHeight` (120), with `m_bLowMantleUseStepUp` (true) and no crouch needed, goes to the step-up 0x00AD35E0 instead: three moves at once (to the finder location, up by the height, forward by `m_fMantleForwardMoveAmount` 20). Every other edge enters the mantle state (0x00ACD820, params copied by 0x00A63620).
  - `Attribute_MantleAnimRate` is 1 at Normal (`pAttributeTweaksNormal`); Agility adds 50 % of the base.
- **Port:**
  - `MantleConfig` carries the decoded curves (`anims`, built by `dishonored_assets::gameplay::mantle_anims`), `low_use_step_up` and `anim_rate`. The config cache key is now v12x.
  - `movement::try_mantle_from` picks the class from the thresholds, does the step-up at once, or places the pawn at the finder location and runs `mantle_root_position` (the scaled root motion) to the destination. `advance_traversal` plays it at the attribute rate.
  - The two-leg path is gone.
  - New tests: `mantle_path_is_the_sequence_root_motion`, `mantle_traversal_runs_the_sequence`.
- **Not decoded:** where the finder puts its result location (its final-verify states). The port uses the pawn against the edge's face at its own height, and the height that lands the root motion on the top (U-019).
- **Bodies:** the slave sequences of the choke and the carry are authored at the master's root. `viewmodel::place_held_bodies` therefore draws a choked or carried body there. For the local first-person master that is the rig's pawn space, whose `camera_jnt` is the view, so the carried body hangs over the shoulder where `Empty_CarryCorpse_*_Master` puts it. A released body keeps that frame (`BodyFrames` anchors, also used for its ragdoll's floor) while its slave sequence ends. A drop plays `Corpses_CarryCorpse_DropLow_Slave` before the ragdoll. A lift from a won choke plays `Generic_Choke_CarryCorpse_Slave`.
- **Gadgets:** the left hand holds `Grenade.Grenade` (grenades and sticky grenades) and `SpringRazor.SpringRazor` with their own idles. The crossbow shows `Crossbows.bolt_01` / `Bolt_Sleep` / `Bolt_Flare` on its `Bolt_Socket` while loaded. The weapons' own sequences are now the names the sets hold (`CorvoGun_Fire_Hammer`, `Crossbow_Reload_In`, `Sword_Ready_Idle`, ...).
- **Viewmodel states:** `Climb` (the reference pose, `Head_Rope_Climb` while moving) and `Nav_Swim` (`Empty_SwimIdle` / `Empty_SwimN`, LIKELY: that tree was not dumped).
- **Confidence:** CONFIRMED for the curves, the thresholds, the step-up and the scaling. LIKELY for the finder location and the weapon sequence pairing.
## 2026-10-08: Oracle phase 2, the original side (hook)

- **Question:** Can the original answer per-tick behaviour traces and by-name queries cheaply enough for automated fuzzing, from one long-lived game session?
- **Method:** a world-tick boundary hook on the time update 0x009DDDF0 (single caller `FEngineLoop::Tick` 0x009E03B0; prologue `55 8B EC 83 EC 20`); lockstep parking there; reflection layouts read live from `Actor`, `Actor.Location`, `Actor.SetLocation`, `Actor.EPhysics`, `WorldInfo.StreamingLevels`; the native exec path read at `execSpawn` (0x006CD450) with an entry hook (its `FFrame`).
- **Findings:**
  - UObject 0x38 bytes; UField.Next +0x38; UStruct Super / Children / PropertiesSize +0x44 / +0x48 / +0x4C, Script TArray +0x50; UProperty ArrayDim +0x3C, ElementSize +0x40, PropertyFlags (u64) +0x44, Offset +0x5C, subclass pointer (Struct / PropertyClass / Enum / Inner / BitMask) +0x6C, ClassProperty MetaClass +0x70; UFunction FunctionFlags +0x80, iNative (u16) +0x84, NumParms +0x91, ParmsSize +0x92, ReturnValueOffset +0x94, Func +0x9C; UEnum Names +0x3C.
  - `ProcessEvent` silently does nothing for functions with a native index (Abs, VSize, SetLocation returned 0 / false). Their parameter script is `EX_NativeParm` (0x29) per parameter; running the thunk with an `FFrame` (vtable 0x00FB944C, Node +0x10, Object +0x14, Code +0x18, Locals +0x1C) gives Abs(-3.5) = 3.5, VSize(3,4,0) = 5. `Spawn` (iNative 0) works through ProcessEvent; it returns None for bNoDelete classes such as `Emitter`.
  - Dishonored's sneak is `m_bSneakModeToggled`; `bIsCrouched` stays false while sneaking (CollisionHeight 87.5 -> 65). Mana is the int `m_Mana` (+0xA60).
  - Jump at a fixed 60 Hz step, sampled at the end of each tick: the press tick ends with Velocity.Z 800, Physics Falling and no displacement; the next tick 750 with +12.917 uu; apex 106.667. The port applies the launch and the first gravity step in the press tick: one tick early (U-763 (3)).
  - Free-running ticks in the level: 26-38 / s on the loaded machine; one pipe round trip per tick caps traces at 6-9 / s; a tick script inside the game runs 70-110 / s.
  - Boot to in control 27-50 s (waiting on world ticks instead of rendered frames); map switch 4-11 s.
- **Tools:** `crates/dishonored_probe_dll/src/{tick,reflect,script}.rs`, protocol 5 in `crates/dishonored_probe`, `tools/probe_launch/src/{session,reflect,capture}.rs`; `dishonored_oracle::diff::first_divergence` compares the event kinds both sides record. Docs: ORACLE.md "Original side: the hooked session", INSTRUMENTATION.md.
- **Confidence:** CONFIRMED (measured live) for the layouts, the call paths and the jump ticks.
- **Open:** U-800 (Kismet always runs in the original), U-801 (story flags not set by the runner), U-802 (NPC setup from tweaks / AI states), U-803 (sounds are not traced), U-804 (snap height and floor hover differ at tick 0).

## 2026-10-08 â€” Oracle phase 4: port rule fixes found by the scenario library
- **Question:** Which Measured / Static scenario failures are port rule bugs, and what is the original's rule?
- **Evidence:** Ghidra (pool scripts FuncDecomp / OffsetScan / G_Callers): reload contexts 0x00C2AA80 / 0x00C2AC00 (f32 countdown, actions end when the f32 anim time reaches AnimStateExit); DishonoredCamera_Lean (vtable 0x0111E890): spring 0x00AC0A00 (k 80, c 12), 0x00AC5950 (external force, circle around the pivot), 0x00AC5540 (limit and soften), lean speed 1000 via 0x00A76590 -> 0x00AC0950; NPC rotation intents 0x00B799C0 / 0x00B6E970 / 0x00B71570 / 0x00B71600; hook jump trace 2026-10-08; snap line check 0x20DF.
- **Result:**
  - Jump: the press tick only launches (Velocity.Z 800, Falling, no move); the fall starts next tick. `movement.rs` walking(): decide the jump, walk the tick, then launch. Matches the hook trace (`movement/jump_standing`).
  - Lean: rebuilt as the decoded spring point; the head's sideways displacement matches the original per tick (13.33 / 26.31 / 62.7). The angle still differs (lean height smoothing, U-867).
  - Reloads: tick counts from the decoded countdown / anim-time rule (`ReloadTiming::ticks`, `action_ticks`, `countdown_ticks`); the shot leaves one tick after the fire action starts. Crossbow fire action still 2 ticks long (U-868).
  - Sword sheathed state (`PlayerState.sword_sheathed`, protocol 9): the original runs 400 / 600 with the sword away.
  - Hearing: the attention rule matches (0.6); the escalation to combat comes from instant NPC turning and the port-only `criminal()` rule (U-862).
- **Confidence:** CONFIRMED for jump order, head displacement, reload stages; LIKELY for the hearing cause.
- **Implementation consequence:** `oracle scenario --all`: 46 scenarios, 37 pass / 4 fail / 5 unmeasured (before: 35 pass / 6 fail of 41).

## 2026-10-08: Oracle phase 3, the static index

- **Question:** Can the original's code be queried statically, per system, cheaply enough for the porting agents: which native implements a behaviour, what the script does, where every trace call site is and with which flags?
- **Method:** a Ghidra headless pass (`tools/ghidra_index/scripts/BlinkIndex.java`) over the analysed GOG exe writes the native lookup tables, class records and vtables, every call, string reference and trace call site (flags resolved through the decompiler's p-code), and every function's decompiled C; an UnrealScript bytecode decompiler in `dishonored_oracle::index::uscript` decompiles every script class of the install; `oracle index build` joins both into a per-exe-hash index and `oracle index ...` queries it (ORACLE.md "Static index").
- **Findings:**
  - Native classes register a record holding a pointer to their UTF-16 name one character past the `A` / `U` prefix, then the InternalConstructor; 2760 script classes have one with a vtable.
  - All 2560 exec natives sit in 335 `FNativeFunctionLookup` tables; `GNatives` is not statically initialised (U-830).
  - UClass exports carry no tagged properties (`UObject::Serialize` skips them for classes), unlike functions, states and structs.
  - UE3 v801 bytecode: object references are 4 bytes in the file and 8 in memory, jump offsets in memory bytes; a `foreach` ends on `IteratorNext`, `IteratorPop`, and its `break` jumps to the pop.
  - 219 trace call sites, 72 distinct constant flag values; `APawn::physWalking` (0x006BD150) is `Pawn` vtable slot 141 (+0x234) and its floor check passes 0x20DF with the pawn's extent; `DisDoor`'s `ShouldTrace` is vtable +0x1C4 (0x00A2C760) and `m_DoorState` is at +0x311, as COLLISION.md measured.
- **Results:** 3004 classes, 4772 functions (2200 script, 169 KiB), 0 decompile failures; 73960 exe functions, 258262 calls, 49922 string references, 73682 functions decompiled to C (1 failure); queries 90-110 ms per process. `oracle index build` 493 s end to end (385 s of it the decompiler), about 100 s without the C, 6-9 s from existing tables.
- **Confidence:** CONFIRMED for the decompiler's byte-exact parse (both sizes match on every function) and the layout and vtable spot checks; LIKELY for trace flags resolved through parameters (U-831).
## 2026-10-08: Additive clips, head bob, hand effects, parries, slides, rats

- **Additive clips:** Edge stores additive sequences as deltas from the reference pose. `ADD_Head_WalkN` (4 tracks) and `ADD_Guns_Aim_Up` (30 tracks, 1 frame) hold identity rotations and zero translations where nothing moves. The port applies them the UE3 way (`viewmodel::add_additive`): the bone turns by `slerp(identity, delta, w)` before its own rotation and moves by `w` times the offset.
- **Head bob:** `Nav_Default`, `Nav_Sprinting` and `Nav_Crouched` lay `ADD_Head_{Walk,Run,Sprint,Sneak}{N,S,W,E}` (`Ply_Head_Locomotion_as`) over their cycle, synced to it, through `AnimNodeBlendDirectional`.
  - The blend is linear between quadrants: the cooked snapshot at DirAngle 0.959 gives forward 0.389 and right 0.611.
  - Default and sprint pass it through `DisAnimNodeBlendByHeadBob`, weighted by the Head Bob option (LIKELY). Crouched has no such node.
  - The view is the camera bone, so the bobbed `camera_jnt` moves the game camera.
  - The step-up plays `ADD_Head_Mantle_Low` the same way, through `StepUpMantle` (`DisAnimNodeBlendByStepUpMantle`, `m_fBlendTime` 0.2).
- **Aim additive (not done):** `DishonoredSeqBlendByAim` (`ADD_<Secondary prefix>Aim_*`, max 10 degrees each way). 0x00A8FB20 normalises an offset in degrees that it reads from the left-hand item (vfunc 0x00BFEF00, +0x64 / +0x68). The code that writes that offset was not found, so the additive is not applied.
- **Hand effects:**
  - The Mark of the Outsider's glows are `AnimNotify_PlayParticleEffect` on the rig's `Tattoo` socket (`hand_L_jnt`) in `Ply_Powers_as`:
    - Blink In / Loop: `Ps_Tattoo_Glow_02`; Blink Out: `_04`
    - Windblast: `_WB`; Bend Time: `_01`; Swarm: `_04`
    - Possession In: `_04`; Possession Loop and Out: `_03`
    - Switch: `_01`; Out of mana: `Ps_Mana_Empty`
    - The swimming splashes sit on the fingers.
  - `Twk_Powers_Possess.m_pHandCastParticle` and `Twk_DevouringSwarm.m_pHandFX` attach to `m_HandCastParticleAttachSocket` / `m_HandFXSocketName` = `LeftHand_PowerCastA`.
  - The rig's sockets (`Ply_Player.Skm_Player`): `Power`, `LeftHand_PowerCastA`, `Tattoo`, `Tatoo_Left`, `Bash_*`, `Camera_Socket`.
  - The port decodes the notifies (`dishonored_assets::characters::particle_notifies`), fires them on the crossed clip window, and keeps attached instances on the socket (`effects::HandEffects`). Particle sizes now scale with the instance transform: the rig is drawn at 0.2, and UE3 also scales particle size by the component scale.
  - The `Use_Power` arm-material pulse (`m_GlowParameterName`, `m_fGlowDuration` 1) is not done.
- **Parry and stance:**
  - The player's sword sets hold only Ready navigation (no `Sword_Idle` / `Sword_Walk` among `Ply_Sword_*`), so with the sword the non-Ready states have nothing to play and the view stays Ready.
  - `Blocking Parry` is an `AnimNodeRandom` of `Sword_Ready_Parry_A / B / C` at 0.33 / 0.33 / 0.34. It now plays on each won parry (`SwordState::parries`).
- **Slide:** `Ply_Player_Slide_at`: `SlideIn`, then the looping `SlideLoop`. The outro is `SlideOutSprint` when sprinting, else `SlideOutSneak`.
- **Rats:**
  - `ArkAnimNodeRat` (`Default__ArkAnimNodeRat`): moves WalkSlowN 55, WalkN 98, RunN 302 uu/s, max play rate 1.5.
  - The blend (0x00913ED0): the first move faster than the agent and the one below it; weights by speed; rate = dominant length / ((lo.speed lo.w lo.len + hi.speed hi.w hi.len) / speed), capped; one shared phase.
  - The agent: `IdleAnimNames` Idle..Idle4 below `m_fMinMovementVelocity` 12.5; mesh scale 0.8..1.2. `Twk_Rat`: attached attack `SwarmAttack` / `SwarmAttack1`; Devouring Swarm spawn `Spawn` / `Spawn1` / `Spawn2`; jump attack `Jump_Attack_*`; deaths `DeathFast_*`.
  - Spawn, death, windblast and the turn additive are not played: there are no view events for them, and the turn input is not decoded.
- **Placed razor and grenade:** they loop `SpringRazor_Idle` and `Grenade_Idle`, the only sequences their sets hold.

## 2026-10-08: The item aim, the head-bob weight, the Use_Power pulse timing, rat death and spawn rules (static index)

- **Item aim:** `DishonoredInventoryItem.m_CurItemAim` (+0x64, Vector2D).
  - The inventory tick 0x00C19A20 calls 0x00C0B990 for each equipped item. That function eases `m_CurItemAim` toward the item's target by `clamp(DeltaSeconds x DisTweaks_InventoryItem.m_fItemAimSpeed, 0, 1)`; the speed is 3 for Corvo's pistol, crossbow and sword, and the class default.
  - The target (vslot 102) is computed by 0x00C05CD0 for items and the sword: `(LastFrameCameraCache.Rotation.Yaw - CameraCache.Rotation.Yaw)` in degrees, with no pitch.
  - Ranged weapons (0x00C2CA20) add the yaw and pitch toward the aim-assist point (0x00C14460) when there is one. The port does not model aim assist, so that term is left out.
  - `DishonoredSeqBlendByAim` (0x00A8FB20) divides by its max degrees (10 each way) and blends `ADD_<item prefix>Aim_*` bilinearly on its 3x3 grid. Only `ADD_Guns_Aim_*` exists among the player's left-hand prefixes.
  - Port: `viewmodel` applies it to the left arm (`aim_weights`, `add_additive_blend`).
- **Head bob weight:** `DisAnimNodeBlendByHeadBob` (tick 0x00A88C30) weighs "On" by `DishonoredPlayerCamera.m_BobAmount` (+0x544) and "Off" by `1 - m_BobAmount` (1 without a player camera). The camera's begin play (0x00D06080) sets `m_BobAmount` from the options' head bob amount, unless a camera tweak value above 0 overrides it. CONFIRMED.
- **Use_Power pulse (timing decoded, not applied):**
  - The `DisItemContext_UsePower` start (0x00C44E90) sets `TatooGlowInfo` (0x00AA2060) for every power except the DLC06 Dark Vision. It uses the power tweak's `m_fGlowDuration` (1) and `m_fGlowRampDownDuration` (1).
  - The pawn tick (0x00ABB1E0) counts the duration down, then the ramp, and sets the arm material's `m_GlowParameterName` (`Use_Power`, default 0 in `PlayerArms_Tatoo_inst`) to remaining / ramp, so it runs 1 -> 0 over the ramp.
  - What `Use_Power` does in `Shr_DefaultPlayer` is not decoded: the cooked material keeps only its parameter expressions. The shader would have to be read.
- **Rats:**
  - Spawn: the agent's begin play (vslot 233, 0x00C792E0) plays a random `m_DevouringSwarmSpawnAnims` sequence only for rats of a Devouring Swarm. The sim keeps a Devouring Swarm as an aggregate (no rat pawns), so no rat pawn plays it.
  - Death (vslot 232, 0x00C844E0): a directional `m_Death*AnimName` (side from 0x00BBE900) plays only on one damage-class branch, when fewer than 5 ... and `rand < m_fRagdollRatio`. Another branch ragdolls (0x00C6C3D0). The damage classes were not resolved, so dead rats are not drawn yet.
  - Windblast: `m_WindBlastAnimName` (0x00C66500 and others) is not done.

## 2026-10-08: Ranged weapons and gadgets ported (ranged.md F1-F6, F10-F12)

- **Question:** Port the decoded ranged rules (research/dishonored/systems/ranged.md section 10).
- **Evidence:** Re-read in the static index: 0x00C29820 (bullet damage), 0x00C2A3E0 / 0x00C38230
  (fire, spawn, `Attribute_Range`), 0x00C3A660 (pierce), 0x00C503D0 / 0x00C4A310 (arrow damage),
  0x00B66900 / 0x00B66950 (hit regions), 0x00C5D940 / 0x00C5C280 (bounce, sticky sink),
  0x00C36350 / 0x00C35D60 / 0x00C2D650 / 0x00BC0280 (Spring Razor), 0x00C36E50 (bullet blast).
- **Result:**
  - Bullet 10 + 20, x1.5 strictly under 400 from the spawn point (camera + 150), long-range and
    stealth factors, pierce through one pawn (x0.5 on the next). Measured since (ORACLE.md
    2026-10-08): 45 at 300 uu, matches.
  - Crossbow: spawn 150 ahead along the dispersed direction, speed x max(1, Attribute_Range);
    head kill, x2 on unaware head / torso, limb cap. Measured 40 on an idle guard's torso.
  - Hit regions: the elite guard's physics bodies (bind pose) classify the shot; the cooked
    `KAggregateGeom` element `TM` stores each plane as (W, X, Y, Z) (the ragdoll's
    `matrix_origin` reads it shifted by one float).
  - Grenade bounce law with blended restitution / damping and the rest threshold; sticky 5 uu
    sink on pawns.
  - Spring Razor: cylinder from the base to its height, strict bounds; three points at the
    centre +- 2/3 of the half height; a pawn without the razor interface (the player) takes
    `m_ImmuneNPCDamage` (50), not `m_NonNPCDamage`.
  - AI noise: pistol and explosion from their `EAINoiseLoudness` (6000); crossbow and throw have
    no loudness field and no longer make noise.
- **Confidence:** CONFIRMED (decompiled; pistol and crossbow damage measured); the region
  bone mapping and the razor's NPC sample points are the port's reading.
- **Implementation consequence:** gameplay config cache key `v13r`; `ProjectileState` gains
  `spawn` / `pierced` (protocol bump needed). Open: muzzle-socket bolt spawn, throw origin at
  the hand, aim assist, bolt recovery / pinning, the bullet blast (F13), grenade impact damage 6.

## 2026-10-08: Movement findings ported (MV-1, MV-2, MV-3, MV-6, MV-7)
- **Question:** Port the movement findings of `systems/movement.md` from the decoded code.
- **Evidence:** `oracle index fn` 0x006BF0E0 (physFalling tail), 0x00ADBD00 / 0x00ADBE70 / 0x00AA3820 / 0x00AD78D0 / 0x00ACD820 / 0x00AD35E0 (mantle trigger, request, step-up), 0x00AC6B80 / 0x00AD6410 / 0x00ADA6F0 / 0x00AD6600 / 0x00AD69B0 / 0x00ADA4C0 / 0x00ACCAD0 / 0x00AD5F40 / 0x00AC6D40 (edge finder), 0x00AA4700 (sword factor), 0x00AB53A0 / 0x00A63BB0 + `DefaultPlayerState.ini` (stun), 0x00AC5420 / 0x00AC5540 / 0x00AC5950 / 0x00AC0A00 (lean), 0x006BD150 at 0x006BECD8 (ledge fall); class layouts of `DisTweaks_PlayerPawn_Mantle`, `DishonoredMantleEdgeFinderComponent`, `DisMantleFinderState_*`, `Pawn.CrouchHeight` (+0x290).
- **Result:** MV-1, MV-2, MV-3, MV-7 ported; MV-6 ported except the mantleable-actor grid (the 6 x 6 grid is not a landing check, correction in movement.md); MV-4 was already rebuilt (re-read, the remaining angle gap is U-867); MV-5 open (the stun length depends on animation events: `EStunAnimState` phases, only Stunned uses `m_fStunDuration` 0.1); MV-8 open (the ledge-fall condition depends on undecoded +0x280 bits); MV-9 open (no actor data in hits). `PlayerState.right_hand` replaces `sword_sheathed`, `PlayerState.mantle_allowed` added (protocol 11). Scenarios `mantle_330` / `mantle_360` hold Jump (spec hash changed: recapture).
- **Confidence:** CONFIRMED (decompiled code) for the ported rules.
- **Implementation consequence:** `oracle scenario --system movement`: 23 pass / 3 fail / 12 unmeasured (lean, lean_boyle: U-867 and a contradictory reference; mantle_330: a flush-contact collision artifact, movement.md "Port status"). Floor hover (U-804): the Playground needs simple-collision brushes, blocked by the same flush-contact issue in `sat`.
## 2026-10-08: Porting the world findings: chaos as the darkness score, charms, doors, save waits

- **Question:** Port systems/world.md F-1..F-19 by the decoded rules.
- **Evidence (re-read with the static index):** `Add` 0x00C41B50 (sum, then `& ((x < 0) - 1)`: clamp at 0); level 0x00C487D0 (largest i with score >= levels[i], -1 below); deeds 0x00C4D2C0 (counter += (int)amount; at the threshold add `m_Value`, clamp, reset to 0; victim filter by the tweak name test 0x00C6F450 against the victim tweak's name and `m_FallbackChainCooked`); `DisSeqVar_DarknessLevel` slot 93 0x00B8BAA0 / `DisSeqVar_Darkness` 0x00B8B990 write the level / score; `DisConv_CompareDarknessLevel` 0x00CA8FE0 (0 equal, `(cmp <= level) + 1`); `DisSeqAct_MissionStatsTracking` 0x009FE090 (stat type 0x1B: level >= `m_ChaosLevelThreshold` -> 1); `DisSeqAct_NotifyMissionEnd` 0x00B8B110 -> 0x00C4D500; `DisSeqAct_IncrementPlayerStat` 0x00AB9B90 ends in the stat increment 0x00AB9560, which calls the deeds; alarm bell slot 244 0x00A3D080 (stat 3); corpse 0x00A98950 (stat 4, once per corpse).
- **Data:** `m_DarknessLevels` is a `TArray<int>` the generic tweak reader took for object references; read raw (`TweakDb::int_array`): 0, 12, 48, 72, 96, 114, 138, 162, 168, 200, 390 (closes U-W1 from the data). `m_Deeds` as in world.md; tweak names are cooked `Pkg+++Obj Class`.
- **Bone charms (correction of F-7):** `m_Effect` is an effect id: below 10,000,000 an index into `m_Charms`, from it up into `m_ScriptedCharms` (0x00AA8EF0); the owned list is (level byte, id) pairs and taking an owned id raises its level (0x00ABBA90). `m_ScriptedCharms` has 12 entries. Placed charms in Boyle have no `m_Effect` (class default -1): a run-time distribution (pool at +0xB44 of the game object, which `SetBoneCharmEffect` 0x00A22970 returns regular effects to) is not decoded (U-W12).
- **Doors:** Kismet Open 0x00A1A450 only from closed and CanUse, Close 0x00A1A4B0 only from open; `DisSeqAct_Lock` 0x00B8F730 calls usable slots 238 / 239 (0x00A4E870 / 0x00A4E910; the door's 0x00A32490 adds the path blocker). CanUse 0x00A4E2E0: `+0x2C4` is `m_State` (PLAYGROUND.md corrected). No key path in `DisDoor::Use` 0x00A37C10 (U-W4 stays open).
- **Save waits:** pop 0x009F95F0 (`count--`, `timer = max(timer, wait)`); pop sites: Choice_Base 0x00A6C7E0 (0.1), controller cinematic 0x00AAF150 (1.0), combat tracker 0x00AB9FA0 (type 4 pushed when `DisTweaks_PlayerPawn_Combat.m_bDisableManualSaveInCombat`, popped with `m_fDisableManualSaveAfterCombatTime_s` = 1.0: closes U-W8); the others pass 0.
- **Result:** ported F-1..F-5, F-7 (corrected), F-8, F-10, F-16, part of F-17, F-6 as data, F-19; F-9, F-13, F-14, the AddPower level mapping and the store purchase rule left open (world.md 10.1). Oracle: `darkness.*` channels and `adddarkness` console hook; `chaos/adddarkness_sweep`, `civilian_kills`, `guard_kills_sweep`, `corpse_discovered` pass.
- **Confidence:** CONFIRMED for the rules above; the kill-credit side test (U-W2) and whether placed (spawned-dead) corpses count are LIKELY.
- **Implementation consequence:** cache keys: gameplay config `v12x-world2`, pickups `pickups2`, new `<map>.lineage1.json`, dialog `v3`; net protocol 10 (u64 inventory masks, `charms_level2`, `MissionView` darkness fields); saves gain `MissionMatch::darkness` / `corpses_discovered` and autosave timer fields (serde defaults keep old saves loading).

## 2026-10-08: Box-check touch rule, Playground Cube_100 collision, the hard landing decoded
- **Question:** Why does a pawn flush with a face block box sweeps along it (`mantle_330`), what is the original's contact rule, and what does the hard landing do?
- **Evidence:** `oracle index fn` 0x005DEB70 / 0x005DEA10 / 0x005DE780 (kDOP box check), 0x007AF030 / 0x007A8F60 / 0x007A8C80 (convex), 0x004D9430; 0x00AB53A0, 0x00A70F20, 0x00A6E020, class `DisTweaks_PlayerPawn_NavigationActions`; `map_dump tweaks` / `anim_notifies` (`Ply_Generic_as.Generic_JumpLandBig`, `Ply_Player_Land_at`); the captured original `movement/fall_stun` (read with the new `oracle cache show`).
- **Result:** The original's box checks record a touch (move along the axis below 1e-4 uu kDOP / 0.01 uu convex, start outside by less than that) and block at time 0 with the touch normal less 0.05 of the move; ported in `sat`. With it the Playground brushes collide as `Cube_100` simple boxes: hover 3.73 (original 91.23 on the cubes), `step_50` matches the original trace, `mantle_330` passes, 360 neither mantled nor climbed; `oracle collision` unchanged (Ext 99.95 %, Int 99.89 %). The hard landing (faster than 2200) plays `m_Action_JumpLandBig` (`Generic_JumpLandBig`, AnimStateExit 0.935 s at `Attribute_LandAnimRate`), not the Stunned state; the capture keeps running at 360 uu/s after landing, so no movement lock is ported.
- **Confidence:** CONFIRMED (decompiled code, measured trace).
- **Implementation consequence:** `oracle scenario --all`: 72 pass / 11 fail (`mantle_330` fixed, nothing else changed). Playground derived-data cache key bumped (`DERIVED_VERSION` 2).
## 2026-10-08: Melee findings ported (sword damage, NPC damage, offscreen, assassination, drop, multi-hit, guard health)

- **Question:** Port the verified melee findings (research/dishonored/systems/melee.md F1-F9).
- **Evidence:** Re-read 0x00C131C0 / 0x00C07C80 / 0x00C33DD0 (damage), 0x00C24020 / 0x00BBEF40 (offscreen flag:
  `m_bRenderedByPlayer` and the view plane), 0x00C0D8F0 / 0x00C09060 / 0x00C090D0 (assassinate target test),
  0x00BBA970 (move-set side), 0x00C09810 / 0x00C09900 / 0x00C09A50 / 0x00C14960 (drop). Tweaks with the Boyle script
  packages: elite sword MeleeDamage 10/15/20/30, JumpAttack and Bash multipliers 0, every `Pwn_EliteGuard_*` HealthMax
  50. `DisGlobalEnums.EAIAwareness` from DishonoredGame.upk. Assassination clip exits from `Ply_Sword_Assassination_as`.
- **Result:**
  - Damage = `appCeil(m_fAttackDamageMultiplier x (Attribute_MeleeDamage + Attribute_MeleeDamageBonus))` (the handler's
    rounding is appCeil); the health ratios only pick the contact effect. Player 10, guard 15 (Normal), 8 offscreen.
  - Assassinate: NPC within 275 (speed-scaled at 1.25), any side, awareness Unaware / Surprised / Suspicious /
    Begging, not running. Drop: no awareness gate (the drop context never calls 0x00C0D8F0), no fall-speed floor,
    height 0..350, fall-path sweep, every tick.
  - Scenarios (combat): before 12 pass / 4 fail; after 14 pass / 2 fail (crossbow_hit, pistol_pierce: not melee).
    sword_hit_dummy damage 14 -> 10 (pass), melee_guard_offscreen 14 -> 8 (pass), guard_attack 14 -> 15.
- **Confidence:** CONFIRMED for the damage rule, the tables and the drop geometry; LIKELY for the brain-state to
  `EAIAwareness` mapping and the `m_bRenderedByPlayer` stand-in (view frustum + trace).
- **Implementation consequence:** `dishonored_sim::combat` (melee_damage, app_ceil, seen_by, find_assassination,
  find_drop_assassination, multi-hit), `SwordState` (npc_move, hit_ids, offscreen_to, lock_time),
  `PlayerState::ai_awareness`, `Brain::awareness`, config fields (InventoryConfig sword_melee_damage /
  melee_damage_bonus / offscreen_melee_reduction; CombatAiConfig npc_sword_damage / guard_health_max;
  MeleeContextConfig damage_multiplier), gameplay cache `v13m`. U-113 is the PvP mapping only; the oracle cause
  rule is retired. Open: rebound, sword clash, chains, stagger / recoil timings, the context target selection.
## 2026-10-08: Powers findings ported (Blink lock-out and dilation layer, Windblast levels and wall rule, Dark Vision combat, Possession)

- **Question:** Port the findings of `dishonored/systems/powers.md` (F-01..F-11, F-13/F-14 if decodable).
- **Evidence (re-read with the static index):**
  - Blink CanUse 0x00BE71F0 = mana check 0x00BF3060 (whole points >= `m_ManaCost`) and not `m_bActive` (slot +0x160 = `+0xA0 & 1`); the start 0x00BFC020 returns at once when CanUse fails (no sound, no aim). 0x00BFAAA0 sets `m_bActive` and zeroes `m_fCooldownTimer` (+0x138); 0x00BED480 (modes 2/3) adds the step and, past the level block's `m_fCooldownTime` (+0x20), clamps it and calls 0x00BE9180 (clears `m_bActive`, calls 0x00BE7380 again).
  - Dilation channels: `DishonoredGameInfo.m_BendTimeInfo[11]` (`BendTimeChannelInfo`, +0x4CC, 0x40 each; `EBendTimeChannel`: 0 BendTimePower, 1 BlinkPower, 2 PossessPower...). The helper 0x009FA0D0 sets a channel's targets (world target 0 kept, else clamped to [0.05, caster target]) and velocities `(target - current) / blend`. The GameInfo tick 0x009FCBC0 (real time) moves each value linearly, landing on the target, then multiplies: player rate = product of the channels' player dilations (written to `WorldInfo.TimeDilation`), world rate = product of their world dilations (`m_bExclusive` channels override the others; none of the powers' channels is exclusive). `m_fBendTimePowerRealTimeSeconds` (+0x1348, Bend Time's clock, 0x009E9E80) advances by dt x the world dilation of the channels with `m_bAffectsBendTimePower` (Blink's has it, Bend Time's own has not).
  - Blink's calls: 0x00BFC990 (1, 0.01, 1.0, 1.0, `m_fBendTimeDownTime`), 0x00BE7380 (1, 1.0, 1.0, 1.0, `m_fBendTimeUpTime`) from the end/cancel 0x00BFAB80 and from 0x00BE9180 at the cooldown's end (the slope is recomputed from the current value there).
  - Windblast: 0x00BF6780 / 0x00BF7090 read `m_Levels[m_CurrentLevel - 1]`, range `m_Distance x (1 + SpellStrength)`. The wall rule is the NPC pawn's rigid-body collision 0x00B529C0 (vslot +0x290): other side world/static, last instigator the player and last damage type Windblast, the player's Windblast component at `m_CurrentLevel > 1`, contact normal with `n.z^2 < n.x^2 + n.y^2`, speed >= `m_fMinSpeedToExplodeNPCs` (500) -> TakeDamage(`m_HitWallAdditionalDamage` 100, momentum = velocity); if that kills it and `rand <= m_fChanceToExplodeNPCs` (0.3) the body explodes (0x00B52530, impulse `m_fExplodeImpulseStrength`).
  - Windblast and projectiles (F-07): the gather takes every colliding actor in range except one excluded class; a hit first calls the actor's vslot +0x37C: pawns return 0 (TakeDamage + impulse), `DisProjectile` (0x00C4F290) takes no damage and reacts by its tweak's `m_WindblastBehavior` (None / Normal: velocity redirected along the blast, 0x00C4A430 / PushTowardNPC / Homing: 0x00C4A650 homes on the aimed NPC within a 0.8 dot cone, else on the aim trace's hit point, at `m_fWindblastVelocityMultiplier` x speed over `m_fWindblastHomingTime`; class default Homing, 1.2, 1.6 s).
  - Dark Vision combat 0x00BF84C0: count = `DisGlobalCombatManager.m_TargetToSkirmishMap[player].m_Combatants.Num` (0x00C58CD0 / 0x00C5AF60); the first combat step sets `m_fCombatTimer` to `m_fDesactivateInCombatTimer`, later ones count down, zero zeroes `m_fTimeRemaining`; out of combat (or when 0x00B45310 reports its flag) the timer is reset.
  - Possession: 0x00BFA120 extends `m_fMaxDuration` while `clock - start < max - m_fPostEffectCooldownTime` (+0x18 of the Bend Time level block). Scoring 0x00BFB090 / 0x00BFCAF0: `m_fAngleWeight x (fov - angle) / fov + m_fDistanceWeight x (range - d) / range + m_TargetingPriorityWeights[possessable.m_TargetingPriority]`, highest wins; fov = `DishonoredPlayerCamera.m_fCurFOV` in radians (>= 0.1), range x (1 + SpellStrength) (>= 1); the cone 0x00BF8E20 is `m_fMaxAngleFOVFraction x m_fCurFOV`; priorities `EDisPossessTargetingPriority` Animal 0 / Civilian 1 / Wolfhound 2 / Combatant 3, weights {0, 1, 2, 2}; BaseNPC is Combatant, Rat Animal.
  - SetPowerLevel 0x00ACE270: after each new modifier the stored HealthMax (+0x348) is recomputed and, when it grew and the heal flag is clear, Health (+0x344) grows by the difference.
- **Result:** ported in `blink.rs`, `powers.rs`, `left_hand.rs`, `lib.rs`, `movement.rs`, `tuning.rs`, `state.rs`, `dishonored_ai` (`combat_opponents`), with the new tweak fields loaded through the gameplay config. Powers scenarios: 21 pass, 0 fail, 2 unmeasured (were 4 fail).
- **Confidence:** CONFIRMED (decoded) for the rules; the Blink layer's world rate and the Dark Vision combat definition remain unmeasured.
- **Implementation consequence:** protocol 10; the cached gameplay config needs a refresh for the new fields (serde defaults hold the tweak values meanwhile). F-07 (projectile reactions), F-12, F-13, F-14 stay open (see powers.md section 12).
## 2026-10-08: Devouring Swarm as individual rats (F-12), power costs at the cast notify

- **Question:** Rebuild the Devouring Swarm per the measurement (ORACLE.md "Capture requests answered": individual pooled crowd-agent rats, 30 / 60, 30 s, cost 40); check the measured power cost timings.
- **Result:** `SwarmState` is now the swarm actor and its spawner: after `m_fRatSwarmSpawnDelay` it releases its rats as rat pawns over 16 ticks (`state::SWARM_RELEASE_TICKS`), ends at `m_fDuration` and despawns them over 10 s. The rats use the existing rat-swarm brains and attack groups (per-swarm tweak `rats::swarm_tweak`); the aggregate group logic was removed. Bend Time and Devouring Swarm take their cost at the cast notify (Bend Time measured 9 ticks after the press = the 0.140 s notify). Tests: `gaps::a_swarm_releases_rat_pawns_and_they_despawn_after_its_duration`, `player::possession_takes_a_rat_of_a_swarm`.
- **Confidence:** MEASURED counts and times; the rats' placement at release and the even despawn tail are not decoded.
- **Implementation consequence:** `PlayerState.swarm`, `SwarmState.release_ticks / ending / rats_at_end` (protocol 10, same pass). The mission applies `AiEvent::RatEat { devoured: true }` (no body left). Open: Possession's cost timing (12 ticks after the release), the Windblast ragdoll speed (1005 vs the port's 2367 uu/s).
## 2026-10-08: Power screen effects from the captured shaders; launch-tick trails

- **Question:** What do Bend Time, Blink, Dark Vision and Possession draw on screen, from the shaders the hook captured in the running game; and how should a bolt that hits within its launch tick show its trail?
- **Method:** `fxc /dumpbin` of the captured blobs (`oracle/shaders/70da96138fe2171c`), their recorded constants and the post manager channels (`effects/*.json`); `obj_dump` / `export_hex` of `AltScreen_Effects.PostProcessMaterial.*` (parameters, `UniformExpressionTextures` from the native data) and the chain `Test_PPG`.
- **Findings:** EFFECTS.md "Power screen effects": the Bend Time field and its two blur passes (fully decoded with constants), Blink's use of the Bend Time blur, the Dark Vision eyelid pass; Blink's lens field disassembled without constants; no Possession screen pass captured. A trail whose projectile dies at once: started and deactivated in the same tick (the trail component is deactivated with the projectile).
- **Port:** `power_pp.rs` (intensity curves, textures), `ark_post.rs` / `.wgsl` (field, blur, eyelid pass, new bindings), `SessionView.time_bends`; `SimEvent::Fired` now carries kind, origin and velocity; `FxRequest::EndedTrail`, `StopAtOnce`; oracle presenter mirrors it; `Flare_Bolt_Trail` for incendiary bolts.
- **Confidence:** CONFIRMED for the shader terms and the Bend Time constants; LIKELY for the eyelid Alpha curve; curve-fit (from the capture) for Blink's blur intensity.
## 2026-10-08: AI brain rebuilt on the slot model (systems/ai.md ï¿½1.4a, ï¿½11)
- **Question:** How does the original turn attention into behaviours, and does the port's React / select state machine match?
- **Evidence:** The attention-behaviour stimuli (ids 63 / 87 / 17) are built only by 0x00b24d00, called by 0x00b25890, which steps the attention component's behaviour level one level at a time toward the top target's (begins up, ends down: 0x00b22ec0), only for an enemy top target (+0x45 == 2); their delay getter returns 0 (0x008fc830), while `m_ReactionDelay` is the delay of SearchRequest / Help (0x00b01650) and `m_MagicReactionDelay` of WitnessMagic (0x00b016e0). Investigate's PathingFail handler 0x00b33850, Search's decision 0x00afeb00 (StareAtUnreachable / Stand / GenericAction with `m_fSearchAnimCooldown` / TrackTarget), ManageAttacks aggression (0x00b3dc10, 0x00b39e50, 0x00b3e070, 0x00b3deb0), WitnessMagic (0x00b23e50 and the power callers of 0x00bf9fb0), MagicResponse (0x00af2800).
- **Experiment:** Ported into `dishonored_ai::slots` (slot stack, stimulus queue, attention poster) and the behaviours; compared with the original brain channels (NpcBehavior per slot, player attention) of `ai/boyle_hear_shot`, `ai/boyle_doorman_duel` and the measured patrol speed.
- **Result:** The original shows the slot stack directly (Soiree in slot 2 over a dormant Patrol; React in slot 7 then Patrol again). Patrol speed 116.3 = EmptyPatrol x Empty_Walk_Patrol; the port now walks 116.29. Hearing raises attention on the noise's instigator (neutral player attention 1.0 then the 2.0 / s neutral leak). Port scenarios: ai 6 pass, 0 fail, 2 unmeasured.
- **Confidence:** CONFIRMED (decompiled + captured), LIKELY for the loop-speed rule outside the patrol.
- **Implementation consequence:** `Brain::state` is a projection of the current behaviour; external writes become forced behaviours; `PlayerInput::npc_speed` carries NPC locomotion speeds into the sim; gameplay config key `v12x-ai1`.
## 2026-10-08: Stealth port: AI noise table, footfalls, landings, visibility states, stealth volumes
- **Question:** Port stealth.md F1-F8: which player and world contacts make AI noise, how a footfall becomes attention, how the visibility component picks its bonus, how stealth volumes change the light range.
- **Evidence:** 0x00BD6C20 (`DisContactSub_AINoise` fire), 0x00B40B70 (attention process noise handler: Footfall -> anomaly increase, stimulus raised to the walker), 0x00AAB7D0 (footfall striker, `Attribute_SilentRunning` / `Attribute_SilentSprinting`), 0x00A8D5F0 / 0x00B66ED0 / 0x00B66A80 (footfall notifies -> contact), 0x00AB53A0, 0x00A759F0 and 0x00A5FF10 (landing striker, `m_bIsStealthFall`, `m_fFall_Stun_Vel`, no landing for a non-jump fall of at most `m_fAnimateFallTime` 0.25 s), 0x00C2A3E0 (pistol noise at the shooter), 0x00B863C0 (Kismet noise context), 0x00AD1590 (visibility bonus order), 0x00BD0120 / 0x00C8C280 (stealth volumes); `enum_dump` of `EAINoiseContext`; the contact table read by the new loader.
- **Result:** run 600 Footfall, sprint 1200 Anomaly, sneak / silent / stealth landing Silent, landing 400, stun landing 800, sword hit / clash 800 Threatening, bullet impact 3000 Danger, pistol 6000 Danger, corpse / sleeping body 300 Anomaly (the 200 min speed is inert: the sub is not speed-scaled). Footfall times from `Ply_Sword_Locomotion_as`. Prison: 3 stealth volumes (priorities 1/0/2, ranges none / 0.2-0.6 / 0.3-0.8).
- **Confidence:** CONFIRMED for the table, the context mapping, the visibility order and the stealth-volume rule; LIKELY for `JumpLand_Big` as the stun landing (class identity by meaning) and the slide state class; the locomotion loop's play rate (RateScale 1) and the body contacts (one per fall, no ragdoll) are port models.
- **Implementation consequence:** `AiConfig::noise` (`NoiseTable`), `SimEvent::Sound.context`, `dishonored_sim::noise`, `dishonored_ai::{noise_increase, player_visibility, vis_state, light01}`, `AiLevel::stealth_volumes`. Protocol 10; config cache `gameplay_config.v12x-stealth.json`, `<map>.ailevel3u-stealth.json`. The original's detect_600 trace (outer cone first, inner after the head turns) confirms F4/F5; not ported (head pose).

## 2026-10-08: Physics port (thrown NPCs, Windblast cast delay, matrix_origin)

- **Question:** Port research/dishonored/systems/physics.md P-1..P-10 (the sim models what gameplay depends on; limb motion stays presentation).
- **Evidence (static index, re-read):** 0x00BEB6E0 (velocity = momentum / `m_fEffectiveMassForThrowing` at tweak+0x118 -> +0xA4 = config +0x18, 2 without the tweak); 0x00A86B90 -> 0x00BE8710 (min momentum, `m_bComingFromStun` selects the stunned pair) -> 0x00A81860 (`PHYS_Flying`, `AirSpeed = |v|`, Acceleration zero); tick 0x00A98DD0 (`Velocity.z += GravityZ dt` while `m_bRagdoll` is clear); CalcVelocity 0x006ACD50; the enter 0x00A9FAA0 registers 0x00A7CBA0 on pawn event 5 (sets `m_bRagdoll | m_bImpulseNextFrame`), fired by the NPC ragdoll start 0x00B6FBB0, called by `DishonoredNotify_Ragdoll` 0x00A8CFE0; `DisNotify_WindBlast` 0x00A96AE0 -> 0x00C44E10 -> blast start 0x00BF9570, fire mode 0x00BF9720 waits for it; damage path 0x00B5CCD0 -> 0x00B57E60 -> 0x00B57F00 (reaction from the damage type / pawn tweak, not the momentum). Asset scan: `Generic_WindBlast_Front / Left / Right / Back` ragdoll notifies 0.455 / 0.390 / 0.448 / 0.418 s, `Powers_Cast_Windblast` `DisNotify_WindBlast` 0.27 s.
- **Experiment:** The cached original trace of `powers/windblast_guard_l1`: the flight's horizontal step ratio 0.995008 per tick (25-tick ratio 0.88239 = 0.9975^50: friction 0.15 applied twice), no gravity in the first step, physics 10 at the 28th update after the hit.
- **Result:** Ported P-1, P-2, P-3, P-8, P-9 (`movement::{start_throw, flying, clamp_throw_momentum, throw_direction}`, `PlayerState.thrown`, `GameplayConfig.throw`, `CombatTuning::windblast_start_delay`, `npc_anim::{matrix_origin, sequence_notify_time}`). physics + powers scenarios: 26 pass / 0 fail (were 25 / 1); all scenarios 81 pass / 9 fail (were 79 / 11). P-4 (threshold consumer not found), P-5, P-6 (trace contradicts the pelvis-offset model), P-7, P-10 left open (physics.md section 6a).
- **Confidence:** CONFIRMED for the throw velocity, the clamp, the flight physics and its end, the cast delay; LIKELY for the Left / Right index of the throw direction.
- **Implementation consequence:** protocol 15 (`PlayerState.thrown`); gameplay config key `gameplay_config.v15x-physics.json` (new `throw` block, windblast `cast.notify_time`); the tracked `fixtures/config/gameplay_config.json` was not regenerated (serde defaults hold the loaded values). The sim's NPC still gets up on landing (no ragdoll rest time, 3.7).
## 2026-10-08: Kismet port (systems/kismet.md F-1..F-6, F-8, F-11, F-14)
- **Question:** Port the verified Kismet findings: stack positions, latent re-queue, Bend Time scaling and the two event clocks, link-level variable population, the attention events, `SeqAct_SetPhysics`; make the port's Kismet trace comparable with the original's ProcessEvent trace.
- **Evidence (re-read):** `ExecuteActiveOps` 0x00700740 (disassembly: `push edi` with edi 0 at 0x00700948 for a due delayed op, `push 0` at 0x00700f0d for a queued activation, the kept latent list emptied from its end with a `bActive` test and `push 1` at 0x007014db..0x007014f1, AddUnique 0x00514c60; raw dt for op flag 0x400 else `dt * vslot244` at 0x00700a6c..0x00700a75); `DishonoredGameInfo` vslot 244 0x009e9e90 (`+0x3a8 / +0x3ac`); clock chooser 0x006c8350 and `ActivateEvent` 0x006f5dd0 (BendTimeSeconds +0x398 unless flagged); Populate 0x006ef6c0, `SeqVar_Int` 0x006f6da0 (sum), `SeqVar_Float` 0x006f69b0 (float sum), `SeqVar_Bool` 0x006f6740 (AND); attention fire 0x00b363d0, its caller 0x00b3b350 (one call per target batch: the first record's old level, the last record's new level), `DisSeqEvent_Attention::CheckActivate` 0x00b8a2a0.
- **Result:** `dishonored_kismet::runtime`: due delayed ops and repeated activations queue at the bottom; kept latent ops re-queue in reverse when still active; `tick_dilated(dt, ratio)` with per-op raw / scaled dt; `time` (TimeSeconds) and `bend_time` (BendTimeSeconds) clocks; scalar sum / AND population; an activation log (`record`, `activations`, `trace_name`). `kismet_host`: dt x player dilation and world / player ratio from the powers' Bend Time / Blink channels (`powers::player_time_dilation`, `world_time_scale`); `DisSeqEvent_AttentionIncreasedTo / DecreasedTo` from the brains' per-target level changes; `SeqAct_SetPhysics` records the mode. Oracle runner: `Kismet` events are now per-op `Activated` / `Deactivated` named like the hook's `short_path`; notes are `KismetNote`. `kismet/boyle_start`: 301 Kismet events in 5 s, plus 39 notes (the 39 "activations" were notes).
- **Confidence:** CONFIRMED (disassembly) for F-1, F-2, the F-3 formula and F-4/F-5; LIKELY that the dt given to `UpdateOp` is TimeDilation-scaled (U-109 measured TimeSeconds/BendTimeSeconds rates agree); the attention batch = one port tick is LIKELY.
- **Implementation consequence:** `Kismet.bend_time`, `HostState.attn_levels / physics` (serde defaults: saves grow). Still open: F-7 Distracted, F-9 BreakableBroken, F-10 RefillWhaleOilBattery (no such actors in the sim), F-11 rigid-body motion, F-13 Kismet vs actor order; the dup trace name `<Event>_AttachDup` awaits a capture.

## 2026-10-08: Thrown NPC rest and get-up (physics.md 3.7)

- **Question:** When does a thrown NPC get up, and how long until it acts again (the port's guard got up on landing and hit the player in `powers/windblast_guard_l1`)?
- **Evidence:** throw state tick 0x00A98DD0 (rest test after `m_fBodyAtRestTestTimer` 0.5 s from the enter 0x00A9FAA0, dilation `Actor.CustomTimeDilation` +0x100 now and before, `(Velocity.x, Velocity.y, 0).IsNearlyZero(m_fMaxRagdollSpeedToRecover x dilation)` 0x004019A0, or a sleeping body); at rest `SetPhysics(PHYS_Walking)` and the transition 0x00A814B0 to `StateNPCMasterStunned` with `eStunEnterAnim_RagdollFront`; the stunned state 0x00A78C20 / 0x00A61E20 / 0x00A65C50; `DisTweaks_NPCPawn_Body.m_fMaxRagdollSpeedToRecover` 5 (all Boyle NPC tweaks); `DisNPCAnimDefinitions_EliteGuard` Stun group 1 = `Generic_RagdollToFront` 2.333 s, `Sword_Stun_Loop` (one tick with `m_fStunDuration` 0), `Sword_RecoveryA` 1.867 / `Sword_RecoveryB` 1.5 s; chain 7 group 1 orders the Windblast throws Front, Left, Right, Back.
- **Result:** `movement::step_thrown` (rest test, then the get-up hold), `ThrownState.ragdoll / rest_timer / prev_scale`, `ThrowConfig.max_ragdoll_speed_to_recover / getup_anim / getup_recover` (loaded). `powers/windblast_guard_l1` vitals agree again (2 / 2); all scenarios 81 pass / 9 fail, vitals channels 35 agree (were 34).
- **Confidence:** CONFIRMED for the rule and the data; the port's ragdoll stand-in comes to rest at tick 70 against the original's 117 (PhysX dynamics, P-5).
- **Implementation consequence:** protocol 16 (ThrownState fields); gameplay config key `v16x-physics` on the branch (the coordinator renames).
## 2026-10-08: Conversation port: the bark manager, hook outputs, fallback trees, subtitles (systems/conversation.md section 8)
- **Question:** Port conversation.md F1-F12 and F18: rebuild the bark pipeline as `FireHook` + `DisConvGlobalMan`'s queue.
- **Evidence (static index):** `FUN_00ca6bd0` (gates: supported cache, dead owner except 57-59, trial walk `FUN_00ca4690`, refire `min < now - next + 1e-8`, global), `FUN_00c9cc80` (enqueue: list = group x Quiet at audio distance >= `m_fDialogDistanceConsideredLoud`, lock, per-initiator cap), `FUN_00ca5b20` (tick: firing entries re-classified, slots `m_MaxDialogHooksAtOnce(_Quiet)`, groups 12 -> 0, room per attempt, global slot per success, the rest dropped; housekeeping locks on each end), `FUN_00ca2580` (weight `1 - clamp(d, 0, 5000) / 5000 + rand * 0.2 / 32767`), `FUN_00ca5670` / `FUN_00ca5150` / `FUN_00ca49e0` (fire walk, fallback chain, `TryFallbackTree` 0x00ca7b20), `FUN_00c9d1f0` / `FUN_00c9aec0` (availability), +0x158 of `Hook_SuspicionLevel` / `DeathMode` / `Notice` / `Investigate` / `KismetActivated` / `PlayerLoiter` / `PlayerLookAt`, `FUN_00c8d820` row 110 (0xa0), `DisAISubProcessAmbientBarks` 0x00b47020 / 0x00b3d5e0, `FUN_00b733f0` (pain 60); `obj_dump Default__DisTweaks_ConvSettings` (Loud / Quiet caps, distances, ducking).
- **Result:** `dishonored_conversation` (`hooks.rs` `ConvSettings` / `HookQueue` / `LoiterHook` / `LookAtHook`, `runner.rs` `request_hook` / `tick_hooks` / `fire_now`, `HookClass`, `Tree.fallback`, loader of the tweak); mission requests + manager tick with host-set speaker positions, dead speakers and AI facts; AI ambient patrol barks and both pain hooks; oracle `Bark` / `Conv` / `Sound` events; runtime subtitles by voice radius and dominant-hook ducking (not built here). Corrections to the audit: the random divisor is 32767; step 1 re-classifies firing entries; `FUN_00a77d10` fires 62 (section 8).
- **Tests:** `cargo test --release -p dishonored_conversation -p dishonored_ai -p dishonored_mission` (32 + 1 conversation, 44 ai, 5 mission); oracle `--system conversation --system ai`: ai 6 pass / 0 fail / 2 unmeasured (unchanged), conversation 2 unmeasured (no original trace).
- **Confidence:** CONFIRMED for the gates, queue, weights, caps, lock, output classes, fallback, loiter / look-at; LIKELY for the bind-time availability of hook walks (F7) and the Notice context threshold; APPROXIMATION for the audio path distance (straight line) and the duck blend curve.
- **Implementation consequence:** dialog cache key v3 -> v4; protocol (MissionView.dialogue_mix, DialogueLineView.voice_radius / instance / hook) and save (Runner.queue / loiter / look_at, HookClock without last_group, MissionState and Brain fields) need the coordinator's bumps; `AiConfig::ambient_bark_inhibition` (serde default 12 = the tweak value, gameplay config cache key unchanged).
## 2026-10-08: World actors port: distractors, breakables, whale oil batteries (world.md F-12 / F-13 / F-14, kismet.md F-7 / F-9 / F-10, physics.md P-10)

- **Question:** Port the three largest unhandled placed-actor systems: `DisNPCDistractor` / `DisSeqEvent_Distracted`, `DishonoredBreakable` / `DisSeqEvent_BreakableBroken`, `DisWhaleOilBattery` / `DisWhaleOilReceptacle` and their Kismet ops.
- **Evidence (static index, re-read):** distraction: component tick 0x00b84e90 / selection 0x00b842f0 / roll 0x00b83bb0 (chance table 0x00b6eca0 by the locomotion component's modifier index +0x1a0, set by 0x00941b70; suspicion `DishonoredAIBrain.m_SuspicionLevel` +0x41), tweak filter 0x00b74b00 / 0x00b74a60, route filter 0x00b74b60, LOS 0x00b81010 -> 0x00c64480 / 0x00bf1580, manager 0x00b52170 (game info tick 0x00a05ab0) / 0x00b52050 / 0x00b6ec60 / 0x00b80f30, sub-process `DisAISubProcessDistractions` slots 71-80 (0x00b2b0f0, 0x00b2b010, 0x00b2b040, 0x00b2b080, 0x00b2b150, 0x00b28550; owned only by `TweaksBehavior_Patrol_*`), `DisBehaviorInteract` (slot `EAISlot_Distraction`, evaluate 0x00ae3ca0, start 0x00ae9480), follow-up interface 011445c4 (0x00b80e80 link, 0x00b84d60 arrival -> 0x00b84670 / 0x00b84750), ends 0x00b844f0 / 0x00b84810, `FireDistracted` 0x00b79b80, soiree loop counts 0x00b79cf0, `StateNPCMasterDistracted` 0x00a70920 / 0x00a652a0 / 0x00a618f0, pawn countdown in the NPC pawn tick 0x00b6da40, `OnAIStartDistraction` 0x00b63ed0, `OnToggle` 0x00a4c350. Breakables: damage 0x00a26a90 (immune +0xf0, instant +0xfc, disassembly 0x00a26ad6 / 0x00a26b6f), tick 0x00a3b0d0, step 0x00a3a5f0 (event index, the step's noise 0x00b14e80 only with a sound). Batteries: 0x00c81d70, 0x00c85170, 0x00c731d0, 0x00c7b490 / 0x00c65820, 0x00c7b6b0, 0x00c7d190, 0x00c822b0, 0x00c850c0, 0x00c82190, 0x00c7d7c0, 0x00c7d660, 0x00c7d340 (not ported). Data: `obj_dump` of L_BoyleStreet_Ext_Script (7 soiree distractors; the event's `SeqAct_Interp` plays the same `DistractionSoiree.*` matinee), Boyle Ext / Int breakables and batteries.
- **Result:** `dishonored_ai::distraction` (+ `Behavior::Distraction`, `AiEvent::Distracted`, `Brain.lineage / distraction / distraction_countdown / look_distraction`), `dishonored_sim::world_actors` (breakables, batteries, receptacles), `dishonored_ir::world_actors`, `dishonored_assets::world_actors` and `ai_level::read_distractor`, Kismet host events and ops. Scenarios `kismet/boyle_distractor_railing` (Start 4.07 s, End 8.93 s later) and `kismet/boyle_int_bottles_grenade` (8 BreakableBroken output 0): 3 static checks pass, 4 unmeasured; all scenarios 85 pass / 8 fail / 42 unmeasured, the 8 fails identical with the main oracle.
- **Confidence:** CONFIRMED for the rules listed as ported; LIKELY: tweak slot 2 of the pawn's tweak interface is the lineage test, the soiree ends with its matinee, the pawn countdown runs in every NPC tick. Not ported (stated in world.md 10.1): the explosion's line check to actors, non-explosion damage, loot, the battery's damage / shield / chain, the devices that draw charges, carrying batteries, the animated distraction's In / Out lengths, `m_bLowerAIAlertness`.
- **Implementation consequence:** protocol 18 (`SoundCategory::Breakable`); gameplay config key `gameplay_config.v17-world2.json` (`ExplosionConfig.radial_damage_type`; the tracked `fixtures/config/gameplay_config.json` not regenerated, the field defaults to empty); map caches `<map>.ailevel4-distract2.json` and `<map>.worldactors1.json`; saves grow (`MissionMatch.world_actors`, `Crowd.distractors`, Brain fields; serde defaults).

## 2026-10-08: Fresh scenario NPCs from spawners and the stealth light environment in the original (U-807, U-121)

- **Question:** Can scenario NPCs stay where placed (U-807), and how does the original turn its stealth light environment into the raw light value (U-121)?
- **Method:** hook session on the original (probe_launch `session spawn_npc`, `lightenv`, `lights`), Ghidra on the spawner (`ClassDecomp DishonoredSpawner`: 0x00A5E790 spawn with reason codes, 0x00A4B300, 0x00A5C150), the StartSpawn op (0x00B9BCC0, 0x00B90050) and the light value (0x004E1DB0, 0x004E1F40, caller 0x00A57FF0, normalization 0x00BC9040).
- **Result:** scenario NPCs spawn fresh from a copied host spawner (squad, pawn list and queued requests cleared; blocked spawns re-requested higher); the 58 NPC specs re-captured (56 ok). The raw light is `(L / 4 pi)^(1/2.2)`, L = sum of light intensities `Brightness x lum((byte/255)^2.2) x falloff` (band 0 luminance of the state's +0x450 / +0x4E0 x 2 sqrt(pi)), exact to 4 decimals at 10 points; per-light contributions, sky and directional terms and the light-volume share recorded (ORACLE.md "Stealth light environment").
- **Confidence:** CONFIRMED (measured) for the light rule and the contributions; the decompiled max-channel path of 0x004E1DB0 called with mode 1 is not fully read (the measured rule fits the luminance at every point).
- **Implementation consequence:** the port's light probe needs the 1/(4 pi) before the 1/2.2 gamma; the class default StartSpawn counters must stay zero (assertion on the next map load otherwise).

## 2026-10-08: Special NPC classes (systems/ai.md F6, section 12)
- **Question:** How do music-box Overseers, Tallboys, wolfhounds and weepers fight (the port played Overseers as elite guards and tallboys / weepers as panicking civilians)?
- **Evidence:** brain tweaks `Twk_Brain_OverSeerMusical_Default`, `BrainTweaks_TallBoy`, `BrainTweaks_WolfHound` (L_Ovrsr scripts), `BrainTweaks_Weeper`; DisBehaviorMusicalCombat vtable 011322e0 (00b13a00 / 00b13a20 / 00b13a50, 00b138a0, 00b13990, 00b13920), MaintainDistance 00b2e600, tune context 00c2b680, tune component 00c6a240 / 00c6bfc0 / 00c66ef0 / 00c75270, power gate 00bd7390 / 00b49020, Musical reduction 00b5ccd0; DisBehaviorTallBoyCombat 01132510 (00b144a0, 00b14630), FindShootingPosition 00b475e0 / 00b46960, ManageAttacks 00b470d0, bow CanDo 00c25240, launch 00c2fab0, accuracy 00b732e0, arrow 00c550d0; DisBehaviorCombatWolfhound sub-states 00b13c70 / 00b13e40 / 00b14010 / 00b13a80; DisBehaviorWeep 011272a0 (00afe030, 00affe30, 00af6bd0, 00afbe30, 00aff9e0), plague 00c70e10 / 00c86ea0; the class tweaks' contexts, attributes, locomotion configs and DefaultAI.ini inhibitions.
- **Result:** `NpcClass` from the brain's Combat-slot behaviour (`AiLevel::tweak_combat`), fighters spawn as guards with their class health and plague flag; `dishonored_ai::special` (the four combat behaviours), `dishonored_sim::special_npcs` (combat tune damage / repulse / power inhibition, plague cloud, Tallboy arrow), `ProjectileKind::TallboyArrow` exploding on impact, unblockable special attacks; `AiConfig::special` loaded from the tweaks. Scenarios `ai/boyle_tallboy_bow`, `ai/boyle_weeper_plague`, `ai/boyle_overseer_tune`, `ai/hound_bite` pass; ai 10 pass / 2 unmeasured.
- **Confidence:** CONFIRMED for the class selection, the decision rules and the numbers listed in ai.md section 12; LIKELY for the repulse force source, the tune context of sub-state 3, the cough start, the hound's long-distance hold.
- **Implementation consequence:** protocol bump (PlayerState.plague, ProjectileKind::TallboyArrow, DamageCause Musical / Plague / TallboyExplosion, Brain.class / special); gameplay config cache key `v17-ai2`, AI level cache key `ailevel3u-ai2`; fixtures/config/gameplay_config.json not regenerated (special config defaults in the tests).
## 2026-10-08: Stealth light: update rule and the Playground lane
- **Question:** Is the player's light (`m_fCurrentLightValue`) only recomputed on a stance change, as the hook read it?
- **Evidence:** 0x00AD51E0 (writes +0x68 every tick from `m_pStealthLightingDLE`, bytes at 0x00AD51F2), 0x005041A0 / 0x005035D0 / 0x005034D0 (`bRequiresNonLatentUpdates`: full update every tick), 0x00B50720 (OverriddenBounds = the cylinder's Bounds every tick); `Default__DishonoredPawn.StealthLightingDLE` and the player's override.
- **Result:** No stance gate in the code; the value follows the collision cylinder's bounds each tick, so a placement that does not update the components keeps the old value until the cylinder is resized (sneak). Port unchanged (samples every tick). New port channel `pawn.light` / `pawn.light01` and `stealth/light_probe_lane`: port raw 2.6-3.1 x the original; the original Playground normalizes with Boyle's 0.2 / 0.5.
- **Confidence:** CONFIRMED (code path); the hook artifact explanation is LIKELY until a walking capture.
- **Implementation consequence:** re-capture the lane walking by input; the light probe needs the `m_LightProbe` channel and the light volume (U-121).

## 2026-10-08: U-121: the stealth light environment ported
- **Question:** Rebuild the player's light value from the original's stealth light environment.
- **Evidence:** 0x004E1DB0 / 0x004DA330 (band-0 luminance x 2 sqrt(pi), bytes at 0x00F67460), 0x004DB950 (channel test), 0x00516400 (level filter), 0x004FF270 (4-sample visibility, LCG sample points of 0x004FEB30, 1-uu checks for w = 0 lights), 0x00503140 (dynamic lights through the same composite), Engine.upk light templates (`m_LightProbe`), `Default__DishonoredPawn.StealthLightingDLE` / the player's override.
- **Result:** `stealth_light_value`, `Crowd::update_player_light`, `AiLevel::stealth_lights` / `light_volumes` (Boyle Ext: the Env level's 16425 surface samples, ~0.4 MB in the ailevel cache), `WorldQuery::hit_package` with `MeshInstance::level`. Port raw light is about 3 x the original on the Playground lane and 3.5 x the (possibly stale) Boyle reading; cause not found.
- **Confidence:** CONFIRMED for the structure (code read), LIKELY for the Playground's light set; the magnitude is not validated.
- **Implementation consequence:** cache keys `<map>.collision16-stealth.bin`, `<map>.ailevel3u-stealth3.json`, playground DERIVED_VERSION 3; a capture of the stealth DLE's SH state is requested.

## 2026-10-08: The vision head ported (baked camera_jnt, look-at cursor)
- **Question:** Rebuild the NPC vision eye and axis (F4 / F5) from the head bone and the look-at controller, asset-free in the sim.
- **Evidence:** 0x00C676A0 (eye update, full read: `GetBoneMatrixLocal` 0x00718630, angle offset and LocalToWorld 0x00B57110, request / no-request / disabled branches, null zone at 0x00C680E2), `DisTweaks_Pawn_Body.m_fBone_AngleOffset_Pitch` -90 (`Twk_Pawn_DefaultNPC`), 0x00B25890 (HeadTrack request), 0x00CAA060 / 0x00954B30 (request builder, speed 0.5), 0x00955D60 (constructor constants), 0x00953FB0 / 0x00953050 (target, weights, delay, clamp, reach), 0x0094E240 / 0x0094DFF0 (`DynInterp` updates), 0x009176F0 / 0x0090C4C0 / 0x0090A140 (node split and reach), 0x00A8E320 (`DisNotify_NPCDisableLookAt`), DefaultLookatConfig modes.
- **Result:** `dishonored_assets::head_pose` bakes `camera_jnt` (position, vision forward = bone -Z) over every stance's idle / walk loop into `AiConfig::head_poses`; `dishonored_ai::head` ports the cursor, weights and node blend; vision uses the head eye. detect_600: first LOS 0.171 (original 0.353, outer cone), after the turn 0.170 (original 0.169); detect_1300 notices at 0.88 s (original 2.82 s). With the look-at enabled and no request the original's axis is the pawn's rotation, so the outer-cone first sighting is not explained by the head model; ambient idles disable the look-at (bone axis) and the vision-target point (F6) is undecoded.
- **Confidence:** CONFIRMED for the decoded rules (code read); the bake's mesh placement (no mesh offset component) and the stance choice are LIKELY.
- **Implementation consequence:** cache key `gameplay_config.v18-head.json`; the archetype AnimSet lists moved to `dishonored_assets::characters`; the `camera_jnt` capture in detect_600 (queued) decides between the ambient-idle and F6 explanations.
## 2026-10-08: First-person view and arms placement (camera bone, look controls, arm pitch offsets)
- **Question:** The Playground shots of `viewmodel_states.json` showed:
  - no arms in the sword swing, the Blink aim and the mantles;
  - a large hand near the face in Windblast;
  - only the blade tip in the sword idle.

  How does the original place the first-person camera relative to the arms mesh?
- **Evidence (static, Ghidra + cooked data):**
  - `DishonoredCamera_PlayerControl` slot 80 (0x00ACBA40) builds the POV:
    - location = pawn vfunc +0x630 (0x00AABA60): the `camera_jnt` world position after the skeletal controls;
    - rotation = vfunc +0x3E8 (0x00AABB70): controller rotation + `m_RollAmount` (0.7) x the bone's rotation relative to the pawn (pitch minus the look control's pitch);
    - the bone frame is first turned by `DisTweaks_Pawn_Body.m_fBone_AngleOffset_Pitch` -90 (`m_AngleOffsetMatrix`, 0x00B57110).
  - `DishonoredCamera_AnimDriven` has default weight 0 (`DefaultCamera.ini`).
  - The pawn's view-target update 0x00AABDD0 drives `Ply_Player_at`'s three look controls and the arms mesh:
    - the collarbones get the look pitch + the item's arm pitch offset;
    - `spine_3_jnt` gets the look pitch;
    - the arms mesh gets `m_Offset` = the additive camera offset;
    - arms FOV `m_FOV` = lerp(POV FOV, `m_fCurFOV_Arms`, `m_MeshSpecificFOVWeight`). The proxy re-projects with it (0x00ADEC40) only when it differs. FOV targets lock the arms only with `m_bLockArms`.
  - Arm pitch offset (0x00ACB8D0): `m_ArmPitch_OffsetKeys` with a cosine ease, times 65536 / 360 (FMUL at 0x00ACBC0C).
- **fp_probe of the decoded clips:**
  - The sword idle's hand is 46 uu below the eye.
  - `Sword_Ready_AttackRight_A_Small` winds up off-screen to the right and crosses the centre at about 0.27 s.
  - `Sword_*_Mantle*` move only the root and the camera.
  - Windblast draws the hand back to about 18 uu from the eye at 0.2 s.
  - So most of the "missing" arms are the data at the captured instants, not a placement bug.
- **Harness finding:** a `screenshot` step is captured after the fixed tick that already ran the next input step's first tick. The Blink aim shot therefore released the button, the Blink started, and the port's Blinking rule hid the rig.
- **Port:**
  - `viewmodel.rs` adds `apply_look_controls`, `arm_pitch_offset` (with the keys) and `view_rotation` (`ROLL_AMOUNT`).
  - The game camera moves by the camera bone's motion from the additives and the look pivot, and turns to the view rotation.
  - The rig undoes the lean roll, and stays drawn during a Blink's travel (U-781).
  - `fixtures/scripts/viewmodel_states.json` waits one tick after every screenshot. It adds time series of the swing, Windblast, Blink aim and cast, and the mantles, plus look down / up and lean.
  - Tests in `crates/dishonored_runtime/tests/viewmodel.rs`.
- **Confidence:** CONFIRMED (decoded) for the eye, the view rotation, the look controls and the offset keys. LIKELY for a pawn pitch of 0 (UE3 `Pawn.FaceRotation` while walking or falling, U-782) and for the arms staying visible through a Blink (U-781).
## 2026-10-08: Map blockers (MAP_SWEEP.md) and free roam between a level's maps
- **Question:** Fix the map sweep's blockers no other porter owns: silent unsupported ops, movers without collision, spline actors, the Flooded_FIntro entry stall, the next Kismet ops by instance count, the 10 redirected entries; then (coordinator) free roam between a campaign level's interior / exterior maps.
- **Evidence (static index):** `DisSeqAct_BackupAndClearInventory` 0x00ba91d0 / 0x00baf050 / 0x00b4e070 (outputs "Out" and "Finished (After Unequip)", hand slots 1-2); `SeqAct_SetMatInstScalarParam` 0x006d1d90; `PlayerController.OnToggleCinematicMode` / `SetCinematicMode` (script), `DishonoredPlayerController.SetCinematicMode_Native` 0x009eea00 -> 0x00aaf150 (action mask 0x7faf / 0x6010); `OnSetMaterial` (script, three classes); `OnAIGuard` 0x009f4e30 -> 0x00b63a80; `DisSeqCond_IsSentinel` 0x00b8f070 (`GameInfo.MyAutoTestManager`); `DisSeqCond_CompareBoolExtended` 0x00b9b3e0. Level data: `DisRiverKrust` / `DisWatchTower` name their skeletal component only via `CollisionComponent`; the mesh-less usables; the loft actors' cooked `SplineMeshComponent`s; the global `ChangeLvl_*` handlers' branches (kismet bwd / fwd).
- **Result:** 41 / 41 maps reach their own entry (FIntro commits; Pub x5 had the wrong chapter's entry event; Isl_HighChaos by `adddarkness 168`, LightH_LowChaos / Out x3 by their handlers' flags). Unsupported event / op classes are listed once per run (`KismetNote`), per map in MAP_SWEEP. Cold loads: 0 collision warnings (were 79). Spline loft actors: all cooked components imported, none collides (as the Boyle captures). Ops ported: SetMatInstScalarParam, ToggleCinematicMode, SetMaterial, AIGuard (no awareness radius), IsSentinel, CompareBoolExtended, BackupAndClearInventory (outputs only). Free roam: `dishonored_content::{freeroam_def, level_group, entry_event, map_exits}`, `dishonored_assets::travel::{map_exits, global_entries}`; `harness freeroam` 58 / 58 (19 pairs both ways).
- **Confidence:** CONFIRMED for the decoded op rules and the collision sources; LIKELY that the loft actors do not collide (no disagreement near a cable on the Boyle captures, not a direct test); APPROXIMATION: cinematic mode refuses every button (mask bits not decoded), AIGuard without its awareness radius.
- **Implementation consequence:** collision cache key `collision17` (shared-cache files of other branches using 16 stay valid); new cache files `<map>.exits2.json`; `HostState` gains `cinematic`, `mat_scalars`, `pawn_mat_scalars`, `materials` (serde defaults; saves grow); `Kismet::notices` (not saved); `Host::fires_event` (default true). Not wired yet: cinematic HUD / player hiding and the material changes in the view (would need protocol fields). Open: the inventory backup / clear, PatrolPointReached / Possessed / MovablePickedUp / Combat / BehaviorStarted / TaskStateChanged (fire sites not found), the original-side collision for the 40 queued maps.
## 2026-10-08: Map sweep rendering fixes (light beams, water sheets, island sea, sky horizon, door placeholder)
- **Question:** Why do the rendered map starts show solid light cones (Pub x3, Art Dealer), a white slab (Prison sewer), pink / flat seas (Isl x2), a pink screen (Out_P), flat outro skies, an "I'M OBSOLETE" door (TowerRtrn_Int), no frame on Bridge_Part2, and black starts (Prison, Ovrsr, Flooded_FStreets)?
- **Evidence:** `map_dump --material-params / --near / --props`, `mic_static`, `shader_maps` + `fxc /dumpbin` of RefShaderCache: Volume_Light_PMAT 6626 .. 6633 (VS 30 / 3), water_spillway_translu_PMAT 1584 / 1585 / 1596, water_Foam_MAT 37072 .. 37076, Skies_PMAT 6044 / 6046 / 6052 / 6059 (VS 1215), Blockout_PMAT; `export_hex` of `Hub_FX.Light.Hub_Medium_Window` (34 vertex colours); `tweaks doors_twk.door_small_poor_int_wood_twk`; `obj_dump` of DisDoor_2, DisPointLightToggleable_10, StaticMeshActor_2223 and the Ovrsr streaming levels; ParticleModuleSpawn lookup tables of every Bridge_Part2 package.
- **Result:** decoded and rendered from those shaders (RENDERING.md "Light beams", "Water sheets", "Island sea", "Sky dome", "Doors' emissive placeholder"): `volume_light.rs`, `spillway.rs`, `water_foam.rs`, the sky horizon / storm mask / UV0 diffuse; static meshes keep their vertex colours; material cubes are imported face by face; skeletal-prop and character materials drop the slots their `*_Enable_*` switches turn off (the door's `E_Emissive_texture = obsolete_door_d` with `E_Enable_Emissive` false; the tweak's mesh is the component's own, no swap rule exists). Bridge_Part2: no pathological data; the sweep's run was starved (U-935). Dark starts: Prison's PlayerStart is in the cell, lightmapped near black (wall lum 0.001 .. 0.006), lit only by `DisPointLightToggleable_10` (warm, brightness 8, radius 800, falloff 5) behind the camera; the original opens on the `DraggedIN` matinee, not this view. Ovrsr's black ground is the always-loaded `L_Ovrsr_Script` blockout slab `StaticMeshActor_2223` (`Env_Blockout.staticmeshes.Cube_100` x (9.5, 10, 0.5), `GreyLevel_010`, z -236 .. -211, no HiddenGame, lightmap texels zero) over the kennel floor at this dev PlayerStart (the real start is the travel start, 211 uu away). Flooded_FStreets faces `brewery_storehouse_hub_wall_01` instances whose lightmaps are near zero (lum 0.000 / 0.001; the mesh's 104 instances average 0.038): a night exterior; nothing missing found.
- **Confidence:** CONFIRMED for the decoded terms (each read from the listed disassembly); LIKELY for `H_Horizon_UV_Distortion` at 6059's `UniformPixelScalars_6.w` (the listing omits it; 6044 names it at the same use) and for the cube face orientation (same convention as the level cube, map_render).
- **Implementation consequence:** new runtime materials (VolumeLightMaterial, SpillwayMaterial, WaterFoamMaterial; SkyMaterial gains a noise texture and two vectors); `StaticMeshAsset::colors`; importer keeps `LightRays_Mask_R`, the spillway / sea textures, `Tex<i>` and `C_Cubemap_texture.<face>` for those bases, mask channels `F_Foam_Mask_Chanel`, `Storm_mask`. Still drawn "far" without a depth prepass (U-931). Re-screenshots: `scripts/render_fixes.sh`.

## 2026-10-08: First-person swing shots and skinned-mesh culling
- **Question:** the coordinator's render of the swing series showed no arm or sword at the shots named 17t and 22t, and suspected frustum culling by the bind-pose box.
- **Finding:**
  - The first-person arms, weapons and bolts have had `NoFrustumCulling` since they were added. Bevy 0.19 honours it in CPU culling (`check_visibility`) and in GPU preprocessing (`MESH_FLAGS_NO_FRUSTUM_CULLING_BIT`). No camera has `OcclusionCulling`, which would ignore the flag. So culling does not hide the rig.
  - The shot names undercounted the ticks: each `screenshot` is followed by `wait_ticks: 1`, so the shots were at about 6, 13, 19, 25 and 34 ticks after the press. `fp_probe` on `Sword_Ready_AttackRight_A_Small`: the hand is upper right at 0.2 s, crosses the centre at about 0.27 s (51 uu ahead, 13 below the eye), and is 35-41 uu below the eye at 0.33-0.37 s, under the 75-degree view's lower edge. The empty shots are the data at those instants.
  - Third-person skinned meshes (characters, level props, cutscene props) had no `NoFrustumCulling`, so Bevy culled them by their bind-pose box when it could compute one.
- **Port:**
  - Skinned parts get a culling box from their bind-pose vertices grown by 250 uu on every side (`characters::animated_bounds`), spawned with `NoAutoAabb` so Bevy keeps it. Big swings, falls and bodies drawn at their carrier's root no longer pop at the screen edges, and far off-screen pawns are still culled.
  - The swing series now shoots at 6, 13, 15, 17, 22 and 30 elapsed ticks (names match the elapsed ticks).
  - The `DISH_VM_DEBUG` log line prints each layer's clip time.
- **Confidence:** CONFIRMED for the culling flags (Bevy source) and the tick count (script). The swing poses are from the decoded clip.

## 2026-10-08: Stealth light value matched to the hook capture
- **Question:** Make the port's stealth light value match the captured light environment (scale, light selection, occlusion).
- **Evidence:** 0x004E1F02 (`fmul 0.25`, `fmul 1/pi` before the 1/2.2 power), 0x004E0440 (directional `GetPosition` = `-axis x TraceDistance`), 0x004FF270 (`SingleLineCheck` flags 0x2397: `TRACE_ShadowCast`), 0x004DB470 (sky light through the hemisphere functions), hook outputs `le_lane1..5`, `le_boyle1..2`.
- **Result:** raw light `(L / 4 pi)^(1/2.2)`; directional lights traced 100000 uu; sky lights at 2 Y00; layout maps unoccluded (InterpActor cubes have no static shadowing); Boyle's sky light added to the Playground. `stealth/light_probe_lane` passes on raw light (11 / 11), new `stealth/light_probe_boyle` passes (raw and normalized).
- **Confidence:** CONFIRMED (code and capture agree to about 1 % per light).
- **Implementation consequence:** cache keys `<map>.ailevel6.json`, Playground DERIVED_VERSION 5. The `m_LightProbe` channel filter is kept: the live channels of every contributing light have it, and the packages store `false` on the excluded ones.
## 2026-10-08: Chaos scenarios: unpossessed NPCs are Unaware, civilian HealthMax per archetype

- **Question:** `chaos/corpse_discovered` / `civilian_kills` / `guard_kills_sweep` failed: no kill from the back assassination, civilian health 70 against the original's 11, a pawn.x offset of -45.6.
- **Evidence:** the assassination awareness test 0x00C09060 casts the target to an NPC pawn (0x00A490E0, a class test, not the controller) and reads 0x00B5E250, which returns 0 (`Unaware`) when the pawn has no brain (0x00B599B0 null); the captured traces of `civilian_kills` and `guard_kills_sweep` (Passive = unpossessed pawns on the original) show the assassination (player and victim moved, victim 11 -> 0). The party aristocrat tweak `Pwn_AristoParty_MTall_1` has `Attribute_HealthMax` 11 (Normal), the party guard 50, the female aristocrat 12.
- **Result:** a mission's brainless (Passive) NPC is `UNAWARE` (was `AWARE_OF_PLAYER`); civilians take their archetype's `Attribute_HealthMax` (`AiConfig::archetype_health_max` from `characters::ARCHETYPE_TWEAKS`) and original civilians their own tweak's (`AiLevel::tweak_health_max`). `combat/sword_hit_dummy`'s dummy is now Alerted (the measured target was a live, aware doorman). The pawn.x offset is the assassination's lunge on the original (the port's player stays put) plus a capture artefact: on the original the first press lands near tick 240 and the console teleports are not applied (one kill per capture). All scenarios 92 pass / 6 fail (the 6 are the earlier stealth / movement / crossbow ones).
- **Confidence:** CONFIRMED (decode and traces).
- **Implementation consequence:** gameplay config key `gameplay_config.v21.json` (`ai.archetype_health_max`), map cache `<map>.ailevel7.json` (`tweak_health_max`).
## 2026-10-08: NPC, world and map animation parity (special classes, throws, look-at, animated props)
- **Question:** Do the NPC tree, the special classes' moves (`npc_move` 20-28, no animations mapped), thrown NPCs, look-at and placed animated props play what the original's data and code say?
- **Evidence:** `npc_anim_dump defs` of `DisNPCAnimDefinitions_Tallboy` / `_Weeper` / `_MusicalOverseer` / `_Wolfhound` (`find_export`: cooked in the Boyle and Overseer scripts); `tweaks` of the special item contexts (`m_MainAttack.m_Animation`, `DisTweaks_Minigame.m_NPCAnimation` / `m_GroupIndex`), the items' `m_ItemPrefix` (`Sword_`, `MusicBox_`, `Patrol_`, `Empty_`) and the pawn tweaks' loadouts; `npc_anim_dump sets / loco` of the four classes; static index: `StateNPCMasterTallboyAim` sub-state -> chain 0x00A7CEB0, `DisWepMusicBox` slots 81 / 96 (0x00C291F0, 0x00C29220) and `DisItemContext_NPCTune_Combat` slots 103 / 108 (0x00C26F70, 0x00C202B0) writing `m_WalkUpperBodyDesire` (+0xDB0, 0x00B53960) or requesting chain 0x77; `obj_dump` of `DisRiverKrust` (tree `Npc_RiverKrust_at`, tweak `m_AnimSequences`), of Boyle's `SkeletalMeshActor`s (Int's clock loops `Clock_TickAnim`) and of the 20 Boyle `DisNPCDistractor`s (all soiree matinees).
- **Result:** `AnimProfile.item_prefix / empty_prefix`; the nine special moves mapped (ANIMATION.md section 13 table); Tallboy aim in / loop / shot / out; music box walk upper body and `MusicBox` modifier; thrown NPCs play `PushToRagdoll` by direction, the ragdoll from its notify and the `Stun` group 1 get-up matching the simulation's hold; plague and rat deaths; `MatchView::npc_heads` (the vision head's cursor split by `head::split_aim`) drives the look-at grids; river krusts and looping skeletal scenery are animated level props, krust collision posed by its loop (collision19); `DISH_ANIM_DUMP` per-frame bone lines; render scripts `npc_anim_playground`, `npc_anim_boyle`, `props_anim_boyle_int`, `props_anim_krust`; the `session bones` capture request.
- **Confidence:** CONFIRMED for the data tables, the Tallboy sub-state mapping, the music box / combat tune desires and chain 0x77; LIKELY for the minigame steps without a hold (U-893), the death chains by damage (U-890), the walk-upper-body slot and masking (U-891), the `MusicBox` modifier choice (U-892).
- **Implementation consequence:** protocol 19 (AnimProfile, MatchView.npc_heads), appearance cache `appearance5`, collision cache `collision19`, `SkeletalProp::idle` (serde default). Open: turning in place (U-280), distraction matinees (U-895), krust states and watch tower aim nodes (U-894), Tallboy stilts and weeper / hound idle behaviours (U-896); nothing built or run here (coordinator verification).
## 2026-10-08: Menus and settings parity (MENUS.md "How the motion was decoded" .. "Parity automation")
- **Question:** What do the original menus do frame by frame (open / close / focus / swap timelines, layouts, sounds, input locks), and which settings does the port honour?
- **Evidence:** the AS2 of `UI_MainMenu.MainMenu`, `UI_OptionsMenu.OptionsMenu`, `UI_PauseMenu.PauseMenu`, `UI_LoadGame.LoadGame`, `Common_assets.lib`, `UI_Global.Global` (class sprites decompiled with `research/scripts/as2_decompile.ps1`, frame scripts with `research/scripts/swf_actions.ps1`); `swf_dump` placements; `Twk_InGameUI.UISoundTheme_Common` and the global / pause themes; `DefaultEngine.ini m_MapConfig` (`m_ImageName` card pictures); `Default__ArkProfileSettings`.
- **Result:** decoded timings for the start screen, main menu, new-game screen, Missions card slide, options screen (tabs, sub tabs, lists, widgets, secondary screens), title bar, help bar, confirmation box, pause menu and save / load lists (MENUS.md). The port now reproduces them (`menu_anim.rs` tween model, `menu_art.rs` timeline playback with frame scripts and clip overrides, `menu_widgets.rs`, `menu_options.rs`, `launcher.rs`, `pause_menu.rs`); steppers clamp as the original's (`Settings::step`); the start screen, quit / restore / video / back-to-menu / back-to-windows / load / save questions; free roam over the 42 base maps (`--freeroam`). Settings table with what is applied, live or at load, and what is not honoured.
- **Confidence:** CONFIRMED for the decoded scripts and data; the GFx 3D properties are not drawn (U-951).
- **Implementation consequence:** `settings.ron` gains `launcher` (last picks); anti-aliasing is SMAA / FXAA without MSAA (`DISHONORED_MSAA=1` for the old look); the harness launcher suite presses through the start screen and answers the questions.

## 2026-10-08: The stealth light's line check decoded (replaces the layout-map flag)
- **Question:** Which geometry does the stealth light's `SingleLineCheck` (flags 0x2397, 0x004FF270) hit, so the Playground's `InterpActor` cubes need no special flag?
- **Evidence:** `UWorld::MultiLineCheck` 0x0064A860, the primitive octree's line check 0x006B12C0 (the `TRACE_ShadowCast` branch: `CastShadow`, `bUsePrecomputedShadows`, `ULightComponent::AffectsPrimitive` 0x00519760), `UStaticMeshComponent::LineCheck` 0x007717D0 (`& 0x20100`), `InterpActor::ShouldTrace` 0x00568990, the octree attach 0x0052A0B0; Engine class defaults of `StaticMeshActor` / `InterpActor` / `DynamicSMActor` / `KActor` components (precomputed shadows, `LightEnvironment`).
- **Result:** the check meets components that collide (octree), `CastShadow`, `bUsePrecomputedShadows`, `bAcceptsLights` and have no `LightEnvironment`; movement and gameplay trace types do not matter. `InterpActor` (and the other movable classes) fail two of these. Ported as `MeshInstance::stealth_shadow` -> `channel::SHADOW`; `AiLevel::stealth_light_unoccluded` removed. See stealth.md section 15.
- **Confidence:** CONFIRMED (decoded) for the per-primitive rule and the class defaults; the octree entry that selects the traversal is inferred (stealth.md section 15, not decoded).
- **Implementation consequence:** cache keys `<map>.collision20-stealthshadow.bin`, `<map>.ailevel8.json`, Playground DERIVED_VERSION 6.

## 2026-10-08: In-game HUD, popups and wheel on the original movies (dishonored/HUD.md)
- **Question:** Make every HUD element and popup look, animate and time like the original: the HUD movie's classes, the native popup queue, gauges, crosshair, messages, HUDFX, the quick-access wheel, the saving notification.
- **Evidence:** ActionScript of `UI_HUD.HUD`, `UI_HUDFX.HUDFX`, `UI_PowerWheel.powerwheel` (WheelHandler, PC_ShortcutsHandler / ShortcutsHandler, PC_PotionsCount / PotionsCount, MovieClipCreator, ScreenPosition, root timeline functions), `UI_Global.Global` (g_SavingNotification_Icon, RotationAnimation), `Common_assets.lib` (EquipmentIcon, itemIcons); natives 0x00BB2280 (HUD tick), 0x00BAFE70 and 0x00BB2480..0x00BB2B20 (popup queue), 0x00B9F6E0 / 0x00B97D40 / 0x00B97F80 (gauges), 0x00B9FF30 (crosshair), 0x00BA6150 / 0x00BA4620 (interactions), 0x00B95160 (location), 0x00B970F0 (damage), 0x00BE4280 / 0x00BE25C0 / 0x00BD5530 / 0x00BD5250 / 0x00BCD660 / 0x00BCD800 (wheel), 0x00A17890 / 0x00BAA390 / 0x00BAA3D0 / 0x00BAA610 (save UI), 0x00BBD630 / 0x00BBA030 (awareness markers); tweaks Twk_GFxMoviePlayerHUD, Twk_GFxMoviePlayerPowerWheel; AkEvent `RequiredBank`s.
- **Result:** `gfx_stage.rs` (GFx stage), `hud_widgets.rs`, `hud_wheel.rs`, `hud_global.rs`, `gfx_hud.rs`, `hud_popups.rs` (the original queue replaces restart-on-reopen); `Action::Location` for DisSeqAct_ShowLocationDiscovery; the objective window's sounds were silent (Bank_UI_Objective not loaded) and now load; harness steps and scripts `hud_playground.json` / `hud_boyle.json`.
- **Confidence:** CONFIRMED for the class timings, tweens, easings, queue rules and wheel geometry (bytecode and disassembly); APPROXIMATION / OPEN items U-880..U-887.
- **Implementation consequence:** protocol: `MissionView.location` (serde default) needs the coordinator's bump; `LocalInput.wheel_open / shortcuts`; `Saves.written`. Not done: awareness markers (decoded), death screen (pause menu movie), mission stats, journal, tutorial windows.

## 2026-10-08: Level streaming (Kismet-streamed sublevels), orphan StaticMeshActors, cinematic HUD mask
- **Question:** Model the original's level streaming instead of importing and running every streaming level at once (L_Out_P 57 % against the capture); decode the ToggleCinematicMode mask bits; find why Ovrsr / Tower Script-package blockers collide only in the port.
- **Evidence:** a survey of every map's `WorldInfo.StreamingLevels` (311 `LevelStreamingAlwaysLoaded`, 22 `LevelStreamingKismet` on 9 maps, no flags serialized); `SeqAct_LevelStreamingBase::ActivateLevel` 0x006dc230, `SeqAct_MultiLevelStreaming` 0x006e5f90 / 0x006dc370 / 0x006e61f0, `SeqAct_LevelVisibility` 0x006e62e0, `FindLevel` 0x006e5d50 (`LevelStreaming` +0x5c bits 0x80 / 0x100 / 0x200); `CommitMapChange` 0x0062d570 (`LevelStreamingPersistent_%s` for the prepared secondary levels); the global handlers' `InitiallyLoadedSecondaryLevelNames` (L_Out_P's 24 variant triples, the Pub chapter scripts). `SetCinematicMode_Native` 0x009eea00 -> 0x00aaf150: `DishonoredHUD.m_ShowFlags[eDisHUDMaskLevel_Cinematic]` (+0x4e0), controller `m_bInputIgnoreInput_Cinematic` (+0x610 bit 0x1000). The orphan blockers: `StaticMeshActor_2223` / `_2` whose `StaticMeshComponent` is the collection's `StaticMeshActor_SMC_<n>`.
- **Result:** per-sublevel collision (`CollisionWorld::split_levels` / `merged`, `StreamedLevels`, `WithLevels`, `WorldQuery::set_visible_levels`), Kismet sublevel load / unload (`Kismet::load_map_levels`, `set_level_loaded`), the three streaming ops and the secondary levels in the host, sublevel actors (spawners, pickups, movers) in and out (`MissionMatch::set_level_visible`), `MatchView::hidden_levels`, `oracle collision --stream`. Against the captures: Out 57.14 -> 100 %, Pub Day / Dusk / Morning / Night 99.20 / 99.46 / 99.46 / 99.58 -> 99.43 / 99.84 / 99.84 / 99.83 %, Brothel 99.80 -> 99.96, Bridge 2 99.60 -> 99.67, Ovrsr 98.51 -> 99.80, Tower 98.86 -> 99.43, Pub Assault 98.91 -> 98.94; the other 31 maps identical.
- **Confidence:** CONFIRMED for the streaming op rules and the cinematic mask; LIKELY that a prepared secondary level is loaded and visible at the commit (UE3 code, the format string; the capture of a console-opened map has no streamed level, which is what the comparison uses).
- **Implementation consequence:** collision cache `collision20` + `levels20`; protocol: `MatchView::hidden_levels` (the renderer must hide those sublevels' entities: not wired, runtime not built); saves: `MissionMatch::levels_visible`, `HostState::{secondary, streaming, unloaded}`, `Cinematic::{no_buttons, hud_hidden, hud_shown}` (serde defaults); `Spawner::package`, `OriginalNpc::package`, `MissionSetup::streamed_levels`; oracle port snapshot version 3. Open: navigation / AI points / water of streamed sublevels stay map-wide (Pub scripts' nav points, `L_Out_EnvLabPiero`'s pylon, `L_Tower_Water`'s water volume).
## 2026-10-08: Post-process chain: Kismet grades, possession field, arm glow, menu blur, light shafts (systems/render_post.md)
- **Question:** Which post-process features of the original were missing from the port, and what do they do?
- **Evidence:** `package_scan` census, `obj_dump` of the 15 `DisSeqAct_UberPostProcess` ops, `tweaks` (`UI_Blur`), `oracle index` (`FUN_00b91570` / `FUN_00b91450` / `FUN_00be7e90` / `FUN_00bef9b0` the manager's Kismet slot state machine, `FUN_00c47070` the UI blur request, `0x00837480` / `0x00820300` / `0x004e0440` light shafts), `shader_dump` + `fxc /dumpbin` (global shaders 155 / 157 / 158, RefShaderCache 37254), the captured possession shader 0x9312eee076ac74a4 and its timelines.
- **Result:** Kismet screen grades with the slot's fades (protocol 19, `MissionView.post_fx`), UI blur on the pause menu, the possession swirl field and the arms' `Use_Power` glow, UE3 light shafts (11 maps) with the video setting; the KO / Weepers / MaskOn / Adrenaline / Plague / Lens / Blinded / Outsider screens stay blocked (no shader maps).
- **Confidence:** CONFIRMED for the Kismet slot, the translated shaders and the timelines; LIKELY for `Tex#4` = `M_Mask`, the pause menu as the UI blur request, the shaft targets' sampler / factor.
- **Implementation consequence:** `HostState.post / post_effects` (serde defaults), protocol 19, `Environment.light_shafts`, new `ArkPost` uniform fields and a possession noise binding (layout entry 9).

## 2026-10-08: Depth and reflection, round 2 (soft particles, capture fog / post, Kismet capture toggles, lens TV)
- **Question:** Does the sprite depth fade share Volume_Light's constant; which compiled material shaders read the scene depth; what do the capture components' fog / post flags, Kismet toggles and the SceneCapture2DActor need?
- **Evidence:** `shader_maps RefShaderCache-PC-D3D-SM3.upk` + `fxc /dumpbin` of 36033 / 36034 / 35519 / 35911 / 2726 / 6869 / 8137 (constants 250 and 256 against Volume_Light's 50, the TV's uniform expressions); a scan of the 1745 pixel shaders that name `SceneColorTexture`; `oracle index uscript SceneCaptureActor` (`OnToggle`) and `index class SceneCapture2DComponent`; `obj_dump L_TowerRtrn_Int_Script` (SceneCapture2DActor_0, `saili_FT_LensTV.screen_MAT`).
- **Result:** the particle bases' Db = saturate((SceneColor.a x 256 - depth) / ((1 - I) x 250)) multiplies the opacity term (`FxMaterial`, `particle_soft.wgsl`); every other depth-reading base is listed in systems/render_depth.md with its port status. Captures with `bEnableFog` / `bEnablePostProcess` run the level's DisFog / ArkPp on the mirrored view; `SeqAct_Toggle` reaches `reflect::update_cameras` through `MissionView::captures`; the lens TV is a fixed-view capture plus `tv_screen.rs`.
- **Confidence:** CONFIRMED for the 250 / 256 terms of the Db passes, the `OnToggle` rules and the TV material terms; LIKELY for `lightCone_twoSided_PMAT`'s 500 and for applying DisFog / ArkPp to a capture (U-960); not run: the coordinator's screenshot pass is the first run.
- **Implementation consequence:** protocol 23 (`MissionView::captures`); `ReflectCapture::scene2d` (`SceneCamera`); `effects::{FxMaterial, SoftFade, PendingSoft, soft_scale}`; `tv_screen.rs`; `ArkPostSource::capture`.

## 2026-10-08: AI-side Kismet events (PatrolPointReached, BehaviorStarted, Combat, Corpse, PawnSighted, Investigate, FleepointReached, Assassinated)
- **Question:** From which engine code does each AI-owned `DisSeqEvent_*` fire, with which output, originator and instigator, and where does the port's AI reach the same moment?
- **Evidence:** `oracle index fn` of the `FireGeneratedEvents` (0x00bc7b30) callers: 0x00afd350 / 0x00afd430 (patrol arrival, output 0), 0x00af2300 (guard arrival, output 1), 0x00af7670 from the arbitration 0x00b17100 with the override 0x00b8a1a0 / 0x00b00d80 (exact behaviour class), 0x00af7d00 (combat start, `DisBehaviorCombat` slot 91), 0x00b2e050 / 0x00ae9fe0 (investigate), 0x00a47340 from 0x00b29ae0 / 0x00b29be0 (flee point), 0x00a998a0 / 0x00a90b50 / 0x00b40740 (corpse picked up / dropped / discovered, which also fires CorpseDetected), 0x00b24ec0 <- 0x00b26b30 <- 0x00b3b350 (PawnSighted at the 3 -> 4 attention step), 0x00a7beb0 / 0x00a78270 (Assassinated Started / Finished). The 41 maps' instances come from a throwaway dump of the loaded Kismet database (event class, originator, `m_pBehavior`, output names).
- **Experiment:** unit tests with hand-built graphs (`kismet_host::tests`) and brain tests (`dishonored_ai`); a 60 s no-input smoke of all 42 maps (`oracle scenario --system maps` with `ticks: 3600`) before / after: PatrolPointReached activates 13 times (Boyle_Int, Bridge_Part2, Distillery, Pub_Assault, Streets1, Streets2), nothing else changes, no map regresses. The other events fire from the AI in those runs (BehaviorStarted 681, Investigate 18, Combat 11, CorpseFound 30, FleepointReached 3) but without events on those pawns' spawners; the no-input smokes do not reach the scripted encounters that own them.
- **Result:** ported: PatrolPointReached output 0, BehaviorStarted (per behaviour class of the port's behaviours; the routine slot's Idle / Patrol / Guard are told apart by the routine), Combat, Investigate, FleepointReached, Corpse outputs 0 / 1 / 2 and CorpseDetected, PawnSighted, Assassinated Started. The nav point / flee point actors are found by their place (the crowd snaps the points to the floor: same x, y, nearest z). Open: Assassinated Finished (anim notify), PlayerHeard (the writer of brain +0xa8 `m_PendingPlayerHeardKismetEventActorName` is not in 0xaff000-0xb80000), AttackedByRats (NPC master state entry), WatchTower (not in the port), PatrolPointReached output 1 (guard post point actors), BehaviorStarted for React / PatrolSearch / EscapeExplosion / rat-swarm combat / Soiree.
- **Confidence:** CONFIRMED for the fire sites, outputs, originators and instigators read above; LIKELY for the port's moments (the route arrival radius 70 uu, flee arrival 120 uu, the kill moment of an assassination) which are the port's own thresholds; UNVERIFIED against a capture.
- **Implementation consequence:** `AiEvent::{BehaviorStarted, Combat, Investigate, PatrolPointReached, FleepointReached}`; `Brain::{fired (not saved), behavior_started, routine_started, flee_point, flee_reached}` (new serialized fields: saved Brain states change layout); `kismet_host::{ai_events, fire_generated, actor_at}` and `FED_EVENTS` gains the eight classes.

## 2026-10-08, stealth: detect_600 / detect_1300 head analysis (agent detect)
- The cached per-tick captures contradict the specs' old numbers. The capture with bones shows an `Empty_Idle_03` ambient idle, bone axis, aim-ray-clamped target, 7-tick LOS cadence (stealth.md section 16). Added the ambient idle bake with eye axes (`dishonored_assets::head_pose`, `HeadLoop::axes`), validated against the capture (0.03 deg). No AI behaviour change; both scenarios still fail; open items in section 16.

## 2026-10-08, stealth: ambient idle, vision target and sighting schedule ported; detect_600 / detect_1300 re-baselined (agent detect2)

* **Question:** what makes the player's sighting in `stealth/detect_600` / `detect_1300` begin, change cone and end, and can the port reproduce the per-tick captures.
* **Decoded** (details in systems/stealth.md section 17): the ambient idle pick (`rand()` at 0x00B39728, patchable by the hook: `NpcSetup.ambient` pins the sequence), the player target (`Observable` vslot 0, 0x00C669C0: the segment `Location.z +- CollisionHeight`, the point in the eye's up-plane), the five LOS points of the vision region (0x00C77FC0), the 0.1 s LOS check period (`Twk_Component_Observable_Player`), the candidate list schedule (0x00C788E0) and the accumulator's missing upper bound (max FLT_MAX).
* **Port:** `head::Head` ambient state (eye from the bone while it plays), `Brain::sight` / `target_point` / `los_clear`, one-tick latency of the accumulation, `AttentionEntry::change` unbounded, `VisionConfig::vis_regions` / `los_period`, `AiConfig::ambient_anim_freq`; cache key `gameplay_config.v23-detect` (a plain bumped number collides with other branches' bumps; main already took v22).
* **Oracle:** `NpcSetup.ambient` / `ambient_age`; `probe_launch scenario` patches the pick; detect_600 / detect_1300 re-captured pinned with `Bones` / `AnimState` and re-baselined to those captures (old numbers dropped, reasons in the spec headers); `Alerted` on the original now also writes level Busted (it is still reduced to Search: see combat/sword_hit_dummy).
* **combat/sword_hit_dummy:** the cached "Alerted" capture was the Search behaviour (value cut back to 0.7, level 3) and the guard was assassinated from the front (throat fatality), so the port's InCombat at tick 0 could not agree. Re-captured with a hostile-faction Doorman that settles 120 ticks (real `DisBehaviorCombatEliteGuard`, level 4, value 2.18 and rising); it steps back 73 uu, the swing whiffs, timing and damage are Unverified.
* **Results:** detect_600 level 1 / Busted at ticks 22 / 87 (capture 22 / 87; was 24 / 90), window rate 0.0844 (0.0820); detect_1300 first block, loss, re-sighting; stealth 5 pass (was 4), ai unchanged (10 pass).
## 2026-10-08: Lean onset and crossbow shot time re-read against the scenario captures (movement.md MV-4, ranged.md F14)
- **Question:** Why does the port's lean ramp run slow against the original (`movement/lean`, `lean_boyle`), and why is `combat/crossbow_hit` shot -> load 1.583 s against 1.555 s?
- **Evidence:** the original-side scenario captures (`harness_out/oracle/scenario/traces/*.original.json`): lean angle 2.175, 4.307, 6.383 ... 15.007 (tenth tick) per tick at a fixed 1/60 s, first head step on the third tick after a press (three presses); the port's angle series, replayed in a standalone model, equals it to 0.01 degrees one frame earlier. 0x00AC5950 / 0x00AC5540 / 0x00ACAAB0 / 0x0041D720 / 0x00A76590 / 0x00AC0950 / 0x00AC5420 re-read (roll = atan2(dy, dz) of the interpolated head from the pivot; fixed-step fields +0x48 / +0x4C / +0x58). Crossbow: `Crossbows_as.Crossbow_as` `Crossbow_Fire` `DishonoredNotify_FireProjectile` 0.0336 s (`anim_notifies`), `Ply_Crossbow_as` `AnimStateExit` 0.3984, capture: fire sound tick 6, bolt tick 8, reload sound tick 30, `Reload_Out` sound tick 102.
- **Result:** the lean spring was right; the old checks (2.264 / 12.206 / 15.0 on the sixth tick, 14.2 at 12 ticks) came from a hand trace that the captures contradict. What the port lacked: the lean influence is a fixed-step influence (0.016667 s step, accumulator) whose first 1/60 s frame spends no step, and the lean state's release timer. The crossbow bolt leaves on the fire clip's `FireProjectile` notify (third tick of the action), not on the next tick; the action end, reload In and timed stage already agreed with the capture.
- **Confidence:** CONFIRMED for the angle series, the notify and the capture ticks; LIKELY for the stepping loop (not located; read from the interpolation in 0x00ACAAB0). Open: the lean release (original pins the angle six steps, the port three; a head that the limiter does not re-place fits at 0.14 deg rms), the camera POV position easing in the captures.
- **Implementation consequence:** `PlayerState.lean_state / lean_release / lean_acc / lean_ext` (protocol 24), `LeanConfig.fixed_time_step / lean_speed / release_time`, `movement::{lean_state_tick, step_lean, lean_step}`; `CombatTuning.crossbow_fire_notify / crossbow_fast_fire_notify`, `left_hand::bolt_shot_tick` (crossbow, sleep dart, incendiary bolt), the crossbow fire times read from the AnimSets into `crossbow.values["fire.*_time"]`; `lean` / `lean_boyle` checks re-based on the captures.
## 2026-10-08: Player- and world-side Kismet events (Possessed, crouch, combat, tasks, choke, Windblast, powers, climb, lock)
- **Question:** From which engine functions do the Kismet events of the player and the world fire, with which outputs, and which can the port feed?
- **Evidence:** `oracle index fn` of the `FireGeneratedEvents` (0x00BC7B30) callers and of the firing functions listed in KISMET.md "Player and world events" (0x00B7D110 / 0x00B7D2F0, 0x00AB0960, 0x00AB9FA0 with 0x00AB6960 / 0x00B0B780 / 0x00B0C1E0, the objective dispatcher 0x00AD4F60 with 0x00AD1080 / 0x00AD1320 / 0x00AC17B0 / 0x00AC63C0, 0x00A7C270 / 0x00A6D380 / 0x00A74720 / 0x00A7C4D0, 0x00C4DE90 / 0x00C53790, 0x00BF32F0 / 0x00BF30C0 and the power activations, 0x00A72B00 / 0x00A6E2C0, 0x00A4E870 / 0x00A4E910), `exe_probe dis` of the `SeqEvent_Destroyed` call (0x00652E61: originator and instigator the actor), `kismet find` of the events in the maps.
- **Result:** the host fires ten event classes from the sim and mission events (`kismet_host/player_events.rs`, 154 Possessed, 4 PlayerCrouch, 15 PlayerCombat, 79 + 2 Task events, 6 Choke, 9 Windblasted, 21 PowerUsed (four powers), 25 Lock, 1 Climb); `DisSeqCond_IsPlayerInCombat` reads the decoded `m_bIsInCombat` flag instead of the autosave set. The rest (movables, keyholes, interact, tripwires, rigid bodies, particles, destroyed, holster, inventory, journal, mana threshold, objective completed) have a decoded firing rule but no system in the port.
- **Confidence:** CONFIRMED for the firing functions, outputs and conditions of the fed classes; LIKELY for the port's `PlayerCombat` (the registry and DLC terms of 0x00AB9FA0 are not modelled), for the PowerUsed moments of Windblast and Swarm (the activations were read, the port's cast press / notify stands for the call), and for `Windblasted` output 1 (the pushed/thrown choice by the resulting state class was read, not the class name).
- **Implementation consequence:** `SimEvent::PowerUsed` / `Windblasted`; `MissionState::task_log`; `kismet_host::step` takes the climbable names; `HostState.player_ev` (serde default); `FED_EVENTS` gains the classes (and `NOT_FED_EVENTS` keeps `PowerEquipped` out); no protocol change.
## 2026-10-08: Lean onset and crossbow shot time re-read against the scenario captures (movement.md MV-4, ranged.md F14)
- **Question:** Why does the port's lean ramp run slow against the original (`movement/lean`, `lean_boyle`), and why is `combat/crossbow_hit` shot -> load 1.583 s against 1.555 s?
- **Evidence:** the original-side scenario captures (`harness_out/oracle/scenario/traces/*.original.json`): lean angle 2.175, 4.307, 6.383 ... 15.007 (tenth tick) per tick at a fixed 1/60 s, first head step on the third tick after a press (three presses); the port's angle series, replayed in a standalone model, equals it to 0.01 degrees one frame earlier. 0x00AC5950 / 0x00AC5540 / 0x00ACAAB0 / 0x0041D720 / 0x00A76590 / 0x00AC0950 / 0x00AC5420 re-read (roll = atan2(dy, dz) of the interpolated head from the pivot; fixed-step fields +0x48 / +0x4C / +0x58). Crossbow: `Crossbows_as.Crossbow_as` `Crossbow_Fire` `DishonoredNotify_FireProjectile` 0.0336 s (`anim_notifies`), `Ply_Crossbow_as` `AnimStateExit` 0.3984, capture: fire sound tick 6, bolt tick 8, reload sound tick 30, `Reload_Out` sound tick 102.
- **Result:** the lean spring was right; the old checks (2.264 / 12.206 / 15.0 on the sixth tick, 14.2 at 12 ticks) came from a hand trace that the captures contradict. What the port lacked: the lean influence is a fixed-step influence (0.016667 s step, accumulator) whose first 1/60 s frame spends no step, and the lean state's release timer. The crossbow bolt leaves on the fire clip's `FireProjectile` notify (third tick of the action), not on the next tick; the action end, reload In and timed stage already agreed with the capture.
- **Confidence:** CONFIRMED for the angle series, the notify and the capture ticks; LIKELY for the stepping loop (not located; read from the interpolation in 0x00ACAAB0). Open: the lean release (original pins the angle six steps, the port three; a head that the limiter does not re-place fits at 0.14 deg rms), the camera POV position easing in the captures.
- **Implementation consequence:** `PlayerState.lean_state / lean_release / lean_acc / lean_ext` (protocol 24), `LeanConfig.fixed_time_step / lean_speed / release_time`, `movement::{lean_state_tick, step_lean, lean_step}`; `CombatTuning.crossbow_fire_notify / crossbow_fast_fire_notify`, `left_hand::bolt_shot_tick` (crossbow, sleep dart, incendiary bolt), the crossbow fire times read from the AnimSets into `crossbow.values["fire.*_time"]`; `lean` / `lean_boyle` checks re-based on the captures.

## 2026-10-08: Movable props (pick up, carry, drop, throw) and the MovablePickedUp / MovableDropped events
- **Question:** What does the original do when Corvo picks up a `DishonoredMovable` (bottle, glass, pan, crate), carries it, drops it and throws it, and what do the hits do (damage, break, noise)?
- **Evidence:** `oracle index fn` of the grab state `StatePlayerGrabMovable` (0x00A6FEF0 / 0x00A698C0 / 0x00A77540 / 0x00A77600 -> 0x00A69860 / 0x00A698E0), the empty hands' item `DishonoredItemEmpty` (0x00C3D030 grab, 0x00C4C440 tick, 0x00C4C510 -> 0x00C4C190 hold place, 0x00C45340 release), the component (0x00A45FD0 pick-up, 0x00A46380 release, 0x00A465E0 tick), the pawn slots 380-383 (0x00AB34C0, 0x00AAD900, 0x00B519F0, 0x00B51A20), the Use handler 0x00AD5BA0 and the action handler 0x00AB4240, the breakable collision 0x00A40CF0 with the movable's `GetDamage` / `SetDamage` (0x00A428C0 / 0x00A428D0); `tweaks` of `Twk_EmptyHands`, `movable_twk.*`; DefaultItem.ini `[DisMovableComponent]`; the Startup AnimSets (no prop clip); dishonored-bevy `props.rs` / `carry.rs` read (controls agree, positions and the weight shares are guesses; nothing copied).
- **Result:** systems/movables.md (M-1 .. M-9). Use picks the focused interactable up (`MovablePickedUp`, originator the PROP: KISMET.md had it reversed), the prop hangs at `eye + forward x (box.min.x + box.max.x + 100) - up x (box.min.z + box.max.z + 70 - show height)` with the camera's rotation after a 0.066 s slide; Use drops, Primary throws it along the view at `Attribute_ThrowStrength` 2000 (`MovableDropped` for both); damage and the other states drop it; hits above `m_fMinSpeedToBeDamaged` / `CauseDamage` / on-drop speeds deal `m_Damage` to the pawn hit and to the prop (a bottle breaks), with the contact type's AI noise (Env_Glass 500 uu Anomaly). No carry animation exists for props.
- **Confidence:** CONFIRMED for the structure, addresses, tweak data and the hold formula; LIKELY for the throw units (velocity), the `m_Damage` amount on a struck pawn, the `m_bDropped` setter (drop only), the contact notification gates (50 / 0.3) and the focus cone; OPEN: the height test on a struck pawn, the `m_Damage` 0 speed-and-mass formula, bounce / friction, the swept `MoveActor` of a held prop, the candle prefabs' pick-up events (candlesticks have no `m_bInteractable`).
- **Implementation consequence:** protocol 27; `dishonored_sim::movables` (new), `WorldState::movables`, `PlayerState::holding`, `SimEvent::MovablePickedUp / MovableDropped / MovableImpact`, `SoundCategory::Thrown`, `DamageCause::ThrownObject`; `MovableDef` (`WorldActors::movables`, cache `worldactors2`), `MovableConfig` (`items.movable`), `NoiseTable::prop_contacts / environment`; Kismet host feeds the two events; `MissionMatch` spawns / saves / removes the props per map and streamed level; runtime `movables.rs` (placed mesh follows the sim, HUD prompt "Carry"); oracle channels `mov.*` and `fixtures/oracle/scenarios/movable/*` (queued in capture_queue.txt).
