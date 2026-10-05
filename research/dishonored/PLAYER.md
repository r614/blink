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
- `CalcVelocity` / `physWalking` / `physFalling` are the engine's (no Dishonored override in
  the pawn vtables). `GroundFriction` 8 and `TerminalVelocity` are stock values: LIKELY.
- `AirControl` is copied from `DisTweaks_PlayerPawn.m_fAirControl` (0.2) by 0x00AA7910.
- Jump (StatePlayerMasterJump enter, 0x00A75EE0): `Velocity.Z = Attribute_JumpZ` (or
  `JumpZCarryingCorpse`), plus the base's upward velocity when standing on a mover; physics
  Falling. Power-jump styles are for Agility (not in multiplayer).
- Falling state (0x00A759F0 / 0x00A75B80): bookkeeping only. `m_fFall_Stun_Vel` 2200
  (DefaultPlayerState.ini) stuns on landing (not modelled).

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
`m_fSlideTime` (0x00A7B220), or on a fall. CONFIRMED except the stick-release cancel
(`m_fSlidePercentNotCancelable`) and slide impact damage, not modelled.

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
- The sim applies that momentum as a velocity change on players (the original only
  blasts NPCs, which go ragdoll): LIKELY.

## Melee

- Block (`UDisItemContext_MeleeBlockPlayer`, hit handler 0x00C22B60): the attack is
  **parried** when the block started less than `m_fTapForPerfectParryWindow` (0.2 s) ago or
  the block animation's `DisNotify_BlockParryZone` is active (0.001-0.431 s, U-021), unless
  the forced block after a tap parry (`m_fForceBlockTimeForPerfectParry`, set to
  `m_fForceBlockTimeOnPerfectParryTap` 0.3 s) is running, in which case it is a plain
  block. A tap parry keeps the block up for those 0.3 s even if the button is released
  (0x00C2E470). Block cone: `m_fBlockAngle` 50 (vertical 30 up / 50 down). CONFIRMED
  (the anim-zone tracker itself is LIKELY).
- Attack reach (0x00C0A060): `m_fMaxContextRange (210) x max(Attribute_Range, 1)`, scaled by
  `m_fRayScalePercent` (0.5) against an unidentified factor (0x00AB2050), doubled in a
  special state. The sim keeps 210.
- Damage ratios `m_fHealthRatio_NormalDamage` 0.2 / `_BigDamage` 0.7: data CONFIRMED;
  how player-vs-player hits map to them is our rule (the original has no PvP).

## Ranged

- Crossbow bolt speed: `DisTweaks_FireCrossbow.m_fArrowFireSpeed` (20000) x
  `DisTweaks_Projectile.m_fSpeedMultiplier` (1), gravity x1.3. CONFIRMED (data).
- Arrow damage (`DisProjectile_Arrow`, 0x00C503D0): base damage from the projectile
  (source not found, U-049); unaware targets hit in the head/torso regions take
  `m_fDamageMultiplier_Stealth` (2) with the stealth damage type, limb hits are capped at
  `m_fLegsAndArmsDamageCap` (15). Headshots kill (`m_bKillOnHeadshot`).

## Vitals

- Mana spend (0x00AA93E0): `mana -= cost`; regen pool = `ManaRegenAdditivePortion` (20);
  timer = `ManaRegenInitialDelay` (3).
- Mana tick (0x00AA7A20): timer -= dt; if the pool is empty and mana is below the portion
  the pool becomes `portion - mana` (mana always refills to 20); pool clamped to the
  portion; when the pool is positive and the timer below zero, add `min(ManaRegenAmount,
  pool)`, set (not add) the timer to `ManaRegenStepTime` (0.1). An active power of one class
  suspends the regen (class not identified, U-050).
- Health tick (0x00B57FD0): while health < `HealthRegenLimit` (10) the countdown runs; each
  time it reaches 0, `HealthRegenAmount` (1) is added and `HealthRegenRate` (0.03 s) added
  back to the countdown. Damage leaving health below the limit resets the countdown to
  `HealthRegenInitialDelay` (10 s) (0x00B5CCD0). Health and mana are integers.
- Elixirs: `m_nHealthElixirValue` 40, `m_nManaElixirValue` 50, max 10 each (Corvo tweak);
  consumption code not decoded (no elixirs in the multiplayer loadout).

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
| Damage | fractional | whole points (`TakeDamage(int)`) | |

## Not decoded / not modelled

See `research/UNKNOWNS.md` U-048 to U-053: lean camera, arrow base damage, regen-blocking
power, the mantle finder's later states and timings, pawn reaction to Windblast momentum,
`physFalling` air control details, the melee reach scale factor, swimming and climbing.
