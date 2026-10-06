# NPC and third-person animation in Dishonored

How the original picks the animation an NPC plays, and how the port reproduces it for NPCs and
remote players. Executable: `Dishonored.exe` SHA-256 `70da9613…` (addresses below). Retail data
is read at runtime; nothing here is copied into the repository.

Implementation:
- data: `crates/dishonored_ir/src/npc_anim.rs` (types), `crates/dishonored_assets/src/npc_anim.rs`
  (decoders), `crates/dishonored_assets/src/characters.rs::resolve_appearances` (per-tweak profile
  and material variations);
- runtime: `crates/dishonored_runtime/src/npc_anim.rs` (bank, locomotion, chains, layering),
  `characters.rs::animate_characters`;
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
`spine_0_jnt` up, additive hit reactions and look-at on top. The port reproduces the locomotion,
full-body, upper-body, blink, face and look-at layers (§3-§4, §9); foot placement and the
additive small hit reactions are not reproduced (U-280). Matinee anim tracks (the
`MatineeStackNode` channels) play over the result (`PawnAnim::scripted`); keys with
`m_MatineeRootMotionMode` keep `root0_jnt` at its bind translation because their root
translation moves the pawn on the server (CUTSCENES.md, U-085 / U-202).

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

Port (`npc_anim::loco_pose`, `smooth_velocity`, `diagonal_factor`): all of the above except the
turn sequences. The pawn's velocity relative to its facing is smoothed with the node's
parameters; directions blend linearly between the two neighbouring loops with the 0.98 snap; the
diagonal ratio scales the speed used for the bracket and the rate; S and W play backwards; all
loops share one normalized phase advanced at the sync-group rate; modifier changes crossfade over
0.5 s and transitions wait for the speed to drop below `m_fVelocityToStopMoving`. Start and stop
sequences play when the pawn's speed crosses `m_fVelocityToStartToMove` (60) upward or
`m_fVelocityToStopMoving` downward (the stop foot from the loop phase), with the root
translation discarded since the simulation moves the pawn (SPECULATIVE trigger, U-280). Turns
in place are not played (the simulation turns the pawn; U-280).

