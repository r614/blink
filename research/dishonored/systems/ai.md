# AI behaviour: the core model, checked against the port

Scope: the NPC brain (`DishonoredAIBrain`, its behaviours, processes and stimuli), patrol, investigate,
search, combat, ranged and special NPCs, civilians, bodies and magic. Perception (vision cones, light,
attention accumulation) belongs to the stealth system doc; where AI consumes it, only the interface is
noted. Detailed decodes that already exist stay in `research/dishonored/AI.md` (attention code, combat
moves, rats, go-to completion, alarm bells, hearing through audio cells); this file adds the brain's
decision machinery, the missing rules and the port comparison.

Sources: `oracle index` (decompiled `Dishonored.exe` 70da9613, UnrealScript index), the cooked tweak
data (`tweaks`, level packages `L_Boyle_Ext_Script` and `L_Boyle_Int_Script` loaded), `enum_dump`
(`DishonoredGame.upk`), `DefaultAI.ini`, the port source, and the port runner
(`oracle scenario`, results of 2026-10-08). Nothing decompiled is reproduced here. Evidence grades:
CONFIRMED (read from code or data), LIKELY (consistent reading, not traced), UNVERIFIED (needs the
original running).

## 1. The brain: stimuli, slots, behaviours

### 1.1 Rule: behaviours are chosen by stimulus, and priority is the slot number (CONFIRMED)

`DishonoredAIBrain` (size 0x1f4, vtable 0x01133ce0) owns:
- `m_BehaviorArray` (+0x178): the behaviours built from the brain tweak's `m_BehaviorTweak[]`, in tweak
  order (the difficulty column picks the Easy variant of combat only);
- `m_ActiveBehaviorStack[19]` (+0x100): one behaviour per `EAISlot`; `m_pCurrentBehavior` (+0x14c);
- `m_StimQueue` (+0x164): stimuli with a delay in seconds (`DisStimRef` = stimulus + float);
- `m_BrainProcesses` (+0x150): the processes (below).

Per brain tick (0x00b17100 -> queue drain 0x00b16eb0 -> dispatch 0x00b11960):
1. Every queued stimulus whose delay is `<= 0` leaves the queue and is dispatched; the others wait. The
   delay is how a reaction delay is applied: a stimulus is queued with `m_ReactionDelay`
   (`DefaultAI.ini`: 0.18-0.30 s), `m_MagicReactionDelay` (0.20-0.30 s), `m_TetherDelay` (6-8 s),
   `m_BullyCooldownPeriod` (3-8 s).
2. A brain whose sense flags (+0x88) exclude the stimulus drops it (0x00b03f10: the deaf / blind /
   numb switches of `DisSeqAct_AISetSenses`).
3. Dispatch order: the already-active behaviours are offered the stimulus from the highest slot down
   (filter mask `m_pFilterStimMask`, the sub-process filter, the "complete" mask, and
   `m_pShouldFinishWhileDormantStimMask` for the paused ones). If the top one filters it, dispatch
   ends. Otherwise every behaviour in `m_BehaviorArray` order whose **evaluate mask**
   (`m_pEvaluateStimMask`) contains the stimulus is asked to evaluate it; for each slot only the first
   behaviour in tweak order that accepts wins. The winner replaces whatever occupies its slot (that
   one is aborted) and starts; if the brain's current behaviour sits in a **lower** slot than the new
   one, the current one is dropped and the new one becomes current.
4. After the drain (0x00b17100), behaviours whose complete flag is set (or that report finished) are
   removed from their slot, and if no current behaviour remains **the highest occupied slot becomes
   current**. A behaviour in a lower slot therefore stays alive but dormant while a higher one runs and
   resumes when the higher one finishes; this is how patrol resumes after a notice or a search.

So the "priority" of a behaviour is exactly its `EAISlot`, and the state machine of the NPC is the
slot stack, not a single enum.

`EAISlot` (enum_dump, ascending priority): 0 Idle, 1 Fulfill, 2 Act, 3 Notice, 4 Perform, 5 Search,
6 Distraction, 7 React, 8 Engage, 9 Evade, 10 Resign, 11 Combat, 12 Adapt, 13 Alert, 14 Awe,
15 Escape, 16 Preserve, 17 Comply, 18 Inhibited.

### 1.2 Behaviour list of the Boyle elite guard (`BrainTweaks_Guard_Elite_Party`), slot and activation

Slot from each behaviour tweak's `m_DesignatedSlot` (a blank is the enum default, Idle). Activation is
the evaluate mask of the behaviour class (read from the class's mask getter; one stimulus each unless
listed):

| behaviour (tweak order) | slot | evaluates | notes |
|---|---|---|---|
| TriggerAlarm | 13 Alert | EnemyBusted, ForceRingAlarm, Help, WitnessDeath | chance tweak, see AI.md "Alarm bells" |
| EscapeExplosion | 16 Preserve | Explosion, ImpendingExplosion, WitnessMagic | ported (1.4c) |
| Follow (not interruptible) | 17 Comply | | |
| GoHome | 10 Resign | TetherRequest | |
| EnemyUnreachable | 12 Adapt | ReachabilityChange | menace / stare |
| CombatRatSwarm(EliteGuard) | 8 Engage | | rats |
| **Combat** (CombatEliteGuard) | 11 Combat | CombatBegin | |
| BattleVictory | 2 Act | BattleVictory | |
| **Search** | 5 Search | SearchBegin | |
| TakePosition (high / default) | 17 Comply / 1 Fulfill | | scripted go-to |
| PatrolSearch | 0 Idle | PatrolSearchRequest | |
| **Patrol** | 0 Idle | PatrolRequest | |
| **Idle** | 0 Idle | BrainInit, IdleRequest | |
| BlockingDialog / Soiree / Interact | 4, 2, 6 | BlockingDialogRequest, Soiree, ... | |
| RatStomp, Stolen | 7 React | | |
| Shoot (Kismet) | 17 Comply | ShootRequest | |
| Guard | 0 Idle | GuardRequest | |
| **Notice** | 3 Notice | NoticeBegin | |
| React | 7 React | ReactionRequest | |
| MagicResponse | 14 Awe | WitnessMagic | |
| ReturnTravel | 0 Idle | ReturnTravelRequest | |

Civilian list (`BrainTweaks_Civilian`): StationarySearch (5 Search, SearchBegin), **Panic** (15 Escape,
Panicked), EscapeExplosion, Follow, TakePosition, BlockingDialog, Interact, RatStomp, PatrolSearch,
Patrol, Idle, Soiree, Notice (3), React (7), MagicResponse (14). City guard (`BrainTweaks_Guard`) has
the same layout with `CombatCityGuard`, a Panic behaviour (`TweaksBehavior_Panic_CityGuard`, Escape),
EscapePlague (9 Evade) and aggressive patrol / idle / guard variants. Overseers use
`BrainTweaks_OverSeerMusical`: `DisBehaviorMusicalCombat` in slot Combat, a musical patrol, no pistol
elite behaviours.

Consequences (all CONFIRMED from the slot numbers):
- Routine behaviours (Patrol, PatrolSearch, Idle, Guard, ReturnTravel) share slot 0; a request for one
  replaces the other. They are triggered by explicit requests (spawner flags, Kismet, the end of a
  search), never by attention.
