# Stealth and detection: core model, port comparison, findings

Scope: how an NPC sees and hears the player (vision cones, the seen value, light, stance, speed),
how attention accumulates and decays, what noise the world emits, and what the port
(`crates/dishonored_ai`, `crates/dishonored_sim`) does for each rule. Method: rules come from the
decompiled exe (`oracle index`, GOG exe `70da9613...`), the cooked tweaks and the packages, never
from guesses; each check is `Verified` (original data/code and port agree), `Differs`, `Missing`,
or `Unverified` (needs an original capture; listed in `fixtures/oracle/capture_queue.txt`).
Earlier decoding stays in `research/dishonored/AI.md` (attention, hearing through cells, alarms);
this file adds the noise sources, the checks and the findings. Decompiled code is described, not
pasted. Addresses are exe addresses.

## 1. Vision

Pipeline per NPC and tick (`ArkComponentVisionNPC`):

1. **Candidates** (0x00c754c0): for every registered vision target that passes the filter, the
   target gives a point (`target vslot 0`, called with the eye position; see F6), which is
   transformed to eye space and tested by 0x00bf1580.
2. **Cone test** (0x00bf1580 -> 0x00bec910): the inner cone is tried first (range
   `m_fVision_InnerMaxRange` 3500, `InnerFOV` 45 deg), then the outer cone (1700, 160 deg). The
   result is `norm = distance / range of the cone that contains the point`, plus the inner flag.
   The cone is an ellipse: a point is inside when `x >= cos(half)` and `2x^2 - 1 >= (c2v + c
   (cos(fov) - c2v) + cos(fov)) / 2`, `c = cos(2 atan2(sin(half) z, sin(v) y))`, `v` = the up
   (inner 0.25 / outer 0.1) or down (0.7 / 0.7) scale times the half angle, chosen by the sign of
   `z`. `Twk_Vision_Default`: bone `camera_jnt`, `BoneNullzone` 5 deg, inner 45 deg / 3500 (up 0.25,
   down 0.7), outer 160 deg / 1700 (up 0.1, down 0.7). The wide-cone (> 180 deg) branch is unused by
   the shipped tweak.
3. **Line of sight** (0x00c788e0 time-sliced scheduler, 0x00c67520): a trace from the eye to each
   test point of the target with trace flags `0x08023086` (extent 0, or 5x5x5 for the accurate
   variant); a candidate is sighted when one point is clear. The player's test regions come from
   `Twk_DefaultVisRegion` (three settings: 30-150, 50-90, 30-60 uu above the feet).
4. **Sight flag** (0x00b4f970): LOS sets `m_bIsSighted` and `m_fLastLOSDistance = norm`. Without
   LOS an enemy stays sighted while within `m_fDistanceToLoseSight` (300) horizontally and
   vertically.

Port: `Cone::new/contains` and `vision_distance` (`dishonored_ai/src/lib.rs`) follow 0x00bec910 /
0x00bf1580 line by line (checked again in this pass: horizontal and vertical limits, the
`x >= cos(half)` gate, `a <= 0` rejection). `Brain::look` supplies the geometry.

| item | original | port | status |
|---|---|---|---|
| Cone ranges, FOVs, vertical scales | tweak above | same (`VisionConfig` from the tweak) | Verified (data) |
| Ellipse test, inner then outer, `norm = d / range` | 0x00bec910, 0x00bf1580 | `Cone::contains`, `vision_distance` | Verified (code) |
| Vision origin and axis | bone `camera_jnt` (position and rotation; `BoneNullzone` 5 deg, `BonePullback`) | pawn centre + 70 uu, body yaw (`EYE`, `eye_local`) | Differs/Unverified (F4) |
| Target point tested against the cone | `target vslot 0` (not decoded) | the player's centre | Unverified (F6) |
| LOS test points | the target's regions, flags 0x08023086 | centre +60 / 0 / -50, blocked when the hit is more than 40 uu short | Approximation |
| Sight kept within 300 uu without LOS | 0x00b4f970 | `near` in `perceive_with` | Verified (code) |
| Head tracking | attention level HeadTrack drives the head | the port turns the whole pawn (yaw follows the stimulus at level >= HeadTrack; seen in `sweep_angle`) | Differs/Unverified (F5) |

The cone geometry is ellipse-based, so a target below the eye leaves the *inner* cone at a short
range: at 200 uu with the player level with the guard (70 uu below the eye, 19 deg down, inner
down limit 15.75 deg) the port uses the outer cone (`norm = d / 1700`, 0.124 instead of 0.060) and the
rate is 0.82/s instead of 1.0. The size of this effect depends on the real eye height (F4).

## 2. The seen value and attention accumulation

Per sighted target and tick (`DisAttentionInfo_Complex` tick 0x00b69150, seen value 0x00b4fa50):

```
x     = 1 - m_fLastLOSDistance                       (also stored as m_fDebug_RawDistance)
v     = DistanceValueTuning(x) + Attribute_Visibility(target)      (DVCTPAO_Add)
v    *= 1 + Attribute_Visibility[Not]MovingMultiplier              (both 0 unless a power sets them)
v     = clamp(v, 0, 1)                               (m_bClampTargetVisibility, guard profiles: true)
rate  = v * scalar            scalar: m_SeeingPawnAttentionIncreaseScalar_ToPlayer = 1.0 Normal,
                                      1.3 Hard, 1.6 Very Hard; live NPC target 10; dead NPC target 10
value += rate * dt            (pending slot DACRT_Seen, processed in reason order, 0x00b60ca0)
```

`DistanceValueTuning` of the guard profiles: piecewise linear points (0.55, 0) (0.875, 0.8) (0.93,
2.0), clamped to the end outputs. So the distance curve is zero beyond 45 % of the cone range
(1575 uu inner, 765 uu outer), 0.8 at 12.5 % (437 uu inner), reaches 1 (clamped) near 405 uu inner,
and only the target visibility term remains far away.

