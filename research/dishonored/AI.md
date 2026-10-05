# NPC AI: Arkane's model and data

How Dishonored's NPC brains are configured, what `dishonored_ai` reproduces, and what is
inferred. Data read with `tweaks` (map_dump, `TWEAK_PKGS=L_Boyle_Ext_Script,...` adds level
packages), `obj_dump`, `enum_dump` (package_dump) and the INIs. Loader:
`crates/dishonored_assets/src/ai_config.rs` -> `dishonored_ir::ai::AiConfig` (cached inside
`cache/gameplay_config.v2.json`). GOG Definitive Edition.

## Where the configuration lives

- Pawn tweak (`DisTweaks_NPCPawn`, cooked into the level script packages, e.g.
  `Twk_Pawn_Boyle_Party.Pwn_EliteGuard_MTall_0_Party`): `m_pBrainTweak`, `m_pVisionTweak`
  (`Twk_Vision_Default`), `m_pVisionTweak_InConversation`, `m_DeathRattleLoudness`
  (`EAINoiseWhisper`), `m_Ammo` (one bullet for elite guards), locomotion config.
- Brain tweak (`DisTweaks_AIBrain`, e.g. `AI_BrainTweaks_Elite.BrainTweaks_Guard_Elite_Party`):
  the behaviour list `m_BehaviorTweak[]` (per difficulty), `m_MinAttentionForAwareness =
  DAL_Investigate`, `m_fReachableMeleeRange/Height = 150`, reaction tweaks
  (`DisTweaks_AIBrain_Reaction`: responses to being hurt, pickpocketing...), steering, and
  brain processes: Attention, BattleSense, RatCrusher.
- Arkane's tweak inheritance: `m_FallbackChainCooked` lists the tweaks a tweak falls back to
  (nearest first, `Pkg.Obj:SubObject` for sub-objects) and `m_FallbackSkip` names properties
  the tweak owns outright. `TweakDb::resolve` now applies class defaults, then the fallback
  chain, drops the skipped properties, then merges the archetype chain and the instance
  (CONFIRMED necessary: e.g. `Twk_Attn_Guard_Suspecting` skips `m_IncreaseForAllyCombatTarget`,
  whose struct default amount is 0, while the fallback has 1.0).

Guard behaviour list (Boyle party elite guard), in tweak order: TriggerAlarm, EscapeExplosion,
Follow (not interruptible), GoHome, EnemyUnreachable, CombatRatSwarm, Combat, BattleVictory,
Search, TakePosition (high), TakePosition, Follow, PatrolSearch, Patrol, Idle, BlockingDialog
(high/low), Interact, RatStomp, Stolen, Soiree (high/low/distractor), Shoot, Guard, Notice,
React, MagicResponse, ReturnTravel. Civilian list (`BrainTweaks_Civilian`): StationarySearch,
Panic_Civilian, EscapeExplosion, Follow, TakePosition, BlockingDialog, Interact, RatStomp,
PatrolSearch, Patrol, Idle, Soiree, Notice, React, MagicResponse. Each behaviour declares a
slot (`EAISlot_Combat`, `_Search`, `_Escape`, `_Notice`, `_React`, `_Alert`, `_Act`).

## Attention (perception)

`DisAIBrainProcessAttention` keeps an attention value per target and selects an attention
profile (`DisTweaks_PawnAttention`) by relationship and suspicion level:
`m_pAttentionTweaks_Enemy_Unsuspecting/_Suspecting`, `_Neutral`, `_Ally`.

| Profile | Leak/s | HeadTrack | TurnToFace | Investigate | Busted (start/stop) |
|---|---|---|---|---|---|
| `Twk_Attn_Guard_Unsuspecting` (guards; civilian `Twk_Attn_Noncombatant_Uns` falls back to it, same values) | 0.025 | 0.125/0.05 | 0.35/0.25 | 0.6/0.3 | 0.85/0.8 |
| `Twk_Attn_Guard_Suspecting` | 0.015 | 0.125/0.05 | 0.25/0.15 | 0.5/0.1 | 0.85/0.8 |
| `Twk_Attn_Default_Neutral` | 2.0 | 0.25/0.2 | 1.0/0.9 | 1.0/0.9 | 1.0/0.9 |

