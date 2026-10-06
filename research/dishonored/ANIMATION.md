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
full-body and upper-body layers (§3-§5); look-at, foot placement, face and blink are not
reproduced (U-140).

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

`ArkAnimNodeLocomotion` (vtable 0x0109DCB8; TickAnim slot 85 = 0x0091B280):
- 0x0090BC00 moves the node's polar velocity (angle +0x200, speed +0x204) toward the target
  (+0x1F4/+0x1F8) at a limited rate.
- 0x009331D0 maps the movement angle to the two neighbouring direction entries and weights
  (directions in octants; with n anims the step is 8/n octants).
- 0x009105B0 finds, over the speed modes of the current modifier, the two modes whose
  direction-blended authored speeds bracket the current speed and weights them linearly; weights
  within 0.98 snap to 1.
- 0x00910970 sets each playing sequence's weight; **entries whose octant is below 4 (S and W)
  play with rate -1**: the `_Revert` loops are authored reversed.
- 0x00910E30 corrects the speed for diagonals: the blended stride is the chord between the two
  direction vectors (table of 8 unit vectors at 0x014527C0), the factor is 1 / its length at the
  movement angle.
- 0x00911230 starts a `m_Transitions` sequence when the modifier changes.

Port (`npc_anim::loco_pose`): the modifier comes from the pawn's stance (§6); direction weights
blend the two neighbouring loops linearly by angle (anim k covers 180 + 90k degrees from facing);
the bracketing speed modes blend linearly by speed with the 0.98 snap; S and W play backwards;
all loops share one normalized phase (`LocoGroup`), advanced at `speed / blended authored speed`.
Not ported: the velocity smoothing (the simulation already accelerates), the diagonal correction,
start/stop/turn sequences (U-140).

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
   and attaches a weighted pick of `m_HatMeshes` / `m_MaskMeshes` (static meshes on sockets).

Port: `Appearance::materials(seed)` (same rule, deterministic per pawn id) feeds `Look::materials`,
which the renderer already applies through `load_model_with`. Elite guards get
`EliteGuardHead_v2_inst` / `EliteGuardHead02_v2_inst` half the time. Accessories are not rendered
(U-144).

## 6. What drives selection in the port

The host publishes `MatchView::npc_poses` (pawn id -> `NpcStance` from the brain state:
Relaxed, Patrol, Search, Combat, Panic, Cower, Notice) and `MatchView::anim_profiles` + `Look::anim`
(the pawn tweak's sets, locomotion config and definitions class). Clients see simulation events
(`SessionView::anim_events`).

| Situation | Original mechanism | Port |
|---|---|---|
| walking / running | locomotion modifier and speed modes | §3; modifier: guards `SwordPatrol` when relaxed, `SearchSword` searching, `Sword` in combat or noticing, `PanicSword`; civilians `Empty`, `SearchEmpty`, `PanicEmpty`; players `Sword` armed (`Aim` aiming a gun), `Empty` unarmed (Contracts disguise) |
| modifier change | `m_Transitions` | same sequences (`Sword_SwordToPatrol` ...) |
| standing still | `AmbientIdle` subprocess | §4 rule with the class-default 2-6 s timer (U-145) |
| melee swing (`SwordSwing`) | `AttackShort/Medium/Long` chains from the AI attack pattern | `AttackShort`, random group, the `Small` step (the swing: it carries `DishonoredNotify_AttackZone`) |
| blocking | `Parry_AwaitingImpact` | same, held while blocking |
| parry (`SwordParried`) | `Parry` groups | defender: `Parry` group 0 |
| stagger | `Stun` | `Stun` group 0, loop held while staggered |
| hit (`Damage`) | `HitReactStrong` / `HitReactProjectile` / `HitReactWindblast` / additive small | directional step by attacker position; projectiles pick head (headshots) or body steps |
| assassinated | `Assassination` (`_Slave` sequences synced to the player's) | directional step, then held as the corpse |
| death | `Death` / `DeathFast` ... then `DishonoredNotify_Ragdoll` (e.g. 2.299 s into `Generic_DeathFrontA`) | `Death`, directional step, last frame held (U-146) |
| panic start | `Panic` | same, then `PanicEmpty` locomotion |
| cower | `DisAISubStateCower` (0x00B12840; animation not identified) | `Beg` chain held (SPECULATIVE, U-141) |
| noticing | `ReactToNoticing` | same |
| talking | soiree matinees / FaceFX (not decoded) | upper-body `Gesture_*` loops from the profile's gesture set (APPROXIMATION, U-142) |
| crouched armed player | no crouch locomotion in the NPC sets | `Sword_Cover_Loop` held |

Directional steps (U-143): groups of 4 are Front, Left, Right, Back; groups of 8 or more start
Front, FrontLeft, FrontRight, Left, Right, Back (inferred from the sequence names; the native
index computation was not traced).

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