Footsteps: the NPC loops carry no `DishonoredNotify_Footfall` (e.g. `Empty_WalkN` has no
notifies at all). The node instead keeps `m_bFootOnGround` per `m_Feet` entry (0x009114A0): a
foot is planted while the circular distance between the group's sync position and the foot's
`m_fOnGroundPct` (class defaults: left 0.44, right 0.94; Boyle configs keep them) is at most half
the weighted `m_fFootOnGroundPct` of the moving speed modes (guard walk 0.4, run 0.2; normalized
when their weights sum below 1). The port evaluates the same and reports each foot touching down
to `AnimSounds::foot_down`, which plays a footfall like the notify path (0.25 s per foot), so
third-person NPC steps no longer use the per-stride fallback (LIKELY: the consumer of
`m_bFootOnGround` that posts the original's NPC footstep was not traced). The sync-group master's clip window
is also reported, for loops that do have notifies (player rigs, other sets).

## 4. Animation definitions (`DisAnimDefinitions`)

`m_Animations` is indexed by `eDisNPCAnimations` (138 entries, `dishonored_ir::npc_anim::NPC_ANIM_NAMES`).
Each entry is a `DisAnimationChain`: alternative `m_Groups` (one is picked); each group a chain of
`DisAnimation` steps (`m_Name` alternatives, `m_bLoop`, `m_fBlendInTime` default 0.2,
`m_FirstItemPrefix`), `m_fBlendOutTime` (0.1) and `m_bEndChainAfterOneAnimation`. Steps with an
item prefix get the held item's prefix (`Sword_` / `Empty_`). The subclasses' default objects
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
relaxed civilians do not play ambient idles. The port maps the patrol stance (patrol and guard
behaviours) to the first set and the search stance to the second.

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

The host publishes `MatchView::npc_poses` (pawn id -> `NpcStance` from the brain state:
Relaxed, Patrol, Search, Combat, Panic, Cower, Notice) and `MatchView::anim_profiles` + `Look::anim`
(the pawn tweak's sets, locomotion config and definitions class). Clients see simulation events
(`SessionView::anim_events`).

| Situation | Original mechanism | Port |
|---|---|---|
| walking / running | locomotion modifier and speed modes | §3; modifier: guards `SwordPatrol` when relaxed, `SearchSword` searching, `Sword` in combat or noticing, `PanicSword`; civilians `Empty`, `SearchEmpty`, `PanicEmpty`; players `Sword` armed (`Aim` aiming a gun), `Empty` unarmed (Contracts disguise) |
| modifier change | `m_Transitions` | same sequences (`Sword_SwordToPatrol` ...) |
| standing still / patrolling | `AmbientIdle` subprocess of the behaviour | §4: patrol stance 30-60 s upper body, search stance 2-6 s full body, relaxed none |
| melee swing (`SwordSwing`) | `AttackShort/Medium/Long` chains from the AI attack pattern | `AttackShort`, random group, the `Small` step (the swing: it carries `DishonoredNotify_AttackZone`) |
| blocking | `Parry_AwaitingImpact` | same, held while blocking |
| parry (`SwordParried`) | `Parry` groups | defender: `Parry` group 0 |
| stagger | `Stun` | `Stun` group 0, loop held while staggered |
| hit (`Damage`) | `HitReactStrong` / `HitReactProjectile` / `HitReactWindblast` / additive small | directional step by attacker position; projectiles pick head (headshots) or body steps |
| assassinated | `Assassination` (`_Slave` sequences synced to the player's) | directional step, then held as the corpse |
| death | `Death` / `DeathFast` ... then `DishonoredNotify_Ragdoll` (e.g. 2.299 s into `Generic_DeathFrontA`) | `Death`, directional step, then the presentation ragdoll from the notify (§8) |
| first seen dead (late join, corpse spawners) | the corpse is already a ragdoll at rest | the end of `Death`, then the ragdoll run to rest at once (§8, U-147) |
| sleep dart | `m_bSlowDeathOnly`: the "death" is the knock-out | `Death`, then the ragdoll |
| held in a choke | `StateNPCMasterBeingChoked`: the `Choke` chain, one group per choke state | `Generic_Choke_In_Slave` -> `Loop_Slave` held while choked; on the choke's end `Win_Slave` (then the ragdoll), `Lose_Slave` or `Cancel_Slave` |
| carried | `CorpseBeingCarried` (steps = carry states: In, Idle, Walk, DropLow, DropLowSneak, DropHigh, DropNoMove, DropFetal) | In, then Idle or Walk with the carrier's speed, drawn at the carrier's root; on the drop the ragdoll starts from the carried pose |
| panic start | `Panic` | same, then `PanicEmpty` locomotion |
| cower | `DisAISubStateCower` (0x00B12840): body stance `PanicRecover` (`Equipped` when armed), face the enemy | `PanicRecoverEmpty` modifier (`Empty_Idle_Panic_Recover`), `Sword` when armed |
| noticing | `ReactToNoticing` | same |
| talking | the blurb's FaceFX animation on the face and head (§12); body gestures only from soiree matinees (U-142, resolved: blurbs carry no animation, `DisBehaviorBlockingDialog` only faces the listener, no `eDisNPCAnimations` chain is a gesture, and the FaceFX actors pose `head_jnt` and the face bones only) | FaceFX lip sync and head motion (§12); body: the soiree matinee's AI-group anim tracks (CUTSCENES.md) |
| crouched armed player | no crouch locomotion in the NPC sets | `Sword_Cover_Loop` held |

Directional steps (U-143): the step index is a direction enum's value. Groups of 4 follow
`ECardinalDirection` (Forward, Left, Right, Back): `StateNPCMasterStrongHitReact` plays its
chain step `m_Direction` (+0x6C) and turns the pawn by +90 / -90 / 180 degrees for Left / Right /
Back (0x00A78980). Groups of 8 or more follow `eDisDeathDirection` (Forward, Forward_Left,
Forward_Right, Left, Right, Back, Back_Left, Back_Right, then the `_Up` variants Forward_Up ...
Back_Up at 8..13), matching the 14-entry `Death` / `DeathFast` groups. CONFIRMED from the enums.
The port computes the direction from the attacker position with 90-degree (cardinal) and
45-degree (eight-way) sectors; the sector thresholds and when the `_Up` variants are used were
not traced (LIKELY).

Cower (U-141, resolved): the cower sub-state's entry (0x00B12840) issues a body-intention request
with stance `eDisNPCBodyStance_PanicRecover` (7), or `Equipped` (3) when the pawn holds a weapon,
then a face-to request toward the enemy proxy (rotation speed -100: default). No chain is
played: the cowering pose is the `PanicRecoverEmpty` idle loop (`Empty_Idle_Panic_Recover`).

Layering: full-body chains blend in over their step's `m_fBlendInTime` and out over the group's
`m_fBlendOutTime`; the upper-body slot is masked to `spine_0_jnt` and its descendants
(`UpperBodyFilter.BranchStartBoneName`). Rotations retarget by bone name; translations are taken
from the clip for the root and its children only (the pelvis drop of deaths, kneels and falls).

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
starts from the death sequence sampled at exactly the notify time (and one step earlier for the
initial velocities) and catches up in fixed steps, so a corpse falls the same way on every run
and frame rate. Bones are rebuilt from the masses (each body turned by the arc from its start
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

Port: the mode is picked from the locomotion modifier (Search / Sword / Empty) and gait; the
target's yaw and pitch against the mode's ranges weight the 3x3 grid bilinearly, for the torso
and then the head, and each delta is applied as `delta * local rotation` (UE3 additive
blending). Which target the AI looks at, the split between torso and head
(`m_HeadAndTorso_*Percentage`, `Start_Torso`, `End_Head`) and the turn speed were not decoded:
the port looks at the nearest player while alert (2000 uu) or talking (600 uu) and eases in about
0.25 s (APPROXIMATION, U-281). Blinks play the profile's `Blink` sequence on those timings.

## 10. Bodies, chokes and carrying

Data (`DisNPCAnimDefinitions_*`, `Ply_Sword_Choke_as`, `Ply_Empty_CarryCorpse_as`):
- `Choke` (95): six one-step groups `Generic_Choke_{In,Loop,Win,CarryCorpse,Lose,Cancel}_Slave`,
  each linked to the player's `Sword_Choke_*_Master`.
- `CorpseBeingCarried` (71): `Corpses_CarryCorpse_{In,Idle,Walk,DropLow,DropLowSneak,DropHigh,
  DropNoMove,DropFetal}_Slave`; the player's `Empty_CarryCorpse_{In,Idle,Walk,DropLow,
  DropLowSneak,DropHigh,DropNoMove}_Master`.

Port (third person, driven by `PlayerState::condition`, `carried_by` and the choke / knock-out /
drop events): see the §6 table. Carried bodies are drawn at their carrier's root (the slave
sequences are authored there), also in the carrier's own first-person view, where the body hangs over the shoulder in front of the camera. Skinned bodies
are no longer tipped over by the renderer (their sequences and ragdoll lay them down) and are
placed on the standing cylinder's bottom whatever their collision height (this also fixes
crouched pawns standing below the floor). Weapons are hidden on unconscious and carried pawns.

## 11. First person (`Ply_*_as`)

Player sets in Startup: `Ply_Sword_{Idle,Locomotion,Attack,Versus,Hit,Assassination,Choke,
Fatality}_as`, `Ply_Empty_{Idle,Locomotion,Hit,FullBody,CarryCorpse}_as`, `Ply_Pistol_as`,
`Ply_Crossbow_as`, `Ply_Guns_as`, `Ply_Powers_as`, `Ply_Gadgets{,_Grenade,_Heart,_SpringRazor}_as`,
`Ply_Generic_as`, `Ply_Head_Locomotion_as`. The viewmodel now plays:
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

## 12. FaceFX (lip sync and facial animation)

Reader: `crates/dishonored_assets/src/facefx.rs` (decoded from the data alone; no FaceFX SDK
code). Runtime: `crates/dishonored_runtime/src/facefx.rs`, applied in `npc_anim::animate`. Tool:
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
  - `npc_anim::animate` applies them last: rotation = delta x animated rotation, position =
    animated (or bind) position + delta. Bones are bound by name in every part.
  - Masked party heads have no `FaceFXAsset`, so they do not lip sync.
