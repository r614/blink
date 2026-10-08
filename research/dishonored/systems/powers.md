# Powers: the core model and what the port does with it

Scope: the mechanics of Blink, Windblast, Dark Vision, Possession, Bend Time, Devouring Swarm, the
passives (Vitality, Agility = `Celerity`, Blood Thirsty, Shadow Kill) and mana. Not visuals. Sources:
the decompiled exe and UnrealScript through `oracle index`, the tweak objects (`.bin/tweaks.exe`),
`DefaultPlayer.ini` (the `m_Powers` blocks), and the earlier research docs (PLAYER.md "Blink",
"Windblast", "Vitals"; INVENTORY.md "Powers"; HOOK_TRIAGE.md measurements). Port code:
`crates/dishonored_sim/src/{blink,left_hand,powers,rats,vitals}.rs`. Addresses are for the GOG exe
`70da9613...`. Describe, do not paste: no decompiled code is reproduced here.

Status of this pass (2026-10-08): the rules below are decoded from the exe and tweaks. The new
scenarios were run on the port with `oracle scenario --dir <worktree>/fixtures/oracle/scenarios
--system powers` (section 10 has the results); **no original trace exists for any of them yet**:
every expected value cites its source and is marked `Static` (decoded code or tweak data),
`Unmeasured` or `Unverified`, and the capture requests are in `fixtures/oracle/capture_queue.txt`.
Note for other agents: the oracle writes `harness_out/oracle/scenario/results.json` and the traces
into the main checkout, shared by every concurrent run; read the results in the same command that
runs the scenarios.

## 1. Common machinery

- **Tweaks.** Each active power has a tweak object under `Twk_Powers*` (`DisTweaks_<Power>` <
  `DisTweaks_ActivePowerBase`): `m_ManaCost` (+0x8C), `m_RuneCosts[2]`, and a per-level array
  (`m_Levels[2]`, +0x15C). Level `m_CurrentLevel` 1 or 2 selects element `m_CurrentLevel - 1`
  (Blink: HOOK_TRIAGE.md, level 1 = `m_Levels[0]`; Windblast 0x00BF7090 and 0x00BF6780 read the block
  at `0x15C + (level - 1) x 0x40`). Possession's possessable tweak is indexed the same way.
- **Usable** (slot +0x1A4): mana >= `m_ManaCost` and the power's "in use" flag (slot +0x160) clear. The
  spend (0x00AA93E0) takes the cost, sets the regeneration pool to `ManaRegenAdditivePortion` and the
  timer to `ManaRegenInitialDelay`; it is skipped when a roll is below `Attribute_ChanceUseNoManaForSpell`.
- **Spell strength.** `Attribute_SpellStrength` multiplies durations and ranges: Dark Vision and
  Possession durations, Bend Time duration, Windblast distance (cap and gather radius,
  0x00BF6780 / 0x00BF7090), Possession range (0x00BF63B0).
- **Costs (tweaks).** Blink 20, Windblast 50, Dark Vision 20, Possession 60, Bend Time 60,
  Devouring Swarm 40; rune costs Blink 1/3, Windblast 3/4, Dark Vision 1/2, Possession 3/5, Bend Time
  2/8, Swarm 3/4, Vitality 1/3, Blood Thirsty 2/3, Shadow Kill 2/4, Agility 2/3. The port reads all of
  them from the same tweaks (`GameplayConfig.items`, `tuning.rs`): **match** (Blink and Windblast also
  hook-measured). The brief's 20 / 60 / 60 / 40 for Dark Vision / Possession / Bend Time / Swarm are
  confirmed from the tweak data.
- **Mana tick** (0x00AA7A20): timer -= dt; if the pool is empty and mana is below the portion the pool
  becomes `portion - mana`; pool clamped to the portion (20); when the pool is positive and the
  timer has gone below zero, add `min(ManaRegenAmount 1, pool)`, timer := `ManaRegenStepTime` 0.1.
  Skipped entirely only while a `DisDLC06ActivePowerComponent_Transversal` is owned (Daud's Blink; not
  base game). Port `vitals::spend_mana` / `tick_with`: **match** (hook-measured: the 20-point refund
  begins 3.00 s after a Blink, one point every 7 ticks). The pawn tick passes its own (dilated) dt, so
  regeneration runs at the caster's Bend Time rate; whether the port feeds `vitals::tick_with` the
  scaled dt was not checked (open: Unverified).
