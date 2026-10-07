# Engine-hook triage of the open unknowns (2026-10-06)

Every row of `research/UNKNOWNS.md` whose status was LIKELY, APPROXIMATION, SPECULATIVE or
OPEN (95 rows), sorted by whether the engine hook (`tools/probe_launch`, INSTRUMENTATION.md)
can settle it in a live session or whether it needs reverse engineering first.

- **M**: measurable live: a number, a timing, a value read from memory or an observable outcome.
  The plan names the fields (offsets from `class_layout` / `prop_layout`) and the scenario.
- **R**: needs reverse engineering (the answer is a code path, not a value), or a human test.
- **Done** marks the rows measured this round; the results are in the second table.

Counts: 95 rows: **31 M** (12 measured: 7 in the first hook round, 5 in the second; U-340 observed, 18 planned), **64 R**.

## Triage

| ID | Status | Class | Plan / reason |
|---|---|---|---|
| U-008 | OPEN | R | Edge decoder (duplicate of the resolved row) |
| U-012 | OPEN | R | trace-mask semantics per primitive; code |
| U-015 | OPEN | M | collision source: Blink / walk the pawn against a `Collision_model*` actor and see whether it blocks (`measure run at=`), plan only |
| U-020 | OPEN | R | human playtest |
| U-022 | OPEN | R | Edge fixed-format path |
| U-023 | OPEN | M | root motion: sample an NPC's `Location` during a known root-motion anim (soiree `Empty_Distraction_Piss_In`, 72 uu), planned |
| U-024 | OPEN | R | Edge track tables |
| U-025 | LIKELY | M | weapon socket: back-buffer capture of the first-person sword vs the port, planned (graphics) |
| U-037 | LIKELY | R | soiree pacing in the port |
| U-038 | LIKELY | R | bark queue code |
| U-039 | LIKELY | M | play Samuel's intro and watch which blurbs post (event trace on the conversation component), planned |
| U-046 | OPEN | R | serialization field |
| U-047 | OPEN | R | spawner code |
| U-048 | MEASURED | M **Done** | lean: `CameraCache.POV` while holding Q / E (`measure lean`): 20 degrees (15 + 5 soften), roll 10; fixed |
| U-052 | LIKELY | R | writers of timed state 1 |
| U-054 | LIKELY | R | slide cancel request source |
| U-080 | LIKELY | R | level-load events on return visits (needs travel, code) |
| U-086 | APPROXIMATION | R | `DisBehaviorGoTo` result |
| U-087 | APPROXIMATION | R | vanish rule is a port representation |
| U-161 | LIKELY | M | read the Wwise state of group 0x0956DE29 at Boyle start (AK state query by native call), planned |
| U-163 | APPROXIMATION | R | LPF mapping in code |
| U-141 | SPECULATIVE | M | make a civilian cower (draw the sword near an aristocrat) and read its anim node's sequence name, planned |
| U-222 | LIKELY | M | door events: event trace `DisSeqEvent_Door` activations vs the door's rotation samples, planned |
| U-223 | LIKELY | M | usable stage timing: time `DisSeqAct_ActivateUsable` output against the stage anim, planned |
| U-224 | APPROXIMATION | R | engine LOS check |
| U-143 | LIKELY | R | chain-step picker |
| U-145 | LIKELY | M | ambient idle frequency: sample an idle guard's anim slot over minutes, planned |
| U-147 | OPEN | R | spawner data (no Boyle user) |
| U-121 | LIKELY | M **Done** | light value at the player: `DishonoredVisibilityComponent` +0x68..+0x78 at 14 points; the collection lights at full brightness overshoot 35x (RE of the light environment needed) |
| U-201 | LIKELY | R | attach helper code |
| U-203 | LIKELY | R | soiree camera |
| U-205 | APPROXIMATION | R | port RNG choice |
| U-206 | LIKELY | R | `SeqAct_Interp` +0x1e8 writer |
| U-207 | APPROXIMATION | R | port loop handling |
| U-209 | APPROXIMATION | R | locomotion track decode |
| U-123 | LIKELY | R | bark mapping |
| U-124 | LIKELY | M | NPC attack travel / dodge speed: sample an attacking guard's velocity in `measure sword --defend`, planned (data recorded, not analysed) |
| U-125 | LIKELY | R | alarm callbacks |
| U-100 | LIKELY | M **Done** | Vitality levels: `Pawn.HealthMax` +0x348 after `OnAddPower` 1 then 2 |
| U-101 | LIKELY | M | choke time: hook the choke context end, planned |
| U-102 | MEASURED | M **Done** | grenade fuse: hook on the component tick 0x00C5A2A0: armed at the throw, 3.0 s; fixed |
| U-103 | LIKELY | R | choke availability test |
| U-107 | LIKELY | M | incendiary bolt: hook the NPC damage path after a hit (damage per tick, type), planned |
| U-108 | LIKELY | R | possession scoring code |
| U-109 | MEASURED | M **Done** | Bend Time: `WorldInfo.TimeDilation`, `CustomTimeDilation`, `BendTimeSeconds` (`measure bendtime`): as the port |
| U-111 | LIKELY | R | pickup code |
| U-112 | LIKELY | R | use-target search |
| U-320 | OPEN | M | base-pass override constants via `ps_constants` (graphics agents) |
| U-321 | LIKELY | R | fog CPU side |
| U-322 | OPEN | M | lightmap-only capture vs port (graphics agents) |
| U-300 | OPEN | R | menu code |
| U-301 | LIKELY | R | attacker query |
| U-302 | LIKELY | R | audio environment code |
| U-303 | APPROXIMATION | R | DSP algorithms |
| U-305 | APPROXIMATION | M | volumetric light brightness via capture (graphics agents) |
| U-280 | OPEN | R | presentation features |
| U-281 | APPROXIMATION | R | look-at node |
| U-282 | APPROXIMATION | R | PhysX vs point masses |
| U-340 | OPEN | M | push / pop of engine +0x770: watch it (seen 1 while guards fought the player); the writer needs RE |
| U-341 | LIKELY | M **Done** | `DishonoredEngine.m_fAutosaveWaitTime` +0x7e4 read live |
| U-342 | APPROXIMATION | R | disable-save pushers |
| U-343 | OPEN | R | save format |
| U-344 | OPEN | R | controller save checks |
| U-400 | LIKELY | R | shader maps |
| U-402 | OPEN | R | dominant shadow query |
| U-403 | LIKELY | M | spot DF constants (graphics agents) |
| U-520 | OPEN | R | sky shader terms |
| U-521 | OPEN | R | bloom parts |
| U-522 | OPEN | R | Kuwahara users |
| U-380 | OPEN | R | FaceFX |
| U-242 | MEASURED | M **Done** | drowning: pawn held under the river: 1 per 61 ticks, no regeneration while drowning; fixed |
| U-243 | SPECULATIVE | R | swim notifies (data) |
| U-244 | MEASURED | M **Done** | under-water flag: +0x8A0 bit 11 flips with the camera at the volume top (2 uu steps); as the port |
| U-245 | LIKELY | R | finisher reset |
| U-421 | LIKELY | M **Done** | slot 98: shot dispersion vs the order shot-then-max (`measure gun`) |
| U-422 | OPEN | M **Done** | movement / camera dispersion increase: dispersion while strafing and turning |
| U-440 | OPEN | R | climbing flag writer |
| U-442 | APPROXIMATION | R | PhysX chains |
| U-443 | LIKELY | R | trace result layout |
| U-444 | OPEN | R | input mask bits |
| U-500 | OPEN | R | armour pieces (Boyle Int) |
| U-501 | LIKELY | M **Done** | crossbow reload: hooks on the reload context (`measure gun --weapon crossbow`) |
| U-502 | LIKELY | R | rat target flags |
| U-504 | LIKELY | R | carry state transitions |
| U-540 | APPROXIMATION | R | root motion path |
| U-541 | OPEN | M | boat wake: capture during Samuel's intro, planned |
| U-542 | OPEN | R | soiree camera code |
| U-543 | LIKELY | R | mesh conventions |
| U-544 | APPROXIMATION | R | server-side poses (port rule) |
| U-460 | OPEN | R | DF scale source (code) |
| U-461 | OPEN | M | spot DF constants (graphics agents) |
| U-462 | OPEN | M **Done** | landing speed at `TakeFallingDamage` 0x00B4DCC0 at a fixed step |
| U-480 | OPEN | R | protect-neutrals response |
| U-481 | APPROXIMATION | R | port camera protocol |
| U-482 | OPEN | R | NPC accuracy override |

