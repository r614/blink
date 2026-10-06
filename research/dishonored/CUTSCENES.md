# Cutscenes: how Dishonored plays matinees and soirees, and how the port plays them

Dishonored's cutscenes are UE3 matinees (`SeqAct_Interp` playing an `InterpData`) extended by
Arkane's "soiree" system: AI and player groups with stage marks, stretched animation keys,
dialog keys that speak conversation blurbs, attachments to bones, loops that hold until Kismet
breaks them. Findings come from the GOG build (`Dishonored.exe`, the same Ghidra project as
KISMET.md), the cooked packages (`obj_dump`, `kismet matinee`) and the UnrealScript class
layouts of `Engine.upk` / `DishonoredGame.upk`. Nothing from the game is copied here.

Confidence tags follow `research/LOG.md` (**CONFIRMED** read from data or code, **LIKELY**
inferred, **APPROXIMATION** the port's stand-in).

Code:
- `crates/dishonored_kismet/src/interp.rs`: data model, loader, evaluation, playback control,
  scene binding (`Scene`); unit tests for curves, move frames, anim keys (plain and
  stretched), director cuts, fades, soiree loops / pauses, dialog waits, attachments.
- Interpreter hooks `[interp]` in `runtime.rs` (soiree control of the position, `SCT_` pins).
- Server: `crates/dishonored_multiplayer/src/kismet_host/cutscene.rs` (hooks `[interp]` in
  `kismet_host.rs`, one line in `mission.rs`); conversation runner `external_matinees`,
  `matinee_done`, `speak_blurb`.
- Client: `crates/dishonored_runtime/src/cutscene.rs` (hooks `[interp]` in `npc_anim.rs`,
  `map_render.rs`, `main.rs`).
- CLI: `kismet matinee PACKAGE EXPORT` prints a matinee's groups, tracks and a sampled move
  timeline.

## 1. Data (CONFIRMED)

- `InterpData` in Dishonored is a stub: `m_Data` points at a `MatineeData` whose
  `m_RunData` (`RuntimeMatineeData`) holds `InterpLength`, `InterpGroups` and, for soirees,
  `m_ConversationNodeGUID`. Soiree matinees live in their own packages
  (`Boyle_SamuelBoatIntro`, `SimpleSoiree`, `DistractionSoiree`, ...) cooked into the level
  package as forced exports.
- Groups: `InterpGroup` (`GroupName`, `InterpTracks`, `GroupAnimSets`, `bIsFolder`,
  `bIsParented`), `InterpGroupAI` (`StageMarkGroup`, `LockRequests` default true,
  `CanInterruptMatinee` default true, `SnapToRootBoneLocationWhenFinished`,
  `bShouldBeDetached`), `InterpGroupPlayer` (`m_StageMarkGroup`, `m_fTimeToBlendToStageMark`
  default 0.2 s, snap flags, `m_bDisablePlayerCollision` default true,
  `m_bLeaveCameraControlToDialogTree`), `InterpGroupDirector` (default name `DirGroup`).
- A group is bound by name: the `SeqAct_Interp` has one variable link per group (the AI group
  "Samuel The Boatman" -> a `DisSeqVar_SpawnerPawn`, its stage mark group
  "Samuel The Boatman_StageMark" -> the spawner actor). The player group has no link: it binds
  to the player.
- Track classes in the Boyle data (counts over the Boyle packages): StretchAnimControl 316,
  LookAt 90, SoireeControl 48, FaceTo 46, FaceFX 44, Locomotion 42, Dialog 37, Attachment 36,
  AkEvent 20, Visibility 15, Event 13, Toggle 12, Move 5, Fade 2, AllowPlayerLook 1. No plain
  AnimControl, Director cuts, Slomo or FloatProp tracks in Boyle (the model reads them).
- Key properties are separate objects (`Properties` / `m_pProperties`), e.g.
  `InterpTrackDialogKeyProperties.m_BlurbGUID` / `m_AltBlurbsGUID` / `WaitForEndOfBlurb`,
  `InterpTrackAttachmentKeyProperties.m_KeyType` (attach / detach) + `m_AttachmentInfo`
  (`BoneName`, `RelativeOffset`, `RelativeRotation`, `bUseRelativeOffset/Rotation`,
  `bHardAttach`), `InterpTrackSoireeControlKeyProperties` (`m_SoireeControlType` loop /
  pause, `m_BreakMode` at next loop / immediately, `m_LoopCount`, `m_PinName`,
  `m_BreakImmediatelyBlendOut` 0.5), `InterpTrackStretchAnimKeyProperties`
  (`m_MatineeRootMotionMode`, `m_MeshTranslationMode`, `m_bDisableCollision`),
  `DisPlayerLookAngleKeyProperties` (yaw / pitch limits).

## 2. Evaluation (CONFIRMED unless marked)

- Curves: UE3 `FInterpCurve::Eval` (value before the first / after the last point; constant
  holds; linear lerps; the curve modes use the cubic Hermite with the stored leave / arrive
  tangents times the gap). Arkane's data stores the tangents, so automatic modes need no
  recomputation.
- `InterpTrackMove` (vtable 0x01029930): `PosTrack`, `EulerTrack` (degrees, X roll, Y pitch,
  Z yaw), `LookupTrack`; `MoveFrame` `IMF_RelativeToInitial` composes the key with the actor's
  transform when the matinee started. Checked on the data: Samuel's boat moves +4450 / +834
  in its own frame, which with its initial yaw (49776 units) ends at (-8163, 3242), 182 uu from
  the "Shore" target point the matinee also uses.
- `InterpTrackStretchAnimControl` key evaluation (0x00905740; key record 0x38 bytes: start,
  name, start offset, end offset, rate, reverse, `SoireeOffsets[6]`, properties): the key
  plays the sequence from its start offset to `length - end offset` (end offsets are often
  negative: the key is longer than the sequence) at `(t - start) * rate`; the sequence's soiree
  markers (`AnimNotify_SoireeAccent/Apex/Loop/End/In/Out`) sit at their time plus the key's
  offset for that marker; between markers the stretched time maps linearly onto the sequence;
  the segment that ends at the End marker loops `round(span / original)` times (at least once)
  instead of stretching; after the last marker the tail maps linearly. Past the end the key
  reports no key (index -1). The track's float curve is the slot channel weight.
- `InterpTrackAnimControl` (plain UE3): looping over the trimmed length or clamped.
- `InterpTrackDialog` (`InterpTrackInstDialog` update 0x00c73750): the last key whose start
  was crossed since the previous update (searching from the end) starts its blurb, with a
  random pick among the main and alternative blurbs when there are alternatives, `t - start`
  seconds in. `WaitForEndOfBlurb` (0x00c735c0): while the blurb plays, the position is held
  0.0001 s before the key's end.
- `InterpTrackAttachment`: target group `m_TargetName`; the last key at or before t attaches
  the target to the owning group's actor at `BoneName` with the relative offset / rotation, or
  detaches it (it stays where it was).
- Director: `InterpTrackDirector` cuts (last cut at or before t; none before the first cut: the
  player's view), `InterpTrackFade` (0 clear .. 1 black; `bPersistFade`), `InterpTrackSlomo`.
- Event / Toggle / Visibility / AkEvent keys fire when crossed (forwards `from < t <= to`).

### Soiree control of the playback position

`SeqAct_Interp::StepInterp` (0x00639700) advances the position, then asks every track
instance whether to hold (`FUN_006193e0`, track-instance slot 77), then handles the end and
the loops (`FUN_006340f0`):
- **Loop keys** (`InterpTrackInstSoireeControl`): crossing a loop key's start makes it the
  current loop (0x009005e0). Passing its end wraps the position back by its length unless its
  pin is broken or its `m_LoopCount` is reached (0 = forever) (0x00900480, wrap loop in
  0x006340f0). A key broken with `BreakImmediately` while inside it jumps just past its end
  (0x00900a40) with `m_BreakImmediatelyBlendOut`.
- **Pause keys** hold the position at their start until broken (the editor preview uses
  `m_PreviewPauseDuration`).
- **Pins**: each key finds the `SeqAct_Interp` input named `SCT_<m_PinName>` (init 0x00906b30);
  an impulse on that input breaks the key. Example: Boyle's "Fade (Start)" matinee holds black
  in its `BlackLoop` (0..0.98 s) until Samuel's intro "Started" output, through a delay and the
  remote event `TerminateFadeLoop`, pulses `SCT_BlackLoop`; the fade-up follows.

### Groups at runtime

- `InterpGroupInstAI` (vtable 0x01028c50) holds its stage mark actor and locks the pawn's AI
  requests; `InterpGroupInstPlayer` (vtable 0x01096fc0) saves the player's start transform and
  hands control to `StatePlayerMasterSoiree` (movement disabled; looking limited by
  `DisInterpTrackAllowPlayerLook`).

## 3. Samuel's intro on Boyle (CONFIRMED from data)

`L_BoyleStreet_Ext_Script` `SeqAct_Interp_5` [13384] "Samuel Boat Intro", data
`Boyle_SamuelBoatIntro` (`MatineeData` [9782], 29.81 s, conversation node GUID set: a soiree).

| Group | Bound to | Tracks |
|---|---|---|
| Sam's_Boat | `SkeletalMeshActorMAT_0` (mesh `SmallBoat.SmallBoat`) | Move (relative, 6 keys, 0..29.05 s), attachments of "Samuel The Boatman" (`Npc_attach`, offset (0, 98, 0), yaw -90°, at 0.50 s), "Boat" (`Boat_Attach`, 0.50 s), "PlayerGroup" (`Player_Attach` 0.50 s, detach 29.64 s); stretched anims Rudder / boat idle / turn; AkEvents motorboat loop 0 s, stop 21.38 s, impact 27.62 s |
| Samuel The Boatman (AI) | Samuel's spawner pawn; stage mark = his spawner | dialog keys 5.68 s (11.33 s), 16.90 s (5.52 s), 22.65 s (5.61 s); look-at keys; stretched body anims (boat idle, waves, point up, head bobs, stop) |
| PlayerGroup (player) | the player; stage mark `MatineePreviewPlayerSpawner_2` | `SC_boat` full-body stretched anim (0..29.77 s), music `LVL_Boatman_Music_03` 0.36 s and stop 24.89 s, look limits yaw ±170°, pitch -15..60° from 0.50 s |
| Events | - | `TallboyTrigger` 5.26 s, `LocationDiscovery` 23.90 s |
| FX_SmallBoat | `Emitter_179` | toggle on 0.55 s, off 21.19 s |
| Boat, Poles, Shore, stage marks | level actors | attachment and look-at targets |

## 4. The port

### Server (deterministic)

- `SeqAct_Interp` started -> `cutscene::started` binds the groups (variable links; spawner
  pawns; the human players for the player group), records each object's initial transform,
  holds the AI pawns (`stay`, idle, no goal), and links the conversation instance whose soiree
  asked for it.
- Every step after the Kismet tick, `Scene::update` at the matinee position: move tracks and
  attachment chains (to the reference-pose bones of the skeletal mesh actors, read at load
  into `Actor::bones`) place level actors and pawns; the player group's players lose movement
  control (look stays) and are carried; locomotion keys walk AI pawns to their target groups
  (brain `WalkToPoint`), face-to keys turn them, visibility keys hide actors.
- Dialog keys speak their blurbs through the conversation runner (`speak_blurb`: the blurb node
  of that GUID plays as a line of its own); soirees in Kismet missions wait for their matinee
  (`external_matinees`) and continue (output 0) when it ends (`matinee_done`), instead of the
  preview chain.
- The interpreter applies the soiree control (loops, pauses, `SCT_` pins, dialog waits) to the
  position.
- Replicated: `MissionView::cutscenes` (data package and export, position, bindings with the
  actors' initial and current transforms, held players).

### Client (presentation)

- `InterpActor` meshes are kept out of the static batches (`map_render` hook) and moved by the
  replicated actor transforms.
- Skeletal mesh actors bound to groups are spawned as props while their matinee plays (loaded
  with the character pipeline) and posed by the group's anim tracks (`AnimBank::sample_parts`).
- AI pawns get the anim tracks of their group as `PawnAnim::scripted` overlays (sequences
  looked up in `GroupAnimSets` first); the talk-gesture stand-in (U-142) is off while a matinee
  drives the body.
- AkEvent tracks post their events on the bound object (2D for the player group).
- Director fade -> black overlay; director cuts -> the game camera at the camera group's actor.

## 5. Open points

U-200.. in `research/UNKNOWNS.md`.
