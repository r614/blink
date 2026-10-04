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
