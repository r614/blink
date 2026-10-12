# NPC and third-person animation in Dishonored

How the original picks the animation an NPC plays, and how the port reproduces it for NPCs and
remote players. Executable: `Dishonored.exe` SHA-256 `70da9613…` (addresses below). Retail data
is read at runtime; nothing here is copied into the repository.

Implementation:
- data: `crates/dishonored_ir/src/npc_anim.rs` (types), `crates/dishonored_assets/src/npc_anim.rs`
  (decoders), `crates/dishonored_assets/src/characters.rs::resolve_appearances` (per-tweak profile
  and material variations);
- simulation: `crates/dishonored_sim/src/anim.rs` (the layers, the requests, the notifies, the
  NPC trees stepped with `crates/dishonored_anim`);
- runtime (draws the simulation's state, research/ARCHITECTURE.md step 7):
  `crates/dishonored_runtime/src/npc_anim.rs` (bank, the evaluator's clips, `pose_tree`),
  `characters.rs::animate_characters`, `anim_notify.rs` (the notifies), `viewmodel.rs`;
- tool: `npc_anim_dump defs <Class> | loco <pkg> <path> | sets <pkg> <set>... | names <pkg> <export> <prop>`.

## 1. What a pawn tweak says

`DisTweaks_Pawn` (resolved through the tweak fallback chain, like the looks):

| Field | Meaning | Boyle examples |
|---|---|---|
| `m_AnimSets` | AnimSets bound to the mesh, in order | guards: 16 sets (`Npc_EliteGuard_Generic/Tracking/empty/Sword/Face/Gun/Corpses/Distractions/Gestures*/Postures/Blink_as`); party aristocrats: the guard sets minus Sword and Gun, then `Npc_CivTall_Empty_as` and `Npc_Aristo_Empty_as`; women (`Twk_Pawn_Boyle_AristoF*`, servants, the Boyle sisters): the 14 `Npc_CivFemale_*` sets (`Npc_LadyBoyle_DramaticDeath_as` added for Esma) |
| `m_pAnimTreeTemplate` / `m_pAnimTreeWhenDeadTemplate` | AnimTree | everyone: `Npc_Common_at.NPC_Human_at` / `NPC_Dead_at` (Startup) |
| `m_pAnimDefClass` | `DisAnimDefinitions` subclass: the one-shot chains | `DisNPCAnimDefinitions_EliteGuard` (`_NoPuke`), `_Civilian` (`_NoPuke`), `_MusicalOverseer`, `_Weeper`, `_Tallboy` |
| `DisTweaks_NPCPawn.m_pComponentLocomotionConfig` | `ArkComponentLocomotionConfig`: locomotion loops | guards and male aristocrats: `DisLocomotionConfig.EliteGuardLocomotionConfig`; women: `FemaleLocomotionConfig`; weepers and tallboys have their own |
| `m_bRagdollOnDeath` | ragdoll at once on death | false for every Boyle tweak |

**Sequence lookup.** UE3 `USkeletalMeshComponent::FindAnimSequence` searches `AnimSets` from the
last to the first, so a later set overrides a name. This matters: `Npc_Aristo_Empty_as` holds only
`Empty_WalkN`, the aristocrats' own forward walk, replacing the guard's (CONFIRMED by the data; the
search order is stock UE3).

**Edge joint order.** Each set's Edge joints follow the set's tracks in the bone order of the
skeleton it was compressed against (EDGE_ANIM.md). The port takes the locomotion config's
`m_SkeletalRefMeshName` (e.g. `Npc_EliteGuard.Mesh.Skm_EliteGuard_Body`) as that skeleton when the
mesh is loaded, else the animated body's own skeleton (`AnimBank::binding`).

## 2. The AnimTree (`Npc_Common_at.NPC_Human_at`)

Node graph (from the cooked tree, Startup.upk), root first:

```
AnimNodeBlendByPhysics "NPCPhysicsBlend"   (PHYS_RigidBody -> a sequence node; everything else ->)
 DisAnimNodeSlot "AdditiveReactSlot"        (additive hit reactions, e.g. ADD_Generic_SmallHit_*)
  AnimNodeAdditiveBlending
   base: ArkAnimNodeBlendPose "PoseBlender"
          ArkAnimNodeStack "MatineeStackNode" (6 matinee channels)
           AnimNodeBlendPerBone "FaceFilter" (branch head_jnt)
            source: ArkAnimNodeStack "BlinkBlender" (eyelid blink sequence)
                    AnimNodeBlendPerBone "UpperBodyFilter" (branch spine_0_jnt)
                     source: DisAnimNodeSlot "FullBodySlot" (3 channels)
                              ArkAnimNodeLocomotion "LocoBlender" (8 speed-mode slots + "Specific anim")
                     target: DisAnimNodeSlot "UpperBodySlot" (3 channels)
            target: AnimNodeSlot "FaceSlot"
   additive: ArkAnimNodeLookAt "LookatBlender" (ADD_Empty_AimHead*/AimTorso* 3x3 grids)
 skeletal controls: DisSkelControl_FootPlacement (both feet), SpineBender, eye controls
```

`NPC_Dead_at` is only `AnimNodeBlendByPhysics` -> one `DisAnimNodeSlot` "FullBodySlot".

The loco sequence nodes are in sync group `LocoGroup`; slots 1-4 have `Rate = -1` in the template.

So an NPC is: locomotion loops, overridden by full-body actions, with upper-body actions from
`spine_0_jnt` up, additive hit reactions and look-at on top. The port evaluates this tree
(`dishonored_anim`) in the simulation, which steps every NPC's state from its requests
(`dishonored_sim::anim`), and the renderer poses that state per part of the pawn's model
(`npc_anim::pose_tree`, 2026-10-11); foot placement and the additive small hit reactions are not
reproduced (U-280), and nothing requests the blink and face slots yet. Matinee anim tracks are
the `MatineeStackNode` channels the renderer sets on its copy of the state (`PawnAnim::scripted`,
`TreeState::present`); keys with `m_MatineeRootMotionMode` keep `root0_jnt` at its bind
translation because their root translation moves the pawn on the server (CUTSCENES.md, U-085 /
U-202).

Node details read from the cooked tree:
- `FaceFilter` has `m_bExcludeBranchBones`: the face slot drives the bones *below* `head_jnt`,
  not `head_jnt` itself.
- `BlinkBlender` channel 1 is an `AnimNodeSequence` playing `Blink` (`m_bDontUseRefPoseOnNonDefinedBones`:
  only the eyelid tracks). The node is cached by 0x00956660, which sets the channel weight to 1
  and stops the sequence; the timing comes from the pawn's `ArkComponentLookatConfig` (§9).
- `DisSkelControl_FootPlacement` (both feet): `bOnlyEnableForUpAdjustment`, `FootOffset` 5,
  `m_fFootRadius` 15, `m_fMaxPredictDist` 150, `m_fFootZMoveSpeed` 200, effector on
  `lower_leg_*_jnt`. It raises a foot onto ground found by traces (stairs, slopes); not ported
  (U-280). Decode so far (vtable 0x0110eba0): the bone update (slot 79, 0x00a9b360) takes Arkane's
  "new IK" (0x00a95190) when the control's +0x138 bit 25 is set, bit 0 clear, the AI level
  object exists (0x00bbf0b0) and its +0x3d4 bit 5 is set (`DefaultAILocomotion.ini`:
  `m_bDisableNewIK=FALSE`), else the UE3-style path (slot 89, 0x00a8a810: one trace along the
  leg from above the animated foot, the adjustment clamped to the max up / down values, skipped
  when negative with `bOnlyEnableForUpAdjustment`, a lock within `m_fLockMaxDistance`, the
  two-bone limb solve and the foot oriented to the hit normal within the reject angle). The new
  IK keeps the owner's location, mesh offset and yaw (+0x148.., +0x160.., +0x184), runs the
  ground tests and their blend (0x00a83a80: in / out over +0x3a4 / +0x3a8 seconds into the
  +0x358 weight), the prediction from the locomotion's foot data (0x00a92ee0) and the solve
  (0x00a937e0, 800 lines) with the `GroundTestResults[6]` array and the flat / stairs / slope
  shapes. The renderer can now trace the level (the ragdoll's floor uses
  `CollisionWorld::raycast`), so the remaining work is decoding 0x00a92ee0 and 0x00a937e0.

## 3. Locomotion (`ArkComponentLocomotionConfig`, `ArkAnimNodeLocomotion`)

Config layout (decoded by `read_loco_config`):
- `m_Modifiers[]`: stances with acceleration, deceleration and `m_fSpeedMultiplier`. Elite guard
  config: `Empty, SearchEmpty, PanicEmpty, PanicRecoverEmpty, EmptyPatrol, Sword, SearchSword,
  PanicSword, PanicRecoverSword, Aim, SwordPatrol, ExplosionEscape_Empty, ExplosionEscape_Equipped,
  MeleeRush, MusicBox`. Female config: `Empty, SearchEmpty, PanicEmpty, PanicRecoverEmpty`.
- `m_SpeedNames` (guard: Idle, Walk, Jog, Run; female inherits the class default Idle, Walk, Run
  from `Engine.Default__ArkComponentLocomotionConfig`).
- `m_SpeedModes`: `modifier x speed -> index` into `m_SpeedModePropDictionary`.
- Each speed mode: `m_MoveAnimProps` (sequence, authored ground speed). Moving modes list four
  directions in the order **S, W, N, E**; idle modes one loop at speed 0. E.g. guard `EmptyWalk`
  = `Empty_WalkS_Revert` 131, `Empty_WalkW_Revert` 124, `Empty_WalkN` 157.7, `Empty_WalkE` 125.8;
  `SwordRun` N 809; `PanicEmptyRun` N = `Empty_RunN_Panic1` 503.5. Also start, stop and turn
  sequences, and `m_Transitions` (sequence played when the modifier changes, e.g.
  `Sword_SwordToPatrol` from Sword to SwordPatrol).

`ArkAnimNodeLocomotion` (vtable 0x0109DCB8; TickAnim slot 85 = 0x0091B280, which calls the
steps below in this order: 0x0090BC00, 0x00911230, 0x0090BDB0, 0x00916770, 0x00916950,
0x00917440, 0x009114A0, 0x009113A0). Field names from `class_layout`:
- 0x0090BC00, velocity smoothing: the node's polar velocity (`m_SmoothedAngle` +0x200, a
  rotator int, `m_fSmoothedSpeed` +0x204) moves toward the pawn's (`m_Angle` +0x1F4, `m_fSpeed`
  +0x1F8) in the plane: with `d` the distance between the two velocity vectors, the step is
  `min(1, m_fVelocityMaxBlendSpeed * clamp(d / m_fVelocityMaxDif, m_fVelocityMinBlendSpeedRatio, 1)
  * dt / d)` of the way. Class defaults (`Engine.Default__ArkAnimNodeLocomotion`, not
  overridden by `NPC_Human_at`): 2000, 100, 0.1. CONFIRMED.