Levels: `EDisAttentionLevel` = Unaware, HeadTrack, TurnToFace, Investigate, Busted
(`enum_dump DishonoredGame.upk Attention`). Increases (`DisAttentionIncrease {m_fByAmount,
m_ToThreshold}`, unsuspecting guard): Danger sound +0.4 up to Investigate, Threatening +0.3
up to TurnToFace, Anomaly +0.2 up to HeadTrack, Help +1.0 up to Investigate, DeathRattle +0.4
up to Investigate, AttackedBy / AttackedEnemy / TalkedTo -> MaxAttention, DiscoveredCorpse,
Alarm, SearchRequest -> Investigate, AllyCombatTarget +1.0 -> Investigate, AllyPanic/Search ->
TurnToFace. The struct default (from the `DisAttentionIncrease` ScriptStruct defaults) is
`m_fByAmount = 0, m_ToThreshold = DALFI_None`. The threshold is a **floor**, not a cap: the
amount is added, then the value is raised to at least the threshold's start (CONFIRMED,
0x00b60ca0, see "Decompiled attention code" below), so a danger sound sends an unaware guard
straight to Investigate (0.6) and an increase with no amount sets the value to its threshold.
Stopping variance 0.025 on every guard threshold (neutral: 0). Busted-capable reasons
(`m_ReasonSettings[i].m_bCanIncreaseToBusted`, same in all three profiles): HeardHelpRequest,
SeeAllyCombat, TalkedTo, Magic, HideOutTransition, AttackedBy, AttackedEnemy, Touched, Seen,
Psychic. Hearing (`DACRT_Heard`), corpses, alarms, death rattles and search/notice requests
cannot raise attention above Investigate.
Seeing: `m_SeeingPawnAttentionIncreaseScalar_ToPlayer` 1.0 (Normal; 1.3 Hard, 1.6 Very
Hard), touching 1.5/s, `m_fDistanceToLoseSight` 300, `m_DistanceValueTuning` points
(0.55 -> 0, 0.875 -> 0.8, 0.93 -> 2.0) applied with `DVCTPAO_Add`.

Vision (`Twk_Vision_Default`, `camera_jnt`): inner cone 45 deg to 3500 uu (vertical scale up
0.25 / down 0.7), outer cone 160 deg to 1700 uu (0.1 / 0.7). In conversation: 35/120 deg,
3000/1500 uu (not used yet).

Noise radii (`DefaultAI.ini [DisAINoiseManager] m_LoudnessDistanceArray`, indexed by
`EAINoiseLoudness`): 2m=200, Whisper=300, 4m=400, Loudness1=500, 6m=600, 8m=800, 10m=1000,
12m=1200, Loudness2=1500, 20m=2000, 25m=2500, Loudness3=3000, 40m=4000, 50m=5000,
Loudness4=6000 (so 1 m = 100 uu). Global reaction delay 0.18-0.30 s
(`DishonoredGlobalAIManager.m_ReactionDelay`).

## Decompiled attention code (Ghidra, `Dishonored.exe` 70da9613…)

Method: `ClassDecomp.java` (main UObject vtable, the largest one the constructor stores),
`FuncDecomp.java` (functions and callees, `str:` / `ref:` / `vt:` seeds, constants listed),
`OffsetScan.java` (functions using given field offsets) and `class_layout` (native field
offsets computed from the cooked reflection data; it reproduces `sizeof` exactly, e.g.
`DisAttentionInfo_Complex` = 0x66c, the size its destructor frees). Offsets below are from
`class_layout`. Nothing decompiled is in the repository.

**Per-target state** (`DisAttentionInfo_Complex`): accumulator `m_AttentionAccumulator`
(+0x8c: value, leak rate, min, max), rolled stop thresholds `m_StopThresholds[4]` (+0x9c),
level (+0xac), min levels (+0xad..), 21 pending amounts `m_PendingAttentionChangeAmount`
(+0xcc, one per `EDisAttentionChangeReasonType`: amount and max threshold) with their
contexts (+0x174), flags (+0x510: HasLOS, IsSighted, IsTouched, IsPsychic, ...),
`m_fLastLOSDistance` (+0x514) and the debug copies of the vision value (+0x518..+0x528).