- **Elixirs** (0x00C04FC0): +40 health / +50 mana, max 10 each (INVENTORY.md); not re-examined.
- **Dilation layers.** The engine's time-dilation helper (0x009FA0D0) keeps an array of 0x40-byte
  layers on one singleton: `(layer, world target, caster rate, ceiling, blend seconds)`; a target of
  exactly 0 is kept, any other target is clamped to at least 0.05; the layer moves linearly from
  where it is to the target over the blend time (rate `(target - current) / blend`). Bend Time uses
  layer 0 (0x00BFBE60 and 0x00BF8870); **Blink uses layer 1** (section 2). How layers combine is not
  decoded (Unverified).

## 2. Blink (`DishonoredActivePowerComponent_Blink`, vtable 0x01168698)

Tweak `Twk_Powers.Blink.Twk_Blink`: per level `m_HorizDistance` 1100 / 1600, `m_VertDistance` 500,
`m_fDefaultBlinkStepDistance` 100, `m_fDefaultTimeBetweenBlinkSteps` 0.01, **`m_fBendTimeDownTime` 0.9,
`m_fBendTimeUpTime` 2.5, `m_fWarmupTime` 1.0, `m_fCooldownTime` 1.0** (the port's `BlinkConfig` carries none of
the last four); `m_TargetTestExtent` 20^3,
`m_fCloseCollisionDistance` 300, `m_fCloseCollisionOffsetStep` 30; flags `m_bKnockdownAwarePawns`,
`m_bTargetThroughAwarePawns`, `m_bLimitVerticalDistanceFromGround` all false; mana 20.

State machine (`m_BlinkMode`, +0xC9; tick 0x00BFC5D0):

| mode | meaning | entry | exit |
|---|---|---|---|
| 0 | aiming | start (0x00BFC020 -> 0x00BFAAA0): requires CanUse; sets `m_bActive` (+0xA0 bit 0), zeroes the timers | release (0x00BFC990) |
| 1 | moving | release with a target farther than the pawn tweak's cancel radius: save location / velocity / physics, `m_MaxDistanceFromOrigLoc`, physics Flying, **layer 1 of the dilation helper to 0.05 over `m_fBendTimeDownTime`** | step conditions below (0x00BFC160) |
| 2 | cooldown after a move | end (0x00BFAB80 with 0): restores velocity and physics, **layer 1 back to 1.0 over `m_fBendTimeUpTime`** (0x00BE7380), Blink mantle (0x00BE7420) | `m_fCooldownTimer` > `m_fCooldownTime` (0x00BED480) -> 0x00BE9180 clears `m_bActive` |
| 3 | cooldown after a cancel | end with 1 (release too close, no target, power cancelled) | same |

Rules:

1. **Targeting** (0x00BF5520, every tick while aiming): range vector (0x00BF52E0): looking level or
   down, view direction x `m_HorizDistance`; looking up, `t = min(m_VertDistance / d.z, m_HorizDistance)`
   along the view, height capped at `m_VertDistance` (minus the ground height only with the false
   flag). Box sweep (extent 20^3) from the camera, retried from the pawn when it starts inside
   geometry; a hit nearer than `m_fCloseCollisionDistance` retries from offset starts past it; back
   off along the aim in half the pawn's bounds-sphere radius until a crawl-height box fits; lift to
   at least `CollisionHeight` above the floor. Port `blink::range_vector` / `compute_target`:
   **match** (hook-measured: 1096.7 / 1597.7 level, 1385.8 / +500.0 at 19.84 deg).
2. **Release** (0x00BFC990): cancel when the target is within the pawn-tweak distance (+0xE4; PLAYER.md
   reads it as the bounds radius 100.5) of the pawn, else move. Port `blink::update`: match.
3. **Move** (0x00BFC160, 0x00BECDF0): a timer gaining `m_fDefaultTimeBetweenBlinkSteps` back each time it
   runs out (so at most one step per frame); a step moves 100 uu toward the target without collision;
   the move ends when the target is behind the view, within 25 uu horizontally, farther from the start
   than the target was, or the pawn did not move. Port `blink::travel`: **match** (16 ticks = 0.267 s,
   hook-measured). The original's speed depends on the frame rate (steps are per frame).