Levels (`Twk_Attn_Guard_Unsuspecting`, start/stop): HeadTrack 0.125/0.05, TurnToFace 0.35/0.25,
Investigate 0.6/0.3, Busted 0.85/0.8 (each stop rolled with +-0.025 per brain); max 1.0; leak
0.025/s (0.015 suspecting, 2.0 neutral profile) applied only on ticks with no pending change; the
threshold of an increase is a floor (value raised to at least the threshold start). Busted is only
reachable for reasons with `m_bCanIncreaseToBusted` (Seen, Touched, AttackedBy, ... but not Heard
or corpses) and is refused for an unsighted target whose proxy is stale (0x00b4fce0). Port:
`seen_rate`, `level_after`, `Attn::change`, the leak loop at the end of `perceive_with` follow this.

Verified in this pass: `stealth/attention_thresholds` (port, 300 uu ahead, rate clamped to 1.0)
reaches the four thresholds at 0.133 / 0.350 / 0.600 / 0.850 s; and the distance sweep (section 6)
reproduces `DistanceTuning(1 - norm) + 0.1 light01` at each distance. The two original anchors
(HOOK_TRIAGE.md: 0.164 -> 0.703/s, 0.358 -> 0.226/s) agree with the same curve evaluated at the
*original's* norm (0.8 (0.836 - 0.55) / 0.325 = 0.704; 0.8 (0.642 - 0.55) / 0.325 = 0.226), so the
rate formula is not in doubt; the norm is (next section).

## 3. U-864: the port's LOS 0.173 / 0.380 against the measured 0.164 / 0.358

Not a formula difference: it is the geometry. The facts:

* The port's value is exactly `|guard eye - player centre| / 3500`: at the 600 placement the two
  are level and 600.0 apart horizontally, the eye is 70 uu above the guard's centre, so
  `sqrt(600.0^2 + 70^2) = 604.1` and 604.1 / 3500 = 0.1726 (port 0.1726). At the 1300 placement the
  player stands on the sunken courtyard floor (Boyle Ext drops about 200 uu between 700 and 900 uu
  along the lane): horizontal 1300.6, 270 uu below the eye, 1328.4 / 3500 = 0.3796 (port 0.3795).
* The original's norms are 0.164 and 0.358, i.e. 574 and 1253 uu. Both are *shorter than the
  horizontal distance of the placement* (600 and 1300). Whatever point the original measures from
  and to, no point on the eye-to-centre line of the nominal placement can give that. So the original
  was not sampled at the nominal geometry. `probe_launch measure detect` places the player at the
  guard's Location + d along its facing and samples for 1.5 s; the guard reacts (turns, may walk)
  while the attention rises, and the table reports the middle sample; the player is placed at
  `z + 20` and falls to the floor. None of the actual positions at the sample were kept in
  HOOK_TRIAGE.md (the csv has them: `px,py,pz` and the NPC location).
* Candidate causes, ranked (all need the per-tick positions to tell apart):
  1. Setup/sampling (most likely): the guard moved or the placement was not what was nominal.
  2. The tested target point is not the pawn centre but a point on the target's bounds (the target
     record holds a box, 0x00c86bf0, and `vslot 0` is called with the eye position): the closest
     point of the box is up to the box half extent nearer (34-48 uu for a cylinder radius 34), the
     right order for 26 and 47 uu if the two sessions faced in different directions.
  3. The eye is not at +70 and in the body's plane (`camera_jnt` bone): a bone in front of the centre
     shortens the distance on the facing axis.
* Decision: the two checks stay `Unverified` (never judged). Capture request (queue):
  `stealth/detect_600`, `stealth/detect_1300` with the guard's and the player's Location, the guard
  Rotation, `camera_jnt`'s world location (`GetBoneLocation`) and `m_fLastLOSDistance` per tick; and
  `measure detect --dists ...` runs with the csv kept. With those the comparison is
  `norm * range` against `|bone - player point|` on each side.
* Port fix pending that capture: none for the formula. If the bone/target-point hypotheses are
  confirmed: take the eye from the pawn's `camera_jnt` (F4) and the cone point from the target
  record's `vslot 0` (F6).

## 4. Target visibility (`Attribute_Visibility` of the player)

`DishonoredVisibilityComponent` (tick 0x00ad51e0 -> 0x00ad1590), every tick:

```
light01 = clamp((rawLight - darkest) / (brightest - darkest), 0, 1)       (0x00bc9040)
rawLight = light environment SH (static + dynamic) -> luminance 0.3/0.59/0.11, * 2 sqrt(pi), ^(1/2.2) above 1e-4
range   = the player's stealth volume (+0x978/+0x974) settings, else the map's
          DishonoredMapInfo.m_pMapPlayerVisSettings, else Twk_GlobalPlayerVisSettings (0.05 .. 1.0)
          (Boyle Ext: Boyle_Twk.Pvs_BoyleExterior 0.2 .. 0.5)
speed    = |Velocity.xy| / Attribute_GroundSpeedSprint (0x00ac33b0, 600, not clamped);
           forced to 0 while sneaking (pawn sneak mode) or auto-crouching (m_bForceZeroSpeedWhileSneaking)
base     = SpeedTuning(speed, operand = LightTuning(light01)) = 0.25 * speed + 0.1 * light01   (Add)
bonus    = autocrouch -1 | sneaking 0 | sliding 0 | standing 0 | walking 0 | sprinting 0 | swimming -1 | surface swimming 0 (named in the tweak)
Attribute_Visibility = base + bonus  (Modifier_VisComp_Base + Modifier_StealthBonus; +1 with Modifier_ForcePlayerVisible)
```