The priority mechanics (combat, powers, weapons, stealth) were measured even where their rows
were already RESOLVED, as checks of the decoded values; the second table has them too.

## Measurements (2026-10-06, Boyle Ext, `measure ...` on the running original)

Sessions: the first two at the engine's variable step (20-35 fps), the third at a fixed
1/60 s step (`lab fixedstep 60`: `GUseFixedTimeStep` 0x0141B27C, `GFixedDeltaTime`
0x012696B0), where every game-time interval is exact to a tick.

| Quantity | Original | Port | Verdict |
|---|---|---|---|
| Pistol shot offset radius / dispersion (150 shots at d = 15) | mean 0.486, variance 0.0861, KS 0.051 (5 % critical 0.111) | uniform radius: 0.5, 1/12 | **same** (U-247) |
| Pistol shot angle | 35 / 31 / 45 / 39 per quadrant | uniform | same |
| Traced shot direction vs the view, minus the port's angle for the same offset (75 deg on 16:9, 720-line pixels) | mean 0.002 deg, max 0.085 deg (150 shots, 1280x720) | `offset_direction` | **same**: the 720-line unit and the cone |
| Dispersion after a reload | 45 -> 15 at 70.0 /s (0.43-0.48 s at variable step) | 70 /s | same |
| Pistol: shot -> reload start / timed stage / load (60 Hz) | 0.283 / 0.716 / 1.483 s | 0.30 / 0.726 / 1.476 | same within a tick per stage (the shot leaves one tick after the fire action starts) |
| Pistol: earliest next shot after a shot (60 Hz, fire tapped every 2 ticks) | 1.816 s (load + 0.333), dispersion 21.7 | 1.778 s | same within the stage ticks (ReloadOut 0.302 + 2 ticks) |
| Crossbow: shot -> reload start / timed stage / load (60 Hz) | 0.351 / 0.535 / 1.555 s | 0.398 recovery, In 0.173, `m_fReloadTime` 1.0 | **U-501 confirmed**: the timed stage lasts 1.02 s (1.0 + a tick) |
| Slot 98 (U-421) | shots leave at the current dispersion (15.0 for 150 settled shots), then 45 | shot then max | confirmed unused on the fire path |
| Movement / camera dispersion increase (U-422) | strafing and turning (800 rotator units per tick) leave 15.0; sprinting holds 45 | not modelled | **confirmed unused** |
| Blink range, level 1 / 2, level aim | 1100.0 / 1600.0 uu from the eye, horizontally | 1100 / 1600 | same (`m_CurrentLevel` 1 = level 1 = `m_Levels[0]`) |
| Blink range, 20 deg up | 1385.7 horizontal, +500 | `t = min(500 / d.z, 1600)` | same (the camera pitch was 19.84 deg) |
| Blink travel (60 Hz) | 16 steps of 100 uu, one per tick, 0.267 s; ends 5-19 uu short horizontally (< 25) | same | same |
| Blink mana | cost 20; the regen refunds 20 after 3.00 s, one point every 7 ticks at 60 Hz (0.1167 s) | 20; `spend_mana` sets the pool to 20; timer set to 0.1 | **same** |
| Blink flight cylinder | the pawn centre rises 71.0 uu at the start (`CollisionHeight` 87.5 -> 16.5, top kept), then moves straight to the target | the standing centre moves to the target | differs during the flight only: new U-591 |
| Windblast cost | 50 | 50 | same |
| Windblast hit at 182.8 uu, level aim | 26 damage, momentum (2362, -227, 1317) | 26, (2369, -229, 407) | **differs**: z is `(1 - d/r) x ExtraUp x k`; **fixed** |
| Sword hit on a guard | 10 of 50 per hit (`DisDamageType_FastHit_Right`, momentum ~(500, 0, -230)) | 0.2 x 70 = 14 (NPCs have the player's 70 by rule) | same formula (0.2 x the victim's HealthMax); the NPC health is the port's RULE (U-113) |
| Sword press -> damage (60 Hz) | 0.300-0.350 s (target 110 uu ahead) | 0.300 s (Right_A / Left_A damage zone: the last `m_fDamageZoneMaxTime` of the attack zone) | was 0.13 s; **fixed** (U-590) |
| Guard sword damage to the player | `DisDamageType_NPCSlash` 15 (8 on some first hits) | player sword rule | recorded |
| Detection: LOS norm | `d / 3500` in the inner cone, `d / 1700` in the outer one (d from the eye) | same | same |
| Detection: seen value | `DistanceTuning(1 - norm) + 0.1 x light01 + 0.25 x speed/600`, stance bonus 0 when crouched, clamped to [0, 1]; the rate equals it (scalar 1.0); 13 cases within 0.006 | same | **same** |
| Light normalization (light01) | `(raw - 0.2) / 0.3`: the map's `Boyle_Twk.Pvs_BoyleExterior` (darkest 0.2, brightest 0.5) via `DishonoredMapInfo.m_pMapPlayerVisSettings` | global 0.05 / 1.0 | **differs**: **fixed** (AiLevel `player_light_range`) |
| Raw light at the player (U-121), 14 points | 0.168-0.313 | 0.225 everywhere (sky term only) | **differs**: the probe lacks the local lights; U-121 stays open |
| Falling damage (U-462, 60 Hz, landing speed at 0x00B4DCC0) | v 2401 / 2675 / 2775 / 2925 -> 16 / 45 / 56 / 70 | `trunc(70 x (v - 2250) / 650)` | **same**; port at 60 Hz: 18 / 45 / 56 for the same drops |
| Vitality levels (U-100) | level 1: HealthMax 90; level 2: 90 | the current level's list only | **confirmed** (no stacking) |
| `m_fAutosaveWaitTime` (U-341) | 0.0 | 0 | confirmed |
| Engine +0x770 (U-340) | 1 while guards fight the player | "a guard fighting a player" | consistent; the writer is still unknown |
| Run / sprint with the sword drawn (60 Hz) | 360 / 540 uu/s | 0.9 factor | same |
| Jump apex (60 Hz) | 106.67 uu | 106.7 | same |