- 0x009331D0 maps the angle to the two neighbouring direction entries and weights (octants;
  with n anims the step is 8/n octants; a weight at or above 0.98 takes everything). Angle 0 is N.
- 0x00916770: crossfades the two modifier slots over `m_fModifierBlendDuration` (0.5 s), then
  sets `m_fOutOfShapeRatio` (+0x20C) = 0x00910E30 and `m_fSmoothedSpeedWithBlend` (+0x208) =
  ratio * smoothed speed, and runs 0x009105B0 on it for both slots.
- 0x00910E30, diagonal correction: for the bracketing mode of the larger weight (the other one
  when its speed is below 0.1) with more than one loop, and two direction entries in use, the
  blended stride is the chord between the two octant unit vectors (table at 0x014527C0); the
  ratio is 1 / the distance from the centre to the chord along the movement direction, i.e.
  `cos(phi) + sin(phi)` for neighbours 90 degrees apart (1.414 on the diagonal). CONFIRMED.
- 0x009105B0 finds the two speed modes of the modifier whose direction-blended authored speeds
  bracket +0x208 and weights them linearly; weights within 0.98 snap.
- 0x00910970 sets each playing sequence's weight; **entries whose octant is below 4 (S and W)
  play with rate -1** when the mode has more than one loop: the `_Revert` loops are authored
  reversed. It returns `authored speed * sequence length * weight`.
- 0x00916950, sync-group rate (`m_fMostRelevantRateScale` +0x21C, sent to the `LocoGroup` with
  0x0059BBE0): `master length * speed(+0x208) / sum(authored speed * length * weight)` times the
  modifier's `m_fSpeedMultiplier` (e.g. 0.92 for `EmptyPatrol`, 0.8 `SearchEmpty`) and the node's
  `m_fSpeedMultiplier` (1); 1 when the speed or the sum is below 0.1. The master is the heaviest
  sequence (UE3 sync groups: it alone fires notifies).
- 0x00911230 starts a `m_Transitions` sequence in the "Specific anim" slot when the main slot's
  modifier differs from the requested one, its weight is 1 and the smoothed speed is below the
  config's `m_fVelocityToStopMoving` (config +0x94, class default 20). 0x0090BDB0 blends it out
  over `m_fTransitionBlendDuration` (0.1).
- 0x009114A0 sets `m_bFootOnGround` per `m_Feet` entry from the sync position (foot placement).
- Start, stop and turn sequences (`m_StartAnimProps`, `m_StopAnimProps`, `m_TurnAnimProps`, each
  with `m_fStraightDist`, `m_fMoveAnimIn/Out`, `m_fCutIfMoving`) are not chosen by the node: the
  locomotion component requests them through the same "Specific anim" slot
  (`m_bSpecificAnimRequested`) while following its path; blends `m_fStartBlendDuration` /
  `m_fStopBlendDuration` 0.1, `m_fTurnFastBlendDuration` 0.15 (tree) / `m_fTurnSlowBlendDuration`
  0.155. Only the idle modes have start / stop props (guard `EmptyIdle`: `Empty_Walk_Start`,
  `Empty_Walk_StopLeft` / `StopRight`); every mode has turns (`Empty_TurnLeft180` ...,
  `Empty_TurnToWalkRight45` ...).

Port (`dishonored_anim::locomotion`, ticked by the simulation's tree): all of the above except the
turn sequences. The pawn's velocity relative to its facing is smoothed with the node's
parameters; directions blend linearly between the two neighbouring loops with the 0.98 snap; the
diagonal ratio scales the speed used for the bracket and the rate; S and W play backwards; all
loops share one normalized phase advanced at the sync-group rate; modifier changes crossfade over
0.5 s and transitions wait for the speed to drop below `m_fVelocityToStopMoving`. The runtime's
speed-crossing trigger went with its picker (2026-10-11, ARCHITECTURE.md step 7): the renderer
draws the simulation's tree.

The locomotion component's requests (2026-10-11, static decode; `dishonored_sim::npc_loco`, the
simulation's tree through `LocoInput::specific`). The component (+0x218 state: 0 none, 1-5 turn,
6-9 start, 10-14 stop) runs in the per-tick 0x0094BEC0, after the pose barrier: the turn machine
0x0094B9D0 with its triggers, the start rule 0x00946F60, the stop rule 0x00947300, in that order,
on the path of the brain's request (the point `P[k]`, the segment, the destination, the accurate
flag of the request record +0x24):

- **Start** (CONFIRMED): speed below `m_fVelocityToStopMoving` (20, not the start speed), the yaw
  within 22.5 deg of the segment, the point ahead and farther than the start prop's
  `m_fStraightDist` (70), not arrived, nothing playing: `m_StartAnimProps[0]` (only the first).
- **Stop** (CONFIRMED): a non-accurate request, the idle mode with stop props, the yaw within 22.5
  deg of the segment; within `m_fUnaccuracyRadius` (120) of the destination the window opens;
  when the move loops' sync position crosses a stop prop's `m_fMoveAnimIn` (StopLeft 0.488,
  StopRight 0.0: the foot is the phase; `m_bFootOnGround` is not read) the stop is asked unless
  `|r - s| <= |r|` with `r = d - m_fStraightDist` and `s` the walk loop's length times its speed
  over the stop count (the next foot lands nearer). The request arrives only after the stop
  (0x00944390: within 120 and below 20 in states 0 / 5); an accurate one never stop-animates and
  arrives within 0.1 uu.
- **Turn** (CONFIRMED): static (below `m_fMaxSpeedForStaticTurn` 40, on a moving path more than
  `m_fMaxDistToPathEndToTurn` from its end, the path `m_fMaxDistToFirstPathTurn` ahead within 90
  deg) or moving (40 or more, the segment off the yaw), both over 15 deg; `RequestTurn`
  (0x0094AF40) takes the speed mode bracketing the speed, the prop by 45-degree buckets (left
  even, right odd; half-width 22.5, 45 with two props), the `TurnToWalk` variant (+8) on a moving
  path whose point is farther than its `m_fStraightDist`, played to `m_fCutIfMoving` of its length;
  `RootRotationScale` = angle / nominal (180, 135, 90, 45); the mesh goes to (`RMM_Accel`,
  `RMRM_RotateActor`) and is not restored by the component.
- **Node side** (0x0090BDB0, 0x00917440): the request plays from the next update at the
  modifier's times the node's `m_fSpeedMultiplier`, blends in by `max(0.01, t x Rate / Blend)` and
  out over the last blend time before its effective end (start / stop 0.1, turn fast 0.15 / slow
  0.155 from `NPC_Human_at` over the class defaults: `NpcAnimConfig::loco_node`); "fully in" is
  the move loops' weight at 0.
- **Movement** (CONFIRMED): root-motion driven only while fully in (and in states 4, 8, 13, 14):
  the sequence's root translation over `dt` times the modifier's multiplier, as the chains'.
  Otherwise the path following's speed per state (turn 1-3 the turn's mode speed, 0 for an idle
  mode; start 6-8 and stop 13 zero; stop 10-12 the walk mode's forward speed) and yaw (turn 1-3
  the from-yaw, 4-5 the to-yaw; stop held; otherwise toward the point).
- Not decoded (U-981): the static turn without a path, the corner turn, the rotation step,
  braking toward the stop, the loops' re-seat at a blend-out; who applies the turn's root yaw
  (the port turns the pawn by it while fully in, LIKELY). Every NPC counts as full LOD (the
  component's +0x1AC bit 0 is the AI LOD flag, U-980).

Footsteps: the NPC loops carry no `DishonoredNotify_Footfall` (e.g. `Empty_WalkN` has no
notifies at all). The node instead keeps `m_bFootOnGround` per `m_Feet` entry (0x009114A0): a
foot is planted while the circular distance between the group's sync position and the foot's
`m_fOnGroundPct` (class defaults: left 0.44, right 0.94; Boyle configs keep them) is at most half
the weighted `m_fFootOnGroundPct` of the moving speed modes (guard walk 0.4, run 0.2; normalized
when their weights sum below 1). The port evaluates the same as an output of the simulation's
tree (`TreeState::feet_on_ground`, against the config the node ticked with) and the renderer
reports each foot touching down to `AnimSounds::foot_down`, which plays a footfall like the
notify path (0.25 s per foot), so third-person NPC steps do not use the per-stride fallback
(LIKELY: the consumer of `m_bFootOnGround` that posts the original's NPC footstep was not
traced). A full-body action at full weight leaves the loops irrelevant: no foot is planted then.
Notifies on the loops are not fired (the tree dispatches none; the simulation's layers fire
theirs as `SimEvent::AnimNotify`).

## 4. Animation definitions (`DisAnimDefinitions`)

`m_Animations` is indexed by `eDisNPCAnimations` (138 entries, `dishonored_ir::npc_anim::NPC_ANIM_NAMES`).
Each entry is a `DisAnimationChain`: alternative `m_Groups` (one is picked); each group a chain of
`DisAnimation` steps (`m_Name` alternatives, `m_bLoop`, `m_fBlendInTime` default 0.2,
`m_FirstItemPrefix`), `m_fBlendOutTime` (0.1) and `m_bEndChainAfterOneAnimation`. Steps with an
item prefix get the held item's `m_ItemPrefix`: the pawn tweak's primary item
(`m_ContentInventoryLoadout[0]`: elite guard sword `Sword_`, music box `MusicBox_`, wolfhound bite
`Patrol_`, Tallboy bow `Empty_`) or its empty hands (`m_pEmptyHandsItem`: `Empty_`); a pawn
without a primary item (weepers) uses its empty hands' (`AnimProfile::item_prefix` /
`empty_prefix`). The subclasses' default objects
are cooked into the level script packages (`DishonoredGameContent.Default__DisNPCAnimDefinitions_*`),
the base class defaults into Startup; a default object without `m_Animations` inherits its super
class's.