4. **Cost**: spent at the release (mana check at the start as well, rule 6).
5. **World slow-down** (new): release starts layer 1 toward 0.05 over 0.9 s; the end/cancel blends it
   back to 1.0 over 2.5 s starting from the current value. With a 16-tick move the world rate bottoms
   at about 0.718 and is back to 1 about 2.5 s after the move ends. The caster rate passed is 1.0.
   How layer 1 maps to `WorldInfo.TimeDilation` and each actor's `CustomTimeDilation` is not decoded
   (the Bend Time measurement shows layer 0 giving world 0.15 x real and `TimeDilation` 0.85 = the
   caster's rate): **Unverified**, capture requested. **Port: missing** (F-03).
6. **Usable gate and cooldown** (new): CanUse needs mana >= cost **and** `m_bActive` clear. After any
   release (move or cancel) `m_bActive` stays set for the move plus `m_fCooldownTime` 1.0 s, so Blink
   cannot be re-aimed for 1 s after the move ends (about 1.3 s after the release). With less than the
   cost the power never enters aiming. **Port: missing** (F-01, F-02).
7. `m_fWarmupTime` 1.0 drives the warm-up distortion (0x00BFC5D0) and a check in slot +0x17C (0x00BE7500,
   meaning not decoded). The original does not delay the move by the warm-up (hook: the release
   after 10 ticks moves at once). Not modelled; presentation as far as known.
8. Knockdown / touched pawns (`m_bKnockdownAwarePawns` false, `m_TouchedPawns`): the move stops at pawns
   (`m_bStopAtPawns`, the forward probe in 0x00BECDF0); the port ignores pawns during the move. Not
   decoded further; unlikely to matter against the flags' defaults.

## 3. Windblast (`DishonoredActivePowerComponent_WindBlast`, vtable 0x01168F48)

Tweak `Twk_Powers.Windblast.Twk_WindBlast` per level (`m_Levels[0]` / `m_Levels[1]`): force 1800 / 2700,
extra up force 1000 / 1600, damage 20 / 30, angle 135 / 135, distance 3000 / 3000, speed 5000 / 5000;
shared: `m_bUseDirectionalWindblast` true, **`m_HitWallAdditionalDamage` 100, `m_fChanceToExplodeNPCs` 0.3,
`m_fMinSpeedToExplodeNPCs` 500, `m_fExplodeImpulseStrength` 100**; mana 50. Class defaults:
`m_fWarmupTime` 0.1, `m_fCooldownTime` 1.0.

State machine (`m_WindBlastMode`, tick 0x00BFB9E0): 1 warmup (0.1 s) -> 2 fire (0x00BF9720: waits while
the caster is in a refusing state, then 0x00BF9570) -> 3 execute (expanding front, 0x00BF7090) ->
4 cooldown (1.0 s) -> 0. Use (0x00BFB950) needs CanUse and starts the warmup. Port: `LeftHand::Windblast`
in `left_hand::update` sets the lock-out `0.1 + distance / speed + 1.0` s (1.7 s at level 2) and
`step_windblasts` runs warmup / start / front: **match** in structure.
Correction (physics port, 2026-10-08, physics.md P-8): the fire mode waits while the cast action
(`Powers_Cast_Windblast`) plays, and that action's `DisNotify_WindBlast` at 0.27 s starts the blast
(0x00A96AE0 -> 0x00C44E10 -> 0x00BF9570); the port now starts it 0.27 s after the cast
(`CombatTuning::windblast_start_delay`, also in the lock-out): damage 19 ticks after the press at
183 uu (original 20).

1. **Start** (0x00BF9570): origin and direction from the camera; direction flattened unless
   directional (it is); the targets are gathered once (0x00BF6780): every actor (not of the excluded
   class) whose bounds centre is within `distance x (1 + SpellStrength)` and whose factor is positive.
2. **Factor** (0x00BE8570): zero unless `dot(flat direction, flat bearing) > cos(angle / 2)`; else
   `(1 - min(d / range, 1))^2`, times `((dot - cos) / (1 - cos))^2` when `d > 100` (3D distance, eye to
   the target's bounds centre). Port `windblast_factor`: **match**.
3. **Front** (0x00BF7090): reach = elapsed x speed capped at the range; a target is hit when the front
   passes its bounds centre and (for non-pawn classes) a sight check passes (0x00BEDC90).
4. **Hit** (0x00BF6AF0, 0x00BE9770): impulse = direction x force with its z raised to
   `(1 - min(d / range, 1)) x extraUp` (added to a downward z); damage `round(damage x k)` through
   `TakeDamage` with momentum `impulse x k`. Hook-measured at 182.8 uu, level 2: 26 damage, momentum
   (2362, -227, 1317). Port `windblast_momentum`, damage: **match for level 2**.
5. NPCs are thrown by the momentum; players are not pushed (PLAYER.md, U-051).

Not in the port: the level (F-04), `SpellStrength` (F-05), the wall bonus damage and NPC explosion
(F-06), the gather of every actor class including projectiles and props (F-07; the port deletes
projectiles in the cone and has no props), the caster's tick rate under Bend Time (F-13).

## 4. Dark Vision (`DisActivePowerComponent_DarkVision`, vtable 0x01168870)

Tweak: both levels `m_fDuration` 30, `m_fDistance` 2000, `m_fFadeStartAtDistance` 1700; fade-in
`m_EyeLidClosingDuration` 0.05, fade-out `m_PowerFadeOutDuration` 0.2; `m_fCooldownTimer` 1;
**`m_fDesactivateInCombatTimer` 1.0**; mana 20.

State: `m_fFadeInTimeRemaining`, `m_fTimeRemaining`, `m_fFadeOutTimeRemaining`, `m_fCooldownTimer`,
`m_fCombatTimer`. Use (0x00BE89E0 -> 0x00BF82A0) with mana: fade-in 0.05, duration x (1 + SpellStrength),
fade-out 0.2, cooldown 1; pressing again once the cooldown ran out ends it (zero fade-in and duration, the
fade-out plays). The tick (0x00BFBC90) spends the step on the fade-in, then the duration step
(0x00BF9AB0: reveals the souls of pawns/actors within `m_fDistance` of the player, fading from 1700), then
the fade-out. One-time cost, no drain. Port `use_dark_vision`, `tick_dark_vision`,
`dark_vision_reveals`: **match** for timing, cost, distance (occlusion ignored, as the x-ray).

Not in the port: **combat deactivation** (F-08): inside the duration step 0x00BF84C0 runs first; while
the player has combat opponents (the count 0x00C5AF60 reads is >= 1 and 0x00B464A0 is false) it counts
`m_fCombatTimer` down from 1.0 s and then zeroes `m_fTimeRemaining`. Sound visualisation
(`m_AINoiseVisualEffectScaleParameterName`, `Twk_DarkVision.m_AINoiseVisualEffect`) is presentation of
AI noises; no gameplay rule found.

## 5. Possession (`DishonoredActivePowerComponent_Possess`, vtable 0x011684B8)

Tweaks: `Twk_Powers_Possess`: `m_Levels` `m_fPossessRange` 1000 / 2000, **`m_fBendTimeExtension` 0 / 4**,
`m_IntroDuration` 1.1, `m_fCancelCooldown` 1.0, `m_fDistanceWeight` 1, `m_fAngleWeight` 10,
`m_fMaxAngleFOVFraction` 0.3, `m_fTraceExtent` 15, `m_TargetingPriorityWeights` {x, 1, 2, 2},
`m_bCanUseDoorsWhilePossessing`, mana 60. Possessables: `Twk_Possessable_BaseNPC` (required power level 2,
duration level 2 = 15 s, ground speed 300, awareness states 0-6 allowed, state 7 (being choked)
refused), `Twk_Possessable_Rat` (required level 1, 20 s / 30 s, ground speed 300, all states, collision
radius 25 x height 20), `Twk_Possessable_BaseAnimal`.

State machine (`m_PossessionStage`, +0xB8; tick 0x00BFD6D0): 0 inactive (0x00BFB320), 1 targeting
(0x00BFD300), 2 pre-possess / intro (0x00BFCD40), 3 active (0x00BFB390), 4 cooldown (0x00BED880).

1. **Targeting** (0x00BFB090 builds the candidates from the actors in range, 0x00BFCAF0 picks the one with
   the highest score, 0x00BFCEC0 checks the line to it with two traces of extent `m_fTraceExtent`): per
   candidate, `score = m_fAngleWeight x (maxAngle - angle) / maxAngle + m_fDistanceWeight x (range -
   distance) / range + priorityWeight[class]`, range = level range x (1 + SpellStrength) (at least 1),
   `maxAngle` = a controller value in radians (at least 0.1) from a field not identified
   (`m_fMaxAngleFOVFraction` is read elsewhere; Unverified). Port `possess_target`: lower-is-better
   `d / range + 10 angle / (0.3 x 90)`: same ordering **when the priority weights are equal** and the
   max angle is 27 deg; no priority term, no SpellStrength on range (F-10).
2. **Start** (0x00BFB6C0, reached when the intro ends): stage 3, timer = `m_fPossessDuration` of the
   possessable's level x (1 + SpellStrength) (+ `Attribute_WhiteRatPossessDurationBonus` for white
   rats, not in the port); **if the caster's Bend Time is running (and not in its final blend-out),
   its maximum duration is extended by `m_fBendTimeExtension` (4 s at level 2)** via 0x00BFA120 (F-09).
3. **Active** (0x00BFB390): ends on a cancel request, no host, or a destroyed/pending-kill host;
   otherwise the timer runs down by the tick dt except in cinematic mode. Using the power again
   requests the end (0x00BFB520). Exit (0x00BF9330): the possessor appears at the host (`DUET_UpFromFeet`
   for people), `m_bPossessorShouldDie`, `m_bCanBeCrushedOnPossessEnd` flags exist (semantics not decoded);
   stage 4 cooldown 1.0 s. Port `step_possession` / `end_possession`: structure and numbers **match**
   (intro 1.1 s, 15 s / 20 s / 30 s, cancel cooldown 1 s); the port ends the possession when the host
   dies (`!alive`), the original's test is object deletion: whether a killed host ends it is
   Unverified (F-15).
4. Rats: `Twk_Possessable_Rat` at level 1; aiming at a swarm possesses a rat (U-104).

## 6. Bend Time (`DishonoredActivePowerComponent_BendTime`)

Tweak per level: world dilation 0.15 / **0.0**, player dilation 0.85 / 0.9, duration 12 / 8,
warm-up 0.15, cool-down 0.25, projectile early wake-up 0.25; mana 60. Start (0x00BF5190 / 0x00BF8810)
asks the helper for layer 0 with `(world, caster, ..., warm-up)`; the tick (0x00BFBE60) runs in real
time: post-effect fade, at `duration - cooldown` the layer goes back to 1 over the cool-down, projectiles
wake `earlyWakeup` before the end (0x00BF36F0), the end at `duration` (0x00BFA200). Using it again
(0x00BF8950) moves the start so only the cool-down remains. Port `start_bend_time`, `ramp`,
`pawn_time_scale`, `world_time_scale`, `step_time_bends`, `end_bend_time_early`: **match for level 1**
(hook-measured: 0.85 over 0.15 s, 12.01 s, back from 11.76 s); level 2 is data only (`world 0.0` is an
exact stop through the helper's zero rule); Possession's extension is missing (F-09). The port ramps
linearly; the helper is linear per its slope formula (CONFIRMED by the decode).

## 7. Devouring Swarm (`Twk_DevouringSwarm`)

Per level: rats 30 / 60, duration 30, detection radius 700, escape radius 750, initial damage delay
2 s (none on the player), damage interval 0.5 s, `clamp(rats x 0.4, 2, 5)` per bite, slowdown 40 % for
1 s, eat startup 1.5 / 0.75, per limb 1.5 / 0.75, `EatRequiredRatCount` 5, spawn delay 0.65 s after the
cast notify (0.165 s); mana 40. The original's rats are crowd agents (`RatSwarm.CrowdAgent_SmallRat`,
`DisGameCrowdAgentSkeletalRat`: health 1, `MaxSpeed` 350, `Twk_Rat.m_fAttackDistance` 120); the attack
group update (0x00C891E0), condition (0x00C87EA0), detection and eating (0x00C6D760) are decoded
(INVENTORY.md). The port runs one aggregate group per swarm with those rules (`powers::step_swarms`,
`rats::group_damage`): **known difference** (U-503); the actor count and behaviour per rat are a
capture request (the hook agent is capturing it). Corpse eating: level 1 12 s (+0.5 s after a kill),
level 2 6 s: data **match**, not verified in motion.

## 8. Passives

`DishonoredPowersComponent.m_Powers` (from `DefaultPlayer.ini`): each level lists its **own full set**
of modifiers; `SetPowerLevel` (0x00ACE270) removes the current level's modifiers, stores the level and
applies the new level's list only (not cumulative: Agility level 2's list repeats `JumpZ_PowerJump +1300`
and adds the sprint / water / mantle changes). Port `powers::attribute`: **match** (current level only).
Values: Vitality 1 `HealthMax +20`; Vitality 2 `HealthMax +20`, `HealthRegenLimit +20`,
`HealthRegenInitialDelay` set to 5; Blood Thirsty 1 nothing, 2 `AdrenalineMax` -20 %; Shadow Kill levels carry no modifiers (rule in 0x00A87B60); Agility 1 `JumpZ_PowerJump
+1300`, falling-damage / death speeds set to 2550 / 3500, the `*_PROTOTYPE` attributes (registered, no
consumer found); Agility 2 `JumpZ_PowerJump +1300` more, `GroundSpeedSprint +30 %`, water speed +10 %,
land / mantle animation rate +50 %, sprint strafe / back multipliers set to 0.5 / 0.4.
Raising the level raises the stored max and **current health by the increase** when the new maximum is
larger (the code adds the difference to current health when called with the heal flag clear): Vitality
1 gives +20 health at once; going from 1 to 2 does not (same `HealthMax`). **Port: missing** (F-11).
Shadow Kill (0x00A87B60), Blood Thirsty (0x00AAC510 and neighbours): see PLAYER.md / INVENTORY.md; the
port's `powers::shadow_kill_applies` and `adrenaline.rs` follow them (not re-verified here).