- Notice (3) < Search (5) < React (7) < Combat (11) < Alert (13, the alarm) < Awe (14, magic) < Escape
  (15, panic) < Comply (17, scripted orders). A guard in combat is not interrupted by a new Search or
  Notice stimulus (they take their slots but stay dormant); the alarm run (13), a magic response (14)
  and any scripted order (17) do interrupt combat.
- Combat ending posts `CombatToSearch` / `CombatEnd`; the Search behaviour (already dormant in slot 5)
  or the paused routine behaviour then resumes.

### 1.3 Brain processes (`m_BrainProcessTweaks`)

Guards: Attention (`DisAIBrainProcessAttention`, drives the stimuli via the attention levels),
BattleSense (victory barks `m_VictoryBarkDelay` 2.5-3 s) and RatCrusher. Civilians add Panic
(`DisAIBrainProcessPanic`: stimulus table of AI.md "Patrols, combat and panic"). Wolfhounds have a
Wolfhound process and sub-process (count); `DisAIBrainProcessHideoutWatcher` handles the player's
hideout transitions. Behaviour sub-processes (per behaviour tweak): StandardBodyIntention, AmbientAnims,
AmbientBarks, WatchPoints, PersonalSpace, Distractions, FleetingGaze, HeadTrackBark / Pause,
ManageAttacks (combat), CombatBarks.

### 1.4a The attention poster (CONFIRMED, port pass 2026-10-08)

Traced from the three constructors of the attention-behaviour stimulus class (0x00b17a80 with id
0x3f / 0x57 / 0x11 through 0x00b19890 / 0x00b19ba0 / 0x00b19cf0) to their only caller 0x00b24d00:

- The attention component keeps a behaviour level (+0x44) and the relationship of its top target
  (+0x45, `ERelationship`: 0 Neutral, 1 Ally, 2 Enemy). 0x00b25890 steps +0x44 one level at a time
  toward the target's attention level. Each step up posts the begin of the new level (0x00b24d00:
  1 HeadTrackBegin 44, 2 NoticeBegin 63, 3 SearchBegin 87, 4 CombatBegin 17); each step down posts
  the end of the level left (0x00b22ec0: HeadTrackEnd 45, NoticeEnd 64, SearchEnd 88, CombatEnd 18).
  Both only when +0x45 == 2 (an enemy top target); the begins of levels 2-4 also need the target
  pawn not dead (its vslot 13).
