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
- Players start **neutral** (the Boyle party guests are neutral; `Boyle_Factions`), so guards
  only head-track them. Seen violence (sword attack, firing), being attacked, a witnessed
  kill, or an ally guard fighting them makes a player an enemy at full attention.
- Vision, rate, increases, leak, level update, stop rolls and the Busted check follow the
  decoded code above (`Cone`, `vision_distance`, `seen_rate`, `player_visibility`,
  `level_after`, `Attn::change`). The eye is 70 uu above the pawn centre, oriented by the
  pawn's yaw; line of sight is traced to three points (60, 0, -50 uu from the centre).
- Light: the simulation has no light probe; `LIGHT_LEVEL` = 1 (lit) is SPECULATIVE. Speed
  value: horizontal speed / 600 uu/s (SPECULATIVE normalisation). The moving multipliers
  are 0.
- Combat ends when the target's attention leaves Busted; unseen targets are chased at the
  proxy. Melee uses the BerserkerElite Short/StepBack/Medium contexts in pattern order with
  their cooldowns and the per-target inhibition (all three are one sword swing on the shared
  pawn).
- Guards adopt the level's routes (nearest with free capacity; squads are not modelled),
  wait `m_fGuardDuration` at points, stay forever on single-point routes and
  `m_bGuardForever` points; without routes the previous loop of nearby nav points is used.
- A civilian entering Panic, or a guard entering Combat, emits an AllyBusted group stimulus;
  civilians within 1000 uu panic (from a guard only if they see it).

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

## Inferred or missing (see UNKNOWNS U-041..U-046)

- Light at the player (no light probe) and the speed normalisation of the visibility.
- Which test points the vision scheduler traces for NPC targets, and the target-side
  virtual that returns the cached visibility (LIKELY the `Attribute_Visibility` copy).
- Route squads (`m_SupportedSquads` / spawner squads) and the "neglected route" priority
  (0x00a43860, `m_NeglectTimer` / `m_NeglectThreshold` 30 s).
- Melee context ranges (per-weapon item-context tweaks) and attack selection among
  contexts beyond pattern order; the long/jump attacks and dodges.
- Hearing: radii are `m_LoudnessDistanceArray`; whether propagation is straight-line or
  path-based was not traced.
- Alarm bells (`DisBehaviorTriggerAlarm`), conversations, distractions, watch points and
  head-track barks are not implemented.