## 9. Findings

(Port status after the 2026-10-08 port pass: section 12.)

| id | class | finding | fix for the port |
|---|---|---|---|
| F-01 | port missing | Blink has a 1.0 s lock-out: `m_bActive` stays set from the aim until `m_fCooldownTime` after the move or cancel (0x00BE71F0 CanUse, 0x00BFC020, 0x00BED480, 0x00BE9180) | add an active/cooldown timer to the Blink state: set when aiming starts, after a release run the move (mode 2) or cancel (mode 3) then 1.0 s; refuse aim start while set. Scenario `powers/blink_cooldown` (Count BlinkStart 2) |
| F-02 | port bug | The original starts aiming only with mana >= cost; the port aims and fizzles at the release | in `blink::update` require `mana >= cfg.mana_cost` (and not cooling down) before setting `blink_target`; play the "empty" feedback instead. `powers/blink_no_mana` |
| F-03 | port missing (consumer Unverified) | Blink drives dilation layer 1: 0.05 over 0.9 s on release, back to 1.0 over 2.5 s after the move/cancel, caster rate 1.0 (0x00BFC990, 0x00BE7380) | add a second dilation layer to `WorldState` with those ramps; fold it into `pawn_time_scale` (non-casters) and `world_time_scale`; decide the combination with Bend Time after the capture. `powers/blink_world_slow` |
| F-04 | port bug | Windblast always uses the level-2 values (`tuning.rs` reads `m_Levels[1]`): a level-1 caster should deal damage 20, force 1800, extra up 1000 | read `m_Levels[level - 1]` per caster in `tuning.rs` / `step_windblasts` (store the level on the `WindblastState`). `powers/windblast_guard_l1` (18 vs 26) |
| F-05 | port missing | Windblast range (gather radius and front cap) is `distance x (1 + SpellStrength)` | multiply by `1 + attribute(SpellStrength)` |
| F-06 | port missing (consumer Unverified) | `m_HitWallAdditionalDamage` (100), NPC explode chance 0.3 above speed 500 with impulse 100 are tweak fields of Windblast; `windblast_wall_damage` is loaded but unused | locate the consumer (candidates: NPC pawn functions around 0x00B59400-0x00B87680 reading tweak +0x1E0..+0x1EC), then apply wall damage when a thrown NPC hits geometry; capture first |
| F-07 | port difference (Unverified) | The original gathers every actor in range (props, doors, projectiles) and applies the factor; the port deletes projectiles in the cone and has no props | after a capture of a bolt in flight, replace the deletion by the blast's impulse on the projectile |
| F-08 | port missing | Dark Vision ends about 1 s after combat starts (`m_fDesactivateInCombatTimer`, 0x00BF84C0) | in `tick_dark_vision` count a combat timer while the AI reports opponents engaged with the player; at 1.0 s zero `remaining`. Definition of "combat" needs the capture. `powers/dark_vision_combat` |
| F-09 | port missing | Possession start during Bend Time extends it by `m_fBendTimeExtension` (4 s at level 2; 0x00BFB6C0 -> 0x00BFA120); the port parses `bend_time_extension` and never uses it | when `step_possession` enters `Active`, add `cfg.items.possess.bend_time_extension[level-1]` to the caster's bend `duration` if it is running and `elapsed < duration - cool-down`. `powers/bend_time_possess_extension` |
| F-10 | port bug (minor) | Possession targeting: no per-class priority weight, range not scaled by `1 + SpellStrength`, max angle fixed at 27 deg although the original's is read from a controller field | add the three; capture the field |
| F-11 | port missing | Buying/setting a passive level raises current health by the HealthMax increase (0x00ACE270) | in `powers::buy_power` (and the loadout path) compute the max before and after and add the positive difference to health. Session probe queued |
| F-12 | known difference | One aggregate swarm group vs individual crowd agents (U-503) | hook agent's capture decides |
| F-13 | Unverified | The Windblast front is advanced with the world dilation; the component belongs to the caster, whose rate is the caster dilation under Bend Time | capture the time to damage with Bend Time active |
| F-14 | Unverified | Possession end on host death: the original tests for a destroyed host, the port for `!alive` | capture: kill the host during possession |

