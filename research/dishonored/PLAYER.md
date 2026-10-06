# Player mechanics of the original executable

How `Dishonored.exe` (GOG build, see LOG "Installation fingerprint") moves the player,
runs Blink and Windblast, resolves blocks and parries, and regenerates health and mana,
as recovered from its native code with Ghidra, and how `dishonored_sim` reproduces it.
Findings are written in our own words; addresses are virtual addresses in the image.
Evidence levels as in the rest of `research/`: CONFIRMED (read in code and data),
LIKELY (code read, one link inferred), SPECULATIVE.

## Method

- Ghidra headless project of the executable (analysis in a private scratch folder; no
  decompiled output is committed).
- `research/ghidra/ClassDecomp2.java`: per class, the static registration record, the
  constructor and the **primary** vtable (the longest one the constructor stores; native
  interfaces add short secondary vtables), every slot listed, and the slots that differ
  from the parent class at the same index decompiled.
- `research/ghidra/Decomp.java`: decompile functions by address, optionally their callees,
  or every function referencing a global (`refs:<addr>`); prints the values of the
  read-only constants each function reads, and initial values of globals (`mem:<addr>`).
- `research/ghidra/FNameMap.java`: the game builds its hard-coded `FName`s in large static
  initializers (`PUSH L"Attribute_GroundSpeed"`, `CALL FName::FName` at 0x00454FC0,
  `MOV [global], ECX`). The script maps 1,111 such globals to their strings, so decompiled
  code that reads `DAT_0145DF60` can be read as `Attribute_GroundSpeed`.
- `research/ghidra/StrUsers.java`, `Disasm.java`: string users, instruction listings.
- `tools/package_dump/src/bin/prop_layout.rs`: **property offsets**. UE3 does not
  serialize them; `UStruct::Link` computes them when the class loads. The tool replays
  that from the cooked packages (Core, Engine, GameFramework, IpDrv, AkAudio,
  DishonoredGame): super class size, fields in `UField::Next` order, alignment (byte 1,
  struct alignment, 4 otherwise; Plane/Matrix/Quat 16, QWord 8), consecutive bools
  packed into one dword, native interface vtable pointers present as the
  compiler-added `VfTable_I*` properties. Checks: `DishonoredPawn.VfTable_IDisStoryGroupInterface`
  lands at +0x4D4, exactly where the interface thunk of `DishonoredPawn` adjusts `this`;
  `DisTweaks_PlayerPawn.m_fAirControl` at +0x1CC is what the attribute update copies into
  `Pawn.AirControl` (+0x310); `DisTweaks_Blink.m_TargetTestExtent` (+0x22C) and
  `m_fCloseCollisionDistance` (+0x238) are what the Blink targeting reads. (Map
  properties are sized as 60 bytes, unverified; fields after a map may be off.)
- Script classes (`StatePlayerMaster*`, `DishonoredPlayerController`, ...) were read from
  the UnrealScript decompilation; almost all player logic is native.

## Class map

| Role | Class | Notes |
|---|---|---|
| Pawn | `DishonoredPlayerPawn` : `DishonoredPawn` : `GamePawn` | vtable 0x0111A3B0 (515 slots), 0x011199D0 (475) |
| Controller | `DishonoredPlayerController` | vtable 0x01118738; movement flags each tick |
| Input | `DishonoredPlayerInput` | `RawJoyUp`/`RawJoyRight` (+0x17C/+0x180) hold the raw axes (keyboard = 1) |
| Locomotion states | `StatePlayerMasterWalk/Jump/Falling/Mantle/Slide/Leaning/Climb/Swim/...` | thin: physics is UE3 walking/falling |
| Edge finder | `DishonoredMantleEdgeFinderComponent` + `DisMantleFinderState_*` | two finders: normal and Blink |
| Powers | `DishonoredActivePowerComponent_Blink/_WindBlast`, `DisActivePowerComponent_DarkVision` | |
| Melee | `DisItemContext_MeleeAttackPlayer`, `DisItemContext_MeleeBlockPlayer`, `StatePlayerMeleeAttack/Block` | |
| Tuning | `DisTweaks_PlayerPawn` (+`_Attributes`, `_Mantle`), `DisTweaks_PlayerInput`, power/weapon tweaks | attributes are read by FName through `GetAttribute` (0x00C86A50) |

## Movement

### Per-tick flags (controller, 0x00AAE610, called from 0x00AAEB10)

With `m = |(RawJoyUp, RawJoyRight)|` and the `Twk_Pawn_DefaultPlayer.Twk_Pawn_PlayerInput`
tweak (CONFIRMED):

- **Sprint** (`SetSprinting`, 0x00B497C0, `m_bSprinting` = +0x508 bit 1): sprint held,
  `m > m_fStopSprint_Threshold` (0.8), not sneaking, the current state allows it. There is
  no forward-only rule: sprinting sideways or backwards is allowed (with the sprint
  strafe/backward multipliers).
- **Walking** (`Pawn.bIsWalking`): not sprinting and `0 < m <= m_fWalk_Trigger_Threshold`
  (0.95). `WalkingPct = Attribute_GroundSpeedSlowWalk / GroundSpeed` (100/400) for
  `m <= m_fSlowWalk_Trigger_Threshold` (0.45), else `GroundSpeedWalk / GroundSpeed`
  (200/400). Keyboard input (m = 1 or 1.41) never walks. `CrouchedPct =
  GroundSpeedCrouch / GroundSpeed` (275/400) is set here every tick.
- **Strafing / moving backwards** (`m_bStrafing` bit 2, `m_bMovingBackwards` bit 3,
  exclusive, only when not walking): `a1 = acos(|strafe| / m)` (angle from the pure
  sideways axis), `a2 = acos(|forward| / m)`. Strafing when `a1 < m_fStrafeAngle_Forward`
  (10 deg, stick forward or level) or `< m_fStrafeAngle_Back` (50 deg, stick back);
  otherwise moving backwards when the stick is back and `a2 < m_fBackwardsAngle` (40).
  So W+D (45 deg) is neither, S+D is strafing, S alone is backwards.

### Speed modifier (`ADishonoredPawn::MaxSpeedModifier`, 0x00B56C40, vtable +0x480)