## Not settled this round

The planned M rows above (18) need their own scenarios; most are presentation or mission
details. The R rows need decompilation first.

## Second round (2026-10-06, Boyle Ext, fixed 1/60 s step)

New probe tools: `census [frames]` (draws and primitives per pixel shader), `skip <0xHASH |
CTAB filter>...` (drop those shaders' draws, rendering only), `fnv <files>` (the probe's shader
hash of `shader_maps` blobs), `measure lean | bendtime | grenade`.

| Quantity | Original | Port before | Verdict |
|---|---|---|---|
| Sword: when the hit can land (U-590) | the last `m_fDamageZoneMaxTime` of the clip's `DishonoredNotify_AttackZone` (0x00A8E760 / 0x00C13140 / 0x00C13640) | the zone start | **fixed**: 0.300 s for Right_A / Left_A at 60 Hz |
| ArkPp LUT bake constants (U-582) | exposure 1.231144, gamma 0.431818 (0.95 / 2.2), contrast 1.081794 | same | **same** |
| UE3 gamma pass (PS 1875, `InverseGamma` 1/2.2) | drawn, but skipping it changes nothing; the uber pass writes the screen | not drawn | same result |
| Moon glow (U-580) | the sky dome's `Storm` texture at texture coordinate 1 (skip / `Storm_Intensity` 0 removes it) | sampled at the lightmap set (0) | **fixed** |
| Lean (U-048) | 20 degrees held (15 + 5 soften at about 5 / s), 119.7 uu sideways, 21.4 down, roll 10; 14.2 degrees at 0.2 s | 15 degrees, no roll | **fixed** (the onset is still slower: 10.7 at 0.2 s) |
| Drowning (U-242) | air 30 s, then 1 point per 61 ticks to death; no regeneration while drowning (regeneration works under water with air) | regeneration held health at 10 | **fixed** |
| Under-water flag (U-244) | set while the camera is below the volume top (-2071.02 set, -2068.27 clear) | eye in the volume | **same** |
| Bend Time level 1 (U-109) | `TimeDilation` 1 -> 0.85 over 0.15 s, back over 0.25 s from 11.76 s, ends at 12.01 s (real); `BendTimeSeconds` at 0.15 x real | same | **same** |
| Grenade fuse (U-102) | armed at the throw, 3.0 s to the explosion whatever it hits | armed by the first impact, 0.5 s (the cooked delay misread as 0) | **fixed** (parser and arming) |
| Door light environments (U-620; synthesized `DirectionalLightComponent` / `SkyLightComponent` of `MyLightEnvironment`) | `DishonoredUsableObject_7`: directional (0.138, 0.126, 0.102), upper sky (0.162, 0.146, 0.103), lower (0.196, 0.170, 0.125); `DisDoor_21` / `_2` / `_1` directional within 10-25 %; the sewer door `DishonoredUsableObject_8` lit bluish from above (0.104, 0.210, 0.329) | `_7`: (0.356, 0.328, 0.264), (0.126, 0.113, 0.077), (0.112, 0.094, 0.064); `_8` (0.060, 0.064, 0.052) from the side; sky lights warmer | **differs** on `_7`, `_8` and the sky terms (reported to the graphics side, not changed here) |
| Light probe with the collection lights (U-121) | 0.17-0.31 | 0.225-1.31 with the 358 collection lights at full brightness | experiment not kept; needs the light environment's static-light rule |