## 10. Scenarios and captures

New scenarios (`fixtures/oracle/scenarios/powers/`), Boyle runway unless noted (they can be captured
on the original; the older Playground scenarios cannot):

| scenario | rule | expected from |
|---|---|---|
| `blink_pitch_sweep_l1`, `_l2` | targeting range as a function of pitch (8 pitches per level in one hold; horizontal and vertical offset from the eye) | `Static`: decoded formula, matches the 19.84 deg measurement |
| `blink_cooldown` | F-01 | `Static` |
| `blink_no_mana` | F-02 | `Static` |
| `blink_world_slow` | F-03 | `Unmeasured` (derived 0.718 minimum) |
| `windblast_guard_l1` | F-04 | `Static` (tweak x factor) |
| `windblast_fan_l2` (Playground, port only) | factor over 8 distances / bearings, the d <= 100 branch, front timing | `Static` (formula) |
| `dark_vision_timeline` | cost 20, 30.05 s, second press inside the cooldown ignored | `Static` |
| `dark_vision_combat` | F-08 | `Unmeasured` |
| `possession_timeline` | cost 60, intro and 15 s duration | `Static` |
| `bend_time_l2_stop` | level 2: rate 0, 0.9, 8 s, cost 60 | `Static` |
| `bend_time_possess_extension` | F-09 | `Static` (12 s) |
| `swarm_l2_cast` | cost 40, 60 rats | `Static` data |