Walking physics uses `GroundSpeed * MaxSpeedModifier()`. `GroundSpeed` is set from
`Attribute_GroundSpeed` (400) by the attribute update 0x00B56880, `AccelRate` from
`Attribute_AccelerationRate` (2000), `JumpZ` from `Attribute_JumpZ` (800) when a jump
starts (0x00A60010). The modifier (CONFIRMED):

- base: the stock modifier (`WalkingPct` when walking, `CrouchedPct` when `bIsCrouched`,
  which is the crawl, not the sneak);
- not sprinting, not sneaking: strafing -> `GroundStrafeMultiplierRun` (0.7), backwards ->
  `GroundBackwardMultiplierRun` (0.8), else the base; the multipliers **replace** it;
- sneaking (vfunc +0x67C): strafing -> `GroundStrafeMultiplierSneak` (0.5, of the run
  speed: 200 uu/s), else `CrouchedPct x base` (275). The backward sneak multiplier is never
  reached because the controller never sets both flags;
- sprinting: `GroundSpeedSprint x (StrafeMultiplierSprint 0.6 | BackwardMultiplierSprint
  0.55 | 1) / GroundSpeed`;
- times vfunc +0x5AC (0x00AA4700): `Attribute_SwordUnsheathedSpeedFactor` (0.9) with a
  weapon out, else `Attribute_SwordSheathedSpeedFactor` (1.0), **in every stance including
  sprint**;
- blend: a drop of more than 1e-4 below last tick's value (`m_fLastSpeedModifier`, +0x58C)
  becomes `last + (new - last) * clamp(dt * m_fSpeedBlendDownSpeed, 0, 1)` (8/s, the
  pawn tweak); rises are instant.

Resulting speeds with the sword out: run 360, strafe 252, back 288, sprint 540 (sideways
324, back 297), sneak 247.5 (strafe 180), walk 180, slow walk 90.

### Acceleration, friction, jump, falling

- Acceleration is UE3 `PlayerWalking.PlayerMove`: `AccelRate x Normal(aForward X + aStrafe Y)`
  (no native override found; stick deflection only selects the walk speed). LIKELY.
- `AirControl` is copied from `DisTweaks_PlayerPawn.m_fAirControl` (0.2) by 0x00AA7910.
- Jump (StatePlayerMasterJump enter, 0x00A75EE0): `Velocity.Z = Attribute_JumpZ` (or
  `JumpZCarryingCorpse`), plus the base's upward velocity when standing on a mover; physics
  Falling. Agility's power jump: see below.
- Falling state (0x00A759F0 / 0x00A75B80): bookkeeping only. `m_fFall_Stun_Vel` 2200
  (DefaultPlayerState.ini) stuns on landing (not modelled).

### Engine physics (U-053 resolved)

The pawn physics are the engine's `APawn` functions (Pawn vtable 0x00FFC4D8; no Dishonored
override except `setPhysics` slot 134 and `physLadder` slot 282 on the player). Located by the
fields they touch (`research/ghidra/G_OffsetGroups.java`) and decompiled. CONFIRMED unless noted.