The component selects the bonus like this (0x00ad1590): swimming states first (-1 for the two
swimming levels), then sliding, then *auto-crouching* (the pawn forced low by a low ceiling;
-1), then sneaking (crouch pressed: the sneak-to-stand delay `m_fSneakToStandBonusDelayTime` is 0 in
the shipped tweak, so it is inert), then standing still, walking and sprinting. Pressing crouch gives
bonus 0 and zero speed, so a crouched player is seen at `0.1 light01` only; it is the *forced*
crouch (and water) that gives the -1 that hides a player beyond about 405 uu.

Port: `player_visibility` / `Brain::visibility`: `speed01 = |v.xy| / ground_speed_sprint`, the speed
tuning with the light as operand, bonus selected from `crouched`, `speed01 == 0`, `sprinting`.
`bonus_autocrouching`, `bonus_swimming` and sliding are parsed (`ai_config.rs`) but never selected:
F7. Light: `light_at_with` sums the level's light actors (0.225 with only the sky term on Boyle
Ext; the original reads 0.168-0.313 at 14 points, U-121, known). Stealth volumes
(`DisStealthVolume`, 43 placed in 10 packages, each with its own `m_pVisSettings` and priority) and
hideouts (`DisHideoutVolume` 93, `DisHideoutAccessPoint` 141, 13 packages: `m_bIsWithinHideout`,
`DACRT_HideOutTransition`) are not modelled at all (F8).

Not verified: `Attribute_GroundSpeedSprint` with the sword drawn (the pawn runs 360 / sprints 540
with it drawn; if the attribute stays 600, speed01 is 0.6 / 0.9). Capture: `session dump
class=DishonoredVisibilityComponent` while running with the sword out (`m_fCurrentSpeedValue`).

## 5. Hearing

### 5.1 Noise events and the dispatch

An AI noise event carries a loudness class (`EAINoiseLoudness`, an index into
`DefaultAI.ini [DisAINoiseManager] m_LoudnessDistanceArray`, 17 entries), a context
(`EAINoiseContext`), the source pawn, a position and an attenuation. The dispatch (0x00c5a760):
`radius = array[loudness] * (1 - attenuation)`; every registered listener (the brains, and pawns in
the world's pawn list) except the source's own is tested by 0x00c3d590 -> the hearing test
0x00bb50d0 (strictly inside the radius; inside the audio cells along the precomputed portal paths;
outside them a trace with flags 0x2286). The event also feeds the player's own noise feedback
(0x00aa9b60).

Loudness classes and radii (uu; verified against `DefaultAI.ini` in this pass): Silent 0, 2m 200,
Whisper 300, 4m 400, Loudness1 500, 6m 600, 8m 800, 10m 1000, 12m 1200, Loudness2 1500, 20m 2000,
25m 2500, Loudness3 3000, 40m 4000, 50m 5000, Loudness4 6000, Kismet-specified 0 (the Kismet node
supplies `m_fCustomSoundRadius`).

Contexts: AmbientWhiteNoise, Anomaly, Footfall, Threatening, Danger, Help, AlarmBell, RatSwarm,
ImpendingExplosion, WolZap, AllySearchRequest, DeathRattle, AllyNotice, FlareCombustion,
WatchTowerAttack.

### 5.2 Who makes noise (decoded in this pass)

The 34 call sites of the noise-event constructor 0x00b14e80 are: the contact system
(`DisContactSub_AINoise`, 0x00bd6c20), `DisNotify_MakeAINoise` animation notifies (only on the
wolfhound attack sets in the retail packages), Kismet `DisSeqAct_AINoise` (0x00b8aee0), the alarm
bell (0x00a36dd0, Loudness4, AlarmBell), the player pistol (`DisItemContext_FirePistol` 0x00c2a3e0:
`DisTweaks_FirePistol.m_AINoiseLoudnessWhenFiring` = Loudness4, context Danger), the death rattle
(0x00a994c0; `DisTweaks_NPCPawn.m_DeathRattleLoudness` Whisper, context DeathRattle), help barks
(context Help), rat swarms, explosions (breakables), Wolf-of-Light zaps, flares, watchtowers,
and several tweak-driven callers not decoded (0x00a09f90, 0x00a24fc0, 0x00a2bc40, 0x00a34460,
0x00a39e00, 0x00a3a5f0, 0x00a6a620, 0x00c666a0, 0x00ce3120, 0x00d00ce0). The crossbow has no
`m_AINoiseLoudnessWhenFiring`: firing a bolt makes no AI noise.

The contact system carries most of the physical noise. A contact sub fires when the striker/struck
pair has one (`Phy_ContactSystem.ContactSystem.Dis_ContactSystem`, 29 `DisContactSub_AINoise`
objects); nothing is emitted when the loudness class is Silent (0, the class default), when the impact
speed is under `m_fMinSpeed` of the sub's volume settings, or when the volume scale is under
1e-4. `m_AttenuationAttribute` (glass) reads an attribute of the struck object as the attenuation.
Footsteps depend on the surface only through the sound and particle subs; the AI noise is the
same on every surface.

| striker -> struck | loudness (radius) | context | note |
|---|---|---|---|
| `Footfall_Run_Player` -> Environment | 6m (600) | Footfall | player running |
| `Footfall_Sprint_Player` -> Environment | 12m (1200) | Anomaly (class default) | player sprinting |
| `Footfall_Sneak_Player`, `Footfall_Run_Player_Silent` | none (Silent) | - | sneaking: no noise |
| every NPC footfall type (Run, Sneak, Sprint, Tallboy, Wolfhound) | Silent | Footfall | NPCs make no step noise |
| `JumpLand` -> Environment | 4m (400) | Anomaly | |
| `JumpLand_Big` -> Environment | 8m (800) | Anomaly | |
| `JumpLand_Stealth` | Silent | Footfall | |
| `Sword` -> Body | 8m (800) | Threatening | hit |
| `Sword` -> Weapon | 8m (800) | Threatening | clash / block |
| `Weapon` -> Environment | Loudness2 (1500) | Threatening | |
| `Projectile` (bullet) -> Environment | Loudness3 (3000) | Danger | impact |
| `Arrow_Sleep` -> Environment | Loudness2 (1500) | Threatening | |
| `Arrow_Flare` -> Body | Whisper (300) | Anomaly | |
| `SpringRazor` -> Environment | Loudness2 (1500) | Threatening | |
| `Grenade` -> Environment | 25m (2500) | Threatening | |
| `Corpse` -> Environment | Whisper (300) | Anomaly | `m_fMinSpeed` 200 is inert (not speed-scaled; 0x00BD6C20 gates only scaled subs) |
| `BreakingBones` -> Environment | 8m (800) | Threatening | above speed 200 |
| `BreakingBones_Player` -> Environment | 4m (400) | Anomaly | |
| `Env_Glass` -> Environment | Loudness1 (500) | Anomaly | scaled by `Attribute_BreakingGlassSoundAttenuation` |
| `Env_Metal` -> any (generic) | 20m (2000) | Anomaly | |
| `Environment` -> `Environment` | 8m (800) | Anomaly | |
| `WolfhoundBite` -> Body | 8m (800) | Threatening | |
| `BashPlayer` -> Body | Loudness2 (1500) | Help | |
| `RiverKrustSpit` | 10m (1000) | Anomaly | |
| `Env_WhaleOilBattery` | Loudness1 (500) | Anomaly | |

Which contact type an animation raises for a footstep (`DishonoredNotify_Footfall`) was not traced, and
neither was the lookup order between a base striker type and its player-specific child (the child
entry, e.g. `Footfall_Run_Player`, is read as the one that applies); the type names above are the
reading (LIKELY).

### 5.3 From a heard noise to attention

For a guard (unsuspecting profile) a heard noise adds: Danger +0.4 up to Investigate (floor 0.6),
Threatening +0.3 up to TurnToFace (floor 0.35), Anomaly +0.2 up to HeadTrack (floor 0.125), Help
+1.0 to Investigate, DeathRattle +0.4 to Investigate, alarm sound to Investigate. Hearing cannot
raise a level above Investigate. **Not decoded:** what the other contexts do (Footfall, AmbientWhiteNoise,
RatSwarm, ImpendingExplosion, WolZap, FlareCombustion, WatchTowerAttack, AllyNotice,
AllySearchRequest): the profile has no Footfall field, so Footfall either maps to Anomaly or is
dropped by the listener. The listener's handler (`DisNoiseListenerInterface` slot 2 of the brain
side) is the place to decode; see F1.

