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
  cached data (scale: "Light scale" below).
- Speed (0x00ac33b0): `|Velocity.xy| / Attribute_GroundSpeedSprint` (600), not clamped.
- Target visibility: `Attribute_Visibility` with `Modifier_VisComp_Base` (light and speed),
  `Modifier_StealthBonus` (stance) and `Modifier_ForcePlayerVisible` (+1) (FName globals
  0x0145e1c0 / 2a8 / 2a0 / 270); NPCs keep the base.
- Hearing (0x00c5a760, 0x00bb50d0): radius = `m_LoudnessDistanceArray[loudness] x (1 -
  attenuation)`; a listener hears when strictly inside it and, if the listener or the noise is
  outside the audio cells, when a trace (flags 0x2286) from the listener to the noise is clear;
  between two cells the noise follows the precomputed portal paths ("Hearing through audio
  cells" below).

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
(`TweaksBehavior_CombatEliteGuard`).

## Combat moves (decoded 2026-10-06)

The elite guard's real ranges are its sword's pawn-specific contexts
(`Twk_Inv_EliteGuard.Twk_Inv_SwordEliteGuard.DisTweaks_InventoryItem_PawnSpecific_1.*`):

| Context | min-max (uu) | height | pattern cooldown | inhibition |
|---|---|---|---|---|
| `NPCAttackShort` | 50-235 | 300 | 1-3 s | 2 |
| `NPCAttackStepBack` | 0-170 | 100 | 0.5-2 s | 1 |
| `NPCAttackMedium` | 235-375 | 300 | 3-5 s | 1 |
| `NPCAttackLong` | 375-570 | 200 | 15-20 s | 0.8 |
| `NPCJumpAttack` | 730-850 | 100 | 15-20 s | 3 |
| `NPCBackStep` | to 500, 300 uu step | - | 0 | 8 |
| `NPCSideStep` | to 400, 250 uu step | - | 0 | 6 |

The pistol (`Twk_Inv_PistolEliteGuard...DisTweaks_NPCFireGun_1`): 250-3000 uu.
`TweaksBehavior_CombatEliteGuard.DisTweaks_AISubProcess_ManageAttacks_0`: 2-3 attacks per
flurry, stamina recovery 4-5 s, side step after a ranged threat aimed within 10 deg for 2-3 s
(melee threat angle 30), back step after 1 incoming attack (forgiven after 5 s), primary usage
weight 1, secondary (left arm, ranged aim) 3, block breaker `DisItemContext_NPCBash`.
`DisTweaks_AISubState_FirePistol_1`: first shot 0.75-1.25 s, give up 0.3-1 s, point blank 80,
chance of firing again 1.0.

Code:
- `DisAISubProcessManageAttacks` refresh (slot 78, 0x00b47880): when
  `m_iFlurryAttacksRemaining` (+0x88) is below 1 the process only waits for
  `m_fStaminaRecoveryTimer` (+0x84) to go below zero, then rolls a new flurry and timer;
  otherwise it picks the block breaker (flag bit 0), else bit 1's attack (0x00b3deb0), else the
  primary or the secondary attack line first by the usage weights (`rand(0, primary +
  secondary) > primary` tries the secondary first). A chosen attack whose tweak drains stamina
  (`m_bDrainsNPCStamina`, true for the NPC melee attacks) decrements the flurry. Activation
  (slot 75, 0x00b3dc10) rolls the timers.
- `DisBehaviorCombatEliteGuard` refresh (slot 96, 0x00b0e220): `m_fGunTimer` (+0x130) runs
  down; with the target reachable for melee, `m_bWillingToStartShooting` = clear line, timer
  below zero, the NPC's velocity below 0.0001 on every axis, distance in (min, max) of the gun
  context (+0x144 / +0x148 virtuals); `m_bWillingToContinueShooting` = clear line and distance
  below max. With the target unreachable (0x00ae50f0 false) both are clear line and distance
  below max, no timer. Activation (slot 88, 0x00b0e130) sets the timer from `m_GunUsageStart`;
  leaving the pistol (slot 122, 0x00b10f60) from `m_GunUsageCooldown`.
- `DisAISubStateFirePistol` enter (slot 95, 0x00b32c50): give-up timer from
  `m_TimeBeforeGivingUp`, the `DDH_COMBAT_RANGED_BEGIN` bark (64), state Relaxed, fire timer
  from `m_TimeToFirstShot`; the four pistol states (Relaxed, Aiming, Recovering, Reloading;
  0x00b37c10 / 0x00b46bf0 / 0x00b37e20 / 0x00b37ea0) were not decompiled.

Port (`dishonored_ai::combat`, `Brain::combat`): contexts in pattern order, first one off its
cooldown, not inhibited on the target and with the target in its range and height; the move
kind goes out in `PlayerInput::npc_move` (`dishonored_sim::input::npc_moves`) and
`Brain::attack_kind()`. Attacks beyond the sword's reach (200 uu) travel to the target at
sprint speed for at most 1 s and swing on arrival (the root motion of the long and jump
attacks; LIKELY), the jump attack also jumps; the step-back attack swings while backing off.
Back step after the incoming-attack count, side step after the ranged-threat time, each with
its group inhibition; dodges cover their distance at 600 uu/s. The flurry / stamina pacing
follows 0x00b47880; the outer skirmish ring uses the pistol rules above (the "+0xb0 bit 1"
gate of the reachable case is read as "not a melee attacker", LIKELY).

## Hearing through audio cells (decoded 2026-10-06)

Data (level audio package, e.g. `L_Boyle_Int_Audio`): `DishonoredAudioVolume` (convex brush,
`m_AudioPropagationIndex`, `m_LinkedPortals`), `DishonoredAudioPortal` (`m_WidthDir` +0x248,
`m_HeightDir` +0x254, `m_PlaneNormal` +0x260, `m_Corners` +0x26c, `m_fHalfWidth` +0x29c,
`m_fHalfHeight` +0x2a0, `m_pCellA/B` +0x2b0/+0x2b4, `m_fOcclusion_HeardByAI` +0x2bc,
propagation index +0x2c4) and the WorldInfo's `DisAudioPropagationInfo` (Boyle Int: 41 cells,
55 portals, 2704 paths; `m_fDistCoeff` 0.1827, `m_fMaxAudibleDistance` 12000).

