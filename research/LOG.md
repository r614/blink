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