- A new top target (0x00b25e50) first steps the level down to 0 (the old target's ends), then
  stores the target and steps up to its level.
- These stimuli have no delay: the stimulus class's delay getter (vtable +8) is 0x008fc830
  (`return 0`). The queue (0x00b148c0) asks every queued stimulus whether the new one replaces it
  or is dropped (vslot 3 handler), then stores it with `delay = GetDelay(brain)`.
- Delays exist per stimulus class: 0x00b01650 (`m_ReactionDelay`, 0x00c477f0) for SearchRequest
  (90) and Help (48) (vtables 0x01134100 / 0x01134118); 0x00b016e0 (`m_MagicReactionDelay`,
  0x00c47840) for WitnessMagic (112, vtable 0x01133ab8); a stored delay for Stolen (97). So a guard
  that hears or sees something starts its Notice / Search / Combat on the next brain tick; the
  0.18-0.30 s reaction delay is an ally request's (SearchRequest) and a help call's.
- `DisAISubStateInvestigate.m_fReactionDelayTimer` (+0x110, counted down by 0x00b2a000) delays the
  investigate dialog hook (0x00bd0470), not the movement. `TweaksBehavior_Search_Default
  .m_SearchReactionDelay` (0-1 s) exists; its reader was not found.

So the "React" state of the old port (a reaction delay before every escalation) has no original
counterpart; the escalation rule `Brain::select` assumed is replaced by this poster.

### 1.4b The attention process's stimulus handlers (CONFIRMED statically, 2026-10-09)

`DisAIBrainProcessAttention` (vtable 0x0113cb80) slot 81 (+0x144, 0x00b47d40) returns the handler
for a stimulus id (the byte at +4 of a `DisStim`; the dispatcher 0x00b3b2f0 calls it when the
process's mask byte for the id has bit 0). Each handler is a one-call thunk to the method below.
Stimulus templates store their id in their constructor (`*(byte *)(this + 4) = id`), which gives
the posters (`scripts/audit/stimuli.py`). An attention increase is built as
{reason (`EDisAttentionChangeReasonType`, 0x00b00c00), amount / level (0x00b01550, the level byte
`EDisAttentionLevelForIncrease`), location} and applied to the target's attention entry
(0x00b3b460 finds or adds it, 0x00b482e0 -> entry vslot 79) unless the brain's sense flags for
that kind of target (+0x84 player / +0x88 other, bit 0) block it (0x00b01020).

| stimulus (`EAIStimID`) | method | effect | port |
|---|---|---|---|
| 2 Alarm | 0x00b40f60 | the alarm's increase at its actor | ported (`reason::ALARM`) |
| 14 ClearAllMinAttention, 15 ClearAttention, 16 ClearMinAttentionForTarget | 0x00b30ce0, 0x00b30ae0, 0x00b36620 | min attention / entries cleared | no |
| 22 DialogAttentionChange | 0x00b40050 | TalkedTo (12) at the speaker, the level the stimulus carries | no |
| 23 DeathByWoL | 0x00b41090 | at the victim's location | no |
| 26 DiscoveredCorpse, 111 WitnessDeath | 0x00b40740 (111 also 0x00b16ad0) | the corpse reaction | ported |
| 31 DoorUsedByPlayerWhileWary | 0x00b40150 | Seen (19), level Investigate, at the local player | no (door use by a wary NPC's view) |
| 35 EscapedBeingChoked, 36 EscapedBeingPossessed | 0x00b3fc20 | AttackedBy (16), level Busted, at the stimulus pawn | 35 ported (a lost choke, 0x00A74720 also makes the choker an enemy, 0x00B04480); 36 is posted only for plot NPCs (`DisNPCPlotSettings.m_GameOverWhenPossessedByPlayer`, 0x00A745A0) |
| 37 EvadedMelee_Incoming | 0x00b3f8c0 | AttackedBy (16) at the attacker when the relationship test 0x00b30310 passes | no |
| 38 EvadedMelee_Outgoing, 62 MeleeConnected_Outgoing | 0x00b40990 | the fight's target bookkeeping | no |
| 41 ForceRingAlarm | 0x00b3faf0 | sets +0x40; reason 13 (`DACRT_Magic` by the enum), level Investigate, at the stimulus point or pawn | no |
| 46 HeardSomething | 0x00b40b70 | hearing (section 10) | ported |
| 47 HeardCorpseSplat | 0x00b40240 | Heard (7), level Investigate, at the corpse; only for the local player's body drops | no |
| 48 Help | 0x00b40640 | help request | ported |
| 52 IncomingDamage | 0x00b3f530 | AttackedBy (16) at the attacker, or AttackedBy_Indirect (15) at the stimulus point when its flag (+0x20 bit 0) is set; sense bits 0 and 3 block it | AttackedBy from `SimEvent::Damage` only |
| 61 MaxOutAttention | 0x00b3f6a0 | Psychic (20), level Max, at the target | ported |
| 65 NoticeRequest | 0x00b3f7f0 | HeardNoticeRequest (4) from an ally (relationship 1) | no |
| 74 PlayerHideoutTransition | 0x00b467a0 | the hideout min attention | partial |
| 78 / 79 / 80 PsychicAttention* | 0x00b3f780, 0x00b365e0, 0x00b30b70 | psychic attention on / off | ported |
| 81 ReachabilityChange | 0x00b464e0 | | no |
| 90 SearchRequest | 0x00b40ab0 | ally search request | ported |
| 91 SetMinAttentionForTarget | 0x00b3f7b0 | min attention (type, level) | no |
| 98 SuspicionLevelChanged | 0x00b29140 | +0xb8 bit 1 | no |
| 99 TargetSighted, 101 TargetUnsighted | 0x00b3f3f0, 0x00b365b0 | | structural |
| 100 TargetTouched | 0x00b3f420 | Touched (18) when the stimulus flag is set | ported (touching rate) |
| 110 Traveled | 0x00b40370 | Psychic (20) at the local player's +0xAE0 | no |
| 114 WitnessShadowKill | 0x00b3f9e0 | SeeAllyCombat (10) at the point when the relationship test passes | no |
| 116 DLC06_SmokeBombed, 117 DLC06_WitnessSilentExplosion | 0x00b3fd10, 0x00b3fe40 | AttackedBy_Indirect (15), level Investigate (117: the point pulled toward the NPC to the blast radius) | no (DLC) |

### 1.4c EscapeExplosion and the systemic flee (CONFIRMED statically, ported 2026-10-09)

`DisBehaviorEscapeExplosion` (vtable 0x011274d0, slot Preserve; tweak
`DisTweaks_AIBehavior_EscapeExplosion`, shipped `TweakBehaviors_EscapeExplosion_Default`:
clearance 600, discovery angle 45, end timer 5, threat radius 300, threat timer 2, 1 after the
explosion, surprise chance 1.0; sub-states [0] Flee, [1] GenericAction):
- Evaluate mask (slot 81, 0x00aee5a0): Explosion (39), ImpendingExplosion (51), WitnessMagic
  (112). Only ImpendingExplosion has a poster: the AI noise path 0x00c58560 case 8 turns a noise
  of context ImpendingExplosion into stimulus 51 carrying the noise maker. The only maker of such
  noises is `DishonoredBreakable`'s tick (0x00a3b0d0): a step with `m_bAISendImpendingExplosionStim`
  (the whale oil batteries' fuse steps) sets `m_fImpendingExplosionBroadcastTimer` to the class
  default 1.0 and `m_eImpendingExplosionLoudness` to the step's loudness (0x00a3a5f0); the tick
  counts it down and at 0 broadcasts and restarts it.
- Accept (0x00af13b0): not the pawn's own explosive, nearer than `m_fExplosionClearance`.
- Start (0x00af1660 / 0x00af94a0): timer = `m_fEscapeEndTimer`; inside the `m_fDiscoveryAngle`
  cone of the pawn's facing (0x00bbfa60) the pawn reacts at once, else turns to it (0x00cba420)
  and reacts once facing (0x00af95b0). React: with `m_fSurpriseChance` the GenericAction
  `DisNPCAnim_Panic` (group 0; 0x00af1600 -> 0x00b2ded0(5, 0)), then the flee; else the flee
  (0x00af1570); both bark `DDH_ESCAPE_IMPENDING_EXPLOSION` (hook 116).
- Update (0x00af1720): the timer runs down; once `m_fEscapeThreatTimer` has passed, an enemy
  within `m_fEscapeThreatRadius` (300 uu vertically, 0x00bc0140) ends the behaviour. The
  explosive going off sets the timer to `m_fEscapeThreatTimerAfterExplosion` (0x00af1970).
- Flee (`DisAISubStateFlee`, 0x00b3bea0): with `m_bUseFleePoints` the best flee point
  (`DisNavMeshGoal_BestFleePoint`), else with `m_bUseSystemicFlee` the navmesh search with
  `DisNavMeshGoal_SafeFromThreat` (0x00d12f40 / 0x00d122d0: per poly in path-cost order, each edge's
  point nearest the threat's view point, pulled toward the poly centre by the pawn radius, is the
  answer when its distance is strictly between `m_fSystemicFleeMinEnvellope` and `MaxEnvellope`)
  and `DisFleeConstraint` (0x00b054c0: each link's cost minus the edge's distance from the threat;
  links in the `m_fForbiddenAngle` cone toward the threat within `m_fForbiddenRadius` rejected).
  None found: "No Flee Destination found", `PathingFail` (69).
  The escape's flee sub-state is systemic only, 500 to 30000 uu. Every shipped panic flee
  sub-state has `m_bUseSystemicFlee` false (flee points only), so the port's panic fallback "a
  navmesh point 1500 uu away" has no original counterpart (open: the panic's PathingFail path).
- Port: `dishonored_world::nav::NavWorld::{search_goal, safe_from_threat}`,
  `dishonored_ai::Brain::{escape_from, act_escape, flee_destination}` (`Behavior::Escape`),
  `dishonored_sim::world_actors::BreakableState::impending`, `AiConfig::escape_explosion`. Battery
  damage (their own TakeDamage 0x00c7d340) is not ported, so no shipped breakable reaches a fuse
  step yet.

### 1.4 What turns attention into NoticeBegin / SearchBegin / CombatBegin (superseded by 1.4a)

The three attention-driven behaviours evaluate only their `*Begin` stimulus, and `AttentionBehaviorBegin`
exists as a stimulus, but the poster that maps "top enemy attention level reached TurnToFace /
Investigate / Busted" onto those stimuli was not traced (the brain stores
`m_TopEnemyAttnTargetAttentionLevel` at +0x42 and re-posts on changes in 0x00b24ae0). The port's
`Brain::select` assumes TurnToFace -> Notice, Investigate -> Search (via the Investigate sub-state) and
Busted -> Combat, which agrees with the data in the tweaks and with AI.md's observed hook behaviour but
is not read from this code. Request: trace the poster of stimuli 63 / 87 / 17.

### 1.5 Port structure (`crates/dishonored_ai/src/lib.rs`)

The port keeps one `BrainState` enum (14 states) in `Brain::state` and picks it in `Brain::select`
from the strongest attention entry (`focus`): Busted -> Combat or Flee, Investigate -> Investigate,
TurnToFace -> Notice, with `Brain::escalate` inserting `React` (reaction delay 0.18-0.30 s from
`AiConfig::reaction_delay`). `back_to_routine` always restarts the routine. Differences to the slot
model:
- There is no dormant/resume: leaving Notice, Search or Combat re-enters Patrol (`back_to_routine`)
  or Alert; the original resumes the paused behaviour (same effect for patrol, but a guard that was in
  a Soiree, Interact or scripted TakePosition is not resumed).
- Priorities are hard-coded (Combat/Flee over Investigate over Notice), not slot-ordered: the alarm
  (Alert 13) is a special `RingAlarm` state entered from `consider_alarm`; Awe (14, magic) and
  Preserve (EscapeExplosion) have no state at all.
- `React` is a state with a timer; in the original it is a queue delay on the stimulus, applied to
  every stimulus the same way, plus a `DisBehaviorReact` behaviour (slot 7) on `ReactionRequest`.
- The port's `escalate` restarts the delay each time a different target state is requested while in
  `React`, so a stimulus that keeps firing overwrites the pending transition (see finding F1).

## 2. Patrol

Original (AI.md "Patrols" and "Routes" still hold). New data: the pawn speed is not a constant; it is the
authored root speed of the locomotion loop the behaviour requests, scaled by the stance modifier
(`DisLocomotionConfig.EliteGuardLocomotionConfig`, `m_fAnimSpeed` per direction, forward = the N loop):

| speed mode | forward loop | authored uu/s |
|---|---|---|
| EmptyWalk | Empty_WalkN | 157.7 |
| EmptyWalkPatrol | Empty_Walk_Patrol | 126.4 |
| EmptyRun | Empty_RunN | 643.2 |
| SwordWalk | Sword_WalkN | 192.0 |
| SwordPatrolWalk | Sword_Patrol_WalkN | 145.24 |
| SwordRun | Sword_RunN | 809.0 |

Modifiers: Empty 1.0, SearchEmpty 0.8, EmptyPatrol 0.92, Sword 1.0, SearchSword 0.8, SwordPatrol 1.0,
MeleeRush 1.0; accelerations 550 / 450 (empty), 540 (sword), 1100 (SwordPatrol), 65 / 130 (SearchEmpty).
So a sword-carrying guard on patrol walks at about 145 uu/s (LIKELY: speed = authored loop speed), an
investigating or chasing guard runs at about 809 uu/s forward (sideways loops 350-512). Walk-to-point
arrival: `m_fUnaccuracyRadius` 120 (AI.md "Go-to completion").

Port: patrol, wander and walk-to-point use `PlayerInput` with `buttons::WALK`, which the shared player
movement turns into the player's walk speed (250 uu/s, scenario `ai/patrol` Max speed2d 250.0) and
`SPRINT` for runs (540 uu/s, `ai/boyle_hear_shot`). Route adoption and neglect follow AI.md and are
unit-tested; at a point the port arrives within 70 uu (`Self::reached(.., 70.0)`) where the original
uses the locomotion arrival (120 uu with a stop animation, 0.1 uu for accurate stops).

## 3. Investigate and search

Data (CONFIRMED, `TweaksBehavior_Search_Default`, `DefaultAI.ini`): Search activates on SearchBegin;
`m_SearchFromNoiseTweaks[4..7].m_bShouldRun` and the listed `m_SearchFromReasonTweaks` make the guard run
for Danger, Help, Alarm and DeathRattle noises and for found corpses, deaths, magic, attacks, sightings;
`m_fCancelInvestigateWaitTime` 0.5-1.0 s on arrival; `m_fSearchReachedProxyLocEqualsRange` 250 (proxy
reached); `m_fSearchReachedProxyBroadcastRange` 2000; `m_fSearchAnimCooldown` 10 s;
`m_bAlwaysSearchWithSwordOut`; `m_fInvestigateStareAtUnreachableMaxTime_s` 5 s within
`m_fInvestigateStareAtUnreachableRadius` 500 (sub-state `StareAtUnreachable` when the proxy cannot be
reached); sub-states Investigate, GenericAction, Stand, StareAtUnreachable, TrackTarget; search crumbs
(25, spacing, heartbeat 0.8 s, wander 500-1000 uu within 60 degrees) in `DishonoredSearchCrumbsComponent`.
Search ends when the attention to the stimulus falls under the Investigate stop (suspecting profile:
leak 0.015 / s from 0.6 to the 0.1 stop: about 33 s; unsuspecting 0.025 / s to 0.3: about 12 s) or the
crumbs run out. Locomotion modifier during search is `SearchEmpty` / `SearchSword` (speed x 0.8).

Port: `BrainState::Investigate` -> `Search` with the same constants (`AiConfig::search`). Missing:
`StareAtUnreachable`, the search locomotion modifier (x 0.8) and the proximity range
(`m_fSearchReachedProxyLocEqualsRange` 250 is used as `search.proximity`, matches), the animation
cooldown. Finding F1 below is a port loop when the proxy path fails.

## 4. Combat

Already decoded (AI.md "Combat moves"): attack contexts and ranges, ManageAttacks flurry and stamina
pacing, back step after incoming attacks, side step after a ranged threat, pistol willingness and the
`FirePistol` timers, skirmish caps. New in this pass (CONFIRMED from `DisAISubProcessManageAttacks`
0x00b3dc10 / 0x00b3deb0 / 0x00b3e070 and the tweak):

- **Aggression.** `m_fAggression` (+0x68) starts at a uniform random in [0, 1) each time the process
  activates. The handler registered for the incoming-attack reaction (0x00b3e070, used for the back-step
  logic) adds `m_fAggressionDeltaPerDamage` (-0.5 for the elite guard) to it, clamps to [0, 1],
  decrements the incoming-attack counter and resets the forgiveness timer. Whenever an attack context is
  selected (0x00b3deb0) its cooldown timer is scaled by `(1 - aggression) * m_fCooldownFactorAtMinAggression + 1`
  (factor 1.0 for the elite guard: cooldowns x1 at aggression 1, x2 at aggression 0).
  `m_fAggressionRegenRate` 0.1 / s is the matching recovery (LIKELY; the regen call site was not traced).
  The port has none of this: every cooldown is the plain roll (`combat::pick_context`, `Brain::melee_cd`).
- `m_fStalemateAllowanceTime` 3 s (`m_fStalemateTimer` +0x9c counts down while no action is chosen and is
  clamped to the same value), `m_fBlockedAttackSensitivity` 7 with `m_fBlockedAttackWeight` /
  `m_fBlockedAttackTimer` (+0x6c / +0x70) decide when the process asks for the block breaker
  `DisItemContext_NPCBash` (`m_pBlockBreakerAttack`, usable when `m_bCapableOfBlockBreaker`): an NPC whose
  attacks keep being blocked bashes. Not in the port (`Brain::combat` blocks purely as a reaction).
- `m_fPaddingAgainstNPCs` -150, `m_fPaddingAgainstPlayer`, `m_pShortestFormationAttack` =
  `NPCAttackStepBack`, `m_pLongestFormationAttack` = `NPCFireGun`: the formation distances used by the
  skirmish placement. The port's outer ring is a distance rule (`pistol.max_range.min(1500)`).
- Blocking: the NPC's own block is not a ManageAttacks decision. There is no block context in the
  attack list (`NPCRiposte` and `NPCBash` exist as contexts); the port blocks whenever the target is in
  sword startup and within reach (`Brain::combat` tail). UNVERIFIED against the original.

Ranged and special NPCs (class list, `oracle index search`): `DisBehaviorCombatEliteGuard` (sword +
pistol), `DisBehaviorCombatCityGuard`, `DisBehaviorOverseerCombat` / `OverseerHMasterCombat` /
`MusicalCombat` (Overseers, music box and tune contexts `NPCTune`, `NPCTune_Combat`,
`NPCTune_Protection`, `NPCAttractSpell`, `NPCTeleportSpell`, grenades `NPCThrowGrenade`),
`DisBehaviorTallBoyCombat` + `DisBehaviorTallboyShoot` (bow, `NPCFireBow`), `DisBehaviorAssassinCombat`
(pull / attract spells, DLC), `DisBehaviorThugCombat`, `DisBehaviorCombatWolfhound` (+ short / long
distance sub-states, `WHJumpAttack` inhibition 30 s, `WHArmAttack` 8 s), `DisBehaviorCombatRatSwarm`,
weepers (`DisBehaviorWeep`, arm grab inhibition 20 s). Port: guard role -> elite-guard melee + one
pistol round for every sworded or armed pawn including Overseers (`role_from_chain`); tallboys, weepers
and hounds have role Other and no combat brain (`npcs::role_from_tweak_data`); rats have their own
brains (`dishonored_ai::rats`). Overseer music-box combat, tallboy bows and grenade throwers are not
modelled. (Superseded by the F6 port pass, section 12: the class now follows the brain's Combat-slot
behaviour; grenade-throwing Overseers remain open.)

## 5. Civilians and fear

AI.md already covers the panic triggers (`TweaksBehavior_Panic_Civilian`: SeeEnemy, WitnessDeath,
WitnessCorpse, AttackedMelee/Ranged, WitnessAllyCombat, WitnessAllyPanic, WithinDangerZone, HearNoise
with AlarmBell, Help, DeathRattle), the group distance 1000 uu, flee points (50 degree forbidden cone
within 500 uu), 2 s free flight, cower within 350 uu, help bark at Loudness2 (1500 uu). Slot: Panic is
slot 15 Escape and triggers on the `Panicked` stimulus from `DisAIBrainProcessPanic`; StationarySearch
(civilian investigate, 2-3 s) is slot 5. The port's Flee / Cower states reproduce these constants.
Locomotion for panic uses `PanicEmpty` / `PanicSword` modifiers (acceleration 120 / 100, deceleration
220 / 200, speed x1.0) with the panic run loops (`Empty_RunN_Panic1` 503.5 uu/s forward); the port
sprints at 540.

## 6. Bodies, unconscious and sleeping NPCs

- Corpse discovery is the attention reason `DACRT_FoundCorpse` -> Investigate (AI.md); the vision scalar
  for a dead target is `m_fSeeingPawnAttentionIncreaseScalar_ToDeadNPC`. The reaction table
  (`DisTweaks_AIBrain_Reaction`) reacts to carried corpses: `CarryAllyCorpse` / `CarryNeutralCorpse`
  (Hostile for the Boyle elite) and `CarryEnemyCorpse` (Harmless).
- Whether an **unconscious or sleeping** NPC counts as "dead" for this scalar, and whether guards
  investigate unconscious bodies, was not found in the code (the dead flag of the target record,
  0x00c86bf0 / 0x00b68e10, was not traced). UNVERIFIED. Port: `perceive_with` discovers only
  `!p.alive` pawns; an unconscious pawn (`Condition::Unconscious`, alive) is never discovered and does
  not alarm anyone. `Brain::unaware_of` and sleep darts follow AI.md ("Sleep darts").
- Unconscious pawns never wake in the port (`takedown.rs` note); the original has no wake state for
  choked or darted NPCs (stated in the same note, LIKELY).

## 7. Magic: Windblast, possession, Bend Time

`DisBehaviorMagicResponse` (slot 14 Awe, above Combat and below Panic) is the response to
`WitnessMagic`; its tweak has only a GenericAction and a Stand sub-state. The global
`m_MagicReactionDelay` 0.20-0.30 s delays the stimulus. `DACRT_Magic` is a Busted-capable attention
reason (AI.md), `EAINoiseContext_WolZap` / `FlareCombustion` are separate noise contexts, and
`m_eWallsOfLightConfidence` (Confident for the party elite) tunes the Walls of Light response. Port:
no `MagicResponse` behaviour; Windblast is only a Danger sound (`noise_context`), possession and Bend
Time give no AI reaction, and the world time scale slows the brains with the pawns
(`npc.<name>.time_scale` channel). Window of the original's reaction (what animation, how long, does it
alert) is UNVERIFIED.

## 8. Rule-by-rule comparison with the port

| rule | original source | port | status |
|---|---|---|---|
| slot priority and preempt/resume | 0x00b17100, 0x00b11960 | `Brain::select`, `escalate`, `back_to_routine` | port simplified; structural |
| activation stimuli per behaviour | class evaluate masks | hard-coded attention-level mapping | agrees for Notice / Search / Combat; poster not traced |
| stimulus queue delay | 0x00b16eb0, DefaultAI.ini | `escalate` + `AiConfig::reaction_delay` 0.18-0.30 | match (scenario `ai/investigate_noise`: 10 ticks, one-tick slack) |
| magic reaction delay 0.2-0.3 | DefaultAI.ini | none | port missing |
| patrol route adoption | 0x00c41590, 0x00a4bc70 | `Crowd::set_routes`, `adopt_route` | match for crowd guards; **not applied to original-spawner guards** (F2) |
| patrol / run speed | EliteGuardLocomotionConfig | player walk 250 / sprint 540 | divergence (F3), capture requested |
| search constants | tweak + ini | `AiConfig::search` | match; StareAtUnreachable, x0.8 speed missing |
| attack cadence (flurry, stamina) | 0x00b47880 | `Brain::combat` flurry / stamina_timer | match in structure: scenario shows swing groups of 2-3 about 5 s apart |
| aggression-scaled cooldown | 0x00b3deb0, 0x00b3e070 | none | port missing (F5) |
| block breaker, stalemate | tweak, 0x00b47880 | none | port missing |
| pistol willingness, give-up | AI.md | `combat::pistol_willing` | match (unit-tested) |
| overseer / tallboy / hound / weeper combat | classes above | `dishonored_ai::special`, `dishonored_sim::special_npcs` | core rules ported (F6, section 12) |
| unconscious body discovery | unknown | none | UNVERIFIED (F7) |
| magic response | `DisBehaviorMagicResponse` | none | port missing (F8) |

## 9. Scenarios (fixtures/oracle/scenarios/ai)

Run with `oracle scenario --system ai --dir <worktree>/fixtures/oracle/scenarios --out <dir>`
(port side, 2.0 s for all eight). Values below are the port's.

| scenario | map | what it pins | port result |
|---|---|---|---|
| `ai/investigate_noise` (rewritten) | Playground | a pistol shot 3800 uu behind a guard, out of sight: attention 0.6 (noise entry), React lasts the reaction delay, Investigate, walks to the shot | attention 0.6 pass; Investigate at 0.30 s pass; **walk to the shot fails** (F1) |
| `ai/shot_in_view_of_neutral` | Playground | does a neutral guard that turns to a shot and sees the shooter go Busted | port: Combat at 0.32 s, attention level 4 (UNVERIFIED reference) |
| `ai/patrol` | Playground | route patrol; speed | reaches the far point; speed 250 (reference 145.24 Unmeasured) |
| `ai/combat_cadence` | Playground | alerted guard vs motionless player, 40 s | 15 swings, first at 4.63 s, groups of 1-3 swings about 5 s apart |
| `ai/passive_keeps_rotation` | Playground | unchanged | pass |
| `ai/boyle_patrol_speed` | L_Boyle_Ext_P | spawner 21 (Footbridge guard, patrol_on_startup): patrol speed, route | port: guard never leaves its spawn in 20 s (F2); original capture queued |
| `ai/boyle_hear_shot` | L_Boyle_Ext_P | party guard 46, shot 800 uu behind | port: React, Investigate, runs 540 uu/s to the shot, never Combat; original capture queued |
| `ai/boyle_doorman_duel` | L_Boyle_Ext_P | elite guard vs motionless player 500 uu away | port: React / Notice / Investigate chain, Combat at 1.47 s, hits at 3.4, 8.0, 8.6, 13.4, 14.0 s (14 damage each), player dead at 14.0 s; original capture queued |

Captures requested in `fixtures/oracle/capture_queue.txt`: the three Boyle scenarios plus by-name reads of
`m_ReactionDelay` / `m_MagicReactionDelay`. The original-side observable for NPCs is position / health
per tick; behaviour state is not traced, so state-machine claims stay Static or Unverified until the
hook can read `m_pCurrentBehavior` (class name) of the brain: request to the hook agent, property path
`Controller` -> brain object -> `m_pCurrentBehavior` (`DishonoredAIBrain` +0x14c), plus `m_ActiveBehaviorStack`.

## 10. Findings

F1. **Far investigate loops between React and Investigate (port bug, diagnosed from code and trace).**
`ai/investigate_noise`: the guard hears the shot (attention 0.6 at tick 8), goes React for 10 ticks,
Investigate for 2 ticks, React again, and so on; over 15 s it moves 77 uu instead of 3800. Cause (code
reading, not instrumented): in `act_inner` the Investigate branch sets `self.next = Search` and state
`React` when `reached(..) || follow.failed`; `Brain::select` then still sees the Investigate-level
focus, finds `state != Investigate` and calls `escalate(.., Investigate)`, which replaces the pending
Search with a new reaction delay and so restarts the loop each time the path fails. The failure itself
(path to the shot position on the Playground navigation, presumably the proxy at the player's eye
height or off the generated mesh) is a second question. Original behaviour for an unreachable proxy:
`StareAtUnreachable` for at most 5 s (radius 500), then search (finding F4). Fix: make `escalate` ignore a
request for the state the brain is already heading to *or* returning from (do not restart `React` when
`next` is set by the brain itself), and implement the unreachable-proxy branch.

F2. **Guards from original spawners with `patrol_on_startup` never adopt a route (port bug).**
`MissionMatch::spawn_original` (`dishonored_multiplayer/src/mission.rs:619-665`) creates the brain with
`stay = !patrol_on_startup` but only `Crowd::set_routes` adopts routes, and it runs over the brains that
exist at that moment (line 344 for the crowd, line 580 copies `crowd.routes` without adopting). Trace:
`ai/boyle_patrol_speed` (spawner `DishonoredSpawner_21`, squad Footbridge_Guard, routes 12 / 18 with
that squad exist): state Idle and position fixed for 20 s. The brain should call `adopt_route` (squad,
capacity, adoption range, `m_NeglectTimer`) when spawned with patrol_on_startup and then enter Patrol.
Original: `PatrolRequest` posted at startup for these spawners (Patrol evaluates PatrolRequest).

F3. **Patrol, search and run speeds are the player's (port, needs capture).** The original derives the NPC
speed from the authored locomotion loops (section 2: sword patrol walk 145.24, empty patrol 126.4,
sword run 809 forward, search x0.8, panic run 503.5 forward). The port walks at 250 and runs at 540.
Capture `ai/boyle_patrol_speed` and `ai/boyle_hear_shot` (position per tick) to confirm that speed =
authored loop speed x modifier; then give NPC pawns their own per-mode speeds (and the 0.5 s modifier
blend, acceleration 540-1100).

F4. **Missing branches of Search:** StareAtUnreachable (5 s, 500 uu), search-speed modifier 0.8, search
animation cooldown 10 s (port missing).

F5. **Combat aggression and block breaker missing (port missing).** Aggression random in [0, 1) at
activation; cooldown scale `1 + (1 - aggression) x 1.0`; -0.5 per incoming-attack reaction clamped to
[0, 1]; regen 0.1 / s (LIKELY). Stalemate allowance 3 s, blocked-attack sensitivity 7, block breaker
`NPCBash`. Effect on cadence: a fresh fight has cooldowns between x1 and x2 of the table values; the port
always uses x1 (`ai/combat_cadence` first swing at 4.63 s, 15 swings in 40 s are the baseline to compare
with the doorman capture). Also check why the port's first swing comes 4.6 s after combat starts for a
target 300 uu away (chase plus the initial stamina timer?); no reference yet.

F6. **Special combat NPCs missing (port missing):** Overseers (musical combat, tunes, grenades), tallboys
(bow, `TallboyShoot`), weepers, hounds. Overseers currently fight as elite guards.

F7. **Unconscious bodies are ignored by guards (UNVERIFIED divergence).** Needs the original: place an
unconscious NPC (choked) in a guard's view and record the guard's position / attention. Request: a
hook-driven scenario (`call` the NPC's knock-out or use a spawner with `m_bTreatAsKnockedOut`) and
`read` of the guard's attention level.

F8. **No magic response (port missing):** `WitnessMagic` -> `MagicResponse` (Awe) with
`m_MagicReactionDelay`; Windblast, possession and Bend Time have no AI reaction beyond the Danger noise.

F9. **A pistol shot witnessed after the fact makes a neutral guard hostile (suspected port divergence).**
`criminal()` treats the player's pistol Recovering phase as an attack in progress, so a neutral guard
that turns toward the noise within about 1.5 s turns Busted and fights (`ai/shot_in_view_of_neutral`:
Combat at 0.32 s). The original reaction table has no response for a fired gun (only the listed events),
and `Neutral_Guard` is not an enemy of Corvo. Original behaviour is UNVERIFIED; `ai/boyle_hear_shot`
captures it for the party guard.

F10. **Oracle limitation.** The original trace has only position / health / physics per NPC; the
port's state enum (`npc.<n>.state`) has no original counterpart. Brain behaviour class per tick needs a
hook read (`m_pCurrentBehavior`). Until then AI checks are Static (from data) or position-based.

F11. **The attention-to-stimulus poster is not traced** (section 1.4); the port's mapping is assumed.

Do not edit port code from this document: the fixes above are for the porting agents.

## 11. Port pass (2026-10-08): status per finding

**Core rebuild (§1).** `crates/dishonored_ai/src/slots.rs`: the brain is now the slot stack
(`Brain::slots`, one `Behavior` per `EAISlot`, the highest occupied slot current, lower ones
dormant and resumed), the stimulus queue (`Brain::stims`, per-stimulus delay, dispatch through
the behaviours' complete / evaluate masks) and the attention poster of §1.4a
(`Brain::post_attention`). `Brain::state` is now the projection of the current behaviour
(`Brain::project`); an external write of it (mission control, Kismet, the runner) is turned into
that behaviour (`Brain::force`). Behaviours: Routine (Idle slot), Notice, Search /
StationarySearch, Combat, TriggerAlarm (Alert), MagicResponse (Awe, new `BrainState::Awe`), Panic
(Escape). `Brain::select` / `escalate` / `back_to_routine` / `go_search` and the React state's
reaction delay are gone (§1.4a: no delay on the attention stimuli). Not ported: the active
behaviours' filter masks (dispatch stops at a filtering top behaviour), the queue's
replace/drop handlers (0x00b148c0 vslot 3), HeadTrack stimuli (no behaviour evaluates them in the
guard list; head tracking stays the port's `look`), the behaviours not listed above.

- **F1 ported.** Combined with F4: the Investigate sub-state follows the live proxy (0x00b29f00),
  is reached within the proximity (0x00b38530), and on a path failure runs the PathingFail
  handler (0x00b33850: `m_PathingFailRetryCount` 1, nearest navmesh point within 500 uu around /
  25 up / 500 down, `NavWorld::closest_point_in`), then finishes; the loop of the old React /
  Investigate is gone with React.
- **F2 ported.** `MissionMatch::spawn_original` posts `PatrolRequest` (`Brain::request_patrol`)
  for `patrol_on_startup`; `DisBehaviorPatrol` adopts a route at dispatch.
- **F3 ported, patrol speed measured (116.3 uu/s, EmptyPatrol, see below).** `Brain::loco_speed`: forward loop speed of the speed mode (Walk 1,
  Jog 2) of the stance modifier x `m_fSpeedMultiplier`, from `AiConfig::locomotion` (every
  `ArkComponentLocomotionConfig` of the Boyle script packages); the sim takes it through
  `PlayerInput::npc_speed` (`movement::speed_modifier`, no sword factor). The pawn's config comes
  from its tweak's anim profile (`Brain::loco`), else EliteGuardLocomotionConfig. Acceleration
  (540-1100) and the 0.5 s modifier blend are not ported.
- **F4 ported.** The Search decision after Investigate (0x00afeb00): StareAtUnreachable when the
  failed proxy is > 500 uu above (literal in 0x00b33850), timer `m_fInvestigateStareAtUnreachable
  MaxTime_s` counting only while the target is unseen (0x00af4c40), cancelled when the target is
  farther than the radius from the proxy (0x00af4440); Stand for `m_fCancelInvestigateWaitTime`
  when the proxy was not reached (0x00af4b10); else the search animation (GenericAction,
  SearchLostTarget or CorpseCheck, length loaded from the elite guard's sword AnimSet) unless
  `m_fSearchAnimCooldown` (10 s, set on leaving it, 0x00af4ab0) runs; then TrackTarget (the
  crumbs). The x0.8 search speed comes with F3 (SearchSword / SearchEmpty).
- **F5 partly ported.** CONFIRMED and ported: aggression rolled in [0, 1) at start (0x00b3dc10),
  regen `m_fAggressionRegenRate` per second (0x00b39e50), `m_fAggressionDeltaPerDamage` on being
  attacked (0x00b3e070, the AttackedByEnemy handler: applied on the brain's damage event), cooldown
  scale `(1 - a) x factor + 1` (0x00b3deb0). Decoded, not ported: the block breaker (blocked
  weight += a roll between the pawn attributes `Attribute_BlockBreakRate_Min/Max` on a blocked own
  attack (EvadedMelee_Outgoing, 0x00b3a030), timer = `m_fBlockedAttackSensitivity`, wants
  `NPCBash` at weight >= 1 (0x00b39e50); the sim has no bash move and the attribute owner is not
  resolved) and the formation distance with the stalemate timer (0x00b3e120: the desired distance
  interpolates the shortest / longest formation attacks' ranges by aggression; after
  `m_fStalemateAllowanceTime` without an action it closes in).
- **F6 ported (core rules), see section 12.** Music-box Overseers, Tallboys, wolfhounds and
  weepers run their own Combat-slot behaviours; the sim applies the tune, the plague cloud and
  the Tallboy arrow. Grenade-throwing Overseers (`DisBehaviorOverseerCombat`) are not covered.
- **F7 open** (needs the original).
- **F8 ported.** WitnessMagic per 0x00b23e50 with the modes of the power callers of 0x00bf9fb0
  (Windblast / Swarm / slowing Bend Time end 0; Blink end / possession end / stopping Bend Time end
  1; Blink start / possession start / Bend Time stop 2): Busted on the user -> WitnessMagic after
  `m_MagicReactionDelay`; mode 1 not Busted -> `m_IncreaseForMagic` and a 3 s window in which
  becoming Busted (0x00b24ec0) or leaving Busted (0x00b23160, disappeared) reacts.
  MagicResponse (Awe): face the user unless an ally, bark DDH_MAGIC_WITNESSED and finish, or
  DDH_MAGIC_DISAPPEARED and, unless ally or in sight, the MagicLostTarget animation. Not ported:
  the soiree check (0x00b4b4c0: reactions disabled by a soiree give only the bark), the sense-flag
  check of mode 1, the repeated polling of possession's inactive stage (once here).
- **F9 ported (the rule); turning open.** `criminal()` removed: firing or swinging near an NPC is
  `m_Response_PlayerThreats` (Harmless) in every reaction tweak. NPC turning through rotation
  intents stays open: the intents' rate writers for the AI's face-to requests (`m_fRotationSpeed`
  -100 default) were not traced (U-862).
- **F10** unchanged. **F11 resolved** (§1.4a).

**Checked against the original's brain channels (captures of 2026-10-08, main d42497c).**
- Slot stack: `ai/boyle_doorman_duel` shows `DisBehaviorSoiree` current in slot 2 with
  `DisBehaviorPatrol` dormant in slot 0; `ai/boyle_hear_shot` shows `DisBehaviorReact` entering
  slot 7 above the patrol and the patrol becoming current again when it ends: the model of §1.1.
- Patrol speed: measured 116.3 uu/s = `EmptyPatrol` (x0.92) on `Empty_Walk_Patrol` (126.4), not
  the sword patrol loop (145.24): a patrolling guard walks sheathed. `Brain::loco_speed` uses
  EmptyPatrol for a guard's routine; `ai/boyle_patrol_speed` (now with `original_npcs: true`, the
  level's own spawner pawn) gives 116.29 uu/s on its adopted route.
- Hearing: the neutral party guard's *player* attention goes to 1.0 at the shot and leaks at the
  neutral profile's 2.0 / s (0.667 ten ticks later, level HeadTrack): a heard noise raises
  attention on its instigator, with the profile of the relationship toward it (the port used a
  place entry). Ported in `perceive_with` (pawn-made noises; alarm bells, help barks, death
  rattles stay place entries). With it a neutral guard does not search a shot, as on the
  original; a hostile one does (`ai/investigate_noise`, moved to the navmeshed stealth lane with a
  hostile guard: Investigate the tick after the shot, 647 uu/s run = SearchSword x Sword_RunN).
- Open: the neutral guard's `DisBehaviorReact` (slot 7, 124 ticks, `ReactionRequest` from the
  reaction table: here `m_Response_PlayerThreats`, Harmless). Not ported: the response trigger
  for a heard shot and the behaviour's length (its animation) are not decoded.
- Tooling note: the runner places a spawner NPC without an explicit faction as
  `Faction_Guard_Default`; spawner 46 is `Neutral_Guard` on the original, so `ai/boyle_hear_shot`
  and the stealth scenarios compare a hostile port guard with a neutral original one.

## 12. Special NPC classes (F6 port pass, 2026-10-08)

**Class selection (CONFIRMED, data).** A pawn's class is the behaviour in its brain's Combat slot:
`m_pBrainTweak` -> `m_BehaviorTweak[i].m_Difficulty[1]` with `m_DesignatedSlot` `EAISlot_Combat` ->
`m_pSpawnedObjectClass` (`dishonored_assets::ai_level::combat_behavior_class`, `AiLevel::tweak_combat`,
`NpcClass`). `DisBehaviorMusicalCombat` -> MusicalOverseer, `DisBehaviorTallBoyCombat` -> Tallboy,
`DisBehaviorCombatWolfhound` -> Wolfhound, `DisBehaviorWeep` -> Weeper, other combat classes ->
the elite guard's combat, none -> civilian. A pawn with a combat behaviour spawns as a fighter
(`PawnKind::Guard`; tallboys and weepers were civilians before and panicked). The brains of these
classes have no TriggerAlarm and no Panic (their behaviour lists, CONFIRMED by absence), so the
alarm is gated to the elite class. Tuning: `AiConfig::special` (`dishonored_assets::ai_config`,
packages `SPECIAL_NPC_PACKAGES`: the Boyle scripts plus the Overseer kennel scripts for the hound).

| class | behaviour list differences | combat core rule (ported) | attacks and numbers | senses, movement | not ported |
|---|---|---|---|---|---|
| Music-box Overseer (`Twk_Brain_OverSeerMusical_Default`) | no TriggerAlarm, no pistol; MusicalCombat in slot Combat | MaintainDistance closes at Jog to the protection tune's `m_fIdealContextRange` 1000 and never backs away (0x00b2e600); each refresh the bash first (0x00b13990), else the combat tune on a seen target in range (0x00b13920); the tune plays standing and stops after `m_fStopWhenNotSeeingTimer` 5 s unseen or `m_fStopWhenOutOfRangeTimer` 1 s out of range/cone (0x00c2b680), then `m_ContextCooldown` 2-5 s | tune: 150-2000 uu, 45 degrees, clear trace (0x00c6bfc0); every `m_fMusicEffectPeriod` 0.3 s `0.3 x m_iMusicEffectDamage 90` Musical damage less `Attribute_MusicDamageReduction` (Corvo Normal 0.93): 2 per tick (0x00c66ef0, 0x00b5ccd0); repulse `lerp(1000, 3000, 1 - d/2000)` uu/s per second (0x00c75270, L: `this` inferred); powers inhibited within the protection tune's `m_fMusicEffectRadius_InhibitPowers` 1500 while a tune plays (0x00bd7390); bash `NPCBash` 150 uu, 45 degrees, cooldown 5, unblockable, 10 damage (music box MeleeDamage x 1.0, L); health 40 | EliteGuardLocomotionConfig, `MusicBox` modifier (x1.0) | the protection tune itself (who starts it: the weapon's slot 95, L) and its 1 s `m_fPowersInhibitedUntil` stamp and power cancel (0x00ab0ea0); glass breaking; the tune's danger AI sound; sub-state 3's context is LIKELY the combat tune (the trigger walk 0x00c18230 not proven) |
| Tallboy (`BrainTweaks_TallBoy`) | Search_Tallboy, Patrol/Idle/GoHome Tallboy, TallboyShoot (Kismet, Comply) | FindShootingPosition until the bow can fire (0x00b475e0): the point at the bow's ideal range 1200 on the line from the target, on the navmesh, with a clear line, at walk, given up after 3 s without progress; ManageAttacks takes the bow before the stomp (slot order, 0x00b470d0) | bow `NPCFireBow_1`: 400-4500 uu, 40 degrees, cooldown 1-3 s x aggression scale, allies of the Tallboy within 750 of the target hold fire (0x00bf7550); the shot after the aim is aligned within 20 degrees for 0.25 s (`StateNPCMasterTallboyAim`, NPC ini); hit chance `lerp(MaxAccuracy 1.0, MinAccuracy 0.1)` over the bow's range (0x00b732e0), a hit aims at the ground 300 uu under the target with the velocity lead, the pitch solves the arc (2000 uu/s, gravity x0.5), a miss turns up to `m_fCheatAngleDeviation` 30 off; the arrow (`ProjectileKind::TallboyArrow`) explodes at its first impact 10 uu off the hit (0x00c550d0): `TallboyArrowExplosion_Twk` 40 within 100, 0 at 500, momentum 1500 + 500 up; stomp 0-500 uu, 45 degrees, cooldown 1-3, unblockable, 15 (Normal); health 150 | TallBoyLocomotionConfig walk loop only (213 uu/s; transit speeds all map to walk) | vision `Twk_Vision_Tallboy_Default` (inner 55 / 4500, outer 170 / 2200; brains share one vision config in the port); the searchlight; the stilts (legs immune, `Phm_Tallboy_Legs`, 0x00b83d60) and the rider's height (the port's pawn is human-sized: the arrow leaves at a human eye); the danger sphere 2000; FindShootingPosition's candidate polygons and sort |
| Wolfhound (`BrainTweaks_WolfHound`, `Pwn_WolfHound_0`) | Notice_Wolfhound, Search_Wolfhound, EnemyUnreachable_Wolfhound, RatChomp; no alarm, panic, react, magic response | MeleeChase / Engage: run (RunNormal 990, `TransitSpeedToLocomotionSpeed[2]` = 3) to the medium bite's ideal range 200, walk within `m_fWalkDistance` 300, back to the ideal range when inside the bite's minimum; unreachable target: hold (CombatLongDistance, 0x00b13a80) | bite `NPCAttackMedium` 200-375 uu, 50 degrees, cooldown 0.5-2, 15 (bite MeleeDamage 15 x 1.0); jump attack 550-800 uu, 15 degrees, cooldown 20-30, inhibition 30 (DefaultAI.ini), knockdown `m_DamageOnKnockDown[1]` 10, unblockable; ManageAttacks flurry 2-3, stamina 3-4 s, aggression regen 0.07; attention `Twk_Attn_Wolfhound_*` (lower thresholds, leak 0.025); health 20 | WolfHoundLocomotionConfig (Startup) | the arm attack and the jump's grab loop (sync attacks: 5 / 10 damage per 0.5 s until the player escapes; the sim has no sync-attack lock), the 90-degree turn bites, the bark context and long-range repositioning, vision outer FOV 180, the pack processes (`DisAIBrainProcessWolfhound` count, Houndmaster summon) |
| Weeper (`BrainTweaks_Weeper`) | DisBehaviorWeep in slot Combat, Search_Weeper, GoHome_Weeper; no alarm, no panic, no sword | Weep variation (chances Weep 1.0): follow at walk, run once fewer than `m_iSimultaneousRunnerAllowed` 3 weepers run (0x00afbe30, after `m_TimeBeforeRunning` 0) and grab (`DisTweaks_WeeperGrab_2`: 0-400 uu, feet height 50, 90 degrees, 20 s group inhibition); after the grab weep standing `m_fStandStillWeeping` 2-5 s, resume beyond 220 uu (0x00aff9e0); give up when the target stays beyond `m_fHomeRadius` 1500 of home for `m_LostTimer` 1-3 s (0x00af6bd0) | cough every `m_fCoughingCooldown` 2-5 s: a plague cloud of `m_fPlagueDmgLength` 2-3 s ticking every `m_fPlagueDmgePeriod` 0.5 s: `m_iPlagueDmgPerTick` 4 to every pawn within 350 uu in the front half-space with a clear trace, plague carriers excluded (0x00c86ea0, `PlayerState::plague`); health 30 | WeepTall / WeepFemale configs (walk 83, run 541) | the grab's hold on the player (a 5-press minigame, no damage) and the cough animation (the cloud starts at the cooldown's end, L: anim 0x4b's notify not mapped); the home anchor is the spawn point (L); `Attribute_PlagueDamageReduction` |

Scenarios (port side; original captures queued in `capture_queue.txt`): `ai/boyle_tallboy_bow`
(first arrow at 0.23 s, each explosion 40 to the player at its feet), `ai/boyle_weeper_plague`
(4 per plague tick, first at 2.5 s), `ai/boyle_overseer_tune` (2 Musical damage per 0.3 s, Blink
inhibited at 1270 uu), `ai/hound_bite` (Playground: jump attack 10, bites 15, run 990 uu/s; not
capturable on the port's maps). Unit tests: `special::tests` (tune lifecycle, plague ticks, the
ballistic pitch), `special_npcs::tests` (tune damage, push and inhibition; plague spares carriers
and the back).