`m_PathData` layout (0x00b92320 with the binary search 0x00b86d50): `m_NumPaths` sorted u32 keys,
then as many u32 values, then the records. Key bytes (little endian): portal B index, cell B
index, portal A index, cell A index, cell A being the one with the lower propagation index.
Value: low byte = number of intermediate portals, upper 24 bits = byte offset of the record
from the start of the data. Record: u16 length (times `m_fDistCoeff`), then one byte per
intermediate portal (index into `m_Portals`). A path from a portal to itself is length 0.

Hearing test 0x00bb50d0 (listener, its cell, noise, its cell, radius): outside the radius ->
not heard; either end outside the cells -> trace (flags 0x2286); same cell -> heard; else
0x00ba3700 tries every (listener cell portal, noise cell portal) pair: path lookup with the
radius as limit; an end lying past its first/last portal on the portal's rectangle skips it
(0x00b86e20); quick estimate (path length + each end's distance to its portal's closest
point, 0x00b8be30) below the radius; then 0x00b9d5e0 string-pulls the path (points start at
the midpoints of the closest portal points to both ends, up to 10 passes moving each point to
where the neighbours' segment crosses its portal rectangle, 0x00b92670, until the squared
movement is under max(400, n x d^2 x 0.0016)), and in query mode the noise is heard when
`length / prod(1 - m_fOcclusion_HeardByAI)` (at least the straight distance) is below the
radius. Port: `dishonored_ai::hearing`, data `AiLevel::audio` (`ai_level::read_audio_cells`).
LIKELY details: the off-plane case of 0x00b92670 (clamped end point instead of the exact
rectangle-edge closest point) and an additional point test (0x006b0330) in 0x00b86e20.

