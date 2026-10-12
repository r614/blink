# Melee combat: the original's rules and the port (verification, 2026-10-08)

Scope: sword swings, hit test, damage, block and parry, stagger and riposte, NPC melee, friendly fire,
back and drop assassination, choke. Sources: the static index (`oracle index fn / class / uscript /
field`), the tweak dumps (`tweaks`, `gameplay_config.json`), `research/dishonored/PLAYER.md`
("Melee", "Blood Thirsty"), `INVENTORY.md` (choke), `HOOK_TRIAGE.md` (hook measurements of 2026-10-06),
`AI.md` ("Combat moves"). Port code: `crates/dishonored_sim/src/{combat,takedown,tuning}.rs`,
`crates/dishonored_ai` for NPC attack choice. Nothing here was run against the original in this pass
(the scenario runner and the hooked game were not available): every original-side number is either
read from tweak data / decompiled code (`Static`) or taken from the hook's earlier measurements
(`Measured`, cited); everything else is marked `Unverified` and queued for capture
(`fixtures/oracle/capture_queue.txt`). Decompiled code is described in words, never pasted.

## 1. The headline finding: U-113 is a misreading of the damage rule

The port computes a sword hit as `m_fHealthRatio_NormalDamage (0.2) x the player's HealthMax (70)` = 14
(`combat.rs:resolve_sword_hits`, `dmg = ratio * vitals.health_max`), and 0.7 x 70 = 49 on a staggered
victim. The hook measured 10 on a 50-health guard and the oracle filed the gap as "KnownDifference
U-113 (NPCs have the player's 70 by rule)". That diagnosis is wrong on both counts:

* **The health ratios are not damage factors.** `DisTweaks_MeleeAttack.m_fHealthRatio_NormalDamage`
  (+0xC4, 0.2) and `_BigDamage` (+0xC8, 0.7) are *thresholds on the damage already dealt, as a
  fraction of the victim's HealthMax*, used by the shared hit handler (`DisItemContext_MeleeAttack`
  slot 119, 0x00C131C0) to choose which of three contact types plays (`m_pContactType_SmallDamage`
  below 0.2, `_NormalDamage` between, `_BigDamage` from 0.7; the contact type is the blood / impact
  effect and sound, `DisContactType_Sword[_BigDamage]`). The handler divides the health actually lost
  by the victim's HealthMax (pawn +0x348) and compares.
* **The damage is a flat number from attributes.** The handler asks the context for its damage
  (slot 132, `DisItemContext_MeleeAttack` 0x00C07C80, NPC override 0x00C33DD0), rounds it to an
  integer and hands it to the victim's damage path:

  `damage = round( m_fAttackDamageMultiplier x ( Attribute_MeleeDamage + Attribute_MeleeDamageBonus ) )`

  (`Attribute_MeleeDamage` from the wielded sword's item attributes, `Attribute_MeleeDamageBonus`
  from the wielder pawn; both read by FName through `GetAttribute`, 0x00C86A50). The rounding is
  x87 round-to-nearest-even of `-0.5 - 2d`, arithmetic-shifted right once, negated: for the values that
  occur this is plain rounding (10 -> 10, 15 -> 15, 7.5 -> 8).
  If the attacker's blood-thirsty / finisher state is on, the attribute lookup is replaced by 99999
  (0x00B0FAC0 writes it): an instant kill (the port's `DamageCause::Adrenaline` path models that).

Data (tweaks, all `Static`):

| Quantity | Value | Where |
|---|---|---|
| Corvo's sword `Attribute_MeleeDamage` | 10 on Easy / Normal / Hard / Very Hard | `Twk_Inv_PlayerSpecific.Twk_Inv_SwordCorvo` attributes (fallback `Twk_Inv_SwordBase`) |
| Corvo `Attribute_MeleeDamageBonus` | 0 (all difficulties; bone charms / upgrades can add) | `Twk_Pawn_Corvo`, `Twk_Pawn_DefaultPlayer` |
| `m_fAttackDamageMultiplier` | 1.0 for the player's attack, assassinate and every NPC slash context; **0.0 for `NPCBash` (block breaker) and `NPCJumpAttack`** | every `DisTweaks_NPC*` / `MeleeAttackPlayer` object (dump of 2026-10-08) |
| Elite guard sword `Attribute_MeleeDamage` | **10 / 15 / 20 / 30** (Easy / Normal / Hard / Very Hard) | `Twk_Inv_EliteGuard.Twk_Inv_SwordEliteGuard.DisTweaks_InventoryItem_Attributes_5` |
| Offscreen melee reduction | 0.5 on every difficulty | `DishonoredPlayerPawn.m_fOffscreenMeleeDamageReduction[4]` class default, +0x123C |
| Guard `Attribute_HealthMax` | 50 (hook, `lab npcs`); `Twk_Pawn_DefaultNPC` says 25, the guard base (`Pwn_Guard_Base`) is not in a loadable package | HOOK_TRIAGE.md; `Unverified` statically |
| Corvo `Attribute_HealthMax` | 70 | `Twk_Pawn_Corvo` |

So the hook's two measurements both fall out of the rule: the player hits a guard for **10**
(not 0.2 x 50 by coincidence of 0.2; it is `MeleeDamage` 10), and a guard hits Corvo for **15** on
Normal; the "8 on some first hits" are the **offscreen** reduction (below): 15 x (1 - 0.5) = 7.5 -> 8.

### 1.1 NPC melee damage and the offscreen reduction (0x00C33DD0)

For an NPC attacker the damage function (0x00C33DD0) does, after the base value: if the attack
context's `m_bEffectivelyOffscreen` (context +0x104 bit 0) is set, the base is above 0, the test
0x00BBEF40 on the attacker is false and the victim is the player pawn, then the damage becomes
`(1 - m_fOffscreenMeleeDamageReduction[difficulty]) x damage`, and a result of 1 or less becomes 1.
(0x00BBEF40 is true when the attacker has flag bit 5 of +0x964 set and stands on the viewing side of the
local player, i.e. the dot of a view vector with attacker minus player is positive; the reduction
therefore needs an attacker behind the view plane.) Overrides such as the 99999 attribute are applied
last. The reduction array is indexed by the game's difficulty setting (virtual +0x420 of the
settings object); all four entries are 0.5.
What sets `m_bEffectivelyOffscreen` and what the view vector is are read, not measured:
`Unverified` until `combat/melee_guard_offscreen` is captured.