### 5.4 Port

`SimEvent::Sound { position, radius, category }` carries a hard-coded radius per emitter; the
brain maps the category to Danger / Threatening / Anomaly (`noise_context`) and runs `hearing::heard`.
The hearing test itself follows 0x00bb50d0 (verified earlier, AI.md "Hearing through audio cells").
The emitters and their radii are the issue (F1-F3).

| port emitter (file) | port radius, context | original |
|---|---|---|
| pistol shot (`gadgets.rs`, `left_hand.rs`) | 4000, Danger | 6000 (Loudness4), Danger; plus a 3000 Danger impact noise |
| crossbow shot (`left_hand.rs`) | 500, Danger | no noise on firing; bolt impacts by arrow type (1500 Threatening for sleep darts) |
| sword swing (`combat.rs`, `left_hand.rs`) | 300, Threatening | no noise for a swing in the air; 800 Threatening on a hit or a clash |
| sword hit / block (`combat.rs`) | 600, Danger | 800, Threatening |
| body fall (`combat.rs`, `takedown.rs`, `powers.rs`) | 250-400, Danger | Whisper 300, Anomaly, only above impact speed 200 |
| windblast (`left_hand.rs`) | 1500, Danger | not found in the contact table (tweak-driven caller not decoded) |
| blink (`blink.rs`) | 400, Anomaly | none found |
| footsteps, sprint, jump land, mantle | declared (`SoundCategory::Footstep/Sprint/Land/Mantle`), never emitted | run 600, sprint 1200, land 400/800, sneak none |

## 6. Port measurements (this pass, headless, `oracle scenario stealth/*`)

Distance sweep (`stealth/sweep_distance`, player level or on the sunken floor, still, Boyle Ext party
guard, eye = centre + 70): port values; the formula `rate = clamp(Tuning(1 - norm) + 0.1 light01)`
holds at each point (checked against `seen_rate` with light01 = 0.083).

| d (horizontal) | 3D to eye | cone | LOS norm | rate /s |
|---|---|---|---|---|
| 200 | 211.5 | outer (the player is 19.3 deg below the eye, the inner down limit is 15.75) | 0.1239 | 0.82 |
| 300 | 308.1 | inner | 0.0880 | 1.000 |
| 400 | 406.1 | inner | 0.1160 | 1.000 |
| 500 | 504.9 | inner | 0.1442 | 0.761 |
| 600 | 604.1 | inner | 0.1726 | 0.691 |
| 800 | 807.1 (player 107 below the eye) | inner | 0.2306 | 0.548 |
| 1000 | 1035.8 | inner | 0.2959 | 0.388 |
| 1300 | 1327.8 | inner | 0.3794 | 0.182 |
| 1575 | 1598.1 | inner | 0.4566 | 0.008 (light only) |
| 1700 | 1721.4 | inner | 0.4918 | 0.008 |
| 2000 | 2018.1 | inner | 0.5766 | 0.008 |