## Light scale (decoded 2026-10-06, U-121)

`DAT_01435230` is written once by a static initializer (0x00f67460): `2 * sqrt(pi)` (FSQRT of
the double pi at 0x00fbeeb8, doubled). 0x004e1db0 multiplies the band-0 SH coefficient of the
light environment by it, reduced to luminance with `FLinearColor::GetLuminance` weights 0.3,
0.59, 0.11 (0x004da330) or the max channel. A light projected as `intensity x Y00(dir)`
(Y00 = 1 / (2 sqrt(pi))) therefore contributes exactly its intensity, which is what the
port's probe sums; the probe now uses the 0.3/0.59/0.11 luminance (it used Rec. 709) and adds
the level's enabled sky lights as half of their upper plus lower brightness (LIKELY
hemisphere normalization). Remaining gap: lights inside `StaticLightCollectionActor`s are not
in the probe (their per-component transforms are native data).

## Sleep darts (decoded 2026-10-06, U-105)

The damage code 0x00b83680: a damage type with `m_bPutToSleep` (bit 11 of +0x58) and
`m_bInstantSleepForUnawareNPCs` (bit 13) skips `m_fTimeBeforeSleep` when the pawn's virtual
0x6c0 (`DishonoredNPCPawn`, 0x00b6e340) is false. That virtual returns brain awareness
(0x00b0c1e0) == `EAIAwareness_InCombat` (5); 0x00b0c1e0 returns `Surprised` (2) during a
reaction, `Choked` (7) for one controller class, else the current behaviour's awareness.
Port: `Brain::unaware_of` / `Crowd::unaware_of` (true unless in Combat or ringing an alarm).

## Rat swarms (2026-10-06)