- **Physics dispatch** (`startNewPhysics`, slot 283, 0x006A24A0): Walking slot 141, Falling 140,
  Swimming slot 280, Flying 279, Ladder 282, Custom 143 (the player's goes to the current state).
- **Constants.** This build's engine constants are UE3's times 1.27: `MAXSTEPHEIGHTFUDGE` 2.54,
  `MINFLOORDIST` 2.413, `MAXFLOORDIST` 3.048, target floor distance 2.7305, `MAXSTEPSIDEZ`
  0.1016, `MINSTEPSIZESQUARED` 232.2576 (=144 x 1.27^2), braking stop speed 12.7 (161.29
  squared), swim bob -101.6 / -203.2 (UE3 -80 / -160).
- **Defaults.** `Engine.upk Default__PhysicsVolume`: `GroundFriction` 8, `TerminalVelocity` 4000,
  `FluidFriction` 0.3 (2.4 for `DishonoredWaterVolume`). `Default__Pawn`: `bLimitFallAccel` true,
  `WalkableFloorZ` 0.5, but the attribute update (0x00B56880) overwrites `WalkableFloorZ` with
  `cos(DisTweaks_Pawn.m_fMaxWalkableFloorInDegrees)` (50 deg: 0.643, the sim used 0.7) and sets
  `Pawn.CrouchHeight` from `m_fCrouchHeight` (33) and `MaxFallSpeed` from `m_fMaxFallSpeed`.
- **`CalcVelocity`** (slot 326, 0x006ACD50; args AccelDir, dt, MaxSpeed, Friction, bFluid, bBrake,
  bBuoyant): `modifier = MaxSpeedModifier()`; with `bForceMaxAccel` false the controller's
  acceleration is kept but limited to `GetMaxAccel(modifier) = AccelRate x modifier`
  (0x006A2EE0), so the effective acceleration is 2000 x min(1, modifier) (running with the sword
  1800, walking 900, sprinting 2000; the sim used 2000 everywhere). Without acceleration and with
  `bBrake`: braking (0x006A2AC0) in sub-steps of at most 0.03 s, `v -= 2 v dt Friction`, the
  result being the time-weighted average of the sub-steps while they point the old way, zeroed
  below 12.7 uu/s or when reversed (the sim stopped below 1 uu/s). Otherwise
  `v -= (v - AccelDir |v|) dt Friction` (no clamp of `dt Friction`). Then
  `v = v (1 - bFluid dt Friction) + a dt`, plus `(1 - Buoyancy) g dt` when buoyant, and
  `|v| <= MaxSpeed x modifier`.
- **`physWalking`** (slot 141, 0x006BD150): `Velocity.Z = Acceleration.Z = 0`;
  `CalcVelocity(AccelDir, dt, GroundSpeed, PhysicsVolume.GroundFriction, 0, 1, 0)`; sub-steps of at
  most 0.05 s; a move into a floor sloped below 0.98 counts as hitting it; a blocked move calls
  `stepUp`; then the floor is traced `MaxStepHeight + 2.54` down with the pawn's extent: on a
  walkable floor the pawn is moved back to 2.7305 above it when outside 2.413..3.048, else it
  falls; the velocity becomes the achieved horizontal displacement over `dt`.
- **`stepUp`** (slot 148, 0x006ADC10): a barrier with normal Z below 0.1016 or walkable is
  climbed by moving up `MaxStepHeight + 2.54`, along, and down again; a further near-vertical
  barrier with more than 232.26 uu^2 x time of move left repeats it; otherwise the move slides
  along the barrier flattened to vertical, with `TwoWallAdjust` on a second wall. Mid slopes are
  only slid up when not walking.
- **`physFalling`** (slot 140, 0x006BF0E0): the air-control move is test-swept with the extent
  (a hit zeroes air control this tick); with `bLimitFallAccel` the acceleration is limited to
  `AccelRate x AirControl` (+`(10 - speed2d)/dt` below 10 uu/s); at or above `GroundSpeed` the
  horizontal speed may not grow (`MaxAirSpeed`), or with `AirControl <= 0.05` the acceleration is
  1. Each sub-step: `v1 = NewFallVelocity(v0, a + g, dt) = v0 (1 - fluid dt) + (a + g)(1 - buoyancy)
  dt` (0x006A3860), move `v1 dt`; landing on a walkable normal (the velocity becomes the measured
  one when more than 10 % of the tick moved); walls slide (`ComputeSlideVector`, slot 290, never
  rising faster than the move) with the two-wall and ditch tests; finally
  `v = 2 (moved / dt) - v0` when rising or speeding down, capped at the terminal velocity. The
  velocity therefore gains `2 a dt` per tick: gravity -1500 and air control act twice as hard as
  their values. A JumpZ 800 jump rises 106.7 uu in 0.27 s (the sim rose 213 uu); the impact speed
  after a drop `h` is about `sqrt(4 g h)`, so falling damage starts at 844 uu and kills at 1402 uu.
- **Collision shape (U-016 resolved).** Every world trace of the pawn passes the cylinder's
  extent `(radius, radius, CollisionHeight)` (e.g. the air-control test above), and UE3 extent
  traces against kDOP trees and BSP are axis-aligned box checks. The sim sweeps an axis-aligned
  box (`dishonored_world::PawnBox`, separating-axis box/triangle test) instead of a capsule.
- **Crouch height (U-017 resolved).** The save/restore of the stealth state (0x00AD3950) lifts the
  pawn by `CylinderHeight - m_fSneakHeight` when it was sneaking and by `CylinderHeight -
  CrouchHeight` (and calls `Crouch`) when crouched: sneaking uses `m_fSneakHeight` (65), the
  crawl (`bIsCrouched`, `Pawn::Crouch` 0x006A88E0) `m_fCrouchHeight` (33). The sim already did.

### Agility (`Celerity`)

`StatePlayerMasterJump` with `m_JumpStyle = FixedNormalJump_FixedPowerJump` (DefaultPlayerState.ini),
tick 0x00A76110: while rising, `m_fHeldButtonTime` counts the time the jump button is held (reset
when released); at the top of the jump (`Velocity.Z <= 1e-4`) a hold of more than 0.05 s with
`Attribute_JumpZ_PowerJump > 1e-4` (0x00A60060) sets `Velocity.Z = Attribute_JumpZ_PowerJump`
(0x00A71370) once; otherwise the jump hands over to falling. Corvo's base is 0; Celerity level 1
adds 1300 and sets the falling damage speeds to 2550 / 3500; level 2 adds sprint +30 %, swim +10 %,
sprint strafe / back multipliers 0.5 / 0.4 (DefaultPlayer.ini `m_Powers`). CONFIRMED.

### Swimming

- Water volumes: `DishonoredWaterVolume` (`bWaterVolume`, `FluidFriction` 2.4, `m_bShallowWater`)
  brushes, extracted per map (`dishonored_assets::water`, cache `<map>.water1.json`): Boyle Ext has
  three (L_Boyle_Ext_Env), Boyle Int one (L_Boyle_Int_Basement). A pawn whose centre enters a deep
  one swims.
- `startSwimming` (0x006B6360): entering slowly from above (downward below 203.2) dips to
  `-101.6 - 0.7 |v.xy|`.
- `physSwimming` (slot 280, 0x006B7820): `GetNetBuoyancy` (0x006A41F0) scales `Buoyancy` (1.0) and
  `FluidFriction` by the submerged share of the cylinder; rising faster than 127 is scaled by
  `NetBuoyancy / Buoyancy`; `CalcVelocity(dir, dt, WaterSpeed, FluidFriction / 2, 1, 0, 1)` (with
  Buoyancy 1 there is no gravity in water); leaving the volume switches to falling and a slow
  rise (0..160) becomes `0.4 |v.xy| + 40`.
- `StatePlayerMasterSwim`: the swim acceleration (0x00A60690) is `m_fMaxSwimAccel` (4000) during a
  stroke, else `m_fMinSwimAccel` (500) below `m_fMaxSpeedNoStroke` (350), else 0 (limited to
  `AccelRate x modifier` by `CalcVelocity`). At the surface (`m_pActiveWaterVolume` empty) a stroke
  is due every `m_fSurfaceSwimDelayBetweenStrokes` (2 s) but `m_bConstantSpeedAtSurface` suppresses
  it (0x00A76710). Underwater strokes come from the swim animation (`DisNotify_SwimStroke`), not
  decoded (U-243). `Attribute_WaterSpeed` 600.
- Breath (0x00AD7BE0): while `m_bUnderWater` the air (`m_fApneaMaxDuration` 30 x
  (1 + `Attribute_BreathCapacityIncrease`), 0x00ACF570) runs down; otherwise it refills at
  `max / m_fApneaRecoveryDuration` (5 s) per second. Without air (0x00ABB1E0) health loses
  `Attribute_DrowningDamageAmount` (1) every `Attribute_DrowningDamageStep` (1 s) directly, floored at
  `m_MinimumScriptedHealth`, death below 1. Health regeneration below the limit is not reset by it
  (U-242).

### Climbing (U-240 resolved)

`DisClimbable` (class record 0x012F76B0, vtable 0x010E7A80; interactable interface vtable
0x010EC418 at +0x2A0) is a `DishonoredKAsset`: a skeletal mesh driven by a physics asset.
`StatePlayerMasterClimb` (record 0x01300048) hangs the player on it with physics Ladder (9), which
the player pawn routes to its own `physLadder` (slot 282, 0x00AA82C0). Ported in
`dishonored_sim::climb` as `MoveMode::Climbing`. CONFIRMED unless noted.

- **Grab.** Three ways, all ending in the interactable's `Interact` (interface slot +0x60,
  0x00A23130: the player pawn only; requests the climb state, then sets `m_pClimbable` +0xAA0):
  Use on the focused chain (Use handler 0x00AD5BA0 -> 0x00A3DF60; prompt
  `m_InteractText` "`GBA_Use` Grab"; visibility test slot +0x1C 0x00A1E6F0); touching the chain
  (collision notify, vtable +0x290, 0x00A3FA70) while the pawn has no `m_pClimbable`, is not
  `m_bLettingGoOfClimbable` (+0x8A0 bit 6) and its physics is neither Walking nor None (jumping,
  falling, Blinking); and the Blink step's probe hitting a chain (0x00BECDF0). The sim takes the
  Use target like a pickup's (interact distance, 30 deg cone, clear line; LIKELY) and the touch as
  the pawn box overlapping the bodies' line thickened by their half width (5.05).