All of them are in `fixtures/oracle/capture_queue.txt`; the queue also holds the Vitality session
probe (read health, set the level, read again, twice). Open captures that no scenario expresses
(Unverified rows above): Windblast during Bend Time, Possession with a killed host, the Blink
layer's effect on `WorldInfo.TimeDilation`.

Coverage that is deliberately not duplicated: Windblast level 2 hit (26, momentum), Blink range and
travel, Bend Time level 1 and the mana refund already have hook baselines in the older scenarios and
HOOK_TRIAGE.md.

## 11. Port results (run 2026-10-08, port against the decoded expectations)

| scenario | result | reading |
|---|---|---|
| `blink_pitch_sweep_l1`, `_l2` | pass, 32 / 32 | the port's targeting equals the decoded envelope over 8 pitches at both levels (to 0.1 uu) |
| `blink_cooldown` | fail | three Blinks happen (original rule: two), 60 mana spent instead of 40: **F-01 confirmed** |
| `blink_no_mana` | fail | with 10 mana the port still aims (`left.blink_target` set) and only fizzles on release: **F-02 confirmed** |
| `blink_world_slow` | unmeasured | the port's guard stays at rate 1.0 (original: dips to about 0.718, back at 1.0 2.5 s after the move): **F-03** |
| `windblast_guard_l1` | fail | level 1 deals 26, the level 2 value (original 18): **F-04 confirmed** |
| `windblast_fan_l2` | pass 8 / 8 judged | distance / bearing factor, the front timing (0.5 s to 2000 uu) match the decoded formula; the d <= 100 dummy is unmeasured (the port''s eye is 85 above the pawn centre, so no reachable target is within 100 in 3D) |
| `dark_vision_timeline` | pass | cost 20, 30.05 s |
| `dark_vision_combat` | unmeasured | the port never ends the effect with a hostile guard (no end event in 10 s): **F-08** |
| `possession_timeline` | pass | cost 60, 15.02 s of possession |
| `bend_time_l2_stop` | pass | rate 0, caster 0.9, 7.98 s, cost 60 |
| `bend_time_possess_extension` | fail | Bend Time still ends at 7.98 s (original 12): **F-09 confirmed** |
| `swarm_l2_cast` | pass | cost 40, 60 rats, summoned at 0.27 s |