**Tick** (slot 75, 0x00b69150), once per target per brain tick:
1. 0x00b68e10 copies the profile into `m_PawnAttentionTweaks` (+0x538); the vision scalar is
   `m_SeeingPawnAttentionIncreaseScalar_ToPlayer` (difficulty attribute) for the player,
   `_ToLiveNPC` / `_ToDeadNPC` otherwise; `m_fDistanceToLoseSight` is copied only for the
   enemy relationship.
2. 0x00b4fc40 sets the accumulator: leak `m_fAttentionLeakRate` (or `_InConvo`), minimum =
   start of the highest `m_MinAttentionLevel`, maximum `m_fMaxAttention`.
3. 0x00b4f970 (sight): with line of sight, sighted and `m_fLastLOSDistance` = the vision
   component's normalized distance; without it, a sighted target stays sighted while it is
   within `m_fDistanceToLoseSight` of the NPC horizontally and vertically.
4. 0x00b4fa50 (seen value) when sighted: `x = 1 - m_fLastLOSDistance`; `v =
   DistanceValueTuning(x) (+) target visibility`; for a pawn target `v *= 1 +
   Attribute_Visibility[Not]MovingMultiplier` (moving / still; names at 0x0145eb30/38,
   initialised in 0x009e5670); clamped to [0, 1] when `m_bClampTargetVisibility`; times the
   scalar; times dt into the `DACRT_Seen` pending slot.
5. 0x00b486d0 touching: `m_fTouchingPawnAttentionIncreaseRate * dt` into `DACRT_Touched`;
   0x00b4c400 psychic: `m_fMaxAttention` into `DACRT_Psychic`.
6. 0x00b60ca0 processes every non-empty pending slot in reason order: value += amount, then
   the value is raised to the start of the slot's threshold (`DALFI_MaxAttention` = max);
   then 0x00b5b680 updates the level.
7. Only when nothing was processed: 0x00b61040 leaks (`value -= leak * dt`, clamped to
   [min, max]) and updates the level if the value changed.

**Tuning evaluation** (0x00b0b350, `DisVisCalcTuningParams`): with
`m_bUsePointsInsteadOfFormula` a piecewise-linear curve clamped to the first and last output
(0x00b0b1f0 finds the segment, 0x009ebf70 interpolates), otherwise `(x^m_fExponent +
m_fBias) * m_fScalar`; then `+ operand` (Add) or `* operand` (Multiply). The attention
profiles use points (0.55, 0) (0.875, 0.8) (0.93, 2.0) with Add; the formula fields (3, 0, 4)
are unused.

**Level update** (0x00b5b680 -> 0x00b4c600): rising (new value >= previous) raises the level
past every start crossed; falling lowers it while the value is at or below a stop. Busted
check 0x00b4fce0 when the new level is Busted and no minimum level is Busted already: refused
if the old level was below Busted and the reason's `m_bCanIncreaseToBusted` is false; and for
an unsighted, untouched target, refused when the NPC is within
`m_fMinRangeToUnsightedBustedProxy` (250) of the proxy while the target is farther than that
from it. A refused Busted caps the value at the Busted start and sets Investigate.

**Stop thresholds** (`DisAIBrainProcessAttention` slot 74, 0x00b30270 with 0x00b01570): for
each of the four relationship profiles and each level, `stop + uniform(-variance,
+variance)` using `m_fStoppingVariance`, rolled once per brain.

**Vision component** (`ArkComponentVisionNPC`, `DisCpntType_VisionNPC`, vtable 0x0118bc48;
the controller holds it in `m_pComponentVisionNPC` +0x388):
- 0x00c6c2b0 picks `m_pVisionTweak` (+0x260 of the pawn tweak) or
  `m_pVisionTweak_InConversation` (+0x264) when in a conversation.
- 0x00c67410 caches two cones when the tweak changes: outer (`m_fVision_OuterMaxRange`,
  `OuterFOV * pi/180`, outer vertical scales) and inner; 0x00c66830 precomputes the half
  angle `fov/2`, its cos/sin, `cos(fov)`, and per vertical direction the angle `scale *
  half`, its sine and `cos(2 * angle)` (plus the rear set for fields of view over 180 deg).