- **Enter** (0x00A72B00): physics Ladder; the chain's collision off (0x00A1A170: `bCollideActors`
  and the component's `CollideActors` cleared); `m_fStartTime`; the chain's `DisSeqEvent_Climb`
  output Start (0x00BCFAC0; record 0x01300200, outputs Start / Finish). HUD contexts 12 / 13
  (`DUI_Context_ClimbableDrop` / `_ClimbableJump`, slot +0x174 0x00A67830).
- **`physLadder`** (0x00AA82C0): `Velocity = 0`. With a non-zero acceleration: step =
  `m_fClimbingSpeed x 16383.5` (`DAT_0111870C`; 0.0005 -> 8.19 uu); `m_bClimbingUp` (+0x8A0 bit
  17, default true) becomes `T' < pitch` with `T' = -m_ClimbingAimThreshold` while up, `+T`
  while down (1000 rotator units, 5.5 deg; the pitch is the camera's
  `CameraCache.POV.Rotation.Pitch`, camera +0x33C, through 0x00B515C0); the step is negated when
  `m_bCameraRelativeClimbing` (+0x8A0 bit 16) and not climbing up, and again when the
  acceleration's 2D direction is within `acos 0.9` of the facing or less (`dot <= 0.9`): forward
  climbs up, back or sideways climbs down. Target height `z + step`, clamped to
  `[Location.z - 2 x SkelComp.Bounds.SphereRadius, Location.z - CylinderComponent.CollisionHeight]`.
  Without input the height stays (not clamped). Horizontally the target is the bone the
  skeletal mesh component finds closest (`FindClosestBone`, 0x00713130, over every bone) to the
  pawn's centre raised by `2/3 x CollisionHeight`, at its world location (`GetBoneLocation`
  0x00732CD0), moved 30 uu back along the pawn's facing and 10 uu to its right (`yaw + 0x4000`).
  The pawn goes `VInterpTo(Location, target, dt, 20)` (0x0041E860) with `MoveActor`; a blocked
  move slides along the wall and then gets `TwoWallAdjust` (0x0065FC60). Climbing speed: 8.19 x 20
  = 163.8 uu/s whenever `20 dt < 1` (frame-rate independent above 20 fps).
- **Sound**: when the height changed (relative tolerance 1e-4), a step sound
  (`m_pClimbingSound`, RTPC `m_pClimbingSoundVolumeParameter` = |step|) plays if more than
  `min + (max - min) x |pitch| / 16383.5` s passed since the last (`m_fClimbingSoundMin/MaxDelay`
  0.3 / 1.8 of `Twk_Pawn_Corvo_Release`). The sim emits `SimEvent::ClimbSound`.