Examples (EliteGuard): `Death` = one end-after-one group of 14 directional entries
(`Generic_Death{Front,Left,Right,Back}A` arranged Front, FrontLeft, FrontRight, Left, Right, Back,
BackLeft, BackRight, then six more); `DeathFast` = `Generic_DeathFast_*` with `_DownUp` variants;
`HitReactStrong` = `<Sword|Empty>_Hit{Front,Left,Right,Back}_A`; `AttackShort` = seven groups
(`Attack_ShortSmallA..E`, `Attack_FastA/B` each followed by a `Big` step); `Parry` = normal / win /
lose groups of four (`Ready_Parry_RightRight` ...); `Parry_AwaitingImpact` = `Ready_ParryIdle`
loop; `Stun` = `Versus_Stun_In -> Stun_Loop (loop) -> RecoveryA|B`; `Beg` = kneel in, loop, out;
`Panic` = `<prefix>Panic`; `AmbientIdle` = two groups (`Empty_Idle_01..10`, `Sword_Patrol_Idle_01..10`);
`Assassination` = `Generic_Assassination_{Front,Left,Right,Back}_Slave` (+ carry-corpse variants).
Civilians differ by surprise anims (`Surprised_*` instead of `SurprisedDraw_*`), fewer attacks.

**Ambient idles** (`DisAISubProcessAmbientAnims`, Tick 0x00B39620): a timer starts at
`rand * max` and counts down while the NPC stands still (`m_bCheckForMovement`); on expiry the
`m_eAnimToPlay` chain (default `AmbientIdle`) is asked for the group of the held item, the number of
its leading steps that exist in the pawn's sets is counted, one of them is picked at random, and
it plays full body (`m_bPlayFullBodyAnimation`) or upper body; then the timer is re-rolled in
`[m_fMinAmbientAnimFrequency, m_fMaxAmbientAnimFrequency]` (class defaults 2 and 6 s). Guards have
`Empty_Idle_01..08` and `Sword_Patrol_Idle_01..03`; `_09`/`_10` do not exist, which is why the
count of available leading steps matters.

Which NPCs run it (U-145, resolved from the behaviour tweaks): the subprocess belongs to
behaviours, not brains. `DisTweaks_AISubProcess_AmbientAnims` instances exist only under
`TweaksBehavior_Patrol_Default` / `_Aggressive` and `TweaksBehavior_Guard_Default` /
`_Aggressive` (30-60 s, upper body, `m_bCheckForMovement` false: they also play while walking),
`TwkBehavior_StationarySearch_Default` (class defaults: 2-6 s, full body, standing only), the
weeper, tallboy and musical overseer patrols. Idle, wander and group-idle behaviours have none, so
relaxed civilians do not play ambient idles.

The request (0x00B39620, decoded 2026-10-12): the subprocess rolls the CRT's global `rand()` (the
first timer `rand()/32767 x max`, the next `min + rand() x (max - min)/32767`, the variant
`(int)(rand() x count / 32768)`), counts down while still or with the movement check off, picks
group 1 when a hand holds an item, else 0, and builds a pawn-state request: `StateNPCMasterPlayAnim`
(full body) with `m_bPlayFullBodyAnimation` or when standing, `StateNPCUpperPlayAnim`
(`UpperBodySlot`) otherwise; a refused request retries every tick, and the subprocess's stop
exits the play state. Port: the brain's head runs the timer and the pick (`dishonored_ai::head`,
eligible on routine while still; its CRT stream per head) and the pawn's request carries the pick
(`Request::ambient`), which the simulation plays through PlayAnim (`pawn_states::take_request`).

Face sequences: every `DisAnimation` step has `m_FaceAnimName` (e.g. `Sword_DodgeIn_Face`); it
plays in `FaceSlot` with the step (decoded into `AnimStep::face`, played on the bones below
`head_jnt`). Choke and carry steps name the player's sequence they are synced with in
`m_LinkedPlayerAnimName` (`Generic_Choke_Loop_Slave` <-> `Sword_Choke_Loop_Master`).

## 5. Per-NPC material variations (0x00B7F8A0, 0x00B76390, 0x00B7B0D0)

At spawn the pawn (0x00B7F8A0):
1. picks a head from `m_RandomHeadMeshes` (0x00B76390): `r = rand()/32767 * total chance`, first
   entry whose running sum exceeds it; the index is saved as `m_iRandomHeadMeshSel` (+0xA60);
2. applies that head's `m_MaterialVariation` and the tweak's `m_BodyVariationMaterials` (0x00B7B0D0):
   for every material slot listed, one `rand()/32767` roll, the first option whose running sum
   of `m_fRandomChance` exceeds the roll replaces the slot's material; **chances are not
   normalized**, so a lone 0.5 option applies half the time. The overrides are kept in
   `m_HeadMeshMaterialsOverride` (+0xA64) and the body list (+0xA70) for saves (0x00AC9770,
   0x00AD3E10);
3. rolls each accessory type (hat, mask) against `m_RandomAccessoryInfo[i].m_fRandomChanceOfUse`
   (`rand() * 3.05185e-05 < chance`), picks one of `m_HatMeshes` / `m_MaskMeshes` with 0x00B764B0
   (weighted by `m_fRandomChance`, the head rule), sets it as the type's static mesh component
   (vfunc +0x2A8) and attaches it to `m_SocketName` (0x00746200). CONFIRMED.

Armour: `m_PotentialNPCArmor` (`m_fPercentChance`, `DisTweaks_Armor`) is equipped at spawn; the
armour is drawn as its dropped-item movable tweak's `m_pStaticMesh` on `m_EquippedSocketName`
(LIKELY: the equip code was not traced). Boyle: the `*_Helm1_*` elite guards wear
`Npc_EliteGuard.Mesh.Stm_EliteHelmet` (`Twk_Inv_EliteHelmet` -> `movable_twk.EliteHelmet_twk`) on
the body's `Helmet` socket with chance 1. No Boyle tweak lists hats or masks there: party masks
are the `m_RandomHeadMeshes` themselves.

Port: `Appearance::materials(seed)` (same rule, deterministic per pawn id) feeds `Look::materials`,
which the renderer already applies through `load_model_with`. Elite guards get
`EliteGuardHead_v2_inst` / `EliteGuardHead02_v2_inst` half the time. Accessories
(`Appearance::accessories(seed)`, hats, masks and armour, rolled per pawn) ride in
`Look::accessories`; the renderer loads each static mesh as a one-bone model
(`load_static_model`, the level material path) and places it every frame at the socket of the
look's body or head mesh (`SkeletalMeshSocket`: bone, `RelativeLocation`, `RelativeRotation`).
Cache: `<map>.appearance3.json`.

## 6. What drives selection in the port