- 0x00bec910 cone test of an eye-space unit direction (x forward): for half angles up to 90
  deg the target needs `x >= cos(half)` and `2x^2 - 1 >= (c2v + c * (cos(fov) - c2v) +
  cos(fov)) / 2`, where `c = cos(2 * atan2(sin(half) * z, sin(v) * y))` and `v` is the up or
  down vertical half angle by the sign of z: an elliptical cone whose limit goes from the
  horizontal half angle (c = 1) to the vertical one (c = -1).
- 0x00bf1580 normalized distance: transform the target location to eye space; inside the
  inner range and cone -> `d / inner_range` (inner flag); else inside the outer range and
  cone -> `d / outer_range`; else not seen.
- 0x00c754c0 gathers candidates with that test, 0x00c788e0 is a time-sliced scheduler that
  traces line of sight to each candidate's test points (0x00c67520: trace from the eye,
  optionally with a 5 uu extent) and notifies the target. `Twk_DefaultVisRegion` gives the
  player's test regions (three settings: 30-150, 50-90 and 30-60 uu above the feet).

**Target visibility** (`Attribute_Visibility`, name at 0x0145e1c0): the target record
(0x00c86bf0) caches the pawn's `Attribute_Visibility`. For the player,
`DishonoredVisibilityComponent` (0x00ad51e0 per tick, 0x00ad1590) sets two modifiers:
`SpeedTuning(speed, LightTuning(light))` (the light term is the speed tuning's Add operand;
the speed is forced to 0 while sneaking with `m_bForceZeroSpeedWhileSneaking`) and the stance
bonus (`m_fVisLevelBonus_*`: auto-crouching, sneaking, sliding, standing, walking, sprinting,
swimming, swimming on surface). `Twk_PlayerVisibility`: light `0.1 * light01`, speed `0.25 *
speed`, bonuses all 0 except auto-crouching and swimming -1; light normalized between
`Twk_GlobalPlayerVisSettings` darkest 0.05 and brightest 1.0. NPC pawns keep the base 0.
`Modifier_ForcePlayerVisible` adds a fixed 1.0.

**Consequences** (guard, unsuspecting, Normal): straight ahead, the curve is zero beyond 45 %
of the cone range (1575 uu in the inner cone, 765 uu in the outer one), so far targets are
seen only through their visibility: a lit, standing player adds 0.1/s (Busted from zero in
8.5 s, if the NPC is an enemy), a running one up to 0.35/s; in darkness and crouched, 0.
Close: 600 uu ahead gives 0.69/s plus visibility; within about 405 uu in the inner cone the
curve alone reaches 1 (clamped). Auto-crouching (-1) hides a still player beyond about 405 uu.

## Patrols, combat and panic (decompiled)

- **Patrol** (`DisBehaviorPatrol`, layout: `m_pCurrentRoute` +0xf8, `m_CurrentIndex` +0xfc,
  `m_RouteDirection` +0x108, `m_pCurrentGuardPoint` +0x110, `m_fGuardTimer` +0x114): a guard
  walks a `DishonoredRoute` (engine `Route`: `RouteList` of `DishonoredNavPoint`s,
  `ERouteType` Linear / PingPong / Circle, default Circle; `m_Capacity`, default -1).
  Adoption (`DisPatrolManager`, 0x00c41590): over the linked list of routes
  (`m_pNextRoute`), skip the current one and those the pawn may not adopt (squad /
  capacity check 0x00a4bc70), take the nearest by closest point (routes flagged by
  0x00a43860 first), increment `m_NumNPCAdopters`, start at the nearest index. At a point,
  0x00af3870 sets the guard timer from `m_fGuardDuration`; 0x00a4bab0 treats a point as a
  stop if it guards forever, has a duration or has watch points. Boyle: 8 routes in
  `L_Boyle_Ext_Script`, 4 in `L_Boyle_Ext_P`, 36 in `L_BoyleStreet_Ext_Script`, 55 in
  `L_Boyle_Int_Script`, mostly single-point posts with `m_Capacity` 1 and a squad.
- **Combat loss**: combat is a behaviour on Busted attention; it ends (`CombatToSearch`)
  when the target falls below Busted: unseen it leaks at the profile rate (suspecting
  0.015/s: about 12-15 s from max to the 0.8 stop) or, sooner, when the guard reaches the
  stale proxy (the Busted check above).