Angle sweep (`stealth/sweep_angle`, horizontal angle off the facing): at 600 uu the inner cone holds
up to 20 deg (22 deg is already outer because the player is 6.7 deg below the eye), the outer cone
to 79 deg, not seen at 81 deg or more; at 400 uu the inner limit is between 10 and 20 deg.

Speed and stance (`stealth/sweep_stance_speed`, 2000 uu, distance curve zero): still / crouched
0.008/s (light only); crouched + strafing 0.008/s (speed forced to zero); running 0.12-0.13/s at
about 330 uu/s (0.25 x 330 / 600 + 0.008 = 0.145 predicted; the speed ramps within the window).
Hearing: `stealth/hear_footfalls` gives no noise attention in any of the three gaits (F1);
`ai/investigate_noise` (existing) uses the 4000 radius.

## 7. Findings

| id | class | finding | evidence | fix for the port |
|---|---|---|---|---|
| F1 | port missing | The player's footsteps, sprint, jump landing and mantle make no AI noise; guards never hear a running or sprinting player. | `SoundCategory::Footstep/Sprint/Land/Mantle` are never constructed (grep over `crates`); contact table: `Footfall_Run_Player` 6m (600), `Footfall_Sprint_Player` 12m (1200), `JumpLand` 4m (400), `JumpLand_Big` 8m (800), sneak/silent none (`DisContactSub_AINoise` 22, 25, 29, 12; Silent subs 17-19, 23, 24, 8). `stealth/hear_footfalls`: noise attention 0 in all gaits. | Emit a `Sound` per player footfall (step cadence of the run/sprint animation) with radius 600 (run), 1200 (sprint), none when sneaking; landings 400 / 800 (big fall), none with the stealth landing; source = the player, so the source's own listener is excluded. Needs the Footfall-context mapping (F2) first. |
| F2 | Unverified | How the Footfall context (and AmbientWhiteNoise, RatSwarm, ImpendingExplosion, WolZap, FlareCombustion, WatchTowerAttack, AllyNotice, AllySearchRequest) become attention increases: `DisTweaks_PawnAttention` has no Footfall field. | `index class DisTweaks_PawnAttention`; the contact table gives the sprint footfall the Anomaly context but the run footfall Footfall. | Decode the brain-side noise listener (the `DisNoiseListenerInterface` slot 2 implementation reached from 0x00c5a760 -> 0x00c3d590) or capture the guard's attention after a sprint/run footfall. Until then map Footfall to Anomaly (+0.2, HeadTrack) as the least wrong. |
| F3 | port bug | Hard-coded noise radii/contexts do not come from the original data. Pistol 4000 (original 6000, `DisTweaks_FirePistol.m_AINoiseLoudnessWhenFiring` = Loudness4); crossbow fire 500 Danger (original: none on firing); sword swing 300 and hit/block 600 Danger (original: none in the air; hit and clash 800 Threatening); body fall 250-400 Danger (original: Whisper 300 Anomaly above speed 200); missing bullet impact noise 3000 Danger and grenade 2500, razor 1500. | `index` + `tweaks` + the contact dump (section 5.2); `DefaultAI.ini` array. | Replace the literals with the table in 5.2 (loudness class -> radius from the ini array; context per row); impact noises at the impact point with the same rules (min speed, volume scale). `ai/investigate_noise` and the 4000 comment need the new radius. |
| F4 | port differs, Unverified | Vision origin and axis: the original uses the head bone `camera_jnt` (position, rotation, `BoneNullzone` 5 deg, `BonePullback`); the port uses centre + 70 and the body yaw. Short-range cone choice (outer vs inner) and the norm depend on it. | `Twk_Vision_Default.m_Vision_BoneName`; `DisTweaks_Vision` fields. | After the capture (bone location relative to the pawn Location, head vs body yaw), set the eye from the pawn's `camera_jnt` pose. |
| F5 | port differs, Unverified | HeadTrack turns the whole pawn in the port (yaw follows the stimulus); in the original the level drives a head look-at. | `sweep_angle`: the guard's yaw follows the player once the value passes 0.125. | Verify with a capture of the guard's Rotation and `camera_jnt` yaw; keep body yaw until TurnToFace if confirmed. |
| F6 | Unverified | The point the cone and LOS are computed for: `target vslot 0` (called with the eye position), plus 3 LOS test points. | 0x00c754c0, 0x00c86bf0 (target record caches a bounding box and `Attribute_Visibility`). | Decode `vslot 0` of the vision-target record (candidate: closest point of its bounds); explains U-864 if so. |
| F7 | port missing | `bonus_autocrouching` (-1), `bonus_swimming` (-1) and sliding are never selected: a forced crouch (low ceiling) or swimming does not hide the player. | `player_visibility` only picks sneaking/standing/walking/sprinting; 0x00ad1590 selects swimming, sliding, auto-crouch first. | Pass the pawn's auto-crouch (low ceiling), swim and slide states to `player_visibility` and apply the tweak's bonuses; keep the zero-speed rule for sneak and auto-crouch. |
| F8 | port missing | Stealth volumes (per-volume light range, priority) and hideouts (hiding in wardrobes/under beds: `m_bIsWithinHideout`, `DACRT_HideOutTransition`, `DisAIBrainProcessHideoutWatcher`) do not exist. 43 stealth volumes, 93 hideout volumes and 141 access points are placed in the retail maps. | `package_scan --class DisStealthVolume / DisHideoutVolume / DisHideoutAccessPoint`; no matches in `crates`. | Stealth volumes: `DisStealthVolume.m_pVisSettings` overrides the light range while the player is inside (highest priority). Hideouts: needs its own pass (player hidden = not a target; the watcher). |
| F9 | Unverified | Which attention profile the neutral Boyle party guard uses toward the masked player, and the speed normalisation with the sword drawn (360/540 against `Attribute_GroundSpeedSprint`). The port reaches Busted at 0.85 s from a party guard 300 uu away. | `attention_thresholds` passes with the unsuspecting-enemy thresholds; the neutral profile (`Twk_Attn_Default_Neutral`) has 1.0/0.9 thresholds and leak 2.0. | Capture `PlayerAttnInfo.m_PawnAttentionTweaks` of the guard and the visibility component values while running with the sword out. |
| F10 | oracle limitation | `Console("teleport")` / `calm` exist only on the port. The sweeps need the session `place` op and an attention reset on the original. | `tools/oracle/src/runner.rs console()`; `probe_launch scenario` passes console text to the game. | Run the sweeps through `measure detect` (distance, stance, strafe/sprint) or a session script; see the queue. |
| F11 | known difference | U-121 (light probe lacks the static-collection lights): raw light 0.225 against the original 0.168-0.313; the light term is at most 0.1 x light01, <= 0.03 on Boyle Ext. | HOOK_TRIAGE.md. | open |