`DisRatSwarm` + `DisTweaks_RatSwarm` (`Twk_RatSwarm.Twk_RandomRatSwarm`: aggressive from 11
rats, attack group 12, detection 500 (150 busy), escape 750, damage 0.4 per rat clamped 1-5
every 0.5 s after 2 s, eat with 5 rats; `Twk_DevouringSwarm_Lvl1` (power level 1: 30 rats,
30 s, 1500 uu): group 10, detection 700 (200 busy), damage 2-5). Port: `dishonored_ai::rats`
(one brain per `PawnKind::Rat` pawn, its swarm from `SimEvent::RatSpawned`; it runs at its target and holds `PRIMARY` in bite reach, the sim's attack groups deal the damage; meals as `AiEvent::RatEat`).

### The swarm tick (2026-10-06, U-260, U-261)

Found through the state table rather than the vtable (the class's own vtable dump shows engine
slots only). CONFIRMED from the code unless marked.

- **State machine** (0x00C65380): a static table at 0x01471B30 (filled at 0x00F8F12A) holds, per
  `EDisRatSwarmState`, four functions: condition, enter, exit, update. Each tick the swarm takes
  the first state, in enum order, whose condition returns true, else `DRSS_None` (5). On a change
  it runs the old state's exit and the new state's enter, then the current state's update.
  - Hidden: condition from the table's first entry (not decoded), enter 0x00C76CD0, exit 0x00C6DBB0.
  - Flee: condition 0x00C72900, true when the swarm's `m_fDuration` has run out, its fleeing flag
    is set, or it has too few rats for its counts (any, when it spawned fewer than
    `m_InitialRatCountToBeAggressive`; else `m_MinRatCountPerAttackGroup`), none hidden and no
    `m_bNeverFleeSystemicaly` (the count's meaning, 0x00C6CB80, LIKELY); update 0x00C7A880.
  - Attack: condition 0x00C87EA0, update 0x00C89EE0 (below).
  - Move: condition 0x00C654C0 (a custom behaviour with `DRCB_ReachTarget` and a target); update
    0x00C771B0.
  - Idle: condition 0x00C6DC60 (any living rats, unless a custom behaviour forbids it); update
    0x00C76DF0 (idle attraction, roaming, moving away from a lingering pawn after
    `m_fIdleAwayFromPawnTriggerDuration`, `m_IdleAwayFromPawnMinCount..MaxCount` times).
- **Spawn** (0x00C7C360): state None, `m_FramesUntilDetection = m_PawnDetectionCheckFrameInterval +
  rand() x m_PawnDetectionCheckFrameRandomness / 32768`, the idle-away count rolled in
  `[Min, Max]`.
- **Attack condition** (0x00C87EA0): a forced custom-behaviour target gets a group first; every
  detected target that is valid (0x00C879E0: players only after `m_fPlayerDetectionDelayAfterSpawn`),
  not yet a group's target and reachable gets a new group (0x00C7A390). Then each chasing,
  attacking or eating group is dropped (0x00C76810) when its target is invalid, unreachable
  (`m_fCannotReachDuration` >= `m_fPawnUnreachableDelay` or flagged, with a cannot-reach ratio >=
  0.3), the group has fewer rats than it needs, or a chased target is beyond `m_fPawnEscapeRadius`.
  An attacking group whose target became a corpse turns to eating (0x00C72810(4)) when it has
  `m_EatRequiredRatCount` rats and the corpse is not consumed, else it is dropped. True while a
  group remains.
- **Attack update** (0x00C89EE0), per group: the destination circles the target
  (`m_fAroundTargetRotationRadius / Speed`, `m_fFromTargetMoveFactorSpeed`); each rat moves to
  it, and rats at the target join the group's in-reach list (attacking, or eating for a corpse).
  No rat in reach: state Chasing, the damage interval counter restarts. A living target: state
  Attacking, `m_fAttackDuration += dt`, then the bite rule (0x00C891E0). A corpse: state Eating;
  coming from Attacking, `m_fEatStageDuration -= m_fEatStartupDelayAfterKill`; then the eat loop.
- **Eat loop** (0x00C6D760): runs only with `m_EatRequiredRatCount` rats in reach;
  `m_fEatStageDuration += dt`. A dead target (health < 1, 0x009E9FE0): until its mesh is the eaten
  one (rat-target slot 18), at `m_fEatStartupDuration - m_fEatFXToMeshChangeDelay` the
  `m_pToEatenMeshFX` plays at each limb and at `m_fEatStartupDuration` the mesh swaps (slot 19) and
  the clock restarts; afterwards every `m_fEatPerLimbDuration` the next limb (slot 20, 0x00B79830:
  the NPC's `DisSeveredLimbInfo.m_SeveredLimbInfo`, from a random index in circular order, skipping
  severed ones, the `m_ChestBoneBreakName` limb last) is severed as eaten and the clock restarts;
  when none is left the next interval plays `m_pConsumeEatenMeshFX` and sets
  `eDisEatenState_CorpseConsumed` (slot 22). A target that is not dead is consumed after
  `GetLimbCount (slot 21: the limb info's count, 1 without) x m_fEatPerLimbDuration` (U-502).
  `SLInfo_Default` and `SLInfo_Elite` list 6 limbs, so a corpse takes `startup + 7 x per limb`.
- Port: `dishonored_sim::rats::{CORPSE_LIMBS, eat_duration, eat_clock_start}`; the Devouring
  Swarm aggregate (`powers::step_swarms`) follows the Attack condition / update order;
  `dishonored_ai::rats` uses the same eating time. Open: rat lights, the guards'
  `CombatRatSwarm`, the Hidden condition and the Flee / Move updates in detail.

## Generic dialog voices (2026-10-06, U-122)

`DisTweaks_NPCPawn.m_GenericDialogData` (+0x124) lists the pawn's generic
`DisDialogVoiceData` (each with `m_pDialogTree` and `m_VoiceData[].m_pTree`); `AiLevel::generic_trees`
records them per pawn tweak. Which entry a pawn uses and the runtime search are not decoded.

## Go-to completion: the locomotion request (2026-10-06, U-221)

How `DisBehaviorGoTo_Base` (vtable 0x01129808, `DisBehaviorGoTo` 0x01129e98) and the native
locomotion component (`DishonoredNPCPawn.m_pCpntLocomotion`; type name `ArkCpntType_Locomotion`
from 0x0093dd50; constructor 0x00945ec0, vtable 0x010a55a0) decide that a pawn has arrived.
CONFIRMED from the code unless marked.

- **Issuing the move** (0x00ae4de0): the behaviour clears reached / finished, sets stage 0, then:
  - for a destination actor (`m_pDestinationActor`, +0xa8), it calls MoveToActor (0x00cba540)
    with end threshold -1, funnel 1.0, **accurate stop 1**, speed-from-look-at 0;
  - for a location, it calls MoveToLocation (0x00cba670) with the behaviour's
    `m_bAccurateStop` (bit 2 of +0xa4).
  - From Kismet (0x00af04e0 / 0x00af0750), `DisSeqAct_AIGoToActor.m_bTrackMovingTarget`
    (default true) keeps the actor as the destination. Otherwise the actor's location is used,
    and `m_bAccurateStop` / `m_bExactRotation` come from the go-to tweak's flags (+0xa8 bits 1 /
    2).
  - So `AIGoToActor_0` (Lord Shaw to his mark, no overrides) asks for an accurate stop.
- **The request** (`DisLocoRequest`, filled by 0x00ca7ea0): +0x44 `m_MaxSpeedIndex`, +0x48
  `m_fEndLocationThreshold`, +0x4c funnel multiplier, +0x50 bits (accurate stop, speed from
  look-at, follow), +0x54 / +0x58 follow angle / distance.
- **Submission** (0x00ca9d80): the request goes into a component record (0x00945010 for an
  actor, 0x009450c0 for a location). The record carries the type (0 actor, 1 location), the end
  threshold (0x0093f160, only set when >= 0, clamped to a minimum), the funnel, and the accurate
  flag at record +0x24.
- **Destination** (path-found callback 0x00949890): the component's destination (+0xc4..+0xcc)
  is the **last point of the found path**, not the actor's position.
- **Arrival test** (0x00944390): squared XY distance from the pawn (+0x134) to that destination.
  - If the request is **not** accurate and the current speed mode has stop animations
    (component +0x250 bit 2, set by 0x00941b70 when the mode's `m_StopAnimProps` is not empty), the pawn
    arrives inside `m_fUnaccuracyRadius` (config +0x9c; class default 120, kept by
    `EliteGuardLocomotionConfig`). This holds while the locomotion state (+0x218) is 0 or 5 and
    the speed is below `m_fVelocityToStopMoving` (20).
  - Otherwise it arrives only within 0.1 uu (squared 0.01 at 0x010a5590).
  - While moving inside the radius, with stop animations available and the heading within 0x1000
    (22.5 degrees), state 10 plays the stop animation (0x00947300).
- **Arrival event**: the component reports locomotion event 0x18. The behaviour's handler
  (0x00ae5a00 -> 0x00ae4ea0) sets `m_bHasReachedDestination` when the event names it and
  carries success. The stage step (0x00aedc00) then turns to the destination rotation
  (0x00ae8590, face-to at -100 = default speed, stage 1), or finishes (stage 2 -> 3,
  finished + success). Event 0x45 is the path request result (0x00ae8600: failure reports
  Failed). Event 0x55 is the face-to done (stage 2).
- **Return to the navmesh**: when the pawn is off the mesh as the path starts, and
  `m_bDisableTeleportOnNavmesh` is off (+0x966 bit 0, copied each tick by 0x00b7cd10 into
  component +0x1ac bit 0x40), the component takes the nearest navmesh point (raised by the
  collision height) as a return target. It moves there over `distance /
  m_fReturnToNavMeshSpeed` (0x0093ec80). Branch in 0x00949890, state 6.
- **NPC navmesh walking** (`DishonoredGameInfo.m_bNewNPCPhysWalking`, +0x3d4 bit 4, default
  true). The NPC pawn's physics (0x00b4e9f0) runs the locomotion component's move (0x00943d00)
  instead of the default one (0x006bd150). This happens when the pawn has a component with an
  active request, its vfunc +0x4a0 returns 0 (not traced), and the flag is set. 0x00943d00 then calls
  0x00940d80 when:
  - the pawn was not just teleported;
  - component +0x1ac bit 0 is set (following a path: 0x0093ecf0 from the path callbacks; also
    0x0093e060 / 0x0093e070 around the anim-driven sub-steps of 0x0062fed0);
  - its `Base`'s collision component blocks rigid bodies;
  - the pawn has a navmesh anchor poly.

  0x00940d80 raises the cylinder by `MaxStepHeight` and adds `Velocity x dt`, then checks the
  end point on the navmesh by walking polys from the anchor (0x0068f660). It takes the height
  from the poly's plane, with the cylinder bottom on it, and sets `Location` directly.
  `Velocity` becomes the achieved move. World geometry is not swept, so lips and kerbs below the
  navmesh never block an NPC. When the end point is off the mesh, the component's own walking
  physics runs instead (0x0093fb00).
  - Port: `movement::nav_walk_move` for pawns flagged `nav_walk`. The mission host sets the
    flag each tick for NPCs that move along a held navmesh path.
  - APPROXIMATION: polys are found by position within the step height, not by walking edges;
    the actor box check of 0x0064a6b0 is not made; the port uses the shared pawn cylinder.
- **Consequence for the duel (U-221)**: the elite's go-to (`AIGoToActor_0`, target
  `Shaw_Elite`) is an accurate stop at the end of his path. With navmesh walking, the 4.4 uu lip
  that stopped him 84 uu short no longer blocks him. The port's go-to still completes at 60 uu of
  the goal.
  - Correction (2026-10-06): the pawn stopped short was Lord Shaw, not the elite. A Kismet
    publish bug had written Lord Shaw into `Shaw_Elite` and `Shaw_Elite_Pistols` (KISMET.md
    "Publishing array properties"). With that fixed, each go-to moves its own pawn. Shaw reaches
    his mark and the countdown runs.
  - **Next lead.** After `One`, Shaw (`PawnKind::Civilian` in the port) panics and flees.
    `Twk_Pawn_LordShaw` falls back to `Pwn_EliteGuard_MTall_2` / `Pwn_EliteGuard_Base`. Its
    `m_ContentInventoryLoadout` is `Twk_Inv_SwordEliteGuard` + `Boyle_Twk.Twk_Inv_PistolShaw`, so
    in the original he is an armed elite. The port classes him by tweak name
    (`npcs::role_from_tweak`: "party" -> Civilian).
  - Resolved (2026-10-06, U-221): see "Pawn roles from the tweak data" and "Scripted shoot"
    below.
- **Accurate stop in the port** (2026-10-06): a scripted go-to (`WalkToPoint` with `stay`)
  arrives within 0.1 uu of the end of its held navmesh path (`Brain::accurate_stop` /
  `arrived`). The mission host passes that end to the pawn (`PlayerState::nav_end`, host-only,
  not serialised); the walking move does not pass it: the step that would reach it lands on it
  and the velocity drops to zero. Port fallbacks: a pawn held 60 uu from its goal by collision
  for a second also arrives; walks without a navmesh path and the routine walks (wander) keep
  the 60 uu radius. Before: every go-to completed within 60 uu of the target actor.

## Pawn roles from the tweak data (2026-10-06)

The port's `NpcRole` (Guard / Civilian / Other) decides the brain kind (`PawnKind::Guard` fights,
`Civilian` panics). It was guessed from the tweak's name. It now comes from the resolved pawn
tweak (`npcs::role_from_tweak_data`, cache `npcs7`):
- **Guard**: `m_FallbackChainCooked` (or the tweak's own `m_TweakNameCooked`) reaches
  `Pwn_Guard_Base` or `Pwn_Overseer_Base`, or `m_ContentInventoryLoadout` holds a
  `DisTweaks_WepSword` / `DisTweaks_WepPistol` outside the weeper family.
- **Civilian**: the chain reaches `Pwn_Civ_Base` and not `Pwn_Weeper_Base`.
- **Other**: weepers, tallboys (bow `DisTweaks_WepTallboyBow`, chain `Twk_Pawn_DefaultNPC` only).

The brain tweaks agree: `Twk_Pawn_LordShaw` has `BrainTweaks_Guard_Elite_Party` (the elite
behaviour list, with Shoot and Combat, no Panic) although its `m_pFactionTweak` is
`Faction_Civilian_Default` (a `m_FallbackSkip` property). Boyle changes: Lord Shaw Civilian ->
Guard; `Twk_Pawn_Outsider`, `Twk_Pawn_Boatman_StuckInBoat`, `Pwn_MiddleClass_MTall_1` Other ->
Civilian (no behaviour change: only Guard is read). Factions still come from the tweak.

## Scripted shoot (`DisSeqAct_AIShoot`, `DisBehaviorShoot`) (2026-10-06)

- `OnAIShoot` (0x00b63d00): unless the pawn refuses Kismet (vfunc +0x498), Start replaces a
  running shoot (the old op reports Interrupted, 0x00ba1fc0 with 3), then posts stimulus 0x5c
  (0x00b18870) with the shot target (a pawn's controller when the target is a pawn), "forever"
  = `m_iMaxNumShots < 1`, the count, the accuracy override bit and percentage. Abort posts
  stimulus 9 and ends the behaviour. The target is not made an enemy.
- `DisBehaviorShoot` (class record 0x01324fa8, slot `EAISlot_Comply`, tweak
  `AI_BehaviorTweaks_Shoot` in the elite brain lists) takes the stimulus (slot 110, 0x00afae20:
  target +0xa8, forever bit 2, count +0xac, override bit 3, percentage +0xb0), stands and asks
  to face the target (Stand sub-state); once turned (slot 107, 0x00afaf10) it enters
  `DisAISubStateFirePistol` with a Kismet parameter block (0x00b379e0). Done (slot 106,
  0x00ae7770) when the sub-state exits.
- `DisAISubStateFirePistol` states (slot 77, 0x00b47720): Relaxed 0x00b37c10, Aiming 0x00b46bf0,
  Recovering 0x00b37e20, Reloading 0x00b37ea0. Flags +0xd4: bit 0 fired, bit 1 stop, bit 2 exit,
  bit 3 keep firing, bit 4 forever, bit 6 Kismet mode; +0xd8 state, +0xdc / +0xd0 timers, +0xf4
  shots left. In Kismet mode Relaxed waits `m_TimeToFirstShot` (+0xdc, rolled on enter), then
  Aiming fires once aimed; each shot decrements the count unless forever (stop at < 1);
  Recovering reloads when empty, else back to Relaxed with the weapon's refire delay (vfunc
  +0x15c of the NPC fire item, not decoded).
- `AI_BehaviorTweaks_Shoot.DisTweaks_AISubState_FirePistol_1` sets nothing: `m_TimeToFirstShot`,
  `m_TimeBeforeGivingUp`, `m_fChanceOfFiringAgain` are 0.
- Port: `Brain::order_shoot` / `ShootOrder` and `Brain::kismet_shoot`: stand, face the target,
  pistol out, fire after `aim_time_before_shooting` (0.75 s, `StateNPCMasterAim`) with a round
  chambered, count down unless forever; the order ends with the target or its last shot.
  Not modelled: the accuracy override (NPC shot accuracy is the shared pistol dispersion), the
  refire delay beyond the pistol's own recovery, the turn speed (NPCs turn at once).

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

- Static-collection lights in the probe (U-121).
- Barks on the NPCs' generic per-voice trees: which voice and the tree search (U-122); the
  mapping of bark points onto this port's simpler states (U-123).
- The pistol sub-state internals and the root motion of the attacks (U-124); alarm decision
  details (U-125); the rat swarm tick (U-260).
- Which test points the vision scheduler traces for NPC targets; distractions, watch points.