- **Attack pattern** (`DisTweaks_AIAttackPattern`): a list of item-context overrides
  (enabled, cooldown range). `TwkAttackPattern_BerserkerElite` (level-cooked, falls back to
  `RangedElite` -> `Ranged`): Short 1-3 s, StepBack 0.5-2, Medium 3-5, Long 15-20, Jump
  15-20, BackStep/SideStep/FireGun 0, Taunt disabled; ideal range from the Short attack;
  runner; flank 15 deg. `DisGlobalCombatManager` keeps attack histories per class and
  inhibits attack classes per target (`m_fInhibitionTime*`, DefaultAI.ini: Short 2, Medium
  1, StepBack 1, Long 0.8, Jump 3, SideStep 6, BackStep 8).
- **Panic** (`DisAIBrainProcessPanic`, stimulus table 0x00b695d0): handlers per stimulus:
  Alarm (rule HearNoise with context 6), AllyBusted (0x00b61430: ally's threat is an enemy ->
  `m_WitnessAllyCombat` if it can see it; otherwise `m_WitnessAllyPanic`), AttackedByEnemy,
  DiscoveredCorpse (`m_WitnessCorpse`), EnemyBusted (`m_SeeEnemy`), HeardSomething (scary
  noise context), Help (scary noise 5), IncomingDamage (`m_AttackedMelee/Ranged`),
  WitnessDeath. Group stimuli travel `DishonoredGlobalAIManager.m_fGroupDistance` = 1000 uu
  (DefaultAI.ini); the rules themselves have no range or sight condition (WitnessAllyPanic
  needs no line of sight).

### Implementation (`dishonored_ai`)

- One attention entry per stimulus source; anonymous stimuli (noises, corpses, help barks) use
  one "place" entry whose proxy is the stimulus position.
- Relationships follow the factions section below (`Social`, `Brain::relation`): each tick a
  brain's enemies are its individual enemies, trespassers against its faction and its
  faction's enemies (disposition overrides applied). Neutral and allied pawns are only
  head-tracked. Seen violence (sword attack, firing) by a player, being hurt by a player, a
  witnessed kill, or an ally guard fighting someone makes that pawn an individual enemy.
- Vision, rate, increases, leak, level update, stop rolls and the Busted check follow the
  decoded code above (`Cone`, `vision_distance`, `seen_rate`, `player_visibility`,
  `level_after`, `Attn::change`). The eye is 70 uu above the pawn centre, oriented by the
  pawn's yaw; line of sight is traced to three points (60, 0, -50 uu from the centre).
- Light: `light_at` over the level's lights (1 when a level has none); speed value: horizontal
  speed / `Attribute_GroundSpeedSprint`, unclamped. The moving multipliers are 0.
- Combat ends when the target's attention leaves Busted; unseen targets are chased at the
  proxy. Melee uses the BerserkerElite Short/StepBack/Medium contexts in pattern order with
  their cooldowns and the per-target inhibition (all three are one sword swing on the shared
  pawn).
- Guards adopt the level's routes (adoption check and neglect priority below),
  wait `m_fGuardDuration` at points, stay forever on single-point routes and
  `m_bGuardForever` points; without routes the previous loop of nearby nav points is used.
- A civilian entering Panic, or a guard entering Combat, emits an AllyBusted group stimulus;
  civilians within 1000 uu panic (from a guard only if they see it).

## Factions (decoded 2026-10-06)

Data: `DisTweaks_Faction` (`m_AlliedFactions`, `m_EnemyFactions`), cooked in
`DisFaction_Defaults` and in level packages (`Boyle_Factions` in `L_Boyle_Ext_Script`). A pawn's
faction is its spawner's `m_pFactionTweakOverride`, else its pawn tweak's `m_pFactionTweak`
(`DisSpawnerInfo`); the player's is `Twk_Pawn_Corvo_Release.m_pFactionTweak` =
`Faction_Corvo_Default`. Boyle: the party guards use `Neutral_Guard` (allies: guards,
civilians, `Neutral_Civilian`; enemies: weepers, slackjaws, thugs, assassins - not Corvo), the
guests `Neutral_Civilian` or `Faction_Civilian_Default` (no lists), the street guards
`Neutral_Guard_Street` (enemies: weepers, **Corvo**); spawners without override (alarm squads,
pursuers, `Pwn_EliteGuard_*`) resolve to `Faction_Guard_Default` (enemy of Corvo). The masked
player is neutral to the party and an enemy of the squads that come for him.