## 8. Scenarios and captures

All in `fixtures/oracle/scenarios/stealth/`; port-run with `oracle scenario stealth/<name> --dir
<worktree>/fixtures/oracle/scenarios`. References are `Unmeasured` unless a source says otherwise.

* `sweep_distance` (11 distances, one run): LOS norm and attention gain per distance; one capture
  validates `DistanceTuning` + visibility over its range and the cone switch near 200 uu.
* `sweep_angle` (400 and 600 uu, 14 angles): which cone applies and the visibility edge at 79-81 deg.
* `sweep_stance_speed` (2000 uu: stand, crouch, crouch + strafe, strafe, strafe + sprint, forward):
  the visibility term alone (the distance curve is zero there).
* `attention_thresholds` (Static, passes): level times 0.125 / 0.35 / 0.6 / 0.85 s at rate 1.
* `hear_footfalls` (guard turned away, player approaches from 1800 uu: sneak, run, sprint): first
  heard distance; original expected inside 600 (run) and 1200 (sprint), never when sneaking.
* `detect_600`, `detect_1300` (existing, re-queued with the geometry recording, U-864).

Open captures (queue): the geometry of `detect_*`; guard `camera_jnt` pose; the guard's attention
profile; the footfall noise events and the guard's response; `Attribute_GroundSpeedSprint` with the
sword drawn; live dumps of `DisAINoiseManager`, `DisTweaks_Vision` and the visibility component.

Not covered this pass (listed so they are not assumed): corpse discovery (the port treats a
corpse as found when its seen value is non-zero with the dead-NPC scalar 10; the original raises
`DACRT_FoundCorpse`, mechanism not decoded), group alarms and the alarm bell (decoded in AI.md, unit
tested in the port, no scenario), `DisTweaks_AIBrain_Reaction` responses, the suspicion level switch
(Unsuspecting to Suspecting profile).

## 9. Port status (2026-10-08, stealth port)

Decoded in this pass (re-read with the static index):