### 1.2 What the victim does with the number

The player's `TakeDamage` (0x00AB0A30, already in PLAYER.md "Armour"): `m_bPushWithoutDamage` types deal
0; types without `m_bByPassRegenClamp` snap the damage to a multiple of `Attribute_HealthRegenAmount`
(1 for Corvo: a no-op); types with `m_bApplyDamageReduction` lose `(int)Attribute_DamageReduction`,
never below 1; `Attribute_AdrenalineMultWhenTakingDamage` x damage goes to the gauge (0 = none).
`DisDamageType_NPCSlash` (Startup) has `m_bCausesBleeding` and `m_bSeverLimbsUponDeath`
(25 %); the player's sword types are `DisDamageType_FastHit_{Left,Right,Up}` (`DishonoredDamageType_FastHit`).
There is **no big-damage multiplier**: a "big hit" (`eDisWeaponMeleeState_BigHit`) is an animation /
reaction state (random chance `m_fRandomBigHitChance`, NPC 0.5 to 0.75), not extra health loss.

## 2. Core rules and the port

Each rule: the original, then what the port does (file:function) and the verdict.

### 2.1 Swing state machine and timing

Original: the swing is an animation-driven context (`DisItemContext_MeleeAttackPlayer`, vtable
0x0116D4F8). The clip's `DishonoredNotify_AttackZone` runs the attack zone; at `end - m_fDamageZoneMaxTime
- m_fVersusZoneMaxTime` the versus zone opens, at `end - m_fDamageZoneMaxTime` the damage zone
(slot 115, 0x00C13140: sets `m_bDamageIsPosssible`, clears `m_AlreadyHitActors`); while the damage
zone holds, every tick the context gathers hits (0x00BFF7B0) and applies them (0x00C13640 loop ->
slot 119 per hit). `DishonoredNotify_AttackInterruptable` ends the swing (`m_bAttackNowInterruptable`);
chains use `m_ChainInputZone`, `m_bQueuedChain`, `m_fMaxChainAttackTime` (assassinate 1.0 s).
Port (2026-10-11, `dishonored_sim::anim`): the swing plays its clip (`Ply_Sword_Attack_as`: four
ready attacks cycling, one sneak; the elite guard its `AttackShort` / `Medium` / `Long` /
`StepBack` / `Jump` chains) on a layer from the press's own tick; the clip's `AttackZone` handler
moves `combat.rs:update_sword`'s phase to Active (damage zone, 0.05 s) and to Recovery during the
advance, its `AttackInterruptable` (else `AnimStateExit` or its end) ends it on the next tick. The
first hit lands 17 ticks (0.283 s) after the press, one tick before the hook's 0.300 s (U-976: the
decoded order against that measurement; the port of 2026-10-08 matched it by counting from the
tick after the press). `Unverified`: chained follow-up swings (the port returns to Idle after Recovery
and re-triggers on the press edge; the original has a chain input window and the next clip may start
earlier), the crouch/sneak clip choice, a held attack (`m_fCurHoldAttackTime`, purpose unread).

### 2.2 Hit gathering and the hit test

Original: a weapon-socket sweep (`DisMeleeExtent` through `m_MeleeExtentName`; extents
`m_MeleeCollisionInfo[i]` with `m_fScaleAsPlayer` 2.0 / `m_fScaleAgainstPlayer` 3.0 and non-socket boxes
such as range 150 / 300 / 250), trace flags 0x4002092 (NPC contexts, hit gather slot 117, 0x00C338C0 ->
0x00C0CB80), `m_fSweepingAttackSize` 60. **Every actor the sweep meets is hit once per swing**
(`m_AlreadyHitActors`; the tick loop calls the hit handler for each result, the last flagged), so one
swing can hit several pawns. Hits on the environment give a rebound (`HitEnvironment`,
`m_bDisableRebound`). The context's selection range is `m_fMaxContextRange` (210) x
`max(Attribute_Range, 1)` x the forward-speed factor (0x00C0A060, 0x00AB2050; PLAYER.md, CONFIRMED).
Port: `combat.rs:reach` implements the range and speed factor (no `Attribute_Range`), and
`resolve_sword_hits` tests a segment of that length from the eye along the view against each
victim's body axis, accepting a gap of `m_fSweepingAttackSize / 2` beyond the 35 uu radius (centres up
to about 275 uu apart in a line), MELEE-channel line of sight to the victim's centre, and hits **only
the nearest victim** (`hit_done = true`).
Verdicts: **Differs (port missing)** multi-victim swings and environment rebound; **Unverified** the
effective reach in a line (the original's sweep extent is the sword's mesh socket, not a 210 uu
segment: the scenarios `combat/melee_reach_d160 / d230 / d260 / d300` and `melee_lateral_60 / 90` sweep
distance and offset); Attribute_Range is not read (upgrades do not exist in the port today).

### 2.3 Damage

Section 1. Port: wrong (flat `ratio x health_max`; riposte 0.7 x). Fix in F1.

### 2.4 Block and parry

Original (hit handler of the blocking player, 0x00C22B60, CONFIRMED by re-reading it): the blocker's
block age is `now - m_fTimeBlockStarted` (+0xD0). The attack is **parried** when the age is below
`m_fTapForPerfectParryWindow` (0.2) or the `DisNotify_BlockParryZone` of the block clip is active
(0.001-0.431 s, U-021), unless `m_fForceBlockTimeForPerfectParry` (+0xD4, > 0) is running; then the
attacker's context gets the "parried" call (slot 123, 0x1EC), the blocker gets
`m_fAdrenalineAddedParryWin` (20), and if it was a tap parry the forced block is set to
`m_fForceBlockTimeOnPerfectParryTap` (0.3 s) and the block state returns to Start. Otherwise it is a
plain block: block state 2, the attacker's "blocked" call (slot 122, 0x1E8), knockback impulse
`m_fKnockbackImpulse_Min..Max` (500..800), camera shake `m_fBlockCameraShakeScale` 6. The block
cone is `m_fBlockAngle` 50 horizontally, `_Vertical_Up` 30, `_Vertical_Down` 50.
Port: `combat.rs:parries` matches the timing rule exactly; the cone is `block_half_angle` = 50 deg
horizontal only (no vertical test); the consequence is invented: parry -> attacker `Staggered` 1.0 s
(`stagger_time`, SPECULATIVE), block -> attacker recoil 0.4 s (`blocked_recoil`, SPECULATIVE), no
knockback impulse, adrenaline on a won parry is in `adrenaline.rs`.
Verdicts: **Match** parry decision; **Unverified** whether `m_fBlockAngle` 50 is a half angle or the full
cone, the vertical limits, and every duration after the decision (queue: `combat/block`,
`combat/guard_attack` with `measure sword --block`).

### 2.5 Sword clash (versus)

Original: `eDisMeleeResponse` {None, Dodged, Blocked, PreParry, PostParry_Neutral, PostParry_Lose, Versus};
`m_VersusZone`, `m_fVersusAngle` 20, `m_pVersusTargetContext`, `m_BigVersus_Action`
("ParryVersus" animation), `m_bAllowSwordOnSwordClashing`, NPC `m_MeleeVersusMiniGame_*` attributes
(win 5 / 8, lose 3). Two swings that meet inside the versus zone clash.
Port: **missing**: two attackers swinging at each other both land hits (`resolve_sword_hits` only checks
the victim's block). Unverified how often this matters for FFA; the fix is a clash rule, F6.

### 2.6 Stagger, riposte, finisher

Original: after a parry the attacker is in `PostParry_Lose` / `Parried_Lose`; the follow-up hit is an
ordinary hit (damage rule above). `DisItemContext_Finisher` (and `Assassinate`, `DropAssassinate`)
add the kill animations; `m_NumFinishersBeforeSlow_Min/Max` 4-8 and `m_fTimeBeforeSlow_Min/Max`
90-120 s decide when a finisher plays in slow motion. The 0.7 "big damage" is not a damage multiplier
(section 1).
Port: a staggered victim takes `0.7 x health_max` (`DamageCause::SwordCounter`). **Wrong** (F1).

### 2.7 NPC melee

Original: NPC attacks are `DisItemContext_NPCAttack{Short,StepBack,Medium,Long,Left90,Right90,...,
JumpAttack,Bash,Riposte,Ambush,Push,Stomp}` (< `MeleeAttackNPCBase`), chosen by the AI's pattern
tables (ranges and cooldowns in AI.md "Combat moves"). All real slashes use multiplier 1.0 and the
guard sword's difficulty-indexed `MeleeDamage` (15 on Normal); **`NPCJumpAttack` and `NPCBash` have
multiplier 0.0**: they deal no damage themselves (the jump attack closes distance, the bash breaks a
held block, `m_BlockBreakRate_Min/Max` 0.3-0.6 per swing). Contact: `DisDamageType_NPCSlash`. Offscreen
reduction 0.5 (1.1). Armour / helmets as PLAYER.md.
Port: `dishonored_ai` picks the move kinds (`Brain::attack_kind`), `combat.rs` swings with the uniform
`swing_startup / active / recovery` (0.13 / 0.21 / 0.26, U-600, SPECULATIVE) and damages 14 (or 49 on a
staggered target) whatever the move. **Wrong**: damage 14 vs 15; jump attack and bash would deal
damage in the port; no offscreen reduction; no difficulty. Timing of the NPC clips is unmeasured
(U-124 / U-600).

### 2.7a NPC block and parry (decoded 2026-10-12, U-987)

Static (the dumped decompile, Capstone on the functions missing from it, the class table of
`scripts/audit/exe_classes.py`, `enum_dump`). The context states are `eDisWeaponMeleeState`
{0 Attack, 1 BigHit, 2 HitEnvironment, 3 BigHit_Chained, 4 HitEnvironment_Chained, 5 AwaitingParry,
6 Parried_Neutral, 7 Parried_Lose} (context +0xC4, set by 0x00C07E40, which calls the pawn's slot
0x68C 0x00B536C0: 0-4 re-request `StateNPCMasterActionMelee` with the state as the chain step, 5-7
`StateNPCActionParry`); the response byte +0xC7 is `eDisMeleeResponse` (3 PreParry, 4
PostParry_Neutral, 5 PostParry_Lose, 6 Versus). CONFIRMED:

- The attack contexts' slots 122 / 123 / 124 (0x00C07E90 / 0x00C07EE0 / 0x00C07F30) are blocked
  (BigHit), parried (Parried_Lose, response 5) and neutral (Parried_Neutral, response 4).
- `StateNPCActionParry`'s enter (0x00A6A330): AwaitingParry plays `Parry_AwaitingImpact` (chain 61)
  group 0 step 0 (`Sword_Ready_ParryIdle`, looping, blend 0.1); Parried_Neutral `Parry` (62) group
  0, Parried_Lose `Parry` group 2, at the step of 0x00C174C0 (own side +0xC5 Right 0 / 1, Left
  2 / 3, the second of a pair against an opponent swinging left). Group 1 (`Ready_ParryWin_*`) is
  never requested for an NPC. The elite guard's set has no `Sword_Ready_Parry_RightLeft`.
- `DisItemContext_MeleeBlock` (the player's block's base) answers a blow with Blocked only (its
  slot 107, 0x00C13C10, returns 2; the parry decision, 0x00C22B60, is `MeleeBlockPlayer`'s). No NPC
  attack pattern holds it (AI.md "Blocking").
- `DisItemContext_ParryNPC` (< `MeleeAttackNPCBase`): on an incoming attack (its slot 85
  0x00C34200, event 2) with its response at PreParry / PostParry_Neutral and the chance of two pawn
  attributes (0x00C34150, `rand()`), slot 99 (0x00C2F750) requests its action (tweak +0x128) and
  sets AwaitingParry (response PreParry, its side +0xC5 rolled from an attribute); its tick (slot
  77, 0x00C1F8A0) keeps it while the attacker's context says so (slot 0x1E4).
- A swing meeting an attack context (the parry context included) goes through the victim's slot
  106 (0x00C18640 -> 0x00C13A10): PostParry_Neutral for both (the victim Parried_Neutral, the
  attacker its slot 124), or Versus (0x00C087A0: both in `ParryVersus`'s minigame).

The NPCs carry no `DisNotify_BlockParryZone` (none in `Npc_EliteGuard_Sword_as`): an NPC never
parries the player the way the player parries an NPC.
Port (`dishonored_sim::pawn_states`, `combat.rs`): the brain's held BLOCK starts the parry
context's AwaitingParry (`Parry_AwaitingImpact` while it holds, LIKELY: the AI's choice of the
context and its chance are not decoded); a swing meeting it is the neutral clash (the NPC's
`Parry` group 0, the side the blow comes from, LIKELY; the attacker recoils as from a block, no
damage, no stagger, `SwordBlocked`). Not ported: the versus minigame, the player's own
Parried_Neutral animation.

### 2.8 Friendly fire (U-660)

Re-read: the NPC hit gather (0x00C338C0) keeps every actor of the weapon trace (flags 0x4002092, callback
0x00C102C0 only de-duplicates by actor), and the shared handler (0x00C131C0) applies damage to every
result with no relationship test. The only remaining filters would be in the victim's damage path or
the AI's choice to start an attack. Verdict: still `LIKELY`; nothing decoded contradicts the port's
friendly fire. Capture: swing an alerted guard at a second guard standing in its line (not queued: needs
a two-NPC fight; add `session` steps if wanted).

### 2.9 Assassination (back, front, side) eligibility

The context is `DisItemContext_Assassinate` (< `Finisher` < `MeleeAttackPlayer`), player only
(LOG 2026-10-07); NPCs never assassinate (the port already enforces it).
Original target test (0x00C0D8F0, plain words):

1. the target must be an NPC pawn (cast test) that is not in the "immune" state (0x00B4A330 > 0);
2. the target's own check (virtual +0x680 for the player) passes;
3. **awareness**: `m_AssassinateOnAwareness[awareness]` must be true. The awareness comes from the AI
   (0x00B5E250 -> 0x00B0C1E0). Eight entries; the shipped table allows indices 0, 2, 3 and 6 and
   forbids the others (the dump shows only the true ones; the enum names were not recovered:
   `Unverified`). A debug option (0x00C09060) can make every state assassinable;
4. **runners**: refused when the target's movement state is 2 (`FUN_00B79430 == 2`, running) unless
   `m_bCanAssassinateRunners` (off in the shipped tweak);
5. a kill-flag component test (0x00B0FAC0) on the target;
6. range and angle come from the context: `m_fMaxContextRange` **275**, `m_fIdealContextRange` 250,
   `m_fAllowedAngleFromTargetDir` **360** (any direction), `m_fMaxContextHeightDifference` -1 (unlimited),
   reach scaled like the sword (`m_fRayScalePercent` 1.25 for assassinate).

The kill is a move set chosen by relative direction (Front, Back, Left, Right, each with a Fast and a
BendTimeFrozen_Aware variant; `m_DirectionConstraints` give the angles; `m_Generic` when blocked). Adrenaline:
victim health x `m_fAdrenalineMultOnHit` (1.25).
Port (`combat.rs:find_assassination`): back only (victim's back cone +-60 deg from behind), 0.6 x 275 = 165 uu
(invented factor), attacker view within 45 deg of the target, attacker walking, victim not blocking;
no awareness, no runner rule, no front / side kills, no `Tallboy` type.
Verdicts: **Differs**: reach 165 vs 275, only the back, no awareness table. Whether Corvo can kill a
walking *unaware* guard from the front in the original is `Unverified`; the table above says the
awareness state, not the direction, gates it. A multiplayer fork may keep back-only on purpose
(design rule, not a bug) but then the reach should be argued, not 0.6 x 275: F4.

### 2.10 Drop assassination (decoded, `DisItemContext_DropAssassinate`)

Target search (0x00C09A50) and classification (0x00C09900), CONFIRMED by reading:

* the player must be falling: `Velocity.z < m_fMaxAllowedDropJumpVel` (the tweak leaves it at 0:
  any downward velocity);
* a predicted fall over `m_fHitWindowInSeconds` (**4.0 s**) from the current velocity, with the vertical
  speed clamped to at least `m_fMinDropDownVel` (**100**) downward and gravity added, must drop at least
  `m_fMinDropDistToTarget` (0);
* the NPC target (not ragdolled: physics != 10, and virtual +0x678 false) must be below by the
  height difference (pawn reference point minus target point): above `m_fMaxDropDistToTarget` (**350**)
  the type is **TooHighToStart** (no assassination yet), within it **DoNow**;
* a pawn-extent trace (flags 0x20DF) from the pawn must reach that target;
* the candidate list itself comes from the context range `m_fMaxContextRange` 250 scaled like the
  sword (`m_fRayScalePercent` **2.0**, 0x00C09D80) with the awareness gate of 2.9.

Result: adrenaline victim health x 1.5 (`m_fAdrenalineMultOnHit`), plus the Falling Star bonus
(0x00ABBD10, INVENTORY.md). There is no minimum fall speed and no horizontal cone as such.
Port (`find_assassination`, `drop`): `Falling` and `-vz >= 300` (SPECULATIVE) and the target below within
`0.6 x 275 = 165` uu horizontally and 550 uu vertically; checked **only on the tick the swing starts**.
Verdicts: **Differs**: speed floor 300 vs none, vertical 550 vs 350, horizontal 165 vs 250 x scale,
no awareness gate. F5.

### 2.11 Choke (`StatePlayerMasterChoke`)

Original (INVENTORY.md, CONFIRMED structure, LIKELY duration): required time = `Attribute_ChokeTime`
(Corvo 1.5; Strong Arms charm -1.5) + the loop animation length (5.033 s) = 6.533 s; In (0.667) and
Loop run the timer, releasing the choke button in the Loop loses it, the timer at 0 wins (knock-out,
`DisDamageType_Choke`); hold Use `m_fChokeHoldToAutoCarry` (0.2 s) after a win to lift the body. The range and
angle are not in `DisTweaks_Choke` (it carries only look limits: pitch top / bottom and yaw left / right
degrees); the choke target comes from the Assassinate context geometry (U-103, LIKELY).
Port (`takedown.rs`): same timer, phases and exits (Win 2.470, Lose 2.229, Cancel 1.457); the target rule is
the port's back cone (165 uu, +-60 deg), so it inherits the assassination-eligibility differences (F4):
**Match** for timing, **Differs** for eligibility, and the look limits (`m_fChokePlayerControlled*Degrees`)
are not applied.

## 3. Findings (divergences, diagnosis, fix)

| ID | Class | Finding | Evidence | Port fix |
|---|---|---|---|---|
| F1 | port bug | Sword damage is `0.2 / 0.7 x health_max` (14 / 49). Original: `round(mult x (Attribute_MeleeDamage + Attribute_MeleeDamageBonus))` = 10 for Corvo's sword, no big multiplier. U-113 and the KnownDifference cause in `fixtures/oracle/causes.ron` are wrong | 0x00C131C0, 0x00C07C80, 0x00C33DD0; tweaks `Twk_Inv_SwordCorvo` MeleeDamage 10, `m_fAttackDamageMultiplier` 1.0; hook 10 of 50 | In `combat.rs:resolve_sword_hits` replace `ratio * vitals.health_max` with the attacker's `attribute("Attribute_MeleeDamage") + attribute("Attribute_MeleeDamageBonus")` times the context multiplier; add `melee_damage` (default 10) to `CombatTuning`; drop `big_damage_ratio` / the `SwordCounter` multiplier (keep the cause for the kill feed only if the staggered rule stays). Keep the thresholds `0.2 / 0.7` only to pick effects (blood size) in `present_rules`. Update U-113 to "PvP mapping only" and re-file the oracle cause |
| F2 | port missing | NPC melee damage is difficulty-indexed (10 / 15 / 20 / 30 for the elite guard sword), 15 on Normal | `Twk_Inv_SwordEliteGuard` attributes_5; hook `NPCSlash` 15 | NPC pawns use their sword's `MeleeDamage` (Normal column: 15). Needs the per-NPC sword tweak (`Twk_Inv_SwordEliteGuard` and the City Guard / Overseer / Thug siblings: not all readable, so Unverified for them) |
| F3 | port missing | Offscreen reduction: an NPC melee hit on the player from outside the view loses 50 % (7.5 -> 8) | `DishonoredPlayerPawn` default `m_fOffscreenMeleeDamageReduction` 0.5, 0x00C33DD0 | In the damage step for `PawnKind != Human` attackers: if the victim is a Human (player) and the attacker is outside the victim's view (use the camera frustum, or the 90-degree half-angle in front of the view as a placeholder), `dmg = max(1, round(dmg * 0.5))`. Rounding: nearest, halves up. Mark Unverified until `combat/melee_guard_offscreen` is captured (what sets `m_bEffectivelyOffscreen`) |
| F4 | port differs | Assassination: original range 275 (reach-scaled), any direction, gated by the target's awareness state and not running; port: back cone only, 165 uu | 0x00C0D8F0, 0x00C09060, tweaks `DisTweaks_Assassinate` | Replace the `0.6 x range` with the reach function (`reach(a, t)` with `m_fRayScalePercent` 1.25) or keep the shorter range as a documented multiplayer rule; add the awareness gate (needs the AI awareness enum: recover the names of indices 0 to 7 via `oracle index fn 0x00B0C1E0` and `class DisAssassAware`); refuse running targets |
| F5 | port differs | Drop assassination: no minimum fall speed, target within 350 below, found by a pawn-extent trace; port needs 300 uu/s, 550 uu, cone 165, only on the press tick | 0x00C09900, 0x00C09A50, `DisTweaks_DropAssassinate` | `drop` = falling (`vz < 0`) and target below within `m_fMaxDropDistToTarget` 350 (height difference) and the reach (250 x ray scale 2.0) and a clear pawn-box trace; evaluate each tick while the attack button is held / pressed during the fall |
| F6 | port missing | Multi-target swings, sword clashes (`Versus`), environment rebound, chain attacks | 0x00C13640 loop, `eDisMeleeResponse` | Optional for the fork; a swing should hit every pawn the sweep meets once (`m_AlreadyHitActors`); a clash if two attackers' versus zones overlap within `m_fVersusAngle` 20 |
| F7 | port speculative | Stagger 1.0 s, blocked recoil 0.4 s, NPC swing phases 0.13 / 0.21 / 0.26 | `tuning.rs` comments | Measure with the hook (`measure sword --block`, `--defend`), read the NPC clip notifies (`Ply_Sword_Versus_as`, NPC attack sets) |
| F8 | port bug | `NPCJumpAttack` and `NPCBash` (multiplier 0) deal damage in the port | tweak dump | In the NPC swing, skip damage for the jump attack and bash move kinds; bash instead raises the block-break chance (`m_BlockBreakRate_Min/Max` 0.3 to 0.6) |
| F9 | oracle limitation | `combat/sword_hit_dummy` expects 10 of a 50-health guard but the port's placed NPC has 70: the port's NPC health ("by rule") is a second, separate difference (guard HealthMax 50 measured; `Pwn_Guard_Base` unreadable) | HOOK_TRIAGE, `runner.rs:158` | Give guards 50 (the role archetype) so kill counts match: 5 hits either way (70 / 14 and 50 / 10), but damage numbers differ |
| F10 | known / open | Friendly fire (U-660) | 2.8 | no change |
| F11 | open | Held attack (`m_fCurHoldAttackTime`) and chain input window not decoded | | decode `DisItemContext_MeleeAttackPlayer` slots 73/74/97 (0x00C0E710, 0x00C17640, 0x00C15060) |

### 3.1 Port status (2026-10-08)

Re-read before porting: 0x00C131C0, 0x00C07C80, 0x00C33DD0, 0x00BBEF40, 0x00C24020, 0x00C0D8F0, 0x00C09060,
0x00C090D0, 0x00C146C0, 0x00BBA970, 0x00C09810, 0x00C09900, 0x00C09A50, 0x00C14960 (static index); tweaks with
the Boyle script packages (`TWEAK_PKGS`); `DisGlobalEnums.EAIAwareness` (enum_dump of DishonoredGame.upk);
`Ply_Sword_Assassination_as` notifies. Corrections to the text above found on the way:

* The rounding in 0x00C131C0 (`-(ROUND(-0.5 - 2d) >> 1)`) is UE3 `appCeil`, not plain rounding (7.25 -> 8).
* `m_bEffectivelyOffscreen` is set when the context starts (slot 98, 0x00C24020): the target is the local
  player pawn and 0x00BBEF40 is false for the attacker. 0x00BBEF40 tests `DishonoredNPCPawn.m_bRenderedByPlayer`
  (+0x964 bit 5) and the view plane. The damage function re-tests 0x00BBEF40 at the hit.
* `EAIAwareness`: 0 Unaware, 1 AwareOfPlayer, 2 Surprised, 3 Suspicious, 4 Fearful, 5 InCombat, 6 Begging,
  7 Choked. The assassinate table allows Unaware, Surprised, Suspicious, Begging.
* The drop context (`DisItemContext_DropAssassinate` < `DisItemContext`, not a subclass of Assassinate) never
  calls 0x00C0D8F0: **no awareness gate** for drops (2.10's last bullet is wrong). Its target (0x00C09A50) is
  the context's melee-info target, else the first pawn of a pawn-extent sweep (flags 0x20DF) along the
  predicted fall (0x00C09810: velocity x 4 s with Z at least 100 down, plus gravity), else straight down.
  The height difference is the player's base (location - CollisionHeight) minus the target's point.
* 0x00BBA970 picks the move set from the attacker's offset in the victim's frame (dominant axis): front,
  left, right, back. Clip exits: Front 2.062, Left 2.271, Right 2.281, Back 1.886 s; drops 1.900 (Back 1.973).
* Guard `HealthMax` is decodable with the level script packages: every `Pwn_EliteGuard_*` pawn of the Boyle
  levels resolves to 50 (`pAttributeTweaksNormal`); Lord Shaw 10, tallboys 150, weepers 30, aristocrats 10-11.
* The elite sword's `NPCJumpAttack` multiplier is 0 (class default 1.0); every `NPCBash` is 0.

| ID | Status | Port change |
|---|---|---|
| F1 | ported | `combat.rs:melee_damage` + `app_ceil` (hit handler): `attack_damage_multiplier x (Attribute_MeleeDamage + Attribute_MeleeDamageBonus)` through `powers::attribute` (`InventoryConfig::sword_melee_damage` 10 / `melee_damage_bonus` 0, loaded in `items_config.rs` from the sword's `m_pAttributeTweak[1]` and `Twk_Pawn_Corvo`). `big_damage_ratio` no longer scales damage (kept with `normal_damage_ratio` as the contact-effect thresholds); `SwordCounter` is no longer emitted. causes.ron rule retired, U-113 is the PvP mapping only. |
| F2 | ported (Normal) | `CombatAiConfig::npc_sword_damage` [10, 15, 20, 30] (`ai_config.rs` reads the elite sword's attributes when the level packages are loaded), indexed by `DIFFICULTY_NORMAL` (the port has no difficulty setting). One sword for every NPC (other archetypes' swords not in loadable packages). |
| F3 | ported, LIKELY | `SwordState::offscreen_to` set at the swing start (the human the NPC faces that does not see it), `combat.rs:seen_by` (75-degree 16:9 view, melee trace) stands in for `m_bRenderedByPlayer`; `x (1 - offscreen_melee_reduction[Normal])`, at least 1 (DefaultPlayer.ini). |
| F4 | ported (NPC targets) | `find_assassination`: reach `context_reach(275, 1.25)`, any side, `assassinate_on_awareness[PlayerState::ai_awareness]`, runners refused (`running`: Sprint at speed), the side's clip as `SwordState::lock_time`. `ai_awareness` is written by `Crowd::write_awareness` from `Brain::awareness` (LIKELY mapping of the port's states); brainless pawns keep AwareOfPlayer. Players (PvP, the original has none): from behind, not blocking, same reach. The target selection (`m_PawnInfo_Melee`) is not decoded: the view-within-45-degrees stand-in stays. Choke target unchanged (U-103). |
| F5 | ported | `find_drop_assassination` / `drop_ready`: no fall-speed floor, 0 < height <= 350, pawn trace, fall-path sweep, evaluated every tick of a fall while attack is held. Target point = the target's location (the exact point of 0x00B68010 not decoded). |
| F6 | multi-hit ported; rebound, clash, chains open | `SwordState::hit_ids` (`m_AlreadyHitActors`, up to 8): one swing hits every pawn its sweep meets once, nearest first, stopping at a block or parry. The rebound's consequence and the versus rule are not decoded. |
| F7 | open | No clip mapping for the parried / blocked attacker (`Ply_Sword_Hit_as` / `Versus_as` exits listed, the action-to-clip link not read). |
| F8 | ported | `MeleeContextConfig::damage_multiplier` (elite JumpAttack 0, Bash 0 by class); the sim takes it from the swing's `SwordState::npc_move`. The port has no Bash move (no block-break). |
| F9 | ported | `spawn_npc` gives guards `CombatAiConfig::guard_health_max` 50; `powers::health_max` uses it for `PawnKind::Guard`. Civilians keep 70. |
| F10 / F11 | open | unchanged. |

Multiplayer note: with F1 the player-versus-player sword hit becomes 10 of 70 (7 hits) instead of 14
(5 hits). That is the original's number for any attacker with Corvo's sword; if the fork wants 14 it
should say so as a rule (`melee_damage = 14`), not through a health ratio.

## 4. Scenarios (fixtures/oracle/scenarios/combat/) and captures

Existing: `sword_hit_dummy` (timing 0.325 s, damage 10, blood), `block`, `guard_attack` (15), `spring_razor`.
Added (all `Unmeasured` or `Static`; none judged until the original is captured):

| Scenario | Rule | Original reading | Port today |
|---|---|---|---|
| `melee_reach_d160 / d230 / d260 / d300` | hit vs distance (reach sweep) | 210 from the eye + radius: hit through about 245 (design reading) | hits through about 275 (160, 230, 260 hit; 300 not) |
| `melee_lateral_60 / 90` | sweep width | within about 65 uu | same bound (60 hit, 90 miss) |
| `melee_guard_offscreen` | NPC damage + offscreen | 8 on Normal | 14 |
| `melee_assassinate_back_d120 / d220 / d300` | back assassination range | kill through 275 | kill only through 165 (d120) |
| `melee_assassinate_front` | front kill of an unaware pawn | unknown (awareness table) | normal hit |
| `melee_drop_assassinate_h250 / h450` | drop rule | h250 kills at the press, h450 does not (350 limit) | neither (needs 300 uu/s at the press) |
| `melee_choke_time` | choke duration | 6.533 s | 6.533 s |

Capture queue lines are in `fixtures/oracle/capture_queue.txt` under `# melee`, plus four `session`
reads (the offscreen reduction table on the live pawn, and dumps of the assassinate / drop-assassinate /
attack tweak objects so the awareness table and the unnamed fields are confirmed on the running game).
The scenario runner was not in the shipped `oracle.exe` during this pass, so **none of the new RON specs
has been parsed or run**; they follow the field names of the existing specs and should be run first
(`oracle scenario combat/*`) to catch syntax errors.

## 5. Unverified (do not treat as fact)

* the enum names of the awareness indices (which of the 8 states permit assassination);
* effective sword reach and width on a pawn mesh (the 210 uu context range versus the sweep extent);
* who sets `m_bEffectivelyOffscreen` and the 0x964 test in 0x00C33DD0;
* the meaning of `m_fBlockAngle` (half or full cone) and every duration after a parry or block;
* guard `HealthMax` for each archetype other than the doorman (50); City Guard / Overseer / Thug sword
  damage values (the packages that define them were not loadable);
* chain attacks, held attacks, sword clashes, NPC swing clip timing;
* friendly fire (U-660).
