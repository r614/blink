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
`m_fByAmount = 0, m_ToThreshold = DALFI_None`; an increase with no amount therefore raises
attention straight to the threshold (LIKELY; otherwise AttackedBy would do nothing).
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

### Implementation (`dishonored_ai`)

- One attention entry per stimulus source; anonymous stimuli (noises, corpses, help barks) use
  one "place" entry whose proxy is the stimulus position.
- Players start **neutral** (the Boyle party guests are neutral; `Boyle_Factions`), so guards
  only head-track them. Seen violence (sword attack, firing), being attacked, a witnessed
  kill, or an ally guard fighting them makes a player an enemy at full attention.
- Per tick: sounds and barks apply their increases; humans in a view cone with line of sight
  add `rate * dt`; untouched entries leak; levels follow start/stop hysteresis.
- Vision rate (SPECULATIVE combination of data): `scalar * (cone + curve(1 - d/inner_range))`
  with `cone = 1 - d/inner_range` in the inner cone and `0.5 * (1 - d/outer_range)` in the
  outer cone. An enemy straight ahead is Busted after about 0.6 s at 600 uu, 2 s at 2000 uu,
  6 s at 3000 uu; 1000 uu off to the side (outer cone only) takes about 1.4 s.

## Behaviours

| Original | `BrainState` | Data used |
|---|---|---|
| Idle / Soiree / Patrol / Guard (routine) | Idle, Wander, WalkToPoint, GroupIdle, Patrol, Alert | patrol points without `m_fGuardDuration` (default 0) are passed without stopping |
| Notice (TurnToFace) | Notice | stop and face the proxy |
| React / reaction delay | React | `m_ReactionDelay` 0.18-0.30 s before every escalation |
| Search: Investigate sub-state | Investigate | run for Danger/Help/Alarm (`m_SearchFromNoiseTweaks[4..7].m_bShouldRun`), proximity 250, wait 0.5-1 s on arrival (`m_fCancelInvestigateWaitTime*`) |
| Search crumbs | Search | `DishonoredSearchCrumbsComponent`: 25 crumbs, heartbeat 0.8 s, wander 500-1000 uu within +-60 deg; ends when attention drops below the Investigate stop |
| StationarySearch (civilians) | Investigate (in place) | 2-3 s |
| Combat (elite guard) | Combat | skirmish caps (Normal: 2 melee per target, 3 total, `DisGlobalCombatManager`), melee range 150, outer ring uses the single bullet after 0.75 s aiming (`StateNPCMasterAim`) |
| Panic (civilians) | Flee, Cower | flee to `DisFleePointActor`s outside the forbidden cone (50 deg within 500 uu), 2 s free flight, cower within 350 uu, help bark at talking loudness (EAINoiseLoudness2 = 1500 uu) |

Civilian panic triggers (`TweaksBehavior_Panic_Civilian`): SeeEnemy, WitnessDeath,
WitnessCorpse, AttackedMelee/Ranged, WitnessAllyCombat, WitnessAllyPanic, WithinDangerZone,
and HearNoise with the party's scary noises **AlarmBell, Help, DeathRattle** (the default
panic tweak also lists Danger and Threatening; the party civilians do not panic at gunshots
or sword clashes by ear, they turn and look). Decoded from the raw `m_ScaryNoises` arrays
(`L_BoyleStreet_Ext_Script` export 8079; `Default__DisTweaks_AIBehavior_Panic`).

## Inferred or missing (see UNKNOWNS U-041..U-046)

- Vision rate formula, light and stance visibility (`Attribute_Visibility` exists on Corvo,
  base 0, range -1..1; how it enters the rate is unknown).
- Patrol route topology (routes start at a Kismet-assigned `DishonoredNavPoint`; the next
  point selection is native).
- Attack patterns (`TwkAttackPattern_BerserkerElite/RangedElite`) are referenced but not
  decoded; melee timing is the previous heuristic.
- Alarm bells (`DisBehaviorTriggerAlarm`), conversations, distractions, watch points and
  head-track barks are not implemented.
- Combat loss of target after 5 s unseen is SPECULATIVE.
- Panic contagion range (1000 uu) is SPECULATIVE.