* **Footfall -> attention (F2, decoded).** The brain's noise message (0x00B43E10 -> 0x00B439A0) runs the
  attention process's handler 0x00B40B70 on the `DisAINoiseParameters` (context at +0x19). Its switch:
  Anomaly (1) and Footfall (2) -> `m_IncreaseForAnomalySound` (+0x130); Threatening (3) and WolZap (9) ->
  `m_IncreaseForThreateningSound` (+0x128); Danger (4), RatSwarm (7), WatchTowerAttack (14) ->
  `m_IncreaseForDangerSound` (+0x120); DeathRattle (11) -> `m_IncreaseForDeathRattleSound` (+0x188). For
  Footfall the stimulus position is raised by (noise maker's target point z - its floor z), i.e. from the
  floor contact to the walker. Help (5), AlarmBell (6), AllySearchRequest (10) go through the controller
  path 0x00C58560's own cases; AmbientWhiteNoise, ImpendingExplosion, AllyNotice, FlareCombustion raise no
  attention in 0x00B40B70. Enum order confirmed with `enum_dump` (DisGlobalEnums.EAINoiseContext).
* **Footfall striker (F1).** `DishonoredPlayerPawn` vslot 398 (0x00AAB7D0): not sprinting (or crouch flags):
  sneaking -> `Footfall_Sneak_Player`; `Attribute_SilentRunning` -> `Footfall_Run_Player_Silent`; else the
  body tweak's `Footfall_Run_Player`. Sprinting: `Attribute_SilentSprinting` -> silent, else
  `Footfall_Sprint_Player`. Footfalls are made by `DishonoredNotify_Footfall` (0x00A8D5F0 -> 0x00B66ED0, one
  per foot per 0.25 s) -> 0x00B66A80 (trace under the foot, contact at |Velocity|). The player's
  `Ply_Sword_Locomotion_as` loops carry them: Walk 2.0 s (1.0, 1.915), Run 0.967 s (0.467, 0.933), Sprint
  0.567 s (0.267, 0.533).
* **Landing striker (F1).** 0x00AB53A0: a landing faster than `m_fFall_Stun_Vel` (2200, DefaultPlayerState.ini)
  on world geometry is the stun landing (class `DAT_0146f0c4`, read as `JumpLand_Big` by its meaning; the class
  getters' static order would put it last, so this is LIKELY); otherwise `JumpLand_Stealth` when
  `StatePlayerMasterFalling.m_bIsStealthFall` (set at the fall state's entry from `m_bSneakModeToggled`,
  0x00A759F0), else `JumpLand`. A fall that did not come from the jump and lasted at most `m_fAnimateFallTime`
  (0.25, DefaultPlayerState.ini) makes no landing at all (0x00A5FF10): steps down are silent.
* **Contact noise gate (F3).** 0x00BD6C20: `m_fMinSpeed` gates only speed-scaled subs; none of the AINoise
  subs read here is speed-scaled, so the corpse's 200 is inert. The volume scale only gates (<= 1e-4); the
  radius is `array[loudness] x (1 - attenuation)`. Kismet `DisSeqAct_AINoise.m_eNoiseContext`
  (`EDisAINoiseLimitedContext`) maps Anomaly/Threatening/Danger/AlarmBell to 1/3/4/6 (0x00B863C0).
* **Visibility states (F7).** 0x00AD1590: swim state 2 (head under, `m_pActiveWaterVolume` set) ->
  `m_fVisLevelBonus_Swimming` (-1), 1 (surface) -> `_SwimmingOnSurface` (0); then slide
  (`StatePlayerMasterSlide`), then `Pawn.bIsCrouched` (the engine crouch = crawl) -> `_Autocrouching` (-1), then
  sneaking (`m_bSneakModeToggled` and not crouched, or the sneak-to-stand delay) -> `_Sneaking`, then speed
  < 1e-4 -> `_Standing`, then `m_bSprinting` ? `_Sprinting` : `_Walking`. Speed forced to 0 while sneaking or
  crouched, not while swimming.
* **Stealth volumes (F8).** 0x00BD0120: the player's current stealth volume (+0x978) with `m_pVisSettings`, else
  scripted settings (+0x974), else the map's / global (0x00BC8E60). Touch/untouch (vslots 115/116) go to the
  player's handler; the global list is ordered by `m_StealthVolumePriority` (0x00C8C280 inserts before the first
  lower priority), so the head is the highest priority. 43 volumes in 10 `*_Stealth` packages (none in Boyle).
  Hideouts (`DisHideoutVolume`, `DisHideoutComponent`, `DisAIBrainProcessHideoutWatcher`) are a separate
  system: not decoded.
* **Head (F4 / F5), new evidence.** The original detect_600 trace (ORACLE.md 2026-10-08): first sighting through
  the outer cone (600 / 1700), the inner cone once the head turns (591 / 3500). The vision axis is the head, and
  the head turns before the body. Not ported: the idle `camera_jnt` orientation is an animation pose and the
  head look-at (`m_fHeadTrackTorsoRatio`) was not decoded.

| id | status | what changed |
|---|---|---|
| F1 | ported | `dishonored_sim::noise::step_footfalls` (player footfalls from the locomotion loops' notifies; open: the loop's play rate is taken as its `RateScale` 1, the tree's speed scaling was not decoded), `noise::landing` (JumpLand / Stealth / Big), `noise::track_fall` (`PlayerState::stealth_fall`). Mantle: no AI noise found, none emitted. |
| F2 | ported (decoded) | `dishonored_ai::noise_increase` per 0x00B40B70; footfall stimulus at the walker's height. |
| F3 | ported | `AiConfig::noise` (`NoiseTable`) loaded from the contact system (`dishonored_audio::contact::ContactSystem::ai_noise`) and `m_AINoiseLoudnessWhenFiring`; pistol 6000 Danger at the shooter, bullet impact 3000 Danger (`Bullet` -> `Environment`), crossbow / darts silent on firing, sword swing silent, `Sword` -> `Body` / `Weapon` 800 Threatening, bodies `Corpse` / `SleepingBody` 300 Anomaly when they fall (death, knock-out, drop, landing in `step_body`). `SimEvent::Sound` carries the `NoiseContext`. Not decoded, kept as port values with a context: Windblast 1500 Danger, Blink 400 Anomaly, grenade explosion 4000 Danger, razor detonation 1500 Danger, rat bites 800 RatSwarm. The choke-start and assassination "body fall" noises and the grenade-throw noise (no original source) were removed. Open: grenade bounce (2500) and arrow impacts (sleep dart 1500, flare 300) need the projectiles' contacts (ranged port). |
| F4 | open, evidence | see above; capture of `camera_jnt` per tick queued. `stealth/detect_600` now holds the measured values (fails on the port until the head is modelled). |
| F5 | open, evidence | head-only turning confirmed by the trace; the body-yaw turning is in the AI behaviours (AI port). |
| F6 | open | `vslot 0` of the vision-target record not decoded. |
| F7 | ported (swim, surface, slide); auto-crouch open | `dishonored_ai::player_visibility` / `vis_state`; the sim has no engine crouch (crawl), so `autocrouch` is always false (movement port). |
| F8 | ported (stealth volumes); hideouts open | `AiLevel::stealth_volumes` (`ai_level::read_stealth_volume`), `dishonored_ir::ai::stealth_light_range`, `Crowd::stealth_volumes`, light normalized per player position (`light01`, 0x00BC9040: 0.5 for an empty range). |

Merged with `integ` (2026-10-08): the heard noise now raises attention on its instigator (AI port, from the captures) with the context's increase from 0x00B40B70 (`noise_increase`); ranged and melee noises go through `AiConfig::noise` (pistol at the shooter's Location, sword contacts 800 Threatening); the explosion keeps the ranged port's `m_AINoiseLoudness` radius with context Danger. The oracle runner gives a spawner-placed NPC its spawner's faction (`m_pFactionTweakOverride`, else the pawn tweak's `m_pFactionTweak`): Boyle's spawner 46 is now Neutral_Guard as in the original, and `ai/boyle_hear_shot`'s "heard" check expects the measured 1.0.

### 10. When the light value updates (hook round 4 follow-up, 2026-10-08)

The hook read `DishonoredVisibilityComponent.m_fCurrentLightValue` (+0x68) unchanged (0.588) while standing or walking until sneak was toggled. The code does not gate it on the stance:

* The component tick (0x00AD51E0, vslot 79) writes +0x68 every tick from `FUN_004e1f40` on `pawn.m_pStealthLightingDLE` (`mov ecx,[[esi+0x58]+0x744]`, read from the bytes at 0x00AD51F2), a dedicated `DynamicLightEnvironmentComponent`: `Default__DishonoredPawn.StealthLightingDLE` (`BoundsMethod` ManualOverride, lighting channels `m_LightProbe` + `CompositeDynamic`, `bForceCompositeAllLights`, `InvisibleUpdateTime` 1.0, `MinTimeBetweenFullUpdates` 0.3) with the player's `bEnabled` true, `bRequiresNonLatentUpdates` true, `NumVolumeVisibilitySamples` 4.
* With `bRequiresNonLatentUpdates` the DLE tick (0x005041A0 -> 0x005035D0) takes the full-update branch (0x005034D0: owner 0x004FEB30, static environment 0x00501B70, dynamic 0x00503140) every tick.
* Its bounds are `OverriddenBounds`, which the pawn's Tick (vslot 103, 0x00B50720) copies every tick from the collision cylinder's `Bounds` (+0xCC).
* So the value follows the collision cylinder's bounds every tick. A cylinder whose bounds are not updated (a pawn placed by writing `Location` without a component update) keeps the old light until something re-sizes the cylinder: toggling sneak (65 / 87.5 uu) does. The hook's observation is most likely that artifact; **re-capture with the player walking by input** (and placed through `SetLocation`) before changing the port. The port keeps sampling every tick.
* Lights: the stealth DLE composites only lights in the `m_LightProbe` channel plus the static environment (the precomputed light volume, 4 samples); the port's probe sums every light actor (U-121). On the Playground lane the port's raw value is about 2.6-3.1 x the original's (`stealth/light_probe_lane`, new; port channel `pawn.light` / `pawn.light01`, `Crowd::player_light`).
* The original's Playground is built inside L_Boyle_Ext_P, so its normalized values use Boyle's 0.2 / 0.5 (the measured points fit dark 0.20, bright 0.50); the port's Playground uses the global 0.05 / 1.0.
* References possibly taken on stale values: U-121's 14 original light readings (0.168-0.313) and HOOK_TRIAGE.md's light terms, if they were placed without a component update. `light_vs_dark` and the sweeps hold no original light values (Unmeasured / Static), so no reference there changes.

### 11. The stealth light model (U-121 port, 2026-10-08)

Ported from the decompiled light-environment update, on the same decoding as the props' light environments (RENDERING.md "Light environments", `dishonored_assets::light_env`):

* **Light value** (`dishonored_ir::ai::stealth_light_value`, `Crowd::update_player_light`): `(2 sqrt(pi) x lum(band-0 SH))^(0.454545)` above 1e-4, else 0 (0x004E1DB0: the static and dynamic SH summed per channel, `FUN_004da330` luminance 0.3 / 0.59 / 0.11, `DAT_01435230` = 2 sqrt(pi), confirmed from the initializer bytes at 0x00F67460 and the double pi at 0x00FBEEB8).
* **Lights:** every light list (static, dominant with `bForceCompositeAllLights`, toggleable and movable through the dynamic environment 0x00503140) filtered by the channel test 0x004DB950 against the stealth DLE's overridden channels {`bInitialized`, `CompositeDynamic`, `m_LightProbe`}. A light's `CompositeDynamic` is read as `Dynamic`, so only `m_LightProbe` lights pass: the `PointLight`, `SpotLight`, `DirectionalLight`, `SkyLight` and dominant-light templates set it (Engine.upk); the toggleable and movable templates do not. Lights with `bOnlyAffectSameAndSpecifiedLevels` count only when they list the persistent level (0x00516400). `AffectsBounds` and `GetDirectIntensity` as for the props; each adds `Y00 x intensity x visibility`.
* **Visibility** (0x004FF270, `NumVolumeVisibilitySamples` 4): for lights with `CastShadows && CastStaticShadows`, one line check per update. It starts at the bounds sphere point facing the light plus `extent x sample_k`. The sample points come from the LCG of 0x004FEB30 (seed 0), then the centre. The check ends at the light, or 1 uu along `-axis` for directional and sky lights (`GetPosition` w = 0). The visibility is clear traces / 4 over the last 4 updates, reset when the light was not traced the update before (`Crowd::light_vis`).
* **Light volume:** the floor level's `PrecomputedLightVolume` interpolated at the bounds origin (0x0053DFA0). Each sample's band 0 is `Y00 x lum(Indirect + Environment + Ambient)`. The floor level comes from a line check down: `WorldQuery::hit_package`, from collision sources that now carry the instance's level package (`MeshInstance::level`).
* **Bounds:** the collision cylinder's: origin at the pawn's Location, extent (r, r, h), sphere `|extent|` (`AiConfig::player_visibility.cylinder` from the movement config; sneaking h = 65).
* **Normalization:** the stealth volume's range, else the map's. The Playground uses Boyle Ext's (0.2 / 0.5) because the original builds it inside L_Boyle_Ext_P; its stealth lights are the layout's lights and it has no volume.

**Validation, not reached:**
* On the Playground lane the port's raw light is 2.3 x the original's at the dark points and 2.9-3.1 x at the lit ones. A constant ~3 x on the raw value is ~11 x in linear terms (about 2 sqrt(pi) x pi).
* On Boyle at the detect_600 spot the port reads 0.66 against HOOK_TRIAGE's 0.19. The volume alone gives 0.45 there (band 0 0.048); the sky (0.076) and the directional light (0.151) are the main lights.
* Both references may be off: the Playground lights the hook built may not be the layout's, and the Boyle reading may be stale (section 10).
* Capture requested: at a properly placed Boyle point, the stealth DLE state's SH (+0x450 / +0x480 / +0x4B0 static, +0x4E0 / +0x510 / +0x540 dynamic, band 0) and its per-light contributions, to separate the volume from the lights.
* Until then `stealth/light_probe_lane` keeps failing on raw light.