The simulation picks (`dishonored_sim::anim`, since 2026-10-11): its states request the layers'
sequences and its NPC trees take the requests, the physics, the velocity and the brain's
awareness; the renderer draws that state (`SessionView::anim`) and plays the notifies the
simulation fires (`SessionView::anim_events`, `anim_notify`), choosing nothing. The host still
publishes `MatchView::npc_poses` (`NpcStance` from the brain state), `MatchView::npc_heads` (the
look-at aims the renderer sets on the tree) and `MatchView::anim_profiles` + `Look::anim` (the
pawn tweak's sets, locomotion config and definitions class: the sets the renderer resolves the
simulation's sequence names in).

| Situation | Original mechanism | Port |
|---|---|---|
| walking / running | locomotion modifier and speed modes | §3, the simulation's tree; modifier from the brain's awareness (`dishonored_sim::anim::modifier`): guards `SwordPatrol` when unaware, `SearchSword` surprised or suspicious, `Sword` in combat, `PanicSword` fearful or begging; civilians `Empty`, `SearchEmpty`, `PanicEmpty`; `Aim` in `StateNPCMasterAim`; one profile's config (the doorman's) for every NPC (U-978). The decoded body stance -> modifier table (0x00B71000, U-892 below) is not wired: the brain publishes no body stance |
| modifier change | `m_Transitions` (node side, 0x00911230) | the node's own rule in the simulation's tree (`dishonored_anim::locomotion`) |
| standing still / patrolling | `AmbientIdle` subprocess of the behaviour (0x00B39620: a `StateNPCMasterPlayAnim` request, full body standing, `StateNPCUpperPlayAnim` moving with the upper-body flag) | the brain's head picks it (`dishonored_ai::head`, CRT `rand()` stream per head) and requests it (`Request::ambient`: group, step, serial, age); the simulation plays it through `StateNPCMasterPlayAnim` on `FullBodySlot` with its `<sequence>_Face` on `FaceSlot`; dropping it (moving, off routine) exits the state (LIKELY: the subprocess's stop) |
| melee swing (`SwordSwing`) | `AttackShort/Medium/Long` chains from the AI attack pattern | the simulation's `StateNPCMasterActionMelee` (`dishonored_sim::pawn_states`): the context's chain (`AttackShort`, `AttackStepBack`, `AttackMedium`, `AttackLong`, `JumpAttack`), random group, played whole with its root motion on `FullBodySlot` |
| side / back step, GenericActions | `DisItemContext_NPCSideStep` / `BackStep`, the behaviours' `PlayAnim` requests | `StateNPCMasterPlayAnim` in the simulation (`SideStep` group by side, `JumpBack`; the behaviour's chain); `MatchView::npc_actions` from the pawn states |
| start / stop / turn in place | `ArkComponentLocomotion` from the path (section 3) | the simulation's `npc_loco` (rules decoded) in the tree's "Specific anim" child |
| blocking | `Parry_AwaitingImpact` | not requested (the NPC block context's animation was not decoded) |
| parry (`SwordParried`) | `Parry` groups | the parried attacker's `Parry` group 2 (`StateNPCActionParry`); the defender's won parry not requested |
| stagger | `Stun` | the parried attacker's chain above; `StateNPCMasterStunned`'s group 0 (`Versus_Stun_In`) has no native requester |
| hit (`Damage`) | the damage type's `m_ExpectedHitReaction`, else Weak (0x00B57F00; `m_bDisableWeakHitReaction` false for every cooked combat tweak): Weak plays `HitReactSmall_Additive` on `AdditiveReactSlot` (slot 421, 0x00B733F0, eight-way step, no state), Strong `StateNPCMasterStrongHitReact` (cardinal `m_Direction` from 0x00A6D5F0's sectors), Projectile `StateNPCMasterProjectileHitReact` (hit region step; a non-leg hit from behind falls back to Strong), Knockdown a Thrown request | the simulation (`anim::hit_reactions`): the player's sword and the Windblast Weak, the crossbow bolt Projectile (head for a headshot, else the torso: the hit bone is not in the event), the slide Strong. Not modelled: the pistol bullet's and the grenade's Knockdown (`DishonoredDamageType_Bullet`, a Thrown request with the momentum), StrongHitReact's yaw snap (intent 0xC, U-862) |
| assassinated | `StateNPCMasterBeingAssassinated` (0x00A7BEB0 / 0x00A7B350): the slave chain (`m_SlaveAction`), its step the direction, its group the master's variant; Dead_Limp at its ragdoll notify or end | the simulation's `BeingAssassinated` from the master's tick (`Assassination` by direction, `AssassinationDrop`, the synced short fatalities' forehand / backhand group, a finisher's slave and variant), the ragdoll at its notify; the victim is drawn at its master (the retargeting component, 0x00C88440, is not run by the simulation) |
| death | `Death` / `DeathFast` ... then `DishonoredNotify_Ragdoll` (e.g. 2.299 s into `Generic_DeathFrontA`); every chain step is played with blend-out 0 and the death groups' `m_fBlendOutTime` is 0: full weight up to and past the notify (0x00C52E00 / 0x00C53260) | the simulation's `Death` step by the killer's direction on `NPC_Dead_at`'s `FullBodySlot` at the step's blend-in (0.5), blend-out 0, then the presentation ragdoll at the notify's event (§8) |
| first seen dead (late join, corpse spawners) | the corpse is already a ragdoll at rest | the ragdoll run to rest at once from the pose drawn (§8, U-147) |
| sleep dart | `m_bSlowDeathOnly`: the "death" is the knock-out | not requested: the ragdoll at once |
| held in a choke | `StateNPCMasterBeingChoked`: the choke context's state plays the victim's `Choke` group (0x00C09370: In, Loop, Win, CarryCorpse, Lose, Cancel; blend -1: the step's 0.2); the victim's In end plays the Loop; Win's end puts it down (Dead_Limp, ragdoll at once) | the simulation follows the choker's `ChokePhase` (In, Win, Lose, Cancel; the Loop after the victim's own In; Cancel refused once won); Win's end puts it down, Lose's and Cancel's let it go. The port's gameplay still switches In -> Loop on the master's length and wins at the timer (U-101) |
| carried | `StateNPCMasterDead_BeingCarried` (0x00A998A0 / 0x00A907F0): In from the ground, Idle from a takedown, Walk while the carrier's horizontal speed > 0; the drops by the trace's class (0x00A6C910) | the simulation: In / Idle, Walk / Idle by the carrier's speed, the low drop (the sneaking one crouched; the drop's trace classification is not modelled), the ragdoll at the drop's notify or end; drawn at the carrier's root |
| panic start | `Panic` (GenericAction) | the brain's GenericAction through PlayAnim; the `PanicEmpty` / `PanicSword` locomotion |
| cower | `DisAISubStateCower` (0x00B12840): body stance `PanicRecover` (`Equipped` when armed), face the enemy | not requested (no body stance from the brain) |
| noticing | the Notice behaviour's GenericAction (`DisNPCAnim_ReactToNoticing`, chance 1) | the brain asks it once per Notice behaviour (`NPC_ANIM_REACT_TO_NOTICING`): PlayAnim of `<prefix>PlayerCheck` |
| surprised | `StateNPCMasterSurprised` (PlayAnim subclass, request 0x00A869B0): `Surprised` by the stimulus's cardinal direction, from the attention reactions (0x00B0BB90) | the simulation on the brain's turn to surprised, toward the nearest player (LIKELY: the reaction's stimulus is not in the request) |
| pistol | `StateNPCMasterAim`: In, the hold (no chain: the `Aim` modifier), Shoot, Reload, Out (chain `0x60 + sub-state`) | the simulation's `Aim` from the brain's `FIRE_PISTOL` context (the original enters it at its equip state's end), the shot and the reload at the left hand's, Out when the context ends |
| talking | the blurb's FaceFX animation on the face and head (§12); body gestures only from soiree matinees (U-142, resolved: blurbs carry no animation, `DisBehaviorBlockingDialog` only faces the listener, no `eDisNPCAnimations` chain is a gesture, and the FaceFX actors pose `head_jnt` and the face bones only) | FaceFX lip sync and head motion on the tree's output (§12); body: the soiree matinee's AI-group anim tracks as the `MatineeStackNode` channels (CUTSCENES.md) |
| a human player's body | (the original has none in single player) | its own `NPC_Human_at` in the simulation (`player_anim::Body`, the former third-person picker's rules through the NPC profile, LIKELY): `Sword` / `Aim` / `Empty` modifier; a new swing's `AttackShort` group (group 5 in a paired move), a won parry's `Parry` group 0, the block's `Parry_AwaitingImpact`, the stagger's `Stun` group 0, the gun's `AimIn` / `AimShoot` / `AimReload`, an armed crouch's `Sword_Cover_Loop`, the hits' and the death's directional chains |
| thrown (Windblast, push) | `StateNPCMasterThrown`: `PushToRagdoll` group 1 by direction (blend-out 0), ragdoll at its notify; at rest `StateNPCMasterStunned` (0x00A6B760: group 2 `RagdollToBack` when the recover bone's axis points up, else group 1 `RagdollToFront`; cut in), the loop for `m_fStunDuration` (0: left on the next tick), the recovery | the simulation's throw (`play_throw`), the ragdoll at its notify's event; at rest `Stunned` group 1 (the sim has no ragdoll pose to choose group 2), the recovery the rest test rolled (§13) |
| special classes (`npc_move` 20-28) | the item contexts' `m_MainAttack.m_Animation`, the minigame groups (`StateNPCMasterMinigame`: `SyncAttack` group `m_GroupIndex`, step = the minigame state), the Tallboy aim sub-states, the music box's walk upper body | not requested: one profile for every NPC (U-978), the special classes' swings timed without a sequence (§13) |
| plague / rat death | `DeathPlague` / `DeathSwarmAttack` (by name, U-890) | rats: `DeathSwarmAttack`; plague: `DeathPlague` (the simulation) |

Directional steps (U-143): the step index is a direction enum's value. Groups of 4 follow
`ECardinalDirection` (Forward, Left, Right, Back): `StateNPCMasterStrongHitReact` plays its
chain step `m_Direction` (+0x6C) and, at its enter, snaps the pawn's yaw (rotation intent 0xC at
`m_fYawAdjustmentSpeed` 120) to the hit's reverse momentum offset by +90 / -90 / 180 degrees for
Left / Right / Back, so that the hit comes from the authored side (0x00A78980; a correction of at
most 45 degrees). The cardinal sectors (0x00BBA970 on `Location - momentum x 8888`, CONFIRMED
2026-10-12): in the pawn's frame, Front when |x| > |y| and x >= 0, Back when x < 0, else Right
when y > 0, else Left (diagonal ties to the sides). Groups of 8 or more follow
`eDisDeathDirection` (Forward, Forward_Left, Forward_Right, Left, Right, Back, Back_Left,
Back_Right, then the `_Up` variants Forward_Up ... Back_Up at 8..13), matching the 14-entry
`Death` / `DeathFast` groups. CONFIRMED from the enums. The port computes the cardinal step with
the decoded sectors from the attacker's position (the momentum is not in the events) and the
eight-way one with 45-degree sectors (LIKELY); when the `_Up` variants are used was not traced.

Body stance -> locomotion modifier (U-892, decoded 2026-10-12: 0x00B70FB0 applies the highest of
a 15-level stance stack through pawn slot 497 0x00B71000; indices into the elite guard config's
`m_Modifiers`): 0 SwordPatrol with an unsheathed primary item (not a music box), else Empty; 1
EmptyPatrol; 2 SearchSword with a weapon intended, else SearchEmpty; 3 (Equipped) MusicBox / Sword;
4 SwordPatrol; 5 Aim with a ranged secondary item, else Empty; 6 PanicSword / PanicEmpty; 7
(PanicRecover) PanicRecoverSword / PanicRecoverEmpty; 9 MeleeRush; 10 ExplosionEscape_Equipped /
_Empty; others Empty. The simulation still maps the brain's awareness (`anim::modifier`): the
port's brain publishes no body stance.

Cower (U-141, resolved): the cower sub-state's entry (0x00B12840) issues a body-intention request
with stance `eDisNPCBodyStance_PanicRecover` (7), or `Equipped` (3) when the pawn holds a weapon,
then a face-to request toward the enemy proxy (rotation speed -100: default). No chain is
played: the cowering pose is the `PanicRecoverEmpty` idle loop (`Empty_Idle_Panic_Recover`).

Layering: the tree's own (§2): the slots blend their channels in and out over the requested
times. The anim player (pawn +0xD74, slots 0 `FullBodySlot`, 1 `UpperBodySlot` with
`UpperBodyFilter`, 2 `FaceSlot` with `FaceFilter`, 3 `AdditiveReactSlot` with
`AdditiveReactFilter`; 0x00C488D0) plays a chain step with `PlayCustomAnim(name, rate, the step's
m_fBlendInTime, blend-out 0, loop, override)` (0x00C52E00 -> 0x005B7860: a looping step's pending
blend-out -1, held), stops the chain's last step `m_fBlendOutTime` before its end with
`StopCustomAnim(m_fBlendOutTime)` and the slot's filter to 0 (0x00C48CF0), and on every step start
plays the step's `m_FaceAnimName` (else `<sequence>_Face`) on `FaceSlot` at the body's rate and
blend-in and sets `FaceFilter` to 1 if one played, else 0, over 0.1 s (U-974, resolved): the
simulation requests the same (`anim::step_tree`), landed before the tree's advance (U-975). The
blinks are the look-at component's roll on `BlinkBlender` (§9), in the simulation's tree. The
upper-body slot is masked to `spine_0_jnt` and
its descendants (`UpperBodyFilter.BranchStartBoneName`). Tracks bind by bone name and every clip
translation is used (`dishonored_anim::bind_tracks` with the sets' flags: the Boyle NPC sets leave
`bAnimRotationOnly` false, which the face bones of the `stealth/detect_1300` capture confirm,
LOG 2026-10-11); the runtime's former rule (translations for the root and its children only) went
with its layering.

## 7. Animation notifies and gameplay timing (for the simulation)

NPC attacks carry their hit timing in notifies (`Npc_EliteGuard_Sword_as`):

| Sequence | Length | `DishonoredNotify_AttackZone` start / duration | `m_fDamageZoneMaxTime` | `AttackInterruptable` |
|---|---|---|---|---|
| `Sword_Attack_ShortSmallA` | 1.767 s | 0.490 / 0.454 | 0.122 | 1.020 |
| `Sword_Attack_ShortSmallB` | 1.767 s | 0.494 / 0.460 | 0.130 | 1.011 |
| `Sword_Attack_ShortSmallC` | 1.767 s | 0.521 / 0.432 | 0.130 | 1.003 |
| `Sword_Attack_ShortSmallD` | 1.767 s | 0.592 / 0.375 | 0.142 | 1.003 |

The `Big` follow-ups (0.933 s) have no attack zone. `m_fVersusZoneMaxTime` 0.1 (parry window on
the swing), `m_SwingDirection` per sequence. Deaths drop the held item (`DisNotify_DropItem` at
0.296 s in `Generic_DeathFrontA`) and switch to ragdoll (`DishonoredNotify_Ragdoll` 2.299 s). If the
simulation wants NPC swing timing from the original, these are the numbers (agent A's call); the
port keeps notifies presentation-side.

## 8. Ragdoll (`PhysicsAsset`, `DishonoredNotify_Ragdoll`)

`DisTweaks_Pawn.m_pPhysicsAsset` names the pawn's physics asset (elite guards and male
aristocrats: `Npc_EliteGuard.Physics.Skm_EliteGuard_Physics`; Boyle women:
`Npc_Servants.Physics.Skm_servantA_Physics`). Decoded by `npc_anim::read_physics_asset`:
- `BodySetup[]`: `RB_BodySetup` per bone: `BoneName`, `bFixed`, `MassScale`, `PhysMaterial`,
  `AggGeom` (`KAggregateGeom`: `SphereElems`, `BoxElems` X/Y/Z, `SphylElems` Radius/Length, each
  with a 64-byte `TM`). Elite guard: 18 bodies (Root box 34x28x19, spine_1, spine_3, neck,
  upper/lower arms, upper/lower legs, feet, four coat flaps `Back_pan_*`).
- `ConstraintSetup[]`: `RB_ConstraintSetup`: `ConstraintBone1` (child) / `ConstraintBone2`
  (parent), frames `Pos/PriAxis/SecAxis 1/2`, `bSwingLimited`, `bTwistLimited`,
  `Swing1LimitAngle`, `Swing2LimitAngle`, `TwistLimitAngle` (degrees). Elite guard: 17 joints, e.g.
  spine_1 -> Root swing 30 / twist 6, neck 35 / 35, upper arm 80 / 40 / twist 50, elbow 40.

The original switches the mesh to PhysX at the death sequence's `DishonoredNotify_Ragdoll`
(`m_bRagdollOnDeath` false for every Boyle tweak). The port's ragdoll (`runtime/ragdoll.rs`) is
presentation only: each body is a point mass at its bone, joined to its parent body at the
length it had at the hand-over; the child segment stays inside its joint's swing cone
(max of `Swing1/Swing2LimitAngle`) around its direction at the hand-over, carried by the parent
segment's turn; masses fall under the world gravity onto the floor under the corpse (the standing
cylinder's bottom), with friction. Fixed 1/60 s steps, 8 constraint passes, 4 s then rest. It
starts from the pose drawn at the tick of the simulation's `DishonoredNotify_Ragdoll` event (the
tick before, when a frame drew it, for the initial velocities; else at rest), at the end of a
death or throw without one, or at once when the full body plays none, and steps by the
simulation's ticks, so a corpse falls the same way on every run and frame rate. Bones are rebuilt from the masses (each body turned by the arc from its start
segment to its current one, twist from its parent; the top body from two children); bones without
a body keep their offset to their body. The simulation's corpse position is untouched.

