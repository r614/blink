# Player movement and physics: the core model

System: walking, running, sprinting, sneaking, sliding, acceleration and friction, jump, falling and
fall damage, step-up and floor snapping, mantle, lean, swimming and drowning, chain climbing, the pawn
cylinder. Port code: `crates/dishonored_sim/src/movement.rs`, `swim.rs`, `climb.rs`, `lib.rs`
(`fall_damage`, the `Landed` handler). Earlier research: `PLAYER.md`, `COLLISION.md`,
`INSTRUMENTATION.md`, `HOOK_TRIAGE.md`, `ORACLE.md`.

Evidence levels: CONFIRMED (decompiled code or game data read, or measured on the hooked original),
LIKELY (code read, one link inferred), UNVERIFIED (a rule the port applies but nothing in the code or a
capture settles). Addresses are virtual addresses of `Dishonored.exe` (GOG, SHA-256 `70da9613...`);
decompiled code is described in words, never pasted. `oracle index fn <addr>` shows the code.

## How this was checked (2026-10-08)

- Static: the decompiled bodies of `physWalking`, `physFalling`, `CalcVelocity`, braking,
  `GetMaxAccel`, `NewFallVelocity`, `stepUp`, `physSwimming`, `GetNetBuoyancy`,
  `TakeFallingDamage`, the player's `Landed` handler, `PlayerMove_Walking`, `MaxSpeedModifier`, the
  mantle finder states, the mantle request and trigger, the lean camera influence and state were read
  against the port and `PLAYER.md`. New findings are in "Findings" below.
- Scenarios: 24 new specs in `fixtures/oracle/scenarios/movement/` (Boyle exterior, sweeps over drop
  height, air state, direction), all listed in `fixtures/oracle/capture_queue.txt` with by-name reads
  for the cylinder, the pawn's physics fields and the mantle finder. They ran on the port with a
  prebuilt runner (copy of `target/release/oracle.exe`, `--dir` pointing at this worktree); no original
  trace was available, so no diff against the original is recorded. Every reference value is either `Static` (derived from the formulas below), `Measured` (HOOK_TRIAGE.md /
  INSTRUMENTATION.md) or `Unmeasured` / `Unverified` (listed as a capture request, never judged).
- Like with like: traces are sampled at the end of tick k (`ORACLE.md`, "Sampling point"). The
  original rests 4 uu higher than the port on the snapped runway start (ORACLE.md, first port
  findings), so no check compares absolute `pawn.z` across sides; the scenarios use speeds,
  differences and events. Walk and slow walk need an analog stick and cannot be driven on the original
  (the hook feeds -127 / 0 / 127), so the port-only WALK mapping is not testable.

## The tick, in the order the original runs it

One world tick (60 Hz fixed step in the oracle) does, for the player:

1. The controller reads input. `PlayerMove_Walking` (native, `0x005D3A00`, calling the controller's
   override at `0x0052FDB0`) sets the pawn's `Acceleration` to `Normal(aForward X + aStrafe Y) x
   AccelRate` where X and Y are the axes of the pawn's rotation (yaw only for a walking or falling pawn in stock UE3,
   `Pawn.FaceRotation`; LIKELY, not read here), Z zero. Magnitude of the stick never matters, only its direction. CONFIRMED.
   (Swimming: `Normal(aForward X + aStrafe Y + aUp Z) x CalcPlayerSwimAccelRate()` with the camera's full
   rotation, script `PlayerSwimming.PlayerMove`.)
2. The controller sets the locomotion flags (`0x00AAE610`: sprinting, walking, strafing, moving
   backwards) from the raw axes.
3. The pawn's physics runs (`startNewPhysics` -> `physWalking` / `physFalling` / `physSwimming` /
   `physLadder` / the custom state physics). It calls `CalcVelocity`, which asks `MaxSpeedModifier`.
4. Controller-side state work (jump, mantle, sneak toggles, slide) runs through the state machines.

The jump is the one place where the order shows: the original sets `Velocity.Z = 800` and physics
Falling *after* that tick's physics, so frame k+1 has `vz` 800 and unchanged `z`, and the first
displacement happens one tick later (ORACLE.md, first port findings; the port launches and integrates
in the same tick, 750 at its tick 31). Another agent is fixing that; it is not repeated below.

## Rule 1: input to acceleration, flags, and the speed modifier

### Stick flags (`0x00AAE610`, CONFIRMED)

With `m` the stick magnitude of the raw axes (keyboard 1 or 1.41):

- Sprinting (`SetSprinting` `0x00B497C0`): sprint held, `m > m_fStopSprint_Threshold` 0.8, not
  sneaking, the current state allows it. Any direction.
- Walking: not sprinting and `0 < m <= 0.95`; slow walk at `m <= 0.45`. A keyboard never walks.
- Strafing / moving backwards (exclusive, only when not walking): `a1 = acos(|strafe| / m)`,
  `a2 = acos(|forward| / m)`; strafing if `a1 < 10 deg` (stick forward) or `< 50 deg` (stick back);
  else moving backwards if the stick is back and `a2 < 40 deg`. W+D (45 deg) is neither; S+D strafes.

### Speed modifier (`MaxSpeedModifier`, `0x00B56C40`, CONFIRMED)

`GroundSpeed` is `Attribute_GroundSpeed` 400. Walking physics uses `GroundSpeed x modifier`.

| state | modifier before the sword factor |
|---|---|
| running | 1 |
| strafing | `GroundStrafeMultiplierRun` 0.7 |
| backwards | `GroundBackwardMultiplierRun` 0.8 |
| sneaking (toggle) | `Attribute_GroundSpeedCrouch` 275 / 400, strafing: 0.5 |
| sprinting | `Attribute_GroundSpeedSprint` 600 x (1 / strafe 0.6 / back 0.55) / 400 |
| crawl (`bIsCrouched`) | the stock `CrouchedPct` path (the crawl is not the sneak) |

Then times `Attribute_SwordUnsheathedSpeedFactor` 0.9 when the right-hand item is the sword class,
else `Attribute_SwordSheathedSpeedFactor` 1.0 (`0x00AA4700` walks the class chain of the right-hand
item: it is "a sword is equipped", not a drawn/sheathed animation state), in every stance including
sprint. A modifier lower than last tick's by more than 1e-4 is blended:
`m = last + (new - last) x clamp(dt x 8, 0, 1)` (`m_fSpeedBlendDownSpeed` 8); rises are instant.
Resulting plateaus with the sword: run 360, strafe 252, back 288, sprint 540 (strafe 324, back 297),
sneak 247.5, sneak strafe 180; without it 400 / 280 / 320 / 600 / 275.

Port: `movement::update_locomotion_flags`, `speed_modifier`: match, except that the port always applies
the 0.9 factor unless the pawn carries a body (`lib.rs` `sword_out = carrying.is_none()`): see MV-7.

## Rule 2: CalcVelocity and braking (`0x006ACD50`, `0x006A2AC0`, `0x006A2EE0`, CONFIRMED)

Arguments: acceleration direction, `dt`, `MaxSpeed`, `Friction`, `bFluid`, `bBrake`, `bBuoyant`.

1. `modifier = MaxSpeedModifier()`. `MaxAccel = AccelRate x modifier` (`GetMaxAccel`; `AccelRate` 2000 from
   `Attribute_AccelerationRate`). Without `bForceMaxAccel` the controller's `Acceleration` is kept and
   only scaled *down* to `MaxAccel` when longer. So the effective acceleration is `2000 x min(1,
   modifier)`: running with the sword 1800, sneaking 1237, sprinting 2000.
2. If the acceleration is zero and `bBrake`: braking in sub-steps of at most 0.03 s,
   `v -= 2 v dt_sub Friction` per sub-step, the result being the time-weighted average of the
   sub-steps while they still point the old way; zeroed when it reverses or its speed is below 12.7 uu/s
   (speed squared below 161.29). At 60 Hz it is one sub-step: `v x (1 - 2 x 8 / 60)` = `v x 0.7333`, so
   360 brakes 264, 193.6, 142.0, 104.1, 76.4, 56.0, 41.1, 30.1, 22.1, 16.2, then 0 (tick 12).
3. Otherwise `v -= (v - dir |v|) x dt x Friction` (turns the velocity toward the acceleration
   direction without losing speed along it; no clamp on `dt x Friction`).
4. Then `v = v (1 - bFluid x dt x Friction) + Acceleration x dt`; when buoyant, `v.z += (1 - Buoyancy) x
   gravity x dt` with the *pawn's* `Buoyancy` (1.0, so no gravity in water).
5. Finally `|v| <= MaxSpeed x modifier`.

`Friction` for walking is the pawn's `PhysicsVolume.GroundFriction` (8). Port: `movement::calc_velocity`,
`apply_braking`, `input_accel`: match (unit test `braking_matches_the_substep_rule`).

## Rule 3: physWalking, ground check, step-up (`0x006BD150`, `0x006ADC10`, CONFIRMED)

- Velocity.Z and Acceleration.Z are zeroed; `CalcVelocity(AccelDir, dt, GroundSpeed, GroundFriction,
  0, brake, 0)`. The move is `(Velocity + ZoneVelocity x 25 x dt) x dt`, in sub-steps of at most 0.05 s
  (`dt > 0.05` is halved and capped; at most 8 iterations); a move shorter than 1e-4 in every axis is
  skipped.
- A move into a floor steeper than 0.98 counts as a hit; a blocked move calls `stepUp`.
- `stepUp` constants (this build's engine constants are UE3's x 1.27): `MaxStepHeight` 44 (live) +
  `MAXSTEPHEIGHTFUDGE` 2.54 = 46.54; the barrier qualifies when `Normal.z < 0.1016` (`MAXSTEPSIDEZ`) or it
  is walkable (`>= WalkableFloorZ`); the pawn moves up, along, and down; a further near-vertical barrier
  repeats the step when `|Delta|^2 x Hit.Time > 232.26` and the barrier actor is world geometry
  (flag 0x20000); otherwise the move slides along the barrier flattened to vertical with
  `TwoWallAdjust`.
- Floor check: a `MultiLineCheck` (flags 0x20DF, the pawn's extent) down `MaxStepHeight + 2.54`; on a
  walkable normal (`WalkableFloorZ` = cos 50 deg = 0.6428) the pawn is moved to 2.7305 above it when the
  distance is outside [2.413, 3.048]; no walkable floor: physics Falling with the achieved horizontal
  velocity.
- At the end, if physics is still Walking and the move was not scripted, `Velocity` becomes the
  *achieved* displacement over `dt` (so a wall zeroes the speed), Z zero.

Port: `movement::walking`, `walk_move`, `step_up`, `find_floor`: match, with two small omissions (MV-8,
MV-9). The rest position above the floor depends on the collision back-off (COLLISION.md); the port
rests 4 uu lower than the original on the runway (ORACLE.md).

## Rule 4: jump, physFalling, landing, fall damage

### Jump
`StatePlayerMasterJump` enter (`0x00A75EE0`): `Velocity.Z = Attribute_JumpZ` 800 (`JumpZCarryingCorpse`
when carrying), physics Falling. Agility's power jump (`0x00A76110`): at the top (`Velocity.Z <= 1e-4`)
a jump button held more than 0.05 s sets `Velocity.Z = Attribute_JumpZ_PowerJump` once (0 for Corvo
without Agility). Port: `walking` / `jump_tick`: match, apart from the tick order above.

### physFalling (`0x006BF0E0`, CONFIRMED)
- Air control test: the move `(Velocity.xy + AccelDir.xy x AccelRate x AirControl) x dt`
  line-checked (flags 0x2286) with the full cylinder extent; a hit zeroes this tick's air control.
- With `bLimitFallAccel` (set on Pawn): `maxAccel = AccelRate x AirControl` (2000 x 0.2 = 400), plus
  `(10 - speed2d) / dt` below 10 uu/s; at or above `GroundSpeed` the horizontal speed may not grow
  (`MaxAirSpeed` = the current speed) or, with `AirControl <= 0.05`, `maxAccel = 1`. The acceleration
  is clamped to `maxAccel`.
- Each sub-step (same 0.05 s rule): `v1 = NewFallVelocity(v0, Acceleration + (0, 0, g), dt) = v0 (1 -
  fluid dt) + (a + g)(1 - buoyancy) dt` (`0x006A3860`), `g` = `WorldInfo` gravity (-1500; falls back to the
  default gravity fields when 0), horizontal speed clamped to `MaxAirSpeed`, move `(v1 + ZoneVelocity)
  dt`.
- **No hit:** `v = 2 x (moved / dt) - v0` for all three components when `measured.z < v0.z` or
  `v0.z >= 0` (rising or speeding down). Hence the pawn gains `2 g dt` and `2 a dt` per tick:
  effective gravity 3000, a 106.67 uu jump (measured), air control worth 800 uu/s^2.
- **Wall hit (non-walkable normal):** the slide move; then `v.xy = measured.xy` (no doubling) but
  `v.z = 2 x measured.z - v0.z` under the same condition (the old vz is the saved `v0.z`, the old xy are
  overwritten by the measured values before the formula, so they cancel). This is MV-1: the port keeps
  `measured` (1x gravity) for the whole vector when a wall was hit.
- Landing on a walkable normal: `processLanded`; the velocity becomes the measured one only when
  `Hit.Time > 0.1` and `Hit.Time x dt > 0.003`; terminal velocity cap `PhysicsVolume.TerminalVelocity`
  4000.

### Landing and fall damage
`Pawn.Landed` (script) calls `TakeFallingDamage` first, then the native `Landed_Native` (vtable +0x534;
the player's override `0x00AB53A0`), so the damage sees `Velocity.Z` before it is cleared.
`TakeFallingDamage` (`0x00B4DCC0`): `v = -Velocity.Z`; with `lo` / `hi` the two attributes
`MaxSpeedBeforeFallingDamage` 2250 and `MaxSpeedBeforeFallingDeath` 2900 (sorted):
`damage = (int)(HealthMax x clamp((v - lo) / (hi - lo), 0, 1))` for `v > lo`, `DmgType_Fell`; the damage
path then applies `Attribute_FallingDamageModPercent` (0 for Corvo). Measured at 60 Hz (HOOK_TRIAGE.md
U-462): v 2401 / 2675 / 2775 / 2925 -> 16 / 45 / 56 / 70. With effective gravity 3000 the impact speed of a
drop `h` is `sqrt(6000 h)`: damage starts near 844 uu, kills at 1402 uu.
Port: `lib.rs::fall_damage` and the `Landed` handler: match.

**Hard landing stun (port missing, MV-5).** The player's `Landed_Native` compares `Velocity.Z` with
`-m_fFall_Stun_Vel` (`StatePlayerMasterFalling`, 2200, a drop of about 807 uu): beyond it the stun
action plays and `StatePlayerMasterStunned` is requested (class default `m_fStunDuration` 0.1 s; the
shipped tweak value was not read), otherwise the plain landing (small / big landing action by stance,
camera landing effect). The port has neither.

## Rule 5: sneak, crawl, slide

- Sneak toggle `Dis_SneakOrSlide` (`0x009F4750`): cylinder half-height `m_fSneakHeight` 65 (the crawl,
  `bIsCrouched`, uses `m_fCrouchHeight` 33); feet stay planted (`0x00AD3950` lifts the pawn by
  `CylinderHeight - m_fSneakHeight`). The port: `set_crouch` (standing up needs headroom).
  **Conflict:** INSTRUMENTATION.md reports the pawn dropping 61 uu at a slide start, which is not
  87.5 - 65 = 22.5. `movement/sneak_height` and the by-name reads in the queue decide (MV-10).
- Slide (`0x00A67FA0` enter, `0x00A6F440` physics, CONFIRMED and measured): sprinting at >= 400 uu/s,
  Crouch: `v = v0 + (target - v0)(1 - cos(pi t / 1.1)) / 2` to `GroundSpeedCrouch x sword factor`; ends
  after 1.1 s or when the velocity turns more than 25 deg; cancellable after 0.55 s. Port: `start_slide`,
  `slide`: match (434.9 vs 434.7 at 0.555 s). Cancel source unverified (stick release, LIKELY).

## Rule 6: mantle and step-up

### Trigger (CONFIRMED by the code below; the port differs, MV-2)
- `m_bMantleAllowed` (`DishonoredPlayerPawn` +0x8A0 bit 0) is set every tick the pawn is Walking or
  Swimming (`0x00ADBD00`, the pawn's per-tick vslot 452) and cleared when a mantle starts (`0x00ACD820`).
  While the pawn is Falling and it is clear, the finder is disabled (`m_bMantleCheckDisabled`).
- The mantle request `0x00AD78D0` is made (a) on the **Jump press** (`DoJump`, pawn vtable +0x774,
  `0x00AA3820`, calling `0x00ADBE70`: allowed is set, then if the controller's `m_bJumpButton` is down,
  request) before the jump is started - if it mantles, there is no jump; the Jump state is requested
  only when the physics is Walking, Spider or Ladder (so not in water, and not in the Custom physics of
  a slide: UNVERIFIED, MV-15); (b) every tick while **Falling** with allowed set and `m_bJumpButton` down; (c) while **Swimming**
  with `m_bJumpButton` down (or, if `m_bAutomaticallyMantleInWater`, which no shipped tweak sets, with
  forward input); (d) when a Blink ends (the Blink finder, `pMantleBlinkTweaks`, min edge 0).
  Walking never mantles by itself (a 100 uu curb is walked into only by step-up, a jump is needed for
  anything the step-up cannot do).
- **No forward input is required.** The finder searches along the pawn's facing.
- The request uses the finder's current result: `m_bHasValidMantleSpot`, `m_CurrentMantleSpot` (start
  location, yaw, edge height, impact-anim and crouch flags). An edge no higher than
  `m_LowMantleMaxEdgeHeight` 120, with `m_bLowMantleUseStepUp` and no crouch needed, becomes the
  step-up (`0x00AD35E0`: move to the finder location, up by the height, forward by
  `m_fMantleForwardMoveAmount` 20, and `m_fMantleDisallowFallTimer` = `m_fLowMantleStepUpDisallowFallTime`
  0.4 s during which `Pawn.m_bCanRunOffLedges` (+0x280 bit 22), which the pawn tick sets every other tick, stays cleared; `physWalking` reads it in its ledge handling at `0x006BECD8`); every other edge enters
  `StatePlayerMasterMantle` (`0x00A6EC20`), which plays the Low / Medium / High sequence (<= 120 /
  <= 170 / else) on its root motion; the vertical root motion is scaled by `edge height /
  m_fMantleTravelDistanceZ` (20 / 50 / 80), the forward part (40 uu) is not (`0x00A8F9B0`). The state
  does not move the pawn itself. Mantle rate: `Attribute_MantleAnimRate` 1 (Normal). Sequences decoded
  (`LOG.md` 2026-10-08): 0.633 / 0.700 / 1.033 s.

### Edge finder (`DishonoredMantleEdgeFinderComponent`, a native state machine, time-sliced)
The component ticks in `TG_PostUpdateWork` and steps its FSM for at most `m_fMaxMantleCheckTimePerTick_ms`
0.25 ms (wall clock, not game time): the result lags the pawn by a variable number of frames, which no
fixed-step comparison can reproduce exactly. States, in order (all single-line extent traces of the owner,
flags 0x800020C6 / 0x800024C6):
1. `FindClearDistanceAboveOwner` (`0x00AD6410`): free height above the head, up to `m_MantleMaxEdgeHeight`
   230 (a low ceiling limits the edges that can be found).
2. `ForwardSearches` (`0x00ADA6F0` setup, `0x00AD6600` step): horizontal traces forward from a start height
   (the fall-speed rule below) up to `min(clear height, MaxEdgeHeight)` in steps of `m_LineCheckStepSize`
   12, reach `m_MantleEdgeSearchDist` 50 from the cylinder; a blocker's face is accepted when it is within
   `m_MaxVerticalAngleForEdgeFace` 20 deg of vertical and `m_MaxHorizontalAngleForEdgeFace` 45 deg of the
   facing; the next clear trace above marks the edge, and the search direction is re-aimed along the
   face normal.
3. `CheckDestinationValid` (`0x00ADA4C0`): a trace down on the edge top; rejects a top steeper than
   `m_MaxSlopeAngleForEdgeTop` 45 deg unless the hit actor implements `DisMantleableInterface`.
4. `CheckCanStand` (`0x00ACCAD0`) with a `m_FinalVerifyLocationGridSize` 6 x 6 grid of downward traces
   under the stand footprint (`0x00AD5F40`), then the spot is published (`0x00AC6D40`); the crouch flag is
   set when only the crouched cylinder fits.
- Start height (`0x00AD69B0`): owner `Velocity.Z` at least `-m_fMaxFallSpeedForMantle` 2000 (else no
  search); falling faster than `m_fFallSpeedForLedgeGrab` 1000: `m_fLedgeGrabMantleMinEdgeHeight` 155
  (impact animation); otherwise `m_MantleMinEdgeHeight` 55 (minus one search step).

Port: `movement::try_mantle`, `try_mantle_from`, `advance_traversal`, `mantle_root_position`: the trigger,
the finder's later states and the headroom bound differ (MV-2, MV-6); the root-motion playback, the
class thresholds, the step-up and the 55 / 155 / 2000 fall-speed rules match.

## Rule 7: lean

`StatePlayerMasterLeaning` (`0x00A76590`) writes the camera influence's *head velocity* every tick the
key is held: a vector of length `m_fLeanSpeed` 1000 along the lean direction (`0x00AC0950`). Releasing
runs `m_fLeanReleaseTime` 0.2 s with no request, then the state ends. `DishonoredCamera_Lean`
(`0x00AC5420` activation, `0x00AC5950` update, `0x00AC5540` limiter, `0x00ACAAB0` apply; fixed camera step
`m_fFixedTimeStep` 1/60 with interpolation between the last two steps):
1. A 3D point spring (`DisSpringPoint`, `0x00AC0A00`), constants `m_LeanConstants` 80 / 12, no velocity cap,
   rest point 0: `v += (-80 x pos - 12 v) dt`, then `pos += v dt` (semi-implicit Euler). The request *replaces*
   the velocity each tick (it is not a force and not a target position).
2. The pivot is `m_fCurLeanHeight` below the head: `m_fLeanHeightPct 2.0 x CollisionHeight x 2` = 350 for the
   standing cylinder, approached at rate 10 / s from the value set at activation; the head is projected on
   the sphere of that radius around the pivot.
3. The head's angle from the pivot is limited to `m_fMaxLeanAngle` 15 (crouched 15) plus `m_fCurMaxLeanSoften`;
   while the head is held at the limit the soften term approaches `m_fMaxLeanSoftenAngle` 5 at
   `m_fMaxLeanSoftenSpeed` 5 / s, else decays at that rate. Held: 20 deg, 119.7 uu sideways, 21.4 down,
   camera roll `m_fCameraTiltPercent` 0.5 x angle = 10 (measured 2026-10-06). 14.2 deg 12 ticks after the press.
4. Camera collision (`m_bHandleCameraCollision`, ratio 0.6) cancels the velocity component into a wall.

My integration of this model (velocity replaced each tick, k 80, c 12, sphere 350, limit 15 + soften)
gives 12.3 deg at 6 ticks and 16.5 at 12 ticks for 1000 uu/s, and 13.8 at 12 ticks for 600; the measured
14.2 at 12 ticks sits between, so the speed unit or the interpolation phase is not exactly what I
modelled; the shape (fast onset, hitting the limit within about 0.2 s) is what the port lacks.
Port: `movement::step_lean`, `lean_offset`: the held angle, roll, sideways and drop match the
measurement; the onset does not (10.7 deg at 0.2 s) because it springs a normalized angle toward a
target at the maximum (MV-4). The port also lets a lean stand only below 10 uu/s; the original's
break rule (`m_fBreakLean_Speed_PC` 200 in `DisTweaks_PlayerInput`, `m_fLean_Trigger_Threshold` 0.5 for
sticks) is not decoded (scenario `lean_moving`).

## Rule 8: swimming and breath

- Entry: a pawn whose physics volume becomes a `bWaterVolume` switches to Swimming; `startSwimming`
  (`0x006B6360`): entering slowly from above (down speed below 203.2) sets `Velocity.Z = -101.6 - 0.7 |v.xy|`
  (UE3 -80 / -160 x 1.27). Which point of the pawn (centre, eye, feet) decides the volume is the port's
  assumption (the centre); `movement/swim_entry` shows the tick and height of the switch on both sides.
- `physSwimming` (`0x006B7820`): `GetNetBuoyancy` returns `Buoyancy` and `FluidFriction` scaled by the
  submerged share of the cylinder (a trace of the volume's brush along the cylinder axis); a rise faster than 127
  uu/s is scaled by `NetBuoyancy / Buoyancy`; `CalcVelocity(dir, dt, WaterSpeed 600, VolumeFluidFriction / 2
  (1.2 or 0.15), bFluid, no brake, buoyant)` with the pawn's *full* `Buoyancy` 1.0, so there is no gravity in
  deep water at all (an idle swimmer keeps drifting until the fluid friction stops it; it does not rise).
  Leaving the volume: Falling, and a slow rise (0..160) becomes `0.4 |v.xy| + 40`. Port: `swim::swimming`:
  match.
- Swim acceleration: `CalcPlayerSwimAccelRate` (`0x00A60690`): `m_fMaxSwimAccel` 4000 during a stroke,
  else `m_fMinSwimAccel` 500 below `m_fMaxSpeedNoStroke` 350, else 0; limited to `AccelRate x modifier` by
  `CalcVelocity`. The underwater stroke trigger (`DisNotify_SwimStroke`) is not decoded: the port starts one
  every 2 s while swimming forward (SPECULATIVE, U-243).
- Breath (`0x00AD7BE0`): under water (camera below the volume top, +0x8A0 bit 11) the air
  (`m_fApneaMaxDuration` 30 x (1 + `Attribute_BreathCapacityIncrease`)) runs down; out of it refills in
  `m_fApneaRecoveryDuration` 5 s. Without air, health loses `Attribute_DrowningDamageAmount` 1 every
  `Attribute_DrowningDamageStep` 1 s directly (61 ticks at 60 Hz: the timer must go below zero), floored at
  `m_MinimumScriptedHealth`, death below 1. Port: `swim::step_breath`: matches the measurement.

## Rule 9: chain climbing (not re-derived here)
`PLAYER.md` "Climbing" is the reference (`physLadder` `0x00AA82C0`: 8.19 uu steps, `VInterpTo` rate 20 ->
163.8 uu/s, clamp to the chain's bounds, jump-off impulse 525). The port matches the notes; nothing was
measured on the original. Scenario `movement/climb_chain` adds the first original-side check (the Boyle
exterior chain). Port: `climb.rs`.

## The pawn cylinder
Radius 35, half-height 87.5 standing (sneak 65, crawl 33); traces of the walking pawn pass
`(radius, radius, half-height)` as the extent (kDOP and BSP checks are axis-aligned boxes), which the port
reproduces with `PawnBox`. Engine constants: `MaxStepHeight` 44, `MaxJumpHeight` 122, `WalkableFloorZ`
0.6428 (50 deg, `m_fMaxWalkableFloorInDegrees`), `MaxFallSpeed` 2100, `AirControl` 0.2, `GroundSpeed` 400,
`AccelRate` 2000, `JumpZ` 800, `Buoyancy` 1.0, world gravity -1500, `GroundFriction` 8, `TerminalVelocity`
4000 (fluid friction 2.4 in `DishonoredWaterVolume`). The by-name reads in the queue re-check the live
values.

## Findings (divergences, with diagnosis and the fix)

| ID | Class | Where | Evidence | Fix the port needs |
|---|---|---|---|---|
| MV-1 | port bug | `movement::falling`, the velocity after a move that hit a wall | `physFalling` `0x006BF0E0` tail: after a wall hit the saved old xy are overwritten by the measured xy, the old vz is kept, so the formula `2 measured - old` doubles only z | When `hit_wall`, keep `v.xy = measured.xy` but still set `v.z = 2 measured.z - old_vel.z` under the usual condition (`measured.z < old_vel.z || old_vel.z >= 0`). Effect: gravity against a wall is 1500, not 3000, in the port. Capture: a jump into a wall (`jump_air_*` do not hit one; add a wall case when a wall position near the runway is known) |
| MV-2 | port bug | `movement::try_mantle`, `walking`, `falling`, `swim::swimming` | `0x00ADBD00` / `0x00ADBE70` / `0x00AA3820`: trigger on the Jump press while walking, every tick with Jump held while falling or swimming, gated by `m_bMantleAllowed`; no forward-axis test | Replace `input.forward > 0` by "the Jump button is down"; remove the forward test in the swim and fall branches; on the Jump press try the mantle before launching (already so); keep a `mantle_allowed` flag set when Walking or Swimming and cleared when a mantle starts. The finder searches along the facing whatever the stick does. Capture: `mantle_still_144` (no stick), `mantle_air_98_tap` against `_hold` (button released / held in the air), `mantle_wall_144` (the Jump-press case) |
| MV-3 | port bug | `swim::swimming` mantle exit | `0x00ADBD00` swimming branch: Jump held (or forward only if `m_bAutomaticallyMantleInWater`, false) | Same as MV-2; drop the "head above water and forward" condition unless the finder result requires it (the finder runs regardless) |
| MV-4 | port bug | `movement::step_lean` | `0x00A76590` -> `0x00AC0950` (velocity written), `0x00AC5950`, `0x00AC0A00` (spring integrator) | Model the head as the 3D spring point of the Rule 7 text: replace its velocity by `m_fLeanSpeed` along the lean direction every held tick, spring 80 / 12 toward the rest point 0, project on the sphere of radius 350, limit the angle (15 + soften), fixed 1/60 camera step. Then check the onset (14.2 deg at 12 ticks, 20 held) with `lean_boyle`; the speed unit needs the per-tick capture. Also decode the break-on-move rule |
| MV-5 | port missing | landing | `0x00AB53A0`, `StatePlayerMasterFalling.m_fFall_Stun_Vel` 2200, `StatePlayerMasterStunned` | Add a stun state entered on a landing with `vz < -2200` that blocks movement for the stun duration; read the real duration (the Stunned state's tweak; class default 0.1 s) before porting. Capture: `fall_stun` |
| MV-6 | port missing | mantle finder | `FindClearDistanceAboveOwner`, `CheckDestinationValid`, `CheckCanStand` (6 x 6 grid), `DisMantleableInterface` | Bound the search by the free height above the head; verify the landing footprint with a 6 x 6 grid of down traces (not one box overlap); accept steep tops on mantleable actors; use the finder's spot location (`m_StartLocation`: xy at the face, z = owner z + a cylinder-height term) for the mantle start; the finder's lag (0.25 ms slices) is an oracle limitation. The geometry captures are `mantle_wall_144` and `mantle_air_98_*` plus the by-name reads of `m_CurrentMantleSpot` |
| MV-7 | port missing | `lib.rs` `sword_out` | `0x00AA4700`: the factor is 0.9 only when the right-hand item is a sword | Take it from the equipped right-hand item class; ORACLE.md's `run_sprint` (400 / 600 on the original, 360 / 540 on the port) is this. The new scenarios set `sword_drawn: true` on both sides |
| MV-8 | port missing (low) | step-up | `0x00AD35E0` / `0x00ADBD00`: `m_fMantleDisallowFallTimer` 0.4 s keeps `m_bCanRunOffLedges` cleared after a step-up | Clear that flag for 0.4 s after a mantle step-up and decode what `physWalking` does with it at `0x006BECD8` (probably the walk-off-a-ledge handling) before porting |
| MV-9 | port missing (low) | `movement::step_up` | `0x006ADC10`: the repeat step needs the barrier actor to be world geometry | Add the actor test when moving bodies can be hit |
| MV-10 | needs capture | sneak half-height | PLAYER.md (22.5 drop) vs INSTRUMENTATION.md (61 at a slide) | `movement/sneak_height`, `session read pawn CylinderComponent.CollisionHeight` while sneaking |
| MV-11 | needs capture | Jump pressed while sneaking | the port stands up and jumps in the same tick; the original's transition is not decoded | `movement/jump_from_sneak` |
| MV-12 | oracle limitation | walk / slow walk | analog stick only; the hook sends -127 / 0 / 127 | none; the port's WALK button is port-only |
| MV-13 | known difference | snapped start height | ORACLE.md: original -217.567, port -221.567, then +2.6 | collision/back-off agent; scenarios use differences |
| MV-15 | needs capture | Jump during a slide | `DoJump` `0x00AA3820` requests Jump only from physics Walking / Spider / Ladder; the slide is PHYS_Custom; the port's `walking` checks the jump before the slide branch and jumps out of a slide | `movement/slide_jump`; if the original refuses, test `p.slide.active` before the jump |
| MV-14 | being fixed elsewhere | jump launches one tick early | ORACLE.md | not duplicated here; `jump_standing`, `jump_air_*` and `jump_from_sneak` will show it |

Not findings: the speed table, braking, the 2 g doubling, fall damage, swim and drown rules, the slide and the mantle
root motion agree with the code and with the measurements in HOOK_TRIAGE.md / INSTRUMENTATION.md.

## Scenarios (fixtures/oracle/scenarios/movement/) and capture requests

24 new specs next to the 14 existing ones; the whole directory runs in 0.5 s on the port
(`oracle scenario --system Movement`: 24 pass, 2 fail, 12 unmeasured; the two failures are the known lean
onset). All new ones are on `L_Boyle_Ext_P` (the original only runs the Boyle maps), sword equipped on
both sides, 60 Hz, and are in `fixtures/oracle/capture_queue.txt`, together with the by-name reads below.
Most start at the runway `(-3400, -3150)` facing 270. The mantle, step and slope scenarios start at
places found by scanning the port's Boyle exterior with 648 temporary runs (a 9 x 9 grid, four facings,
walk and walk-and-jump) and use the pawn's resting height as the start height, no floor snap. The
port values below are what the port does now; none is a reference for the original.

| scenario | rule | what varies | reference | port today |
|---|---|---|---|---|
| `ground_moves` | 1, 2 | forward / strafe / back / W+D, braking between | Static: 360 / 252 / 288 / 360, braking 264 then 0 at tick 12 | all six checks pass (359.99, 263.99, 0, 252.04, 288.00, 360.00) |
| `sprint_slide` | 1, 5 | sprint, Crouch at 540, slide into sneak | Static: 540, 393.75 at mid-slide, 247.5 | 540.0, 393.75, 247.5 |
| `jump_air_stand` | 4 | forward held in the air after a standing jump | Unmeasured | speed 406.6 after 31 airborne ticks (13.3 uu/s per tick = 2 x 400 / 60) |
| `jump_air_run` | 4 | the same from a 360 run | Unmeasured | 413.3 |
| `jump_air_sprint_side` | 4 | sprint 540, D pressed in the air | Unmeasured | 542.0 (the turn exceeds the cap by the `2 measured - v0` step) |
| `fall_h700` / `h1000` / `h1200` / `h1350` | 4 | drop height | Static damage 0 / 21 / 46 / 64 (continuous formula, tolerance 3) | damage 0 / 24 / 45 / 67; landing speed (last airborne sample) 2050 / 2450 / 2650 / 2850 |
| `fall_stun` | 4 | forward 16 ticks after a 1200 uu landing | Unmeasured | speed passes 20 uu/s one tick after the press: no stun |
| `jump_from_sneak` | 5 | Jump out of a sneak | Unmeasured | stands and jumps at once (rise 129 = 22.5 + 106.7) |
| `slide_jump` | 5 | Jump 20 ticks into a slide | Unmeasured | jumps |
| `sneak_height` | 5 | sneak toggled twice | Unverified -22.5 | -22.5, back to 0 |
| `mantle_wall_144` | 6 | pinned against a 144 uu face, Jump press | Unmeasured | medium mantle at the press tick, ends 132.2 above the start |
| `mantle_still_144` | 6 (MV-2) | the same, no stick input | Unmeasured; the original should mantle | jumps, no mantle |
| `mantle_air_98_tap` / `_hold` | 6 (MV-2) | run, jump at an edge 97.6 uu high, Jump tapped / held | Unmeasured; the original should mantle only in `_hold` | both mantle at tick 69 |
| `lean_boyle`, `lean_moving` | 7 | held lean, released; lean while running | Measured 20 / 10 / 14.2; Unmeasured | 20.0 / 10.0 / **10.7** (fails); no lean while running |
| `swim_entry` | 8 | fall 220 uu into the sea, drift, Jump 1 s, forward | Static (swims); Unmeasured | switch at tick 24 (centre 19 uu under the surface), speed halves at entry, sinks to 250 uu under, 348 uu/s forward |
| `drown_boyle` | 8 | held under | Measured: 30 s, 61 ticks per point | matches |
| `climb_chain` | 9 | touch and climb the exterior chain | Static 163.8 uu/s | 163.8, holds height without input |
| `walk_step_35` | 3 | a 35 uu step at (-2913, -3900) facing +Y | Static: climbed, no speed loss | rises 34.7 in one tick, speed keeps accelerating |
| `walk_slope_34` | 3 | a 34 degree slope at (-3150, -3300) facing +X | Static: climbed; Unverified: 360 on the slope | rises 200, horizontal speed 360 |

By-name reads in the queue: cylinder radius and half-height; `GroundSpeed`, `AccelRate`, `AirControl`,
`JumpZ`, `MaxStepHeight`, `WalkableFloorZ`, `Buoyancy`, `bLimitFallAccel`; `m_fMantleDisallowFallTimer`;
the mantle finder's `m_bHasValidMantleSpot` and `m_CurrentMantleSpot`; class dumps of the lean camera, the
mantle tweaks and the Stunned / Falling states (live defaults and tweak values).

What each capture will settle, in order of value: `mantle_air_98_tap` against `_hold` and
`mantle_still_144` (MV-2, the trigger); `jump_air_*` (is air control 400 or 800 uu/s^2, does the cap hold);
`fall_h*` and `fall_stun` (the landing speed seen by `TakeFallingDamage`, the stun); `sneak_height` with
the cylinder reads (MV-10); `walk_slope_34` and `walk_step_35` (speed on slopes and steps); `lean_boyle`
per tick (MV-4); `slide_jump`, `jump_from_sneak` (MV-11, MV-15); `swim_entry` (water entry point).
## Not covered / unverified
- Step-up and slope limits against the original (no known step or slope on the Boyle runway; the existing
  step / slope / mantle scenarios are Playground-only, so they never meet an original trace).
- The edge finder's exact spot location and its lag; the mantle sequence variants for impact and crouch.
- Underwater stroke timing (U-243); the lean break-on-move rule; slide cancel source.
- Flying / Blink flight physics (BLINK, `blink.rs`) belong to the powers system.