- **Leave.** Transition test (slot +0x138, 0x00A6E570): to Falling, and to Jump when
  `m_bClimbReuseDelayJump` (true), only after `m_fClimbReuseDelay` (`Twk_Pawn_PlayerInput`: 0.5;
  class default 0.3). Exit (0x00A6E2C0), whatever the next state: the chain's Finish output;
  `Velocity = Rotation.Vector() x m_fJumpImpulse` (525; the pawn's rotation is taken as
  yaw-only, LIKELY), doubled when a trace from the pawn 200 uu straight up (`DAT_00FB96C4`, flags
  0x20DF) reports a hit (LIKELY reading of the trace result, U-443).
  Into Jump: the Jump enter (0x00A75EE0) then sets `Velocity.Z = JumpZ` (800) and sets
  `m_bJumpingFromChain` (+0x54 bit 0); the chain stays in `m_pClimbable` (no re-grab) until that jump state
  ends (0x00A5FFA0), which starts the chain's collision restoration (`m_fNoCollisionDuration =
  m_fCollisionRestorationDelay`, 0.3 s, counted down in the chain's tick 0x00A2BFD0) and clears
  `m_pClimbable`. Use while climbing (0x00AD5BA0 -> 0x00AAA030) requests Falling and sets
  `m_bLettingGoOfClimbable` (cleared again by the Falling state's exit, 0x00A63110: landing,
  water, a mantle); the climb exit then restores the chain's collision at once and clears
  `m_pClimbable` (unless 0x00B50C70 holds: one of the pawn's active power components, +0x788,
  is of the class initialised next to the Blink code at 0x00BECD90 and reports itself active,
  LIKELY "Blinking"; then the 0.3 s restoration). The same test lets a Blinking pawn grab by
  touch even when its physics reads Walking. A chain switched to physics None
  drops its climber the same way (vtable +0x218, 0x00A1E800).
- **No end exits.** `physLadder` only clamps: the pawn stops at the bottom and at the top (head at
  the anchor). Getting off at the top is a jump, whose airborne mantle finds the ledge.
- **Chains** (`dishonored_assets::climbable`, cache `<map>.climb1.json`): the actor's
  `Location`, its `KAssetSkelMeshComponent` mesh and physics asset. Both Boyle meshes are vertical:
  `Climbable.chain_400` (L_Boyle_Int_Basement `DisClimbable_1` at (-3153, -420, -627); root bone
  4.6 above, `chain_jnt_01..05` every 100 uu down; 5 box bodies, the root fixed) and
  `Climbable.chain_1400` (L_Boyle_Ext_Env `DisClimbable_3` at (-2085, 580, 1884); 16 bones,
  `chain_jnt_015` 1400 below). The bounds come from the physics bodies in the rest pose (box
  elements through their bone-space `TM`): sphere radius 198.04 (Int) / 697.57 (Ext), so the
  pawn's centre ranges over z -1023.1 .. -714.5 (Int) and 488.9 .. 1796.5 (Ext).
- **Not modelled**: the chain's rigid-body swing (the sim keeps the rest pose, which is where an
  undisturbed chain hangs; Windblast pushes chains by `m_fWindblastForceFactor` 10, swinging
  sounds), Blink and other powers cast from the chain, the setter of
  `m_bCameraRelativeClimbing` (U-440; no AND/OR write of its bit, dword or byte, exists).

### Blood Thirsty

Decoded in `dishonored_sim::adrenaline` (0x00AAC510 gain, 0x00AB2120 burn, 0x00AA21E0 full test,
0x00C10EF0 finisher test): the gauge (max `Attribute_AdrenalineMax` 250, level 2 -20 %) fills from
sword hits (damage x `m_fAdrenalineMultOnHit` 2.0), assassinations (victim health x 1.25, drop
x 1.5), won parries (+`m_fAdrenalineAddedParryWin` 20), damage taken (x 0) and rat kills; after
`Attribute_AdrenalineCooldown` (10 s) without a gain it burns at 1/s; full, the next melee attack
on an NPC is a finisher (kill, gauge spent). Players are not finished (U-245).

### Dark Vision

`DisActivePowerComponent_DarkVision` (vtable 0x01168870): use (0x00BE89E0) starts it (0x00BF82A0)
with the mana: fade-in `m_EyeLidClosingDuration` (0.05), duration `m_fDuration x (1 + SpellStrength)`
(30), fade-out `m_PowerFadeOutDuration` (0.2), cooldown `m_fCooldownTimer` (1 s); using it again
after the cooldown zeroes fade-in and duration (the fade-out plays); the tick (0x00BFBC90) spends
the step on fade-in, duration, fade-out in turn. One-time cost (20), no drain. It shows the souls
of pawns within `m_fDistance` (2000) at both levels (rats included); the sim reveals those pawns.

### Armour

The player's TakeDamage (0x00AB0A30): damage types with `m_bApplyDamageReduction` (default true;
false for bites, electricity, plague, whiskey fire, music, energy) lose `(int)Attribute_DamageReduction`
(the Armor upgrade, 5), never below 1; `m_bPushWithoutDamage` types deal 0. NPC helmets
(`Twk_Inv_EliteHelmet`, head region) are knocked off by sword fast hits, bolts and bullets on the
head and by Windblast and flare bolts anywhere; what they absorb is in the base tweak (U-246).

### Windblast on players (U-051 resolved)

The blast (0x00BE9770) hands its momentum to `TakeDamage`; the pawn damage path (0x00B5CCD0) passes
it to the hit reaction handlers (the player's: camera / state reactions, 0x00AAC730, 0x00AB2420,
0x00AB26E0, none moves the pawn) and then clears it. NPCs are thrown (ragdoll). The sim no longer
pushes players; NPC pawns keep the velocity change.

### Pistol upgrades

The magazine holds `1 + (int)Attribute_MagazineBonus` rounds, capped by the ammo held (0x00C20DB0);
`Attribute_ReloadSpeed` divides the reload time (0x00C23620, LIKELY: the divided tweak time is the
reload delay). `Attribute_FastReload` selects the fast reload animations (not modelled, U-247).
The accuracy upgrades change the dispersion (next section).

### Ranged weapon dispersion (2026-10-06, U-247)

CONFIRMED from the code unless marked.

- **State.** `DishonoredWeapon_Ranged.m_fDispersion` (+0x12C; slot 91 0x00C1CAC0 returns it).
  It is set to `Attribute_DispersionMax` on equip (slot 76, 0x00C2C1F0) and after every shot.
  The projectile context's camera update (0x00C03A60) fires each queued shot through the item's
  fire function (pistol 0x00C2A3E0), then calls the weapon's slot 109 (0x00C1CB80): set to max.
  Slot 98 (0x00C312A0) also sets max when its third argument is set; when it is called is not
  traced (U-421).
- **Narrowing.** The weapon tick (slot 94, 0x00C1CB10) runs only while a round is loaded
  (+0x124 > 0). It moves the value toward `Attribute_DispersionMin` at
  `Attribute_DispersionInterpolationSpeed` per second (`FInterpConstantTo` 0x0041EBC0).
- **Held at max.** The flags `DisTweaks_WeaponRanged.m_bMaxDispersionOnSprint` / `Jump` /
  `Block` / `Melee` (+0x118 bits 0..3) hold it at max in those states. The falling state's tick
  (0x00A79150) tests the Jump bit every tick. Corvo's pistol and crossbow set Sprint, Jump and
  Block, not Melee.
- **Values** (Normal column, `DisTweaks_WeaponRanged_Attributes` of `Twk_Inv_PistolCorvo` /
  `Twk_Inv_CrossbowCorvo`):

  | weapon | max | min | speed |
  |---|---|---|---|
  | pistol | 45 | 15 | 70 |
  | crossbow | 30 | 0 | 70 |

  Upgrades only apply to their own weapon (`DisTweaks_Upgrade.m_pHostItemType`):
  `Pistol_Accuracy1` max -10 / min -10; `Pistol_Accuracy2` max -15 / min -3 / speed +20;
  `Crossbow_Accuracy` max -10 / speed +40.
- **The shot.** 0x00BFFBA0 draws `r = rand() x d / 32767` and `a = rand() x 2 pi / 32767 - pi`.
  The radius is uniform, so shots gather toward the centre. The offset is `(r cos a, r sin a)`.
  0x00AA2B10 scales it by `screen height / 720`, adds it to the aim point (the screen centre, or
  the aim-assist point), and deprojects that point through the view (0x006C5180). The result is
  the projectile direction. So **dispersion is a radius in pixels of a 720-line screen**. With the
  default 75 degree horizontal FOV on 16:9, 45 px is 3.09 degrees and 15 px is 1.03 degrees.
- **HUD.** 0x00B9FF30 sends `SetDispersion(d, d / max x 100)` to the crosshair when `d` changed
  by at least 0.1.
- **Not found:** `DisTweaks_PlayerPawn.m_fMovementDispersionIncrease` and
  `m_fCameraMovementDispersionIncrease` (+0x2AC / +0x2B0, both 0.5). An offset scan finds no
  player code that reads them; the hits are other classes with the same offsets (U-422).
- **Port** (`left_hand::update_dispersion`, `fire_direction`). The state is
  `LeftHandState.dispersion`. Draws come from the pawn's random stream (two rolls per shot). The
  view is a fixed reference, the default FOV on 16:9 (`DISPERSION_REF_*`): the original uses
  the player's actual view, which the server does not know. "Jump" is the Falling move mode.
  Sleep darts and incendiary bolts count as loaded while not reloading. The crosshair does not
  spread yet (U-420).

### Falling damage (`TakeFallingDamage`, 0x00B4DCC0, vtable +0x528)

`v = -Velocity.Z` at landing; above `Attribute_MaxSpeedBeforeFallingDamage` (2250):
`damage = (int)(HealthMax x clamp((v - 2250) / (2900 - 2250), 0, 1))` with
`DmgType_Fell`; the damage path (0x00B5CCD0) then applies
`Attribute_FallingDamageModPercent` (0 for Corvo). CONFIRMED.

### Slide (`StatePlayerMasterSlide`)

`Dis_SneakOrSlide` while sprinting at `m_fMinSpeedToSlide` (400, input tweak) or more.
Enter (0x00A67FA0): start velocity = horizontal velocity; target = its direction x
`Attribute_GroundSpeedCrouch` x sword factor; the pawn sneaks. Physics (0x00A6F440):
`v = v0 + (target - v0) x (1 - cos(pi t / m_fSlideTime)) / 2` (`m_fSlideTime` 1.1 s) until
the speed falls below the target speed; the slide ends when the velocity turns more than
`m_fSlideVelocityDeactivateAngle` (25 deg) from the start direction or after
`m_fSlideTime` (0x00A7B220), or on a fall. CONFIRMED. Cancel (0x00A71450): a cancel request
ends the slide once `m_fSlideTime x m_fSlidePercentNotCancelable` (1.1 x 0.5 = 0.55 s) has
passed; the sim takes the request from releasing the stick (LIKELY: the caller was not
traced). Slide impact damage (`m_SlideDamageInfo`: 0 to NPCs and rats, 5 to other actors,
`m_fSlideImpactDamageVelRatio` 0.15) is not modelled.

### Lean (`StatePlayerMasterLeaning` 0x00A76590, `DishonoredCamera_Lean`)

The state feeds the camera a lean request of length `m_fLeanSpeed` (1000) in the
left/right/forward direction. The camera influence (activation 0x00AC5420, update
0x00AC5950, apply 0x00ACAAB0) keeps the head as a spring point (`m_LeanConstants` 80 /
12) at `m_fCurLeanHeight = m_fLeanHeightPct (2.0) x CollisionHeight x 2` (350 uu
standing, from `GetBoundingCylinder`) from a pivot that far below it, re-projecting the
point onto that sphere each step, with the angle limited by `m_fMaxLeanAngle` (15;
crouched 15). At full lean the eye moves 90.6 uu sideways and 11.9 uu down (sneaking:
67 / 8.9). The sim swings the eye that way, with the spring acting on the normalized
angle (LIKELY: the original springs the head point itself, U-048).

### Mantle

`DishonoredMantleEdgeFinderComponent` runs `DisMantleFinderState_ForwardSearches`
(0x00ADA6F0 setup, 0x00AD6600 step), then `CheckDestinationValid`, `CheckCanStand`
(`m_FinalVerifyLocationGridSize` 6), spread over ticks (`m_fMaxMantleCheckTimePerTick_ms`).
The forward search traces horizontally (`m_MantleEdgeSearchDist` 50 beyond the cylinder)
every `m_LineCheckStepSize` (12) of height; the face must be within
`m_MaxVerticalAngleForEdgeFace` (20) of vertical and `m_MaxHorizontalAngleForEdgeFace`
(45) of the facing. A second finder uses `pMantleBlinkTweaks` (same values except
`m_MantleMinEdgeHeight` = 0) and is mantled when a Blink ends (0x00BE7420 ->
0x00AD78D0); a found edge no higher than `m_LowMantleMaxEdgeHeight` (120) becomes a
step-up (`m_bLowMantleUseStepUp`). The sim implements the face rules, the 12 uu scan,
the ledge-grab heights and the Blink mantle; the later finder states are reduced to a
capsule clearance test and mantle durations are still guesses (U-019).

### Physics volumes (zone velocity, pain)

- Non-water `PhysicsVolume`s with a `ZoneVelocity` or `bPainCausing` are imported per map
  (Boyle Ext: 24 fence volumes, 1 damage per second of `DisDamageType_SpringRazor` or `_Impact`,
  zone velocity (40, 90, 25), (-40, -70, 25) or (0, 300, 300)). A pawn's volume is the highest
  `Priority` one containing its centre.
- Walking: `DesiredMove = Velocity + ZoneVelocity x 25 x dt` (Z dropped, `bVelocityAffectsWalking`
  default true), moved by `DesiredMove x dt`. Falling: moved by `(Velocity + ZoneVelocity) x dt`.
- Pain (Engine `PhysicsVolume` script): a `VolumeTimer` from level start pops every `PainInterval`
  (default 1) and every damageable actor touching the volume takes
  `int(DamagePerSec x PainInterval)` of the `DamageType`; `bEntryPain` (default true) hurts on entry.

## Blink (`DishonoredActivePowerComponent_Blink`)

Levels: `Twk_Powers.Blink.Twk_Blink` level 2 = horizontal 1600, vertical 500, step 100,
time between steps 0.01; extent 20^3; close-collision 300, offset step 30.

**Targeting** (0x00BF5520, called every tick while aiming):

1. Range vector (0x00BF52E0): view direction `d`. If `d.z <= 0`: `d x m_HorizDistance`.
   Else `t = min(m_VertDistance / d.z, m_HorizDistance)`, `v = d x t`,
   `v.z = min(v.z, m_VertDistance - g)` with `g` the height above ground only when
   `m_bLimitVerticalDistanceFromGround` (false). Its direction becomes the aim.
2. Box sweep (`m_TargetTestExtent`) from the camera along it (trace flags 0x4060DF). If the
   hit is at the start, retry from the pawn view location.
3. A hit nearer than `m_fCloseCollisionDistance`: measure the free headroom above the start
   (point trace up `m_fCloseCollisionOffsetStep`), then try sweeps from the start offset by
   `(0,0,step x headroom - extent.z)`, `-(0,0,step)`, `(0,step,0)`, `-(0,step,0)` to the hit
   point pushed one step further; the first clear one is re-swept to the far end.
4. Back-off (0x00BE8F20): from the hit (or the far end) step back along the aim by half the
   pawn's bounds sphere radius (|(35,35,87.5)| / 2 = 50.25) until a box of
   (radius, radius, `Pawn.CrouchHeight` 33) fits; at most the camera distance.
5. Ground (point trace 20000 uu down): raise the point to at least `CollisionHeight` (87.5)
   above the floor below.

**Release** (0x00BFC990): cancel if the target is within the bounds sphere radius (100.5)
of the pawn; else the move starts (0x00BE7240): save location, velocity and physics,
`m_MaxDistanceFromOrigLoc` = distance to the target, physics Flying.

**Move** (0x00BFC160): the step timer counts down and, when it runs out, gains
`m_fDefaultTimeBetweenBlinkSteps` (0.01) back, so **at most one step per frame**; each
step moves `m_fDefaultBlinkStepDistance` (100) toward the target (0x00BECDF0: 50 uu
sub-steps with `FarMoveActor` **without collision**, only a forward probe for pawns and
special blockers). The move ends when the target is behind the current camera direction,
within 25 uu horizontally (`DAT_01359B8C`), farther from the start than the target was, or
the pawn did not move. End (0x00BFAB80): **the saved velocity and physics are restored**
(momentum is kept, a falling Blink keeps its fall speed), then the Blink mantle runs.
At 60 Hz that is 6000 uu/s (16 ticks for a full 1600 uu Blink); the original's speed
depends on its frame rate.

## Windblast (`DishonoredActivePowerComponent_WindBlast`)

Level 2 of `Twk_WindBlast`: force 2700, extra up force 1600, angle 135, damage 30,
distance 3000, speed 5000; `m_bUseDirectionalWindblast` true. Class defaults: warmup
0.1 s, cooldown 1.0 s.

- Modes (tick 0x00BFB9E0): warmup, start (0x00BF9570: origin and direction from the
  camera; direction flattened unless directional), expanding front (0x00BF7090: reach =
  elapsed x speed, capped at `distance x (1 + Attribute_SpellStrength)`), cooldown.
- Targets (0x00BF6780): actors within range with a positive factor, gathered at start.
- Factor (0x00BE8570): with `d` the 3D distance and the dot product taken in the horizontal
  plane against the direction's x/y: zero unless `dot > cos(angle / 2)`; else
  `(1 - min(d / range, 1))^2`, times `((dot - cos) / (1 - cos))^2` beyond 100 uu.
- Hit (0x00BF6AF0 when the front passes the target's bounds centre, with a sight check
  0x00BEDC90): impulse `dir x force`; its z is raised to `(1 - min(d / range, 1)) x
  extraUp` (added to a downward z); then (0x00BE9770) damage `round(damage x k)` with the
  windblast damage type and momentum `(impulse.xy x k, 0.15 x |impulse| x k)`.
- NPCs are thrown by that momentum; players are not (see "Windblast on players").

## Melee

- Block (`UDisItemContext_MeleeBlockPlayer`, hit handler 0x00C22B60): the attack is
  **parried** when the block started less than `m_fTapForPerfectParryWindow` (0.2 s) ago or
  the block animation's `DisNotify_BlockParryZone` is active (0.001-0.431 s, U-021), unless
  the forced block after a tap parry (`m_fForceBlockTimeForPerfectParry`, set to
  `m_fForceBlockTimeOnPerfectParryTap` 0.3 s) is running, in which case it is a plain
  block. A tap parry keeps the block up for those 0.3 s even if the button is released
  (0x00C2E470). Block cone: `m_fBlockAngle` 50 (vertical 30 up / 50 down). CONFIRMED
  (the anim-zone tracker itself is LIKELY).
- Attack reach (0x00C0A060): `range = m_fMaxContextRange (210) x max(Attribute_Range, 1)`;
  with `m_fRayScalePercent` (0.5) above 0 it becomes `range x ((f - 1) x percent + 1)`, where
  the speed factor (0x00AB2050, for a pawn with a player camera) is
  `f = 1 + clamp((velocity . facing - m_fMinSpeedRayScale) x m_fRaySpeedScale, 0,
  m_fRaySpeedScale_Max)` (DefaultPlayer.ini 400, 0.005, 1.5). Sprinting forward at 540:
  f = 1.7, reach 283.5. CONFIRMED. A further x2 (`DAT_00fb930c` = 2.0) applies while timed
  state 1 of 0x00B4A0E0 is on (its setters not identified; the finisher test 0x00C10EF0 reads
  state 0 of the same table, U-052); the sim uses the speed factor.
- Damage ratios `m_fHealthRatio_NormalDamage` 0.2 / `_BigDamage` 0.7: data CONFIRMED;
  how player-vs-player hits map to them is our rule (the original has no PvP).

## Ranged

- Crossbow bolt speed: `DisTweaks_FireCrossbow.m_fArrowFireSpeed` (20000) x
  `DisTweaks_Projectile.m_fSpeedMultiplier` (1), gravity x1.3. CONFIRMED (data).
- Arrow damage (`DisProjectile_Arrow`, 0x00C503D0): base damage = the int the crossbow fires
  the projectile with, `(int)Attribute_RangedDamage` of the item (20 through the fallback chain
  of `Twk_Inv_CrossbowCorvo`'s attributes, 0x00C257A0), times the projectile's
  `m_fDamageMultiplier` (0x00C4A310): bolt 20, sleep dart 25, incendiary 50 (U-049 resolved;
  the sim used 35). Unaware targets hit in the head/torso regions take
  `m_fDamageMultiplier_Stealth` (2) with the stealth damage type, limb hits are capped at
  `m_fLegsAndArmsDamageCap` (15). Headshots kill (`m_bKillOnHeadshot`).

## Vitals

- Mana spend (0x00AA93E0): `mana -= cost`; regen pool = `ManaRegenAdditivePortion` (20);
  timer = `ManaRegenInitialDelay` (3).
- Mana tick (0x00AA7A20): timer -= dt; if the pool is empty and mana is below the portion
  the pool becomes `portion - mana` (mana always refills to 20); pool clamped to the
  portion; when the pool is positive and the timer below zero, add `min(ManaRegenAmount,
  pool)`, set (not add) the timer to `ManaRegenStepTime` (0.1). The one active power class
  that suspends the regen is `DisDLC06ActivePowerComponent_Transversal` (Daud's Blink, DLC;
  StaticClass record 0x01398D30): no base-game power pauses it (U-050 resolved).
- Health tick (0x00B57FD0): while health < `HealthRegenLimit` (10) the countdown runs; each
  time it reaches 0, `HealthRegenAmount` (1) is added and `HealthRegenRate` (0.03 s) added
  back to the countdown. Damage leaving health below the limit resets the countdown to
  `HealthRegenInitialDelay` (10 s) (0x00B5CCD0). Health and mana are integers.
- Elixirs: `m_nHealthElixirValue` 40, `m_nManaElixirValue` 50, max 10 each (Corvo tweak);
  drinking (0x00C04FC0) only below the maximum, `+(int)((1 + potency bonus) x value)`, capped.
  See `INVENTORY.md`.

## Inventory, takedowns, powers, gadgets

Decoded in `INVENTORY.md`: inventory and pickups, choke (6.53 s to a knock-out, hold to keep
choking), carrying bodies (350 uu/s), sleep darts, Possession, Bend Time (per-pawn time
dilation), Devouring Swarm, Shadow Kill, passive powers, bone charms, runes, grenades, sticky
grenades and Spring Razors.

## Differences fixed in `dishonored_sim` (before -> after)

| Mechanic | Before | After (original) | Evidence |
|---|---|---|---|
| Strafe/back speed | blended by axis weights | discrete flags by stick angle (10/50/40 deg); W+D runs full speed | 0x00AAE610, 0x00B56C40 |
| Sprint | forward only, 600 | any direction above 0.8 stick, 540 with sword (x0.9) | same |
| Sneak strafe | 0.5 x 275 | 0.5 x 400 (x0.9) | 0x00B56C40 |
| Walk / slow walk | WALK button = 200 | stick 0.45-0.95 = 200, <= 0.45 = 100 (WALK = half stick) | 0x00AAE610 |
| Speed drops | instant | blended at 8/s | 0x00B56C40 |
| Accel with analog stick | scaled by deflection | normalized | UE3 PlayerMove |
| Slide | none | sprint + sneak, 1.1 s cosine ease to sneak speed | `StatePlayerMasterSlide` |
| Lean | 40 uu sideways, linear 0.2 s | swing about a pivot 350 uu below the eye, 15 deg max (90.6 uu, 11.9 down), spring 80/12 | `DishonoredCamera_Lean` |
| Blink range | sphere sweep, distance clamped per axis | envelope above, box sweep, close-obstacle offsets, crawl-box back-off, ground lift | 0x00BF5520 |
| Blink travel | 9000 uu/s linear, velocity zeroed | 100 uu per tick, stops on reach/behind view, velocity and physics restored, Blink mantle | 0x00BFC160, 0x00BFAB80 |
| Blink cancel | invalid target only | target within 100.5 uu cancels (no mana) | 0x00BFC990 |
| Windblast | instant, radial push, linear falloff | 0.1 s warmup, front at 5000 uu/s, factor `(1-d/r)^2 x angle^2`, push along aim + up, damage `round(30k)`, 1.7 s lockout | 0x00BF7090.., 0x00BE8570 |
| Parry | block age <= 0.43 | tap 0.2 or parry zone, no parry during the 0.3 s forced block after a tap parry | 0x00C22B60 |
| Block cone | 70 deg | 50 deg | `m_fBlockAngle` |
| Crossbow bolt speed | 4500 | 20000 | `m_fArrowFireSpeed` |
| Mana regen | +1 every 0.1 s after 3 s up to spend + 20 | same pool, first step at 3 s, refill to 20 when below | 0x00AA7A20 |
| Health regen | 1/s after 10 s | 1 per 0.03 s after 10 s, below 10 hp only | 0x00B57FD0 |
| Fall damage | fractional | whole points, truncated | 0x00B4DCC0 |
| Crossbow bolt damage | 35 (guess) | 20 = `(int)RangedDamage` x `m_fDamageMultiplier` | 0x00C257A0, 0x00C4A310 |
| Sword reach | 210 | 210 x ((f - 1) x 0.5 + 1), f from forward speed (283.5 sprinting) | 0x00C0A060, 0x00AB2050 |
| Slide | runs its full time | cancellable after 0.55 s | 0x00A71450 |
| Damage | fractional | whole points (`TakeDamage(int)`) | |

## Not decoded / not modelled

See `research/UNKNOWNS.md` U-048, U-052, U-100..U-113 and U-240..U-251: lean camera collision,
the mantle finder's later states and timings, the melee reach state multiplier's trigger,
climbing on `DisClimbable` chains, level `PhysicsVolume`s (zone velocity, pain), underwater stroke
timing, drowning against regeneration, the finisher on players, helmet absorption, weapon
dispersion and fast reloads, slide impact damage, the random bone charms, and the
inventory/power open points.