Bodies first seen down (late join) run their ragdoll to rest immediately. Carried bodies leave
the ragdoll; a dropped body starts a new one from its carried pose. APPROXIMATION of PhysX: no
collision with the level beyond the floor plane, no twist limits, leaf bodies (neck/head,
forearm/hand, feet) stay rigid with their parent (U-282).

## 9. Look-at and blinking (`ArkComponentLookatConfig`)

`DisTweaks_NPCPawn.m_pComponentLookatConfig` (Boyle: `DisLookatConfig.DefaultLookatConfig`,
DishonoredGame) has `m_LookAtModes` named by stance and gait (`EmptyIdle`, `SwordIdle`,
`SearchIdle`, `EmptyWalk`, `SwordWalk`, `SearchWalk`, `EmptyRun`, `SwordRun`, `AimIdle`, ...),
each with head and torso angle ranges (e.g. `EmptyIdle`: head right 65 / left -65 / up 60 / down
-35, torso 60 / -60 / 35 / -45), `m_HeadAndTorso_Horizontal/VerticalPercentage` and two 3x3 grids
of additive sequences (`Head_AnimName_LU` ... `RD`, `Torso_AnimName_*`, e.g.
`ADD_Empty_AimHeadIdle_UpLeft`). The cooked additive sequences (`bIsAdditive`) are single-frame
deltas. Blinking uses the class defaults: `BlinkEnabled`, a blink every
`[MinDurationBetweenBlink, MaxDurationBetweenBlink]` = 0.5-6 s at a play rate in
`[MinBlinkSpeed, MaxBlinkSpeed]` = 1-2.