Relationship of a pawn toward a target (`DisGlobalFactionManager`, cached):
- 0x00c62f20 -> 0x00b043d0: the pawn's own individual relationships first (the brain's
  `hostile` list: reaction responses such as `m_Response_HurtByPlayerNeutral`, witnessed
  violence, scripted dispositions), then its faction's toward the target (0x00c63070):
- 0x00c5fc20: a target registered as a **trespasser** of a faction the pawn's faction is ->
  Enemy. `DisForbiddenZone` (0x00c953f0 on touch, 0x00c95660 on untouch): a pawn whose faction
  is one of `m_ForbiddenFactions` inside the zone trespasses against every `m_OwningFactions`.
  Boyle Int: zones 1 and 2 (upstairs; owner `Neutral_Guard`; forbidden Corvo and
  `Neutral_Civilian`); zone 0 is disabled until Kismet enables it. The brush hulls are the
  `BrushAggGeom` convex elements' `FacePlaneData` (serialized W, X, Y, Z).
- 0x00c5fca0: `DisSeqAct_SetDisposition` overrides (per recipient pawn, then per recipient
  faction).
- 0x00c5fd00 -> 0x00c4ceb0: faction lists. Same faction (0x00c6f360: equal or along the tweak
  fallback chain) -> Ally; else `a`'s enemies, then `a`'s allies; else Neutral. Only `a`'s
  lists count (not symmetric).
- `DisSeqAct_SetDisposition` -> `DisTweaks_Faction::OnSetDisposition` (0x00c79000): inputs Set /
  Clear / ClearAll; recipients are factions (0x00c638e0) or pawns (0x00c636d0), reciprocated
  with `m_bReciprocate` (a pawn recipient then sets its own relationship to the faction,
  0x00b04430). A pawn target uses `ADishonoredPawn::OnSetDisposition`.
- NPC friendly fire changes no relationship: only the reaction responses (players) do.

## Alarm bells (decoded 2026-10-06)