A trap met on the way: the Boyle runway start of the older scenarios (-3400, -3150, -190, facing
south) has a wall 957 uu ahead in the port''s collision, so a 1600 uu Blink there is cut short; the
sweeps therefore start at (-3201.5, -2763.3, 20) facing west (1594 uu clear at 5 degrees).

## 12. Port status of the findings (2026-10-08, after the port pass)

Re-read with the static index before porting; the decode details are in LOG.md (2026-10-08, "Powers
findings ported"). Scenario run after the port: 21 pass, 0 fail, 2 unmeasured.

| id | status | what the port does now | scenario before -> after |
|---|---|---|---|
| F-01 | **ported** | `BlinkTravel.active / cooling / cooldown_timer`: the press starts the power only when `blink::can_use`; the move's end, a cancel (release too close / no target, or deselecting while aiming) enter the cooldown; `blink::tick_cooldown` (every tick, whatever the left hand holds; dead pawns deactivate) clears it past `m_fCooldownTime` (new `BlinkConfig.cooldown_time`) | `blink_cooldown` fail (3 moves, 60 mana) -> pass (2 moves, 40 mana, no fizzle) |
| F-02 | **ported** | no aim, no target, no event without the mana (0x00BFC020 returns before anything) | `blink_no_mana` fail -> pass |
| F-03 | **ported** (world rate unmeasured) | `WorldState.blink_layers` (one `DilationLayer` per caster, replicated in `WorldExtras`): `set_blink_layer` is the helper's clamp and slope, `step_blink_layers` the GameInfo tick's linear step (real time); `pawn_time_scale` multiplies the other pawns by it, `world_time_scale` everything; the caster's own Bend Time clock runs at dt x its layer (`m_bAffectsBendTimePower`). Combination with Bend Time is multiplicative (decoded, 0x009FCBC0). The cooldown's end re-blends from the current value, so the return takes about 1 s + 2.5 s after the move | `blink_world_slow` unmeasured: the guard's rate dipped to 0.824 (the port's 10-tick move into the runway wall; the scenario's 0.718 assumes a full 16-tick move), back at 1.0 by tick 400 |
| F-04 | **ported** | `CombatTuning.windblast_*` are per level; `WindblastState.level` from the caster's `m_Powers` level | `windblast_guard_l1` fail (26) -> pass (17; tol 1 around 18: the port's factor at this spot is 0.85-0.875) |
| F-05 | **ported** | `WindblastState.range = m_Distance x (1 + SpellStrength)` (gather, front cap, lock-out) | covered by the level-2 scenarios (SpellStrength 0) |
| F-06 | **ported** | `left_hand::windblast_wall_hits`: the falling move records a blasted NPC's wall contact (`PlayerState.wall_hit`, `windblasted_by`); rule as decoded (level 2 only, speed >= 500, wall-like normal, 100 damage, 30 % explode = body removed). The original collides the ragdoll, the port the falling capsule | new test `windblast_level_2_wall_hit_damages_the_thrown_npc` |
| F-07 | **decoded, not ported** | the original never deletes projectiles: `DisProjectile` (vslot +0x37C) takes no damage and reacts by `m_WindblastBehavior` (Normal redirect / PushTowardNPC / Homing on the aimed NPC or point, x1.2 speed over 1.6 s). Porting needs each projectile tweak's behaviour and the homing steer in the projectile tick (the `+0x2B0` mode); the port still deletes projectiles in the cone | - |
| F-08 | **ported** (combat definition LIKELY) | `PlayerState.combat_opponents` written by `dishonored_ai::Crowd::write_awareness` (brains in Combat against the pawn = the port's skirmish); `tick_dark_vision` runs the combat timer in the duration step | `dark_vision_combat` unmeasured: no end -> ends 1.05 s after the cast (expected 1.1 +- 0.4) |
| F-09 | **ported** | `step_possession` adds `bend_time_extension[level-1]` when the possessor's bend has `elapsed < duration - m_fPostEffectCooldownTime` (new `BendTimeLevel.post_effect_cooldown`, 0.5) | `bend_time_possess_extension` fail (7.98 s) -> pass (12.0 s) |
| F-10 | **ported** | higher-is-better score with the angle term normalised by the FOV, the distance term, the per-class priority weight (`PossessableConfig.targeting_priority`, `PossessConfig.priority_weights`), range x (1 + SpellStrength), cone `m_fMaxAngleFOVFraction x FOV`; pawns and swarm rats compete in one list. FOV: the sim's reference 75 deg (`m_fCurFOV` follows the player's FOV setting and camera FOV targets; whether Possession's `m_fCameraFOVZoomSpeed` narrows it while aiming is not decoded) | `possession_guard`, `possession_timeline` pass |
| F-11 | **ported** | `powers::set_power_level` (used by `buy_power` and the oracle's `power` console command) | new test `vitality_raises_current_health_with_the_maximum` |
| F-12 | **ported** (measured 2026-10-08) | the swarm releases its level's `m_RatCount` rats (30 / 60) as individual rat pawns (`PlayerState.swarm`) over 16 ticks once the actor appears `m_fRatSwarmSpawnDelay` after the cast notify; they live `m_fDuration` (30 s), then stop and despawn over 10 s. Each rat is driven by the `dishonored_ai` rat-swarm brain with the Devouring Swarm tweak (sparing the caster); bites go through the attack groups (`rats::step_rat_attacks`) with the level's damage values, credited to the caster as `DamageCause::Rats`; eaten corpses leave no body (the mission now applies the brains' `RatEat` devoured events). Possession takes a swarm rat like any rat pawn. The aggregate `SwarmState` group (target, bites, eating) is gone. Not decoded: where the spawner places each rat (the port spreads them on a sunflower spiral); the 10 s tail is spread evenly | `swarm_l2_cast`, `devouring_swarm_l1` (now measured references) |
| F-13 | open | the Windblast component's tick dt under Bend Time not decoded further | - |
| F-14 | open | the possession end test is object deletion; not changed | - |

Other measurements of 2026-10-08 checked against the port: Bend Time and Devouring Swarm now take their cost with the cast notify (Bend Time 60 mana 9 ticks after the press, measured; the press needs the mana, `left_hand::update`). Possession L2's cost is taken 12 ticks after the release in the original (the cast notify); the port still pays at the release (open). Windblast L2: 26 damage matches; the blasted guard's ragdoll peaks at 1005 uu/s horizontally while the port adds the momentum (2367 uu/s) to the capsule's velocity: the ragdoll's impulse-to-velocity rule is not decoded (open; the wall rule's 500 uu/s threshold is still passed in the measurement: the 100 `DisDamageType_Impact` kill 5 ticks later is that rule).