Port: the mode is picked from the locomotion modifier (Search / Sword / Empty) and gait; an aim's
yaw and pitch against the mode's ranges weight the 3x3 grid bilinearly, for the torso and then
the head, and each delta is applied as `delta * local rotation` (UE3 additive blending). The
aims are the AI's own (2026-10-08): the vision head (`dishonored_ai::head`, stealth.md section
12: the HeadTrack request, the look-at cursor and the head / torso weights) is split by the node's
rule (0x009176F0, `head::split_aim`) into a torso part and a head part, published per pawn
(`MatchView::npc_heads`), and each grid is posed by its own part, so the visible head turns where
the vision cone points. The renderer sets those aims on its copy of the simulation's tree state
(`TreeState::present`, the mode from the locomotion node's modifier and smoothed speed). The
former approximation for a pawn speaking a line without a HeadTrack request (the nearest player
within 600 uu, eased in about 0.25 s, U-281) and the runtime's blinks went with its clock
(2026-10-11). The blinks are the simulation's (2026-10-12): the roll 0x00952EC0 (from the look-at
tick 0x00955B40) counts its timer down every enabled tick, blinking or not, and below 0 rolls the
next interval and restarts `Blink` at a rolled rate (interval first); the timer starts at 0, so
the first enabled tick blinks (LIKELY). `dishonored_anim::lookat::BlinkTimer` on every
`NPC_Human_at` state, seeded per pawn (the original rolls the CRT's global `rand()`).

## 10. Bodies, chokes and carrying

Data (`DisNPCAnimDefinitions_*`, `Ply_Sword_Choke_as`, `Ply_Empty_CarryCorpse_as`):
- `Choke` (95): six one-step groups `Generic_Choke_{In,Loop,Win,CarryCorpse,Lose,Cancel}_Slave`,
  each linked to the player's `Sword_Choke_*_Master`.
- `CorpseBeingCarried` (71): `Corpses_CarryCorpse_{In,Idle,Walk,DropLow,DropLowSneak,DropHigh,
  DropNoMove,DropFetal}_Slave`; the player's `Empty_CarryCorpse_{In,Idle,Walk,DropLow,
  DropLowSneak,DropHigh,DropNoMove}_Master`.

Port (third person): see the §6 table; since 2026-10-12 the simulation plays the choked
victim's `Choke` groups, the carried body's `CorpseBeingCarried` steps and the paired moves'
slaves (`dishonored_sim::pawn_states`: `BeingChoked`, `BeingCarried`, `BeingAssassinated`), and a
down body goes to the ragdoll at its chain's ragdoll notify or end. Carried bodies are drawn at their carrier's root (the slave
sequences are authored there), also in the carrier's own first-person view, where the body hangs over the shoulder in front of the camera. Skinned bodies
are no longer tipped over by the renderer (their sequences and ragdoll lay them down) and are
placed on the standing cylinder's bottom whatever their collision height (this also fixes
crouched pawns standing below the floor). Weapons are hidden on unconscious and carried pawns.

## 11. First person (`Ply_*_as`)

Player sets in Startup: `Ply_Sword_{Idle,Locomotion,Attack,Versus,Hit,Assassination,Choke,
Fatality}_as`, `Ply_Empty_{Idle,Locomotion,Hit,FullBody,CarryCorpse}_as`, `Ply_Pistol_as`,
`Ply_Crossbow_as`, `Ply_Guns_as`, `Ply_Powers_as`, `Ply_Gadgets{,_Grenade,_Heart,_SpringRazor}_as`,
`Ply_Generic_as`, `Ply_Head_Locomotion_as`.

Since 2026-10-11 the viewmodel poses the simulation's layers (ARCHITECTURE.md step 7): the swings
and the block-in on the upper body, the casts (Bend Time, Swarm, the possession's out), gadget
deploys, gun fire and reload stages on the left arm, the paired masters on the full body, the
crossbow mesh's fire; each layer weighted from its playhead and the requesting state's blend time
(`SeqPlay::blend`). Since 2026-10-12 the player's other states are requested too
(`dishonored_sim::player_anim`, the former pickers' rules with the navigation trees' decoded
nodes): the navigation (`Ply_Player_Nav_Default_at` / `_Sprinting_at` / `_Crouched_at`: the
speed blends 0x00594BA0 over 0x00A89900's 2D speed, idle at 0, walk up to 250 uu/s, run above;
sprint above 300; the crouch's walk above 0 on the 3D speed; 0.4 s between speeds, 0.3 s to the
air child, which shows in the jump state or after `m_fAnimateFallTime` 0.25 s of a fall; every loop
at rate 1), the left hand's (`Ply_Nav_LeftHand_at`: idle at 0, `DisAnimNodeBlendByLocoType`'s walk
or sprint, no notifies, in the full body's sync group), the mantle (by edge height and stance,
the empty-handed set with the sword not ready, at `Attribute_MantleAnimRate`), the choke and carry
masters, the stun, the slide, the climb (`Head_Rope_Climb` above an acceleration of 36, the
reference pose below), the swim (`Empty_SwimIdle` up to an input of 0.36, else the input
direction's dominant loop: one layer cannot mix `DisAnimNodeBlendDirectional`'s neighbours), the
block's in / loop / won parry / out, the transitions' one-shots (equip on respawn, crouch in / out,
jump in, small land, the gunshot reaction, the left item's equip / switch, sword slash and jump
reactions), the possession and Blink aims, the Blink and Windblast casts and the step-up's head on
the additive layer. Before 2026-10-11, the viewmodel's pickers played:
- choke: `Sword_Choke_{In,Loop,Win,Lose,Cancel}_Master` by choke state, whole rig;
- carrying: `Empty_CarryCorpse_In_Master` on a lift (`Sword_Choke_CarryCorpse_Master` when the
  lift follows a won choke), `Idle_Master` / `Walk_Master`, `DropLow_Master` over the drop's
  action lock; the sword and left-hand items are hidden while carrying;
- powers: `Powers_Cast_BendTime`, `Powers_Cast_Swarm`, `Powers_Cast_Possession_In` / `_Loop`
  while targeting, `_Out` on possessing;
- gadgets: `Gadgets_Idle/Walk/Sprint/Ready_Equip`, `Gadgets_Grenade_Deploy` (grenades and sticky
  grenades), `Gadgets_SpringRazor_Deploy`; sleep darts and incendiary bolts use the crossbow clips
  and model;
- elixirs: no player set has a drinking sequence (no "Potion", "Elixir" or "Drink" sequence in
  Startup, DishonoredGame or Engine), so drinking has no first-person animation.

Weapon sockets (U-025, resolved): `Ply_Player.Skm_Player` has `RightHandWpn` on
`handAttachment_R_jnt` (rotation yaw +16384) and `LeftHandWpn` on `handAttachment_L_jnt`
(0, -16384, 16384); the weapon meshes' own `RotOrigin` is (0, -16384, 16384). The original
attach is bone, then socket, then the mesh's Origin / RotOrigin. That rotator is a 120-degree
axis cycle, so for the guns it equals the inverse RotOrigin used before; for the sword it keeps
the blade direction and changes its roll (tests in `viewmodel.rs`). NPC skeletons have their own
sockets (elite guard `LeftHandWpn` pitch -16384, `HolsterWpn`, `Helmet`, `Mask`, ...);
third-person weapons use the character mesh's socket the same way, falling back to the bone when
a mesh has none.

### View and arms (decoded 2026-10-08)

The first-person mesh is not glued to the camera. The camera and the arms are placed apart and
meet through the camera bone:

- **Eye position.** `DishonoredPlayerPawn` vfunc +0x630 (0x00AB2030 -> 0x00AABA60) returns the
  world position of `DisTweaks_PlayerPawn_Camera.m_CameraBoneName` (`camera_jnt`), after the
  skeletal controls. `DishonoredCamera_PlayerControl` (slot 80, 0x00ACBA40) uses it as the POV
  location. `DishonoredCamera_AnimDriven` has weight 0 by default (`DefaultCamera.ini`).
- **View rotation.** vfunc +0x3E8 (0x00AABB70) is the controller rotation plus
  `DishonoredPlayerCamera.m_RollAmount` (0.7) times the camera bone's rotation relative to the
  pawn: yaw, roll, and pitch minus the camera look control's own pitch. The bone matrix is turned
  first by `DisTweaks_Pawn_Body.m_fBone_AngleOffset_Pitch` (-90 for Corvo, matrix
  `m_AngleOffsetMatrix`, applied by 0x00B57110). That makes the bone's -Z axis forward and +X up.
- **Look controls.** `Ply_Player_at` has three `SkelControlSingleBone` (add rotation, actor
  space): `LookAtControl_RightHand` on `Collarbone_R_Jnt`, `LookAtControl_LeftHand` on
  `Collarbone_L_Jnt` and `LookAtControl_Camera` on `spine_3_jnt`. The pawn's view-target update
  (0x00AABDD0, reached from the camera through 0x00ABA860) sets them every frame:
  - Each hand gets pitch = (additive POV pitch + controller pitch) + its item's
    `m_ArmFollowOffset_Rot` x (1 - cos(pi x `m_ArmFollowOffset_Weight`)) / 2. That sum is scaled
    by `m_ArmFollowWeight_Rot_*`, and the pawn's own pitch is subtracted.
  - The camera control gets the same pitch without the offset, scaled by 1 - AnimDriven weight.
  - Both hand weights are 1 from `DishonoredCamera_PlayerControl`. The Disable* influences can
    lower them (U-780).
- **Arm pitch offsets.** The player control sets `m_ArmFollowOffset_Rot_Primary` / `_Secondary`
  pitch from the hand's item (0x00ACB8D0). It reads `m_ArmPitch_OffsetKeys` (view pitch,
  difference in degrees) with a cosine ease between the bracketing keys, then multiplies by
  65536 / 360.
  - Sword, powers, grenade and Spring Razor: (-90, 10), (-10, 0), (10, 0), (90, -15).
  - Pistol and crossbow: (-90, 2), (-10, 0), (10, 0), (90, -3).
- **Arm offset and FOV.** The arms mesh (`DishonoredPlayerSkeletalComponent`) gets `m_Offset` =
  (POV location - non-additive location) x `m_ArmFollowWeight`, so the arms follow the additive
  camera moves such as lean and shake. Its render proxy (vtable 0x01124558, slot 31 0x00ADEC40)
  re-projects the arms with `m_FOV` when `m_bUseFOV` is set. `m_FOV` is lerp(POV FOV,
  `m_fCurFOV_Arms`, `m_MeshSpecificFOVWeight`), and it is used only when it differs from the POV
  FOV. `m_fCurFOV_Arms` follows the world FOV target unless the target has `m_bLockArms` (zoom).
  In normal play the arms therefore use the world FOV.
- **Consequence for the poses.** Arms relative to the view are the mesh's arms relative to
  `camera_jnt`, minus 70 % of that bone's animated rotation. The data puts some poses outside the
  75-degree view:
  - the sword idle's right hand is 15 uu ahead, 36 right and 46 below the eye, so only the blade
    shows;
  - the swing winds up off-screen to the right (0.1-0.2 s) and crosses the centre at about 0.27 s;
  - the `Sword_*_Mantle*` clips move only the root and the camera, not the arms;
  - `Powers_Cast_Windblast` draws the hand back close to the face at about 0.2 s before
    thrusting it out (0.5 s);
  - `Powers_Cast_Blink_In` / `_Loop` stay near `Powers_Idle`.

  (`fp_probe` on the decoded clips.)

Port (`viewmodel.rs`): the look controls, the arm pitch keys, the view rotation at
`ROLL_AMOUNT`, the eye moved by the camera bone's motion from the additives and the look pivot,
and a rig that stays level when the lean rolls the view. Not ported: the Disable* / AnimDriven
weights (U-780) and the locked arm FOV during zoom (U-784).

## 12. FaceFX (lip sync and facial animation)

Reader: `crates/dishonored_assets/src/facefx.rs` (decoded from the data alone; no FaceFX SDK
code). Runtime: `crates/dishonored_runtime/src/facefx.rs`, applied on the tree's pose in
`characters::animate_characters`. Tool:
`facefx_dump <package> <export-substring> [-v]`.

**Where the data is.**
- Each head `SkeletalMesh` names its actor in the `FaceFXAsset` property, e.g.
  `Npc_Servants.Mesh.Skm_ServantA_Head` -> `Skm_ServantA_Head_FaceFX`. The actors are cooked
  into the level script packages (Boyle: elite guard heads 01 / 03, male and female aristocrat,
  middle-class, servant A / B, Samuel, the Outsider).
- Each `DisDialogVoiceData` (`m_pFaceFXAnimSet`) has a `FaceFXAnimSet` named after its GUID.
  Its one animation group holds one animation per blurb. The animation is named by the blurb GUID
  (`DisVoiceTextBlurb.m_GUID` = `DisConv_Blurb.m_iBlurbGUID`) and records its source WAV
  (`Voices\English(US)\<package>\<voice data>\<GUID>.wav`).
- Both classes hold no tagged properties. After `None`, they carry a byte-array count and then
  one FaceFX archive.
- The 44 `InterpTrackFaceFX` tracks in the Boyle matinees are empty when cooked: 12 bytes each,
  with no `FaceFXSeqs` and no `FaceFXAnimSets`. A matinee's lip sync therefore comes from its
  dialog keys' blurbs, the same as any other line (CONFIRMED by the data).

**Archive layout** (little endian; CONFIRMED by decoding every Boyle archive to its last byte):
- Header:
  - `FACE`, version 1731, a zero word.
  - Licensee and project strings (count-prefixed, NUL-terminated), the licensee version 1000,
    and a u16 1.
  - The class directory: count, then per class two words, the name, and a u16 version. Actors
    list `FxBone, FxCompiledFaceGraph, FxObject, FxActor, FxMasterBoneList, FxNamedObject,
    FxName, FxPhonemeMap, FxAnimGroup, FxLinkFnParameters`; anim sets list `FxObject, FxAnim,
    FxAnimSet, FxNamedObject, FxName, FxAnimCurve, FxAnimGroup`.
  - The name table: count, then strings without terminators. Everything after it refers to
    names by index.
- Actor:
  - The actor name and a zero word.
  - The **master bone list**: a count, then per bone (all 109 skeleton bones of the servant):
    - name;
    - reference position, rotation and scale;
    - the inverse reference rotation;
    - index, weight (1);
    - its **bone pose links**: a count, then per link a node index, the bone name, a position
      delta from the reference, an absolute bone-local rotation, and a scale delta.
  - The **compiled face graph**: a count, then per node in evaluation order:
    - kind (0 combiner, 4 bone pose);
    - name;
    - min, 1/min (1 when min is 0), max, 1/max;
    - input operation (0 = sum everywhere);
    - inputs: a count, then per input the source node index (always an earlier node), the link
      function type, and a count-prefixed parameter list;
    - a zero word.
  - Then a name -> node hash table (not needed).
- Anim set:
  - Set name, a zero word, the actor name, the group name (the set name), and the animation
    count.
  - Per animation:
    - name;
    - curves: a count, then per curve the node name and its interpolation (0 = Hermite in every
      retail curve);
    - every key of the animation as (time, value, slope in, slope out);
    - the key count of each curve;
    - blend in and blend out (servant lines 0.16 / 0.22 s);
    - two zero words;
    - the source WAV path;
    - -1.
- Coordinates: FaceFX is right-handed. Against the head mesh's bind pose, positions have Y
  negated, and rotations (stored w, x, y, z) have y negated. Example: servant `head_jnt`
  reference (8.77, 0, 0) with rotation y -0.133, against the mesh's +0.133. CONFIRMED on
  `head_jnt`, `eye_R_jnt` and `jaw_jnt`.

**The servant actor (example).**
- 99 nodes: 6 tongue / phoneme bone poses (`FV`, `tBack`, `tTeeth`, `tOut`, `ShCh`, `tRoof`),
  the mouth targets `open`, `W`, `wide`, `PBM`, and FACS-style bone poses (`cheekRaiser`,
  `sneerLeft`, `upperLipRaiser`, `jawClencher`, `pucker`, `browLowerer`, the lid poses ...).
- Emotion combiners (`Joy`, `Fear`, `Anger`, `Sadness`, `Disgust`, `Surprise`) feed the bone
  poses through linear links. Example: `open` = `Joy` x 0.05 + `Surprise` x 0.106 + `Fear` x
  0.205 + its track.
- Head and eyes: `Orientation_Head_Pitch` + `Emphasis_Head_Pitch` -> `Head_Pitch` (range +-10)
  -> `Head_Rot_X` (x 0.1, range +-1). That splits into `Head_RotPos_X` / `Head_RotNeg_X` bone
  poses on `head_jnt` (x +1 / -1, range 0..1). Yaw and roll follow the same pattern, and yaw
  feeds roll through a type-2 link (-0.3).
- The jaw's `open` pose turns `jaw_jnt` about 20 degrees.
- **Link functions.** The type number is the order in which the executable registers the link
  function classes. Each class carries its formula string (0x0120C5A4..0x0120C7F4):

  | Type | Function | Formula |
  |---|---|---|
  | 0 | null | deprecated |
  | 1 | linear | `m x + b` |
  | 2 | quadratic | `a x^2` |
  | 3 | cubic | `a x^3` |
  | 4 | square root | `a sqrt(x)` |
  | 5 | negate | `-x` |
  | 6 | inverse | `1 / x` |
  | 7 | one-clamp | 1 when x <= 1, else 1 / x |
  | 8 | constant | `c` |
  | 9 | corrective | correction factor; needs the corrected node |
  | 10 | clamped linear | `m x` (clampx / clampy / clampdir) |

  The Boyle actors only use types 1 and 2, and so do the actors of six other maps sampled with
  `facefx_dump` (Prison, Streets1, Tower, ArtDealer, Bridge 1a, Brothel; all decode, node
  kinds 0 and 4 only). Types 9 and 10 are not modelled because their
  clamp / correction rules were not decoded.
- Typical line: 19 curves (`open`, `W`, `ShCh`, `PBM`, `FV`, `wide`, `tBack`, `tRoof`, `tTeeth`,
  the head orientation / emphasis and gaze curves, `Eyebrow_Raise`, `Blink`). Many are empty;
  keys start slightly before 0 (-0.02 s).
- The binary also knows `FUnrealFaceFXMorphNode` and `FUnrealFaceFXMaterialParameterNode`
  (morph targets and material parameters). No Boyle actor has them; the renderer has no morph
  targets.

**Evaluation** (`Actor::evaluate`, `bone_poses`, `Anim::tracks`):
- **Curves** (actor instance tick 0x00EF5640 -> 0x00F00B50): each frame every curve's raw value
  is written into its node.
  - Held before the first and after the last key.
  - Interpolation 0 is the cubic Hermite with the keys' slopes scaled by the key gap
    (0x00F01070); interpolation 1 is linear (0x00F01130).
  - The animation stops once its time passes its end.
  - `blend_in` / `blend_out` are not read on this path, so the port applies no ramp.
  - The animation time is the instance clock minus the start and the audio offset.
- **Face graph** (0x00EFBB00), in node order. Node types: 1 delta, 2 current time, others
  combine.
  - The inputs combine through their link functions by the node's operation: 0 sum, 1
    multiply, 2 max, 3 min. The identities are 0, 1, -FLT_MAX and FLT_MAX (table 0x0120B91C),
    or 0 without inputs.
  - Corrective links (type 9) instead accumulate `c = min(1, c + factor x max(v / max, -v /
    min))` of their source (the stored reciprocals).
  - Then the curve value is added. Game-set registers (add / multiply / replace) come next; the
    port has none.
  - The result is scaled by `1 - c` and clamped to [min, max].
  - CONFIRMED.
- **Bone blending**: CONFIRMED from `USkeletalMeshComponent`'s FaceFX update (0x00725EA0). Its
  component fields are `FaceFXBlendMode` +0x38d, `FaceFXActorInstance` +0x390 and
  `m_fFaceFxTickTime` +0x398. The actor instance's master bones have a stride of 0x58, with the
  pose links at +0x4c (count +0x54, 13 floats each). The node values sit at +0x20 of 0x48-byte
  nodes. For every master bone:
  - position = reference + sum over its poses of node value x position delta;
  - rotation starts at the reference. Each pose whose value `w` satisfies |w| > 1.19e-7 gives
    `S = slerp(reference, pose, w)`:
    - signs are aligned by comparing |a + b| with |a - b|;
    - a true slerp when the dot is at most 0.55, a plain lerp above.

    The delta `S x reference^-1` multiplies onto the accumulated rotation, which is normalized
    at the end. In the port's mirrored quaternions the product order reverses, so `bone_poses`
    composes `accumulated x (reference^-1 x S)`.
- **Combining with the animation**: `FaceFXBlendMode` is `FXBM_Additive` (the
  `Default__SkeletalMeshComponent` value; no Dishonored class overrides it). The additive
  branch is:
  - local position += FaceFX position - mesh reference position;
  - local rotation = (FaceFX rotation x mesh reference rotation^-1) x local rotation.

  `FXBM_Overwrite` would copy the FaceFX transform instead. The update runs on the AnimTree's
  output, after every tree node, including the look-at additive and the matinee slots.

**Playback.**
- CONFIRMED: the blurb start (0x00C9FBF0, the same function that adds the subtitle for a
  playing ID) calls the inner `PlayFaceFXAnim` (0x00756EF0, also behind
  `execPlayFaceFXAnim` 0x00757440). It passes:
  - the speaker's FaceFX skeletal mesh component (`m_pSkelMeshCompForFaceFX`);
  - the blurb's animation by name, in the group read from the voice data;
  - the conversation component as the FaceFX audio handler (`m_pFaceFxAudioHandler` +0x39c);
  - a start time: the value from 0x00757070 (an animation lookup, LIKELY its start time) plus
    the blurb's start offset (non-zero for matinee dialog keys).

  When the animation starts, the handler gets its callback (0x00CA0120) and posts the VO. So
  the FaceFX animation drives the VO start, and the two stay in sync. When the anim cannot
  play, the blurb takes the plain path.
- No DishonoredGame script sets FaceFX registers (`SetFaceFXRegister` / `Ex` are only reached
  from their natives, 0x0071A000 / 0x0071A120), so registers stay unused.
- The port:
  - `DialogueLineView.blurb` carries the line's GUID.
  - Every speaking pawn whose look has a head with a FaceFX actor plays the blurb's animation.
    Time 0 is the line start, which is when its VO starts.
  - `runtime::facefx::face_bones` turns the blended transforms into additive offsets against
    each part's bind pose.
  - `characters::animate_characters` applies them on the tree's pose: rotation = delta x
    animated rotation, position = animated position + delta. Bones are bound by name in every
    part.
  - Masked party heads have no `FaceFXAsset`, so they do not lip sync.

## 13. NPC, world and map animation parity (2026-10-08)

### Special classes

Their definitions classes are cooked into the Boyle scripts (`DisNPCAnimDefinitions_Tallboy`,
`_Weeper`: L_BoyleStreet_Ext_Script; `_MusicalOverseer`: L_Boyle_Int_Script) and the Overseer
kennel scripts (`_Wolfhound`). Each AI move code (`dishonored_ir::ai::special_moves`) plays the
chain its item context names (`DisTweaks_ItemContext.m_MainAttack.m_Animation`, read with
`tweaks`). Since 2026-10-11 the renderer draws the simulation's tree, which steps one NPC profile
for every NPC (U-978) and requests none of these chains: the special classes stand in that
profile's loops (sequence names their sets may lack, so the reference pose) and their moves are
timed without a sequence. The runtime's former picker played them as below:

| `npc_move` | context (tweak) | chain | sequences (this pawn's sets) |
|---|---|---|---|
| 20 `WH_BITE` | `pNPCAttackMedium` of `Twk_Inv_WolfhoundBite` | `AttackMedium` | `Attack_Medium` (the second group names elite-guard sequences the hound lacks: the first group with sequences plays) |
| 21 `WH_JUMP_ATTACK` | `DisTweaks_WHJumpAttack_0` (`DisTweaks_Minigame`, `m_GroupIndex` 1) | `SyncAttack` group 1 | `Attack_JumpAttack_Jump_Minigame`, then `_StepBack_Minigame` (steps 1-3 are None) |
| 22 `WH_ARM_ATTACK` | `DisItemContext_WHArmAttack` (group 0) | `SyncAttack` group 0 | `Attack_ArmAttack_JumpGlide_Minigame`, `_GrabMiss_Minigame` |
| 23 `TALLBOY_STOMP` | `DisTweaks_AttackNPCTallboyMelee_1` | `TallboyStompAttack` | `Empty_StompAttack` |
| 24 `TALLBOY_FIRE_BOW` | `DisTweaks_NPCFireBow_1` (no animation: `StateNPCMasterTallboyAim`) | sub-state -> chain at 0x00A7CEB0: 1 / 2 `TallboyAimIn`, 4 `TallboyAim` (loop), 0 `TallboyAimShoot`, 3 `TallboyAimOut`, 5 `TallboyStompAttack` | `Empty_Bow_Charged_In`, `Empty_Bow_Charged` (held while aiming), `Empty_Bow_Fire` on each shot, `Empty_Bow_Charged_Out` |
| 25 `OVERSEER_BASH` | `DisTweaks_NPCBash_8` | `Bash` | `MusicBox_Attack_KickMedium` |
| 26 `OVERSEER_TUNE` | `DisTweaks_NPCTune_Combat_0`: slot 108 (0x00C202B0) requests chain 0x77 | `PlayCombatTune` | `MusicAmp_PlayMusic_In`, `_Loop` (held while the tune plays), `_Out` |
| 27 `WEEPER_GRAB` | `DisTweaks_WeeperGrab_2` (`m_NPCAnimation` `SyncAttack`, group 0, `m_fAnimDuration` 1.33) | `SyncAttack` group 0 | `Empty_ArmGrab_Try`, `Empty_ArmGrab_Miss` |
| 28 `WEEPER_COUGH` | the cough (`DishonoredNotify_StartPlague`) | `Weeper_Cough` (0x4B) | `Empty_Idle_Cough` |

The simulation has no sync-attack hold, so a minigame attack never connects: the port plays the
jump / try step and then the miss or step-back step (LIKELY, U-893).

Music box (CONFIRMED, code): `DisWepMusicBox` slots 81 / 96 (0x00C291F0, 0x00C29220) set the
pawn's `m_WalkUpperBodyDesire` (+0xDB0, via 0x00B53960) to `MusicBoxIdle` (0x79,
`Musicbox_Idle`); the combat tune's end (slot 103, 0x00C26F70) sets it to `PlayMusic` step 1
(0x76, `MusicBox_PlayMusic_Loop`). The port loops the desire on the upper-body slot and fades it
under full-body actions (U-891). The elite guard locomotion config's `MusicBox` modifier (idle
`Musicbox_Idle`, walk `EmptyWalk`) is used for pawns holding the music box (U-892).

Locomotion configs: Tallboy `TallBoyLocomotionConfig` (Idle, Walk; `Empty` only: walk N 213,
S 196, W 149, E 141), weeper `WeepTallLocomotionConfig` (`Empty`, `SearchEmpty`; walk 83 every
direction, run N 541), wolfhound `WolfHoundLocomotionConfig` (Startup; Idle, Walk, Trot, Run;
`Empty`, `SearchEmpty`, `EmptyPatrol`, `Sword`, `SearchSword`, `SwordPatrol` and five `_NotUsed`;
run 990, trot 387, attack walk N 282). The existing modifier rules fall back to the config's
first modifier, so Tallboys and weepers use `Empty`, hounds `SwordPatrol` / `Sword`.

### Thrown NPCs

`StateNPCMasterThrown` (physics.md 3.7; `movement::start_throw` / `step_thrown`): the simulation
plays `PushToRagdoll` group 1 (`Generic_WindBlast_{Front,Left,Right,Back}`, step by the throw's
direction, `dishonored_sim::anim::play_throw`) to its `DishonoredNotify_Ragdoll` (0.455 / 0.390 /
0.448 / 0.418 s), whose event hands the drawn pose to the presentation ragdoll. The get-up is not
requested (2026-10-11): the pawn stands up in its locomotion idle. The runtime's former picker
then played `Stun` group 1: `Generic_RagdollToFront`, the stun loop for one tick
(`m_fStunDuration` 0) and the recovery whose length matches the simulation's hold
(`Sword_RecoveryA` 1.867 s / `_RecoveryB` 1.5 s for guards; `Patrol_Recovery` for hounds,
`Empty_RecoveryA|B` for weepers). The get-up starts from the lying pose without blending from the
ragdoll's.

### World props

- River krusts (`DisRiverKrust`, 48 placed over the Flooded, Bridge, Pub and Streetsewer maps):
  their skeletal component is the `CollisionComponent` (MAP_SWEEP.md); the tree
  `Npc_RiverKrust_at` is a `FullBodySlot` over an unnamed sequence node the actor drives from its
  tweak's `m_AnimSequences[ERiverKrustAnimState]` (`RiverKrustAnimSequenceSpec`: name, play-rate
  range, blend in / out, start-time range, looping). At rest (`RiverKrustAnimState_Ambient`, 0) a
  small-pearl krust loops `ClamUp_Idle` at a rate in 0.9-1.05 from a start in 0-1 s. The port
  draws krusts as level props (`usable_props::is_animated_prop_class`, `SkeletalProp::idle`)
  looping that sequence, rate and start rolled per actor (`level_props::idle_roll`), and poses
  their collision bodies by the loop's first frame (collision cache 19). Their other states and
  the watch towers' orientation nodes are not played (U-894).
- Looping skeletal scenery: a `SkeletalMeshActor` whose `Animations` node is a playing, looping
  `AnimNodeSequence` (Boyle Int's pendulum clock `SkeletalMeshActor_1`, `Clock_TickAnim`) is an
  animated prop (`usable_props::playing_loop`) instead of a reference-pose instance, unless the
  Kismet names it (then the cutscene player draws it).
- Doors: unchanged (stage transitions, rest on the open stage's last frame; RENDERING.md).
- Distractions (U-895): every Boyle distractor plays a soiree matinee (`DistractionSoiree.*`),
  which the port does not start.

### Verification

Render scripts (`fixtures/scripts/`, run with `dishonored_runtime ... --script <file> --out
<dir>`; set `DISH_ANIM_DUMP=<pawn id>:<file>` to write that pawn's component-space bones every
frame as JSON lines, `characters::dump_bones`):

| script | command | shot | expected |
|---|---|---|---|
| `npc_anim_playground.json` | `--playground` | `na_01_dummy_idle` | dummy B 200 uu ahead in the sword-patrol idle, blinking |
| | | `na_02/03_dummy_hit_react_*` | a `Sword_Hit{Front,...}_A` reaction 0.4 s / 0.6 s after the swing (front step: the player is in front) |
| | | `na_04_dummy_projectile_react` | dummy A in a `HitReactProjectile` step (stomach / legs / front sides) |
| | | `na_05_thrown_flight` | dummy C in `Generic_WindBlast_Front` while flying back |
| | | `na_06_thrown_ragdoll` | the ragdoll falling / on the floor (state dump: `thrown.ragdoll`) |
| | | `na_07`-`na_09` | lying, then `Generic_RagdollToFront`, the recovery, standing in the idle |
| | | `na_10`-`na_12` | the patrol guard walking (`SwordPatrol` walk loop, feet planted, no sliding) |
| | | `na_13_patrol_notices_head` | the guard turning its head and torso toward the player (published head aim) |
| `props_anim_boyle_int.json` | `--map L_Boyle_Int_P --bots 0` | `pa_01`-`pa_03` | the pendulum clock with its pendulum in three different positions |
| `props_anim_krust.json` | `--map L_Flooded_FRefinery_P --bots 0` | `pk_01`, `pk_02` | river krust 0 drawn (no longer missing), its shell in two phases of `ClamUp_Idle` |
| `npc_anim_boyle.json` | `--mission missions/boyle_original.ron` | `nb_01`-`nb_04` | Samuel's boat intro at 1, 3, 8 and 15 s (matinee character and camera tracks) |
| | | `nb_05`, `nb_06` | the party guard at its post (idle, ambient, blink) |
| | | `nb_07_street_tallboy` | the street Tallboy if the original Kismet spawned it (walk loop, bow states) |

Original side: `session bones` (a new hook op, specified in `fixtures/oracle/capture_queue.txt`)
records the same per-tick line from the original's `SpaceBases` and tree state for the patrol,
detect, guard attack, Windblast throw, Tallboy, Overseer, weeper, Boyle intro, clock and krust
cases, so poses can be diffed bone by bone.

Not done: the renderer's start / stop / turn from the simulation's component (`npc_loco`, the
skinner of step 7; the rules are decoded, section 3), foot placement (U-280), the additive small hit reactions,
the weeper / hound idle behaviours and the Tallboy's stilts mesh (U-896), the distraction
matinees (U-895).