`DisAlarmBell` with `DisTweaks_AlarmBell` (`SecuritySystems_twk.Alarm.Alarm_Loudness4_twk`:
`EAINoiseLoudness4` = 6000 uu, go-to closeness 5000, use closeness 150, ring delay 2.5 s,
spawner min distance 1500, 2 spawns per ring (Boyle Int's bell overrides 6), 30 s between
uses). Boyle: one bell on Int (`DisAlarmBell_0`, 4 triggered spawners), two on Ext
(`L_BoyleStreet_Ext_Script`).
- `DisBehaviorTriggerAlarm` (first in the guard behaviour list, slot Alert): chance by what made
  the guard aware (`TweaksBehavior_TriggerAlarm_Party`: sighting alone / in group, found corpse,
  witnessed death 1.0, help 0); "alone" = no other pawn within `m_fCompanionRange` 2000 that is
  an ally (`m_bCompanionMustBeAlly`) (0x00af5fe0); an enemy within `m_fFightRadius` 500 is fought
  instead (0x00af6bd0). The bell keeps the pending ringer nearest to it (0x00a1e140). The guard
  runs to the bell, waits `m_fWaitTimeBeforeRing` 0.25 s within use closeness and rings.
- Ring (0x00a37410): ringing, allowed spawns = max per ring, AI-noise timer = ring delay. After
  it (0x00a36dd0) an AI noise of the bell's loudness with context `AlarmBell` (6) goes out (into
  the bell's listener volumes when it has them) and the triggered spawners spawn - those farther
  than `m_fSpawnerMinDistFromPlayer` from the enemy, else all - aware of the ringer's enemy. The
  bell rings while an NPC still hunts that enemy (tick 0x00a3d0c0), then stops;
  `DisAlarmGlobalManager.m_fAlarmCooldown` 10 s.
- `m_SpawnAtSuspicionLevel` is the suspicion the spawned pawn starts with (passed to the brain
  on spawn, 0x00a590e0; PostLoad clamps it to `DAISL_Suspecting`), not a spawn trigger.

## Spawn awareness (U-055)

`m_bAwareOfPlayerUponStartup` (spawner flags bit 9, post-spawn 0x00a590e0) sends the brain
`EAIStimID_MaxOutAttention` (61) for the player (0x00b23be0, 0x00b19ee0). The attention
process' handler (0x00b3f6a0) adds a `DACRT_Psychic` change to `DALFI_MaxAttention` with the
proxy at the player's location. U-055 was a misdiagnosis: the harness' stationary agent was
killed by the pursuers, whose target was then gone.

## Perception details (decoded 2026-10-06)

- Light (`DishonoredVisibilityComponent` tick 0x00ad51e0): the raw light is the pawn's light
  environment (0x004e1db0: static + dynamic SH coefficients summed, reduced to a brightness,
  times a runtime scale `DAT_01435230`, raised to 1/2.2 (constant 0.454545) above 0.0001, else
  0; -1 without an environment, clamped to 0), normalized between `Twk_GlobalPlayerVisSettings`
  darkest / brightest (0x00bc9040). Port (`light_at`): the level's light actors
  (`AiLevel::lights`, static ones included) summed as `Brightness x luminance x
  (1-(d/r)^2)^FalloffExponent` (spot cones, shadow traces for shadow casters, directional lights
  traced along their direction), then `^(1/2.2)`; server and client compute it from the same
  cached data (U-121).
- Speed (0x00ac33b0): `|Velocity.xy| / Attribute_GroundSpeedSprint` (600), not clamped.
- Target visibility: `Attribute_Visibility` with `Modifier_VisComp_Base` (light and speed),
  `Modifier_StealthBonus` (stance) and `Modifier_ForcePlayerVisible` (+1) (FName globals
  0x0145e1c0 / 2a8 / 2a0 / 270); NPCs keep the base.
- Hearing (0x00c5a760, 0x00bb50d0): radius = `m_LoudnessDistanceArray[loudness] x (1 -
  attenuation)`; a listener hears when strictly inside it and, if the listener or the noise is
  outside the audio cells, when a trace (flags 0x2286) from the listener to the noise is clear;
  between two cells the noise follows the cell graph (not modelled, U-120).

## Barks (decoded 2026-10-06)

The behaviours fire hooks through `FUN_00af00f0(hook, initiator)`. Callers: Notice begin / end
(108 / 109); search start `SEARCH_NEAR/FAR_VDETECTION` (21/22, near within
`m_fVisualDetectionRatioNear` 0.3 of the range), `SEARCH_ADETECTION` / `_DANGER` (24/25),
`SEARCH_DETECT_CORPSE` / `SEARCH_WITNESS_DEATH` (27/28), from an ally (23 / 26 / 33); search end
`MUSTHAVEBEENARAT` (29), `GIVEUP_POSTCORPSE` (30), `SEARCH_STOPHUNTING` / `_GROUP` (35/36);
`SEARCH_FIND_ENEMY` (34); combat barks sub-process (0x00b41f70) `COMBAT_THREAT_OFFSCREEN` (42),
`COMBAT_THREAT` / `_GROUP` (40/41), `COMBAT_HELP` (47); `COMBAT_LOST` / `_GROUP` (49/50,
0x00bbb0f0); `COMBAT_RANGED_BEGIN` (64), `COMBAT_GOING_HOME` (65), `ALARM_GO_TO_RING_ALARM` (66),
`ALARM_HEAR_AND_RESPOND` (68), `PANIC_START` (83), `FLEEING` (85), `COMBAT_BEGGING` (51),
`PATROL_ATTACKED_BY_ENEMY` (16); melee `m_AttackBark` (`DDH_COMBAT_ATTACK`). `dishonored_ai` emits
`AiEvent::Bark` at the matching transitions (U-123); the mission host queues them on
`MissionState::ai_hooks`, which calls `Runner::fire_hook` on the trees of the NPC's one-shots
(refire delays and fire groups apply there; generic per-voice trees: U-122).

## Routes (decoded 2026-10-06)

Adoption check 0x00a4bc70 (`DishonoredRoute`): `m_bIsActive`, route points present, the pawn's
squad in `m_SupportedSquads` (any when empty), `m_NumNPCAdopters < m_Capacity`, the pawn within
`m_AdoptionRange` (-1 = any). 0x00c41590 prefers the nearest **neglected** route
(`m_NeglectTimer > m_NeglectThreshold` 30 s, 0x00a43860); a pawn that has a route only switches
to a neglected one.

## Combat details (data)

Item-context class defaults (`DisTweaks_NPCAttack*`): Short max range 250, StepBack 100, Medium
350, Long 450, jump attack 550-800 (`DDH_COMBAT_BIG_ATTACK`), allowed angle 45 deg, height
difference 200. Elite guards: `m_GunUsageStart` 0-0.5 s, `m_GunUsageCooldown` 1-2 s
(`TweaksBehavior_CombatEliteGuard`); the port waits the start delay before the single pistol
shot. Long / jump attacks need NPC animations (U-124).

## Behaviours

| Original | `BrainState` | Data used |
|---|---|---|
| Idle / Soiree / Patrol / Guard (routine) | Idle, Wander, WalkToPoint, GroupIdle, Patrol, Alert | `DishonoredRoute`s; `m_fGuardDuration` waits |
| Notice (TurnToFace) | Notice | stop and face the proxy |
| React / reaction delay | React | `m_ReactionDelay` 0.18-0.30 s before every escalation |
| Search: Investigate sub-state | Investigate | run for Danger/Help/Alarm (`m_SearchFromNoiseTweaks[4..7].m_bShouldRun`), proximity 250, wait 0.5-1 s on arrival (`m_fCancelInvestigateWaitTime*`) |
| Search crumbs | Search | `DishonoredSearchCrumbsComponent`: 25 crumbs, heartbeat 0.8 s, wander 500-1000 uu within +-60 deg; ends when attention drops below the Investigate stop |
| StationarySearch (civilians) | Investigate (in place) | 2-3 s |
| Combat (elite guard) | Combat | skirmish caps (Normal: 2 melee per target, 3 total, `DisGlobalCombatManager`), melee range 150, attack-pattern cooldowns and inhibition, outer ring uses the single bullet after 0.75 s aiming (`StateNPCMasterAim`) |
| Panic (civilians) | Flee, Cower | flee to `DisFleePointActor`s outside the forbidden cone (50 deg within 500 uu), 2 s free flight, cower within 350 uu, help bark at talking loudness (EAINoiseLoudness2 = 1500 uu) |

Civilian panic triggers (`TweaksBehavior_Panic_Civilian`): SeeEnemy, WitnessDeath,
WitnessCorpse, AttackedMelee/Ranged, WitnessAllyCombat, WitnessAllyPanic, WithinDangerZone,
and HearNoise with the party's scary noises **AlarmBell, Help, DeathRattle** (the default
panic tweak also lists Danger and Threatening; the party civilians do not panic at gunshots
or sword clashes by ear, they turn and look). Decoded from the raw `m_ScaryNoises` arrays
(`L_BoyleStreet_Ext_Script` export 8079; `Default__DisTweaks_AIBehavior_Panic`).

## States exposed for conversations

- `Brain::suspicion_level()`: `EDisAISuspicionLevel` (DisGlobalEnums): 0 `DAISL_Unsuspecting`,
  1 `DAISL_Suspecting`; set after an investigation, a corpse or combat, it selects the
  suspecting attention profile (`m_pAttentionTweaks_Enemy_Suspecting`).
- `Brain::conv_combat_status()`: `eDisConvCombatStatus` (`DisConv_SpeakerCombatStatus`, used
  by the `m_bNotWhileInCombat/Suspicious/Fearful` speaker rules): 0 InCombat, 1 Suspicious,
  2 Fearful, 3 Other. The mapping from behaviours (Combat; Notice/Investigate/Search; Panic;
  rest) is LIKELY: the native accessor was not traced.


## Inferred or missing (see UNKNOWNS U-120..U-125)

- Hearing through audio cells (U-120); the light scale and the SH light environment (U-121).
- Barks on the NPCs' generic per-voice trees (U-122); the mapping of bark points onto this
  port's simpler states (U-123).
- Long / jump attacks, dodges, the pistol substates (U-124); alarm decision details (U-125).
- Which test points the vision scheduler traces for NPC targets; distractions, watch points.
