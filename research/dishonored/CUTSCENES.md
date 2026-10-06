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
  the target to the owning group's actor at `BoneName` (a bone or, in Boyle, a `SkeletalMeshSocket`:
  bone + relative location / rotation) with the relative offset / rotation, or
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

### AI groups, root motion and the stage mark (U-085 / U-202, CONFIRMED unless marked)

Field names from `class_layout` (`InterpGroupInst` +0x38 `Group`, +0x3c `GroupActor`, +0x52
`m_CurrentRootMotionMode`, +0x6c `m_bHasAnimControlTrack` / `m_bShouldBackupTransform`;
`InterpGroupInstAI` +0x7c `AIGroup`, +0x80 `StageMarkActor`, +0x88 `SavedPhysics`, +0x89
`m_MeshTranslationMode`, +0x8c bits `bSavedNoEncroachCheck`, `bAlreadyTerminated`,
`bDisableCollision`, `bHaveLocomotion`, `bHaveMoveTrack`).

- Init (slot 76, 0x006304f0), in game (the guard `DAT_0141b21c` is the editor flag): the base
  init (0x0061a2e0), `bHaveLocomotion` / `bHaveMoveTrack` from the tracks' class flags, then
  0x0062bbc0 (the pawn registers the group instance, vfunc +0x4b4; AI requests named `Gesture`,
  `Posture`, `Distraction` and `Face` are locked; the group's `GroupAnimSets` are added to the
  mesh, vfunc +0x4f4) and 0x00628460(1): a group with a move track saves the pawn's physics
  (+0x88) and switches it to `PHYS_Interpolating` (7); a group with anim tracks saves and sets
  `bNoEncroachCheck`. **The stage mark is not used in game**: only the editor path of the update
  (slot 81, 0x006305c0, when no actor is bound) spawns a preview pawn there. In game an AI group
  plays wherever its pawn stands (soirees and distraction sequences walk the NPC to the mark
  first, in Kismet / the behaviour).
- Update (slot 81, 0x006305c0): clears the per-frame flags, the look-at work (0x009082c0: look-at
  tracks to head / torso / eyes requests at the group's `LookatRequestsPriority`), the base track
  update (0x0062fed0), then the pawn's collision (vfunc +0x508 with `bDisableCollision`) and mesh
  translation mode (vfunc +0x314 with the highest `m_MeshTranslationMode` of the frame's keys).
- Stretched-key track instance update (0x00905c70): on a key change it plays the key's sequence
  on the pawn (vfunc +0x2fc, slot, name, start, ...); each frame it hands the group instance
  the key's `m_bDisableCollision` (+0x148 setter), `m_MeshTranslationMode` (+0x150, max) and,
  when the position advanced, `m_MatineeRootMotionMode` (0x00612e20 -> +0x52). Key properties
  live at the key record's +0x34 (`InterpTrackStretchAnimKeyProperties`: +0x38 root motion
  mode byte, +0x39 mesh translation mode byte, +0x3c bit 0 disable collision).
- The base update (0x0062fed0) resets +0x52 before the tracks run, then gives it to the pawn
  (vfunc +0x30c, `DishonoredNPCPawn` 0x00b4d440): 0 disable -> vfunc +0x758; 1 translate -> the
  locomotion component's root motion when the pawn has one (vfunc +0x4d0, then 0x00946520 and
  vfunc +0x324), else the mesh's root motion mode 0 (`RMM_Translate`) through vfunc +0x754;
  2 accel -> `RMM_Accel` (3). A change away from translate ends it (vfunc +0x32c).
- Mesh translation (0x00b4d520): no change -> clears the pawn's mesh offset flag; disable
  conservative / reset -> vfunc +0x764(0 / 1). Presentation smoothing; not reproduced.
- Term (slot 77, 0x0062bd40, then 0x00628460(0)): physics restored (a pawn back in walking
  physics is lifted 10 uu, 0x006cc4d0), `bNoEncroachCheck` restored, and with
  `SnapToRootBoneLocationWhenFinished` (+0x78 bit 0) the pawn moves to its mesh's root bone
  (0x00718190) plus the cylinder height.

Data (`kismet matinees`, Boyle): root motion is used by the distraction soirees
(`DistractionSoiree.*`: `Empty_Distraction_Piss_In` / `_Out`, `_Railing_`, `_Scavenge01/03_`,
`_PlayingPiano_`, `_TableCleaning_`, `_WashHands_` In and Out keys translate, their loops do not)
and the campfire guard (`Sc_Campfire_In` / `_Out`). Every Boyle AI key has mesh translation mode
1 (the Outsider's floats: 2). `Empty_Distraction_Piss_In`'s root (`root0_jnt`, mesh +Z, the
NPC's forward) travels 0 -> 72 uu, `_Out` 0 -> -72: the guard steps up to the wall and back.

Port:
- `interp::Scene::root_motion` sums, over the group's anim tracks, the root bone translation of
  keys with a root motion mode between two positions (the old key to its end and the new one
  from its start when a key change is crossed; loop wraps and backward steps add nothing),
  times the track weight. `Scene::root_offset` gives the final pose's root for the snap.
- The server bakes the root bone tracks (Edge joint 0, 30 Hz) of every sequence AI groups play
  when the Kismet loads (`interp_bake.rs`; the group's sets, then every AnimSet of the loaded
  packages holding the name) and picks the pawn's sets at run time (`Look::anim` profile, last
  set first). Root motion moves the pawn horizontally (mesh rotation `RotOrigin` (0, -16384,
  16384): mesh +Z is the pawn's +X); walking physics keeps it on the floor. APPROXIMATION: the
  move ignores collision (the original moves through physics).
- Clients drop the root bone's translation of such keys (the mesh keeps the root at its bind
  translation, as root motion extraction does).
- Not reproduced: mesh translation modes, `bDisableCollision`, accel versus translate (both
  move by the translation).

### Attachments to animated bones (U-200, CONFIRMED)

The attachment parent's bone is posed by its group's anim tracks: Samuel's boat
(`SkeletalMeshActorMAT_0`, `SmallBoat`) rocks on `Boat_Jnt` with `SC_boat_Idle` (1.26 degrees),
and the hull `InterpActor_1` is hard-attached to that bone in the level (`Base`
`SkeletalMeshActorMAT_0`, `BaseBoneName` `Boat_Jnt`, `bHardAttach`), as are the matinee's
attachments (`Npc_attach`, `Boat_Attach`, `Player_Attach` sockets on `Boat_Jnt`).

Port: the Kismet load bakes, for every attachment bone or socket of a skeletal mesh actor whose
group has anim tracks, its actor-space transform over the matinee at 30 Hz with the pose the
presentation builds (tracks in order, each blended over the previous with its weight, Edge
joints in the mesh's bone order). `Scene::xform` uses it before the reference pose. Samuel's
sockets move by up to 5 uu (`Player_Attach` z 143.2..146.0 in actor space). The hull's mover
collision follows the `Boat` group's pose, which now rocks with the bone like the original's
hard attachment; the player released at 29.64 s stands on it.

### The player group's camera (U-203)

`InterpGroupInstPlayer` init (slot 76, 0x009027f0) saves the player's location and controller
rotation (`m_StartPos` / `m_StartRot`) and calls the pawn's vfunc +0x4ac (`DishonoredPlayerPawn`
0x00aab570): the soiree state is entered with flags from the group (+0x7c bit 2 -> 2, bit 5 and
a camera test -> 0x20, ...). The first-person camera of the player is the mesh's camera bone
(`DisTweaks_PlayerPawn_Camera.m_CameraBoneName`; `DishonoredNotify_CameraPitchTarget`
defaults to `camera_jnt`), so a full-body sequence in the player group's `PlayerStack` moves
the view. `SC_boat` (`Ply_SC_Generic`, 2.67 s, a still pose stretched over the ride) seats Corvo:
`camera_jnt` sits 50 uu back, 11 uu right and 59 uu lower than in `Sword_Ready_Idle`, looking
13 degrees right of the pawn's forward (`fp_probe`).

The soiree state's enter (0x00a66ff0, reached through 0x00a696e0 / 0x00a74fa0) blends both
hands' items to weight 1 (0x00aa1cf0 -> 0x00ac49a0 for hands 1 and 2; back to 0 at the exit,
0x00a628f0), masks inputs (0x00ab1160 with 0x8002), saves and clears the collision flags with
`m_bDisablePlayerCollision` (0x0058a4a0) and switches the pawn to `PHYS_Interpolating` (vfunc
+0x218 with 7). The port hides the first-person rig while the local player is held (LIKELY: the
weight read as "put away"). `InterpGroupPlayer` also has `m_bPutPlayerInWorldDepthGroup`
(+0x7c bit 3, applied at init through 0x00aa7c20) and `m_bDisplayInFirst` (bit 4).

Port (LIKELY): while a player group holds the local player and plays full-body keys, the client
samples them on `Ply_Player.Skm_Player` and moves the eye by the camera bone's offset from the
standing idle pose, in the pawn's matinee frame (the boat socket). The view direction stays the
player's, clamped by the `DisInterpTrackAllowPlayerLook` key (yaw -170..170 around the frame's
yaw plus the camera bone's yaw, pitch -15..60). Not reproduced: the view rocking with the
boat, the blend to the stage mark, and the end transform (`m_StartPos`, snap flags).

### Toggle and visibility tracks (U-208)

Data (`kismet matinees`): the distraction soirees hide and show props in the NPC's hands
(`Glass`, `Cigarette`, `Lighter`, `FeatherDuster`, `Book`, `Rag`, the Boyle letters and notes:
`SkeletalMeshActor`s, attached by the AI group's attachment tracks to `RightHandWpn` /
`LeftHandWpn`) and toggle emitters (`cigarette_smoke` on, off, on, off; `cigarette_puf`,
`FX_Lighter_Sparks`, `PukeFX` triggers; `FX_Piss` on / off). Emitter class defaults set
`bHardAttach`.

Port: skeletal props start with the actor's `bHidden` and follow the visibility keys up to the
position (hide, show, toggle); props and emitters attached to a pawn's bone or socket are placed
on the pawn's animated bone (`PawnPoints`, filled by `characters::animate_characters` from the
rendered pose) times the key's offset, and stay where they were when detached. Toggle keys start
(on), stop (off: no new particles) or flip placed emitters of the level; trigger keys fire one
burst. Not reproduced: emitters based on another actor are not imported by the level loader,
e.g. `FX_SmallBoat`'s `Emitter_179` (`SmallBoat_WaterTrail_01`), whose hard attachment to the
hull keeps the relative location (7660, 8816, 2116) it was given in the editor: as cooked, the
wake would play about 11.7 km from the boat (U-541).

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
  attachment chains (to the bones and sockets of the skeletal mesh actors as their group's anim
  tracks pose them, baked at load into `Matinee::bone_tracks`, else the reference pose in
  `Actor::bones`) place level actors and pawns; the player group's players lose movement
  control (look stays) and are carried; AI pawns take the root motion of their keys
  (`Matinee::root_tracks`); locomotion keys walk AI pawns to their target groups
  (brain `WalkToPoint`), face-to keys turn them, visibility keys hide actors;
  `SnapToRootBoneLocationWhenFinished` moves the pawn to its final root when the matinee ends.
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
  looked up in `GroupAnimSets` first; root-motion keys keep the root bone at its bind
  translation). They are the only body gestures of a conversation (U-142).
- Props follow visibility tracks and, when attached to a pawn's bone or socket, the pawn's
  animated bone; toggle tracks start, stop and fire placed emitters.
- The player group's full-body keys move the first-person eye; its look-limit keys clamp the
  view.
- AkEvent tracks post their events on the bound object (2D for the player group).
- Director fade -> black overlay; director cuts -> the game camera at the camera group's actor.

## 5. Open points

U-200.. in `research/UNKNOWNS.md`.
