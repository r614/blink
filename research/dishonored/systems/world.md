# World interaction, items and economy: the core model

Scope: chaos, doors and usables, breakables, pickups, inventory capacities, the attribute system
(bone charms, upgrades, passive powers), rune costs, Piero's workshop prices, save restrictions,
and the largest unhandled placed-actor classes. Evidence levels as elsewhere: CONFIRMED (read
from the decompiled code or game data), LIKELY, UNVERIFIED (needs a capture; never judged).
Addresses are virtual addresses in the GOG exe (sha256 `70da9613...`); data values come from the
tweak packages (`Startup.upk`, `L_Pub_Craftsman.upk`), the INIs and the Kismet of the maps.
Findings are in our own words.

Contents: 1 Attribute system, 2 Chaos (the game calls it "Darkness"), 3 Doors and usables,
4 Breakables, 5 Pickups and inventory, 6 Runes and powers, 7 Piero's workshop, 8 Save
restrictions, 9 Unhandled actors and Kismet ops, 10 Findings, 11 Scenarios and captures.

---

## 1. Attribute system

**What it is.** Every gameplay number that charms, upgrades or powers can change is an *attribute*
(`DisAttributes`, vtable 0x0118E1B8): a base value plus a set of modifiers. Pawns copy the base
values from the tweak (`DisAttribute`: `m_fBaseValue` and one base per difficulty, Easy / Normal /
Hard / VeryHard). Modifiers have a type (`eDisAttributeModifierType`: `AddVal`, `AddBasePercent`,
`SetVal`) and an optional lifetime (timed modifiers, e.g. the rat-bite slowdown).

**Formula (CONFIRMED, 0x00C863F0, the cached-value update).** Start with `value = base`. Walk the
modifiers in the map's iteration order (a TMap; sparse-array order, LIKELY insertion order, keyed
by source):
- `AddVal`: `value += v`
- `AddBasePercent`: `value += v * 0.01 * base` (percent of the BASE value, not of the running value)
- `SetVal`: `value = v`

Then, only when the attribute enforces a range (`DisAttribute_RangeLimits`), clamp to
`[min, max]`. The result is cached and recomputed when a modifier changes. Only four attributes
carry range limits in the tweaks (`m_MinMeleeHitQuality`, `m_MaxMeleeHitQuality`,
`m_HealthMax_Limits`, `m_Visibility_Limits`); on the player pawn only Visibility is enforced,
`[-1, 1]`.

**Difficulty bases.** Almost every base is the same on all four difficulties. The exceptions in
`Twk_Pawn_Corvo_Release.pAttributeTweaks*`: `HealthRegenLimit` 20 on Easy and 10 on the others,
`WhiskeyFireResilience` -2 / -4 / -6 / -8 and `MusicDamageResilience` 0.96 / 0.93 / 0.90 / 0.86.
(INVENTORY.md's ammo capacities are the Normal set.)

**Passive power levels are NOT cumulative (CONFIRMED, 0x00ACE270).** Setting a power's level
first removes the modifiers of the CURRENT level, clamps the new level into
`[-1, levels - 1]`, then applies the modifier list of the NEW level only. That is why Celerity
level 2 and 3 both list `+1300 JumpZ_PowerJump`: the list of a level is the full state of that
level. Setting any level >= 0 also counts the `PowersAcquired` stat (id 11).

**Bone charms.** `DisTweaks_WhaleBoneCharmList.m_Charms` (29 entries; `m_ScriptedCharms` holds 14
more for scripted effects): each entry is one attribute with a list of LEVELS, each level having
a name and a value (`m_fValue`). Charms with two levels are "I" and "II" variants: Healthy
Appetite 10 / 20, Unnerving Target 0.05 / 0.10, Robust 0.125 / 0.25 (health potion potency),
Spirited 0.10 / 0.20 (mana potion potency), Twist of Fortune 0.10 / 0.20, Whirlwind 0.10 / 0.20.
A placed charm pickup (`DisWhaleBoneCharm`) carries `m_Effect`, a charm effect id (CORRECTED in
10.1 F-7: an index into `m_Charms`, or `m_ScriptedCharms` from 10,000,000 up; the level comes from
taking the same effect again) (-1 = none; the pickup refuses to be taken, vslot 240 0x00A19D50); Kismet sets it with
`DisSeqAct_SetBoneCharmEffect` (vslot 241, 0x00A22970; 12 ops in the maps). Taking it calls the
engine's charm table (0x00ABBA90). Slots: `m_MaxActivatedBoneCharmCount` 3 plus the
`Attribute_MaxActivatedBoneCharmCountBonus` of the owned upgrades: Bone Charms Capacity 1 (+1),
2 (+2), and the four statue upgrades (`StatueFish`, `StatueRat`, `StatueWhale`,
`StatueWolfhound`, +1 each, found in the world) = 3 + 3 + 4 = 10 (`MaxWhaleBoneCharmSlots`).

**Port.** `dishonored_sim::powers::attribute_for` applies the current level's list of every
passive power in order (matches the level rule), then the sum of owned upgrades' attributes, then
`values.first()` of every equipped charm, all by the same three formulas. Matches except:
- charm level II values never apply (F-7);
- order: the original orders by modifier insertion, the port by (passives, upgrades, charms). It
  only matters when a `SetVal` meets an `AddVal` on the same attribute (none in the shipped data:
  the only `SetVal`s are Celerity's fall-speed limits and Vitality's regen delay);
- no range clamp (matters only for Visibility, which the port does not model as an attribute);
- difficulty bases: the port is Normal-only.

---

## 2. Chaos ("Darkness")

### 2.1 The model (CONFIRMED)

Chaos is an integer SCORE kept by one `DisDarknessManager` (class record vtable 0x01187CF8,
fields `m_DarknessScore` +0x38 and `m_DeedCounters` +0x3C, an `int[]`), owned by the game-wide
container (`DAT_0145F628 + 0xA94`, getter 0x00AA15B0). The score never goes below 0 and persists
across missions (it is serialized, 0x00C52CA0). The visible "chaos level" is derived from the
score by a threshold table; maps and conversations read the level.

**Score changes, only two ways:**
1. `Add(n)` (0x00C41B50): `score = max(0, score + n)`. Called by `DisSeqAct_AddDarkness`
   (0x00BB3360; `m_Value`, optional `m_Text` HUD line) and by the cheat `adddarkness N`
   (0x00B753A0; the debug menu's "Chaos Level +/-1" is `adddarkness +/-30`, "Full Chaos" 300).
2. Deeds (below).

**Level from score (0x00C487D0).** `m_DarknessLevels` of the player tweak
(`Twk_Pawn_Corvo.Twk_Pawn_Corvo_Release`, 11 entries). The level is the largest index `i` with
`score >= levels[i]`:

| level | 0 | 1 | 2 | 3 | 4 | 5 | 6 | 7 | 8 | 9 | 10 |
|---|---|---|---|---|---|---|---|---|---|---|---|
| score at least | 0 | 12 | 48 | 72 | 96 | 114 | 138 | 162 | 168 | 200 | 390 |

(Read from the export bytes of `Startup.upk` export 3423; the same array is in the base
`Twk_Pawn_Corvo` export 3422. U-W1: confirm with `session read`.)

**Deeds (0x00C4D2C0, "NotifyDeed").** `DisTweaks_PlayerPawn.m_Deeds` has 9 entries, each:
`{stat, threshold, value, victim tweak filter, excluded tweaks}`. When the player stat system
increments statistic S by `a` with an optional victim (the victim NPC's tweak object), the
manager, for every deed whose stat is S and whose filter accepts the victim: adds `a` to that
deed's counter; when `counter >= threshold` it adds `value` to the score (clamped at 0), compares
the level before and after (a rise would show `m_DarknessLevelIncreasedMessage`, which is the
empty string in the shipped tweak, so nothing is shown), and resets that counter to 0 (no carry
over). Counters are per deed, are serialized, and are cleared at mission end
(`DisSeqAct_NotifyMissionEnd`, 0x00B8B110 -> 0x00C4D500). The score is kept.

Filter: when the stat call carries a victim tweak, a deed is skipped if the victim is, or inherits
from (archetype / parent chain, 0x00C6F450), any tweak in the deed's exclusion list, and is
skipped unless its own tweak name is `None` or the victim is / inherits from it. A call without a
victim tweak bypasses the filters.

The shipped deeds (all `value = 6`):

| # | stat | threshold | victim tweak (and ancestors) | notes |
|---|---|---|---|---|
| 0 | NumKills | 1 | `Pwn_Civ_Base` | civilians; 23 exclusions: the weeper family (Weeper_Base, WeeperAristo, WeeperElite, WeeperMiddle, WeeperOverseer, WeeperThug, WeeperCity, Dead_NPCs weepers, GrannyRags, E3 weeper/noble, Bridge/Griff weepers) |
| 1 | NumKills | 4 | `Pwn_Thug_Base` | |
| 2 | NumKills | 4 | `Pwn_Guard_Base` | |
| 3 | NumKills | 4 | `Pwn_Overseer_MTall_1` | |
| 4 | NumKills | 4 | `Pwn_Assassin_MTBase` | excluding AssassinIntro and the Executioner combatant |
| 5 | NumKills | 4 | `Twk_Pawn_Tallboy.Pwn_Tallboy_Wood` | |
| 6 | NumKills | 4 | `Pwn_Weeper_Base` | |
| 7 | NumAlarmsTriggered | 5 | none | |
| 8 | NumCorpsesDiscovered | 5 | none | |

So: each civilian kill is +6; four kills of any other listed class are +6 (1.5 per kill on
average); five alarms are +6; five discovered corpses are +6. Level 1 needs 12 points (two
civilians), the first "high chaos" gates are 1-8 levels (section 2.3).

**What feeds the stats (CONFIRMED).** The stat increment is 0x00AB9560 (`IncrementStat(stat,
amount, victim tweak, ...)`), which also calls the deed manager at its end (0x00AB9909).
- **Kill report (0x00B73990)**, called from the NPC pawn's death handlers (0x00B77B40,
  0x00B78170, 0x00B7BF50): nothing if the victim is null; `killer == victim` counts only
  `Suicide` (16); otherwise `NumKills` (0) is incremented, with the victim's tweak, only for kills
  credited to the player side that are not in the excluded situations (a victim of the player's own
  faction in some modes, a flag on the killer pawn at +0xCD0 bit 0). On top: `UnawareKill` (12) for
  a kill with awareness below 1, `SameFactionKill` (13), `GrenadeThrowbackKill` (24),
  `HostileKill` (28) or `CivilianKill` (30) according to which of two tweak lists in the stats
  tweak (+0x5A8 / +0x5D8) the victim is in. (The exact side test is LIKELY, U-W2.)
- **Knock-outs do not count as kills**: the sleep / choke report (0x00B738F0) increments only
  `HostilePutToSleep` (29) / `CivilianPutToSleep` (31). So non-lethal play adds no chaos. The
  port's `MissionStats::knocked_out` split is right.
- **NumAlarmsTriggered (3)**: 0x00A3D080 (an alarm trigger firing). **NumCorpsesDiscovered
  (4)**: 0x00A98950, only the FIRST time a given corpse is seen (a set of discovered corpses is
  checked, so re-seeing the same body adds nothing).
- Rats and bodies: no separate chaos term. Rat kills presumably count as kills of the victim's
  tweak (the swarm's damage is credited to the player, LIKELY, U-W2); carrying or hiding a body
  does nothing; only discovery by an NPC counts. `NumRatsSummoned` (6) and `NumExplosions` (5) exist as stats but have no deed.

### 2.2 What chaos changes

Only Kismet and conversations read it:
- `DisSeqVar_DarknessLevel` (36 uses) and `DisSeqVar_Darkness` (6): Kismet variables (0x00B8BAA0
  and 0x00B8B990 read the level / score into the variable). `DisSeqAct_AddDarkness` (41 uses).
- `DisConv_CompareDarknessLevel` (38 uses, 0x00CA8FE0): output 0 equal to `m_LevelToCompare`,
  output 1 current level less, output 2 current level greater (so branches of conversations).
- `DisSeqAct_MissionStatsTracking.m_ChaosLevelThreshold`: the mission-stats screen's High / Low.
- `DefaultGame.ini` `m_MissionChaosThreshold[mission]` (1,1,2,3,4,5,6,7,7 for missions 0-8):
  only feeds the platform rich-presence chaos field (`level >= threshold` -> 2 else 1,
  0x00A01DE0 / 0x00A07740). It is NOT what picks the high-chaos maps.
- The high / low chaos versions of the levels are picked by Kismet in `DishonoredGameFull_P`
  (`fromHub*_to*`): `SeqCond_CompareInt(DarknessLevel, named int)` -> story flag. The named ints:

| gate | level | gate | level |
|---|---|---|---|
| Overseer | 1 | Boyle | 4 |
| Pub1 | 2 | Pub3 | 4 |
| Brothel | 2 | Tower | 5 |
| Pub2 | 3 | Pub4 | 5 |
| RoyalPhys (Bridge) | 3 | Flooded, StreetSewer, Pub5 | 6 |
| PubAttack | 7 | Lighthouse (high / very high) | 8 / 10 |

`L_BoyleStreet_Ext_Script` compares the same Boyle gate (4) to show / hide the high-chaos props.
Mercy routes subtract: Boyle's mercy path ends with `AddDarkness -20` (KISMET.md); the
hub script has `-6` / `+6` ops and a `+100000` cheat op.

### 2.3 The port

- `dishonored_mission::runtime::MissionStats::high_chaos(ratio)`: chaos is "High" when
  `(hostiles + civilians killed) / total_npcs > 0.2` (`GameSettings.high_chaos_kill_ratio`,
  documented as an approximation in MISSIONS.md). Only the HUD line reads it
  (`dishonored_runtime/src/gfx_hud.rs:838`).
- `kismet_host.rs:1390` `DisSeqAct_AddDarkness`: `self.st.chaos += v` (no clamp at 0, no `m_Text`),
  and nothing reads `st.chaos`.
- `dishonored_kismet::runtime::var_value`: `DisSeqVar_DarknessLevel` / `DisSeqVar_Darkness` are
  `SeqVar_Int` subclasses and read `IntValue` (always 0 in the data), so every Kismet chaos
  comparison sees level 0.
- `dishonored_conversation`: no comparison node is evaluated (`DisConv_CompareDarknessLevel`).
- Bone charms, kills, alarms, corpses do not touch a chaos score.

Verdict: DIVERGES (F-1 .. F-4). Everything chaos-gated (which high-chaos map or prop set loads,
conversation branches, mission-stats ending) is wrong or constant today.

**Fix the port needs (precise).** Add a `Darkness { score: i32, deed_counters: Vec<i32> }` to the
player's persistent state (saved, replicated), with `add(n) = max(0, score + n)`,
`level() = largest i with score >= LEVELS[i]` (table above, from the tweak, not hard-coded), and
`notify_deed(stat, victim_tweak, amount)` implementing the loop above with the tweak tables
(`m_Deeds`: stat, threshold, value, tweak name + ancestors, exclusions). Hook the stat events the
sim already raises: `NumKills` on a lethal kill credited to a player (with the victim's NPC tweak
name chain), `NumAlarmsTriggered` when an alarm fires, `NumCorpsesDiscovered` on the first
sighting of each body. Reset `deed_counters` when the mission ends, keep the score. Then: make
`DisSeqAct_AddDarkness` call `add`, make `DisSeqVar_Darkness` / `DisSeqVar_DarknessLevel` read
`score` / `level()`, evaluate `DisConv_CompareDarknessLevel` (0 equal / 1 less / 2 greater), and
drive the HUD High / Low from `level() >= m_ChaosLevelThreshold`. Drop `high_chaos_kill_ratio`.

---

## 3. Doors and usables

### 3.1 The door state machine (CONFIRMED)

`DisDoor` (instance 0x444, ctor 0x00A3FAE0, vtable 0x010E9DB0) extends `DishonoredUsableObject`
(`m_bLocked`, `m_bEnabled`, `m_State`, `m_iActiveStage`...). `m_DoorState` (+0x311):
`DDS_Closed` 0, `DDS_Opened_CW` 1, `DDS_Opened_CCW` 2. Stages (`DisTweaks_Door`): 0 OpenCCW,
1 OpenCW, 2 CloseCCW, 3 CloseCW, 4 SlamOpenCCW, 5 SlamOpenCW.

- **Use** (vslot 242, 0x00A37C10), already in PLAYGROUND.md: closed + user on the side `0x00A2D2E0`
  says -> stage 0 / state 2 or stage 1 / state 1; open -> stage 2 (from state 1) or 3 (from
  state 2), state 0. When the user is the local player it also flags `m_bOpenedByPlayerOrKismet`
  (0x318 bit 1 = state != closed) and `m_bClosingDueToPlayerOrKismet` (bit 3). When the user is
  the local player it also scans the actors around the door and collects those with a clear line
  to it (witnesses of the door being opened; consumer 0x00A2ED20, not decoded, U-W11).
- **Kismet** `DisSeqAct_Door::Activated` (0x00B91D70): the first input with an impulse (0 Open
  CW, 1 Open CCW, 2 Close, 3 Lock, 4 Unlock) applied to every `DisDoor` target. Open
  (0x00A1A450): ONLY from closed AND `CanUse` (0x00A4E2E0); input 0 -> state 1 (stage 1), input 1
  -> state 2 (stage 0). Close (0x00A1A4B0): only from open and CanUse; stage 2 / 3, state 0.
- **Lock / Unlock** (usable vslots 238 / 239, 0x00A4E870 / 0x00A4E910, also reachable by
  `DisSeqAct_Lock`, 0x00B8F730, on ANY usable: its input 0 locks, 1 unlocks): set / clear
  `m_bLocked` and raise a usable event (0x00BC7B30 with a lock / unlock flag; the usable also has
  a `UOSO_UsedButLocked` special output for a use while locked). The door's override
  (0x00A32490 / 0x00A324B0) also switches the path blocker (`ArkAvoidable`, 0x00A2D430): a locked
  door blocks AI paths when the door tweak asks for it (tweak +0x200).
- **Reset to the level's initial state** (0x00A1A700 / 0x00A32170): `m_DoorState =
  m_InitialDoorState`, stage 0 / 2 / 3 for states 0 / 1 / 2.
- **CanUse (0x00A4E2E0)** is NOT the lock test. It says no when the object is deleted, when it is
  disabled (`m_bEnabled`, bit 1 of +0x2B0), and when `m_State != Idle` (a transition is running)
  unless the active stage is a hold-to-use stage (stage flag `m_bHoldToUse`) or the tweak flag
  `m_bKeepFocusedInTransition` (+0xF4 bit 4) is set. PLAYGROUND.md read `+0x2C4` as `m_bLocked`; it is `m_State`. The lock flag is bit 0 of
  +0x2B0 and is consumed by the stage selector (0x00A1A750): stage type 0 (the locked-attempt
  rattle, `m_RattleCCW/CWAnimName`) is allowed only when the door is closed AND locked; the
  opening stages are never selected while locked (UNVERIFIED in detail: the exact gate between
  Use and the locked path, U-W3).
- **Interaction prompt (0x00A4E3D0)**: type 3 (cannot use) when CanUse fails, or when the tweak
  has a `m_RequiredAbstractItem` and the player holds none; type 4 normal, 0xC when the tweak
  flag `m_bInteractableAsDoor` (+0xF4 bit 6) says hold. A locked door with `m_UnlockInteractText`
  ("`GBA_Use` Unlock") means the player can try a key.
- **Keys**: locked doors carry `m_MatchingKeys` (strings) and the player has a `DisKeyRing`
  (`m_Keys`: name, `DisTweaks_Key`; `m_UseMessage` "You used: `k"). Boyle Ext has 4 locked metal
  doors with a key among the 8 doors of the `L_Boyle_Ext_Audio` sublevel (DisDoor_0, _2, _4, _8).
  `DisSeqAct_AddKey` (15 uses) adds a key. The check that
  matches a key and unlocks is NOT decoded (U-W4).
- **NPCs and doors (door Tick, vslot 103, 0x00A2C970)**: pawns that registered to use the door
  within `m_fNPCClosenessToOpen` 200 uu of a closed, usable door make the door open (the door is
  "used" by that pawn). Pawns whose path goes through are tracked with a front / behind / moved-
  through state (`EDoorToCloseState`); once a pawn has moved through and no pawn is within
  `m_fNPCClosenessToNotClose` 400 and the player is farther than `m_fPlayerClosenessToNotClose`
  400, and the door was not opened by the player or Kismet, the door closes. (LIKELY.)
- **Windblast**: `m_fRattleWindblastForce` 0.3 (rattle) and `m_fSlamWindblastForce` 0.5 (slam
  open, stages 4 / 5) are door tweak fields; the consumer is not decoded (U-W5).
- Defaults: `m_bRestrictPlayerApproach` false, `m_fHolePeekingActivateTime` 0.3, `m_fRepathRadius`
  500.

### 3.2 Port

`dishonored_multiplayer::mission::door_use(state, location, axis, user)`: refuses a locked door,
closed -> state 2 if `(user - location) . axis < 0` else 1, open -> 0. `kismet_host` `DisSeqAct_Door`:
input 0 -> state 1, 1 -> state 2, 2 -> 0, 3 locks, 4 unlocks. Matches the state transitions.
Differences: Kismet Open applies from ANY state (original: only from closed, and CanUse); a locked
door is refused by Use (the original lets a key unlock, F-9); `DisSeqAct_Lock` (319 uses) is
not run (F-10); no transition time (the state flips at once; known); NPC auto open / close is
not modelled for original doors (F-11); stage-based usables (lockers, bins) wrap through stages.

---

## 4. Breakables

`DisSkeletalBreakable` (base of every usable) and `DishonoredBreakable` (static, KActor) share the
tweak shape: `m_Health` (-1 = unbreakable by damage), `m_DamageThreshold` (class default 5:
smaller hits are ignored), `m_pImmuneToDamageTypes` / `m_pVulnerableToDamageTypes`, a break-steps
table (`DisUsableObjectBreakSteps`, 551 placed; steps by remaining health fraction), loot
(`DisLootSettings`, dropped when the final step breaks) and `m_bAttractsNPCs`. Damage enters
through the `TakeDamage` virtual (+0x3A4, called by the native `TakeDamage_Native`
0x009ED600) which updates `m_CurHealth` / `m_NextStep`. `DisSeqEvent_BreakableBroken` (233
uses) fires its three outputs "Broken 100% / 66% / 33%" as steps are reached. Windblast, sword and
projectiles reach it through the damage path. The port imports breakable collision but has no
breakable object (no health, no steps, no event). The step thresholds and the damage rule are
not decoded (U-W6). Port needs: a breakable actor with health, threshold and break steps, the
three-output Kismet event, loot drop.

Update (2026-10-08): `DishonoredBreakable` has its own damage function (vslot 235, 0x00a26a90;
`DisSkeletalBreakable`'s is 0x00A283A0, physics.md 3.5): steps are `DishonoredBreakableStep`
(`m_HealthThreshold`, `m_fStepTimer`, sound, AI noise, explosion), the wanted step is applied by the
tick (0x00a3b0d0 -> 0x00a3a5f0) and fires output `Num(m_Steps) - step - 1`. Ported for explosion
damage, see 10.1 F-12.

---

## 5. Pickups and inventory

### 5.1 Capacities and counts (CONFIRMED, INVENTORY.md plus this pass)

Ammo capacities on Normal (`Twk_Pawn_Corvo_Release.pAttributeTweaksNormal`, copied by
0x00B56880): bullets 10, explosive bullets 10, bolts 10, sleep darts 10, incendiary bolts 10,
Spring Razors 5, grenades 5, sticky grenades 5. Upgrade attributes add: Ammo Pouch 2 / 4 each
+10 bullets, Quiver 2 / 4 each +10 bolts (`Attribute_ArrowCapacity`), Grenade Belt +4 grenades and
+4 sticky, Spring Razor Pouch +4. Elixirs: 40 health, 50 mana, at most 10 each (add clamps to
`[0, max]`, an elixir pickup is refused at the maximum). `AddAmmo` adds `min(n, capacity -
count)`; `RemoveAmmo` is all-or-nothing. Interaction reach 200.

Port: matches (`inventory.rs`: `add_ammo`, `capacity`, `add_elixirs`, `remove_ammo`; scenarios
`inventory/ammo_cap_grenade`, `ammo_cap_grenade_belt`, `elixir_cap` pass).

### 5.2 Pickup types

| class | gives | notes |
|---|---|---|
| `DisAbstractItemPickup` (1160) | `m_Quantity` of `m_pAbstractItem` | coins, runes, keys, blueprints; the quantity is per instance (Boyle coins 10-30) |
| `DisAbstractItemPickupNote` (359), `...AudioLog` | a note / audio log into the journal | |
| `DisElixirHealth` / `Mana` (149 / 117) | one elixir | refused at 10 |
| `DisStatPickup` (605) | food (heal), ammo, mana | `(1 + potency) * m_HealthChange`; ammo rolled at level start; taken while it can heal or any ammo slot has room; Scavenger charm may add a round |
| `DisWhaleBoneCharm` (24) | the charm named by `m_Effect` | section 1 |
| `DishonoredInventoryPickup` | an inventory item | |

Runes: one `Rune_AbsItm` per rune pickup (quantity 1); the workshop sells a rune for 500 coins.
Coins: no cap is known (UNVERIFIED, U-W7). Stealable pickups are carried by NPC spawners
(`m_pStealablePickup`).

Port: matches for abstract / elixir / stat pickups. Differences: bone charm identity (F-7),
`DisAbstractItem` keys / notes are held by name with no key ring (F-9), the Kismet inventory ops
`DisSeqAct_AddInventoryItem` (26), `AddKey` (15), `AddPower` (11), `GiveUpgrade` (2),
`ModifyElixirCount` (3), `GetAbstractItemQuantity` (10) and event `PlayerInventoryChanged` (1) are
not run by the Kismet host (the mission `Loadout` covers only the Playground) (F-17).

---

## 6. Runes and power levels

`DishonoredPowersComponent`: 6 active powers, 4 passives, `MaxPowerLevel` 2. Buying (journal
`PowerBuy`, 0x00A00EC0 -> 0x00BCA8D0): from the current level `L`, while `L < target` and
`cost[L] <= runes`: set level `L+1` (0x00ACE270), spend `cost[L]` runes. So a two-level purchase
costs both steps in sequence and stops where the runes run out. Costs (rune cost to go from level
`L` to `L+1`):

| power | L0->1 | L1->2 | power | L0->1 | L1->2 |
|---|---|---|---|---|---|
| Blink | 1 | 3 | Vitality | 1 | 3 |
| Windblast | 3 | 4 | Blood Thirsty | 2 | 3 |
| Dark Vision | 1 | 2 | Shadow Kill | 2 | 4 |
| Possession | 3 | 5 | Celerity | 2 | 3 |
| Bend Time | 2 | 8 | Devouring Swarm | 3 | 4 |

(Active: `m_RuneCosts` of each power tweak; passive: `m_Levels[].m_RuneCost` in
DefaultPlayer.ini.) Passive effects (current level only): Vitality 1 `HealthMax +20`; 2
`HealthMax +20`, `HealthRegenLimit +20`, `HealthRegenInitialDelay = 5`. Blood Thirsty level 1
none, level 2 `AdrenalineMax -20 %`. Celerity level 1: power jump `+1300`, fall limits 2550 /
3500; level 2: the same jump plus sprint speed +30 % of base, water speed +10 %, landing and
mantle animation rate +50 %, strafe / back sprint multipliers 0.5 / 0.4, stronger full-stop and
held-jump values (see the INI). Shadow Kill levels have no modifiers
(the kill rule reads the level, INVENTORY.md).

Port: `powers::next_level_cost` / `buy_power` buy one level at a time with the same table and
`attribute()` applies the current level only. Matches (verified by reading; no scenario can
drive a purchase). The port's `cfg.items.passive_powers` names the ini "Celerity" power
correctly.

---

## 7. Piero's workshop (the Craftsman store)

`DisTweaks_Store` (`Stores_Twk.Stores.Twk_Store_Craftsman`, package `L_Pub_Craftsman.upk`,
movie `UI_Shop`): items of 3 sub-sections (0 supplies, 1 weapon upgrades, 2 other upgrades), each
with `m_Cost` (a quantity of the coin abstract item), `m_Prerequisites` (one object reference per
item, not decoded; likely the previous tier), an item or upgrade tweak, optional story flag requirements and an output link on
purchase. Buying (`Req_BuyItem`, 0x009ECD10) pays coins and grants the item / upgrade; owning an
upgrade applies its `m_Attributes` as attribute additions (section 1).

Supplies (coins): Crossbow Bolt 20, Incendiary Bolt 50, Sleep Bolt 30, Bullet 30, Explosive
Bullet 50, Spring Razor 50, Grenade 70, Sticky Grenade 100, mana elixir (Piero's Spiritual
Remedy) 100, health elixir (Sokolov's) 200, Rewire Tool 100, Rune 500, King Street Brandy 150.

Upgrades (coins):

| upgrade | cost | effect (attributes) |
|---|---|---|
| Crossbow Accuracy / Range / Reload | 300 / 450 / 600 | dispersion and zoom, `Range +2`, `FastReload +0.3` |
| Combat Sleep Dart | 600 | `SleepDelayReduction 1000` (instant sleep) |
| Corvo's Pistol Upgrade | 600 | adds the pistol item if missing (no attributes) |
| Pistol Accuracy 1 / 2 | 300 / 600 | 1: `DispersionMax -10`, `DispersionMin -10`; 2: min -3, max -15, interpolation speed +20, `RecoilReduction 200` |
| Pistol Magazine 1 / 2 / 3 | 300 / 450 / 600 | `MagazineBonus +1 / +2 / +3` |
| Pistol Reload 1 / 2 | 300 / 600 | `ReloadSpeed +2`; `+200` and `FastReload 1` |
| Pistol Explosive Shot | 600 | `ExplosiveBulletUnlock` |
| Springrazor Radius | 600 | `SpringRazorRadiusBonus 2` |
| Sticky Grenade | 600 | `StickyGrenadeUnlock` |
| Sword Crossing | 600 | `VersusMiniGameBonus 3` |
| Mask Optics 1 / 2 | 300 / 400 | `ZoomLevel1`; `SpyglassDistance 10000`, `ZoomLevel2 10` |
| Improved Armor | 600 | `DamageReduction 5` |
| Boot Stealth 1 / 2 | 450 / 600 | `SilentRunning`, `SilentSprinting` |
| Bullet Capacity 1 / 2 | 300 / 600 | `BulletCapacity +10` each |
| Bolt Capacity 1 / 2 | 300 / 600 | `ArrowCapacity +10` each |
| Grenade Capacity | 450 | `GrenadeCapacity +4`, `StickyGrenadeCapacity +4` |
| Springrazor Capacity | 450 | `SpringRazorCapacity +4` |
| Bone Charms Capacity 1 / 2 | 300 / 450 | `MaxActivatedBoneCharmCountBonus +1 / +2` |

Port: no store at all (no purchase, no prices, no prerequisites). Upgrades exist only as a bitmask
given by loadout, and the mask holds 32 entries while there are 34 upgrades (F-5).

---

## 8. Save restrictions

SAVES.md section 2 is the model: a save is refused while any `m_DisallowSavingRefCount[type]` is
non-zero, while `m_fDisallowSavingTimer > 0`, or when the controller says no. This pass found
the pushers (CONFIRMED, the engine wrappers 0x00BEC820 `Push(type)` and 0x00BEC850
`Pop(type, wait)`, 22 push and 24 pop call sites, mostly the player master states), closing
U-340 / U-342 for the type list:

`EDisDisableSaveType`: 0 KillCam, 1 Blink, 2 GameOverPending, 3 CinematicMode, 4
PlayerInCombat, 5 ItemContext_Assassinate, 6 ItemContext_DropAssassinate, 7
ItemContext_Fatality, 8 ItemContext_FireCrossbow, 9 Soiree, 10 StatePlayerMasterAssassinate, 11
StatePlayerMasterChoice_Base, 12 StatePlayerMasterChoke, 13 StatePlayerMasterDead, 14
StatePlayerMasterMinigame, 15 StatePlayerMasterPrePossess, 16 StatePlayerMasterStunned, 17
StatePlayerMasterVersus, 18 StatePlayerMasterHolePeeking, 19 Kismet, 20 Pull.

Two properties the port does not have:
- **Post-pop waits.** The pop carries a wait: `m_fDisallowSavingTimer = max(timer, wait)`.
  Known waits: Choice 0.1 s (the pop of type 11 passes 0x3DCCCCCD), CinematicMode 1.0 s,
  PlayerInCombat a float from a pawn tweak (field +0xEC of the tweak object at +0x100 of the
  object the combat tracker reads, 0x00ABA0xx; not resolved, U-W8); the rest 0.
- **Item contexts, assassinations, fatalities, crossbow fire, stun, versus, hole peeking,
  minigames, soiree, pull** are all disabling states; the port models only Dead, Choke, Blink,
  PrePossess/Active possession, Choice and GameOverPending.

Port (`save.rs::save_blocked`): reason names are the right enum names; the set is a subset and has
no wait timer (F-16).

---

## 9. The largest unhandled placed actors and Kismet ops

Counts from `oracle coverage` (placed objects over the install).

**`DisNPCDistractor` (1364 placed) and `DisSeqEvent_Distracted` (1350).** An invisible actor
holding a `DisNPCDistractionComponent` that makes idle NPCs stop and play a "distraction"
(looking at art, warming hands, peeing, sweeping...). Core rule (LIKELY, ticked by 0x00B84E90 /
0x00B842F0 / 0x00B84C20): while `m_bDistractionEnabled`, the cooldown (`m_fCooldownTime` 45 s)
is over and no NPC is linked, every `m_fDistractionPulseTime` (-1 = once when a brain comes into
range) the component takes the nearby AI brains (up to 5, found within `m_fDistractionRange`
1000 uu, with a line of sight check every `m_fDistractionLOSCheckPeriod` 1 s when `m_bRequireLOS`),
filters them by pawn tweak (`m_DistractablePawnTweaks` / `m_NonDistractablePawnTweaks`, allowed
routes), the `m_DistractionCategories` list and by suspicion (`m_SuspicionLvl1_Chances` vs
`m_SuspicionLvl234_Chances`: `m_fChanceToLook` / `m_fChanceToAnimate`), then sends the winner to
the distractor (go-to, switch weapon away, play `Distraction_<type>_In / Loop / Out`, or a soiree
loop / matinee when `m_bUseSoireeInsteadAnim`) for `m_DistractionTime`, lowering alertness while it
lasts (`m_bLowerAIAlertness`). `DisSeqEvent_Distracted` outputs 0 "Distracted Start" (NPC in the
instigator variable), 1 "Distracted End", 2 "Distracted Looked At start"; the maps use them to
start / stop matinees (Boyle street). The port needs: an AI "distraction" behaviour reading these
fields and raising the three events; the 1350 events otherwise never fire (the matinees do not
play). Pure ambience, no gameplay effect. Ported 2026-10-08 (10.1 F-13). Corrections from the
decode: the soiree variant (all 7 of Boyle Ext, almost all placed ones) is the distraction's own
Kismet `SeqAct_Interp`, which plays the same `DistractionSoiree.*` matinee with the pawn as
`DistractedPawn`; the chance tables are picked by the pawn's locomotion modifier (relaxed,
unarmed and unsuspicious -> Lvl1; armed -> Lvl234), not only by suspicion.

**`DishonoredNote` (163 placed).** A level-design note actor: `m_SummaryText`, `m_MainText`,
`PostRenderFor` draws the text when the viewer is within `m_fSummaryDist` 1400 / `m_fFullDist`
800 (`NativeNotePostRender`). It is an editor / debug annotation (the retail class is hidden and
has no gameplay). LIKELY no port work; confirm `bHidden` on a placed instance (U-W9).

**`DisWhaleOilBattery` (146 placed), `DisWhaleOilReceptacle` (43 tweaks / 31 receptacles),
`DisSeqEvent_WhaleOilBattery` (35), `DisSeqEvent_WhaleOilReceptacle` (125),
`DisSeqAct_RefillWhaleOilBattery` (224), `DisSeqAct_PlugWhaleOilBattery` (20).** A battery is a
carriable movable (`DishonoredMovable`) holding charges: `m_NumberOfCharges` 50 (maximum), start
`m_InitialNumberOfCharges` (50, the empty variant 0), `m_PawnChargeCost` 5 (the security systems'
tweak; class default 4), `m_AmbientAnimalChargeCost` 1, `m_WatchtowerChargeCost` 1. A receptacle
(`DynamicSMActor`, detection cylinder radius / height 30, `m_bAllowEmptyBatteries`, "This tank is
empty") attaches a battery that touches it (`m_pAttachedBattery`, plug / unplug sounds), after
which the powered device draws charges, priced by victim class (`EWhaleOilChargeCost`: a pawn
`m_PawnChargeCost`, an ambient animal 1, a watchtower 1; LIKELY the wall of light, watchtower and
pylon systems); at zero the device stops. Detaching ejects the battery at `m_fEjectSpeed` 300.
The fields `m_bDamaged`, `m_bExplosionChain` and `m_fExplosionChainTimer` 0.33 s say a damaged
battery explodes and sets off its neighbours in a chain (LIKELY; `m_bIgnoreShield`). Kismet:
`RefillWhaleOilBattery` sets the charge to `m_PercentageCharge` of the maximum (and plays
`m_RefillSound`); `PlugWhaleOilBattery` forces a plug; the two events fire on plug / unplug /
empty (outputs not enumerated, U-W10). Port needs: the battery and receptacle actors with
charges, the charge cost per victim class, the two Kismet events and two actions; the powered
devices are AI-visible hazards (AI.md wall of light). Ported 2026-10-08 (10.1 F-14): note the
tweak's `m_NumberOfCharges` is the maximum (class default 3; 50 in `WhaleOilBattery_twk`), a
charge use may take the charges below 0 (no clamp), and a receptacle with
`m_bAllowEmptyBatteries` is a recharger (empty batteries only).

**`DisSeqAct_Lock` (319).** Locks / unlocks any usable (door, locker, gate): see 3.1. Port: not
run, so every Kismet-locked usable stays in its initial lock state.

**`DisSeqEvent_BreakableBroken` (233).** See section 4.

---

## 10. Findings

| id | cause | finding | evidence | fix |
|---|---|---|---|---|
| F-1 | port missing (high impact) | Chaos is a kill ratio; the original is a persistent integer score with deeds and 11 level thresholds | 0x00C4D2C0, 0x00C41B50, 0x00C487D0, `m_Deeds`, `m_DarknessLevels` | section 2.3 |
| F-2 | port bug | `AddDarkness` does not clamp at 0, ignores `m_Text`, and nothing reads the total; `DisSeqVar_Darkness(Level)` read `IntValue` (0) | `kismet_host.rs:1390`, `kismet/runtime.rs:624` | route to the Darkness state; level / score as the variable value |
| F-3 | port missing | `DisConv_CompareDarknessLevel` (38) not evaluated | `conversation/load.rs:211` loads the node, no runner arm | outputs 0 / 1 / 2 as 2.2 |
| F-4 | port approximation | HUD High / Low from a kill ratio; original uses level vs `m_ChaosLevelThreshold` | `gfx_hud.rs:838` | level compare |
| F-5 | port bug | Upgrade bitmask is 32 wide; 34 upgrades: `StickyGrenade` (index 32) and `Sword` (33) cannot be given (`take(32)`) | `powers.rs:62`, `give_upgrade`, `state.rs:401` `upgrades: u32` | `u64` / a set keyed by name |
| F-6 | port missing | Piero's workshop (prices, prerequisites, sub-sections, rune purchase of 500) | `L_Pub_Craftsman` store tweak | section 7 as data; `buy_item(owner, store_item)` |
| F-7 | port missing | Bone charm level II values never apply; a placed charm's identity (`m_Effect`) is ignored (uses a deterministic stand-in) | 0x00A22970, charm list levels | flat effect index = (charm, level); `SetBoneCharmEffect` op |
| F-8 | port bug | Kismet Door Open applies from any state; original only from closed and CanUse | 0x00A1A450 | guard on state 0 and enabled |
| F-9 | port missing | A key door can never be opened by Use in the port (locked refused); the original has key rings and matching keys | `m_MatchingKeys`, `DisKeyRing`, `AddKey` | decode the check (U-W4) then add a key ring |
| F-10 | port missing | `DisSeqAct_Lock` (319) | 0x00B8F730 | lock / unlock any usable |
| F-11 | port missing (LIKELY) | NPCs opening and closing level doors | 0x00A2C970, tweak radii 200 / 400 / 400 | door Tick rule in AI |
| F-12 | port missing | Breakables and `BreakableBroken` | section 4 | breakable actor |
| F-13 | port missing | Distractors | section 9 | AI behaviour |
| F-14 | port missing | Whale oil battery system | section 9 | actors and Kismet |
| F-15 | port: none | `DishonoredNote` is a design note | | none after U-W9 |
| F-16 | port approximation | Save-block set is a subset, no post-pop wait (choice 0.1 s, cinematic 1 s, combat tweak) | 22 push / 24 pop sites | add the remaining types and the wait timer |
| F-17 | port missing | Kismet inventory ops (AddInventoryItem 26, AddKey 15, AddPower 11, GiveUpgrade 2, ModifyElixirCount 3, GetAbstractItemQuantity 10) and the `PlayerInventoryChanged` event | coverage table | map each to the sim function (`give_item`, `buy_power` without cost, `give_upgrade`, `add_elixirs`) |
| F-18 | known difference | Attribute modifier order and difficulty bases | 0x00C863F0 | Normal only; low priority |
| F-19 | doc correction | PLAYGROUND.md "Can use" reads `+0x2C4` as the lock flag; it is `m_State`. The lock flag is bit 0 of `+0x2B0` | 0x00A4E2E0 | edit PLAYGROUND.md |

### 10.1 Port status (2026-10-08)

| id | status | what changed / why not |
|---|---|---|
| F-1 | ported | `dishonored_sim::darkness` (`Darkness { score, counters }`: `add` 0x00C41B50, `level` 0x00C487D0, `notify_deed` 0x00C4D2C0, `mission_end` 0x00C4D500). Tables from the tweak: `ItemsConfig.darkness` (`m_DarknessLevels` read as a raw int array, `TweakDb::int_array`; `m_Deeds` with tweak and exclusion names). Victims match by tweak lineage (the tweak and its `m_FallbackChainCooked`, `TweakDb::lineage`, cached per map as `<map>.lineage1.json`, `Spawner::lineage` / `OriginalNpc::lineage`); mission NPCs use their role's base tweak (`Pwn_Civ_Base` / `Pwn_Guard_Base`). Hooks in `MissionMatch::step`: a player kill -> `NumKills` with the victim lineage; `AiEvent::AlarmRang` -> `NumAlarmsTriggered`; the new `AiEvent::CorpseFound` (first sight of a corpse by a brain) -> `NumCorpsesDiscovered` once per corpse (`MissionMatch::corpses_discovered`). The score is saved with the match (`MissionMatch::darkness`) and a mission can start from a carried-in score (`MissionSettings::darkness`). The kill-ratio chaos (`high_chaos_kill_ratio`, `MissionStats::high_chaos`) is gone. |
| F-2 | ported | `DisSeqAct_AddDarkness` -> `Darkness::add` (clamped) and its `m_Text` as a HUD line; `DisSeqVar_Darkness` / `DisSeqVar_DarknessLevel` read `Kismet::darkness` (score, level) that the host syncs; `DisSeqAct_IncrementPlayerStat` (0x00AB9B90) feeds the deeds; `DisSeqAct_NotifyMissionEnd` clears the counters. |
| F-3 | ported | `NodeKind::CompareDarknessLevel` (`m_LevelToCompare`), outputs 0 equal / 1 lower / 2 higher; `ConvEnv::darkness_level`. Dialog cache key v2 -> v3. |
| F-4 | ported | `MissionView::high_chaos` = level >= `m_ChaosLevelThreshold` of the last `DisSeqAct_MissionStatsTracking` (0x009FE090; an unset threshold is the class default 0), else `MissionSettings::chaos_level_threshold`; `MissionView::darkness_score` / `darkness_level` added. |
| F-5 | ported | `Inventory::upgrades` (and the charm masks) are `u64`; `give_upgrade` / `attribute_for` take all 34 upgrades. |
| F-7 | ported, with a correction | The decoded rule differs from the finding: `m_Effect` is a charm effect id, below 10,000,000 an index into `m_Charms` and from it up into `m_ScriptedCharms` (0x00AA8EF0), not a flat index over levels. The owned charms are (level, effect id) pairs; taking an effect already owned raises its level (0x00ABBA90), which is how the "II" values apply. Port: `Inventory::charms_level2`, `powers::take_charm` / `charm_value` / `charm_slot`, `ItemsConfig::scripted_charms` (12 in the shipped list, not 14), the placed actor's `m_Effect` read into `PickupKind::BoneCharm(Option<i32>)` (pickups cache key 1 -> 2), `DisSeqAct_SetBoneCharmEffect` sets the targets' effect, and Kismet charm pickups give theirs. Left open: the placed charms of the shipped levels have no `m_Effect` (the class default -1); the run-time distribution that assigns them (a pool at the game object's +0xB44) is not decoded (U-W12), so -1 charms keep the deterministic stand-in. |
| F-8 | ported | `DisSeqAct_Door` Open applies only to a closed door (0x00A1A450), Close only to an open one (0x00A1A4B0). CanUse's other terms (disabled, mid-transition) never apply to the port's doors. |
| F-9 | left open | U-W4: `DisDoor::Use` (0x00A37C10) has no key path and the key match against `m_MatchingKeys` / `DisKeyRing` was not found in this pass; `DisSeqAct_AddKey` is therefore not run either. |
| F-10 | ported | `DisSeqAct_Lock` (0x00B8F730): Lock / Unlock on every usable target; doors switch `m_bLocked` and the AI's path blocking (door version), other usables keep a lock that makes a use fire their `DisSeqEvent_Used` "used but locked" output instead of a stage. |
| F-16 | ported (the types the port has) | `save::disallow_types` lists the pushed `EDisDisableSaveType`s the port can see (Blink, GameOverPending, PlayerInCombat, StatePlayerMasterAssassinate, Choice, Choke, Dead, PrePossess / Possess); a pop keeps saving off for its wait (`m_fDisallowSavingTimer = max(timer, wait)`, 0x009F95F0): Choice 0.1 s (0x00A6C7E0), CinematicMode 1 s (0x00AAF150), PlayerInCombat `m_fDisableManualSaveAfterCombatTime_s` 1.0 of `DisTweaks_PlayerPawn_Combat` when `m_bDisableManualSaveInCombat` (0x00AB9FA0; this closes U-W8; `ItemsConfig::combat_save`). Not modelled: KillCam, CinematicMode, the item contexts Fatality / FireCrossbow / DropAssassinate, Soiree, Minigame, Stunned, Versus, HolePeeking, Kismet, Pull (no such state in the port). |
| F-17 | partly ported | `DisSeqAct_GiveUpgrade`, `AddInventoryItem`, `ModifyElixirCount` (add / remove / set, clamped by 0x00C04E10), `GetAbstractItemQuantity` run. Left open: `AddPower` (0x00AA38A0 calls the level setter 0x00ACE270 with `m_PowerLevel`; how the original's level index maps to the port's 0..2 levels of active powers is not settled), `AddKey` (F-9), `PlayerInventoryChanged`. |
| F-6 | data ported | `ItemsConfig::store` (`DisTweaks_Store.m_StoreItems`, 42 items from `L_Pub_Craftsman`: name, sub-section, item, cost list, prerequisites, visibility prerequisite, purchase output). The purchase rule (`Req_BuyItem`, a store movie slot behind the shared thunk 0x009ECD10) is not decoded; no buy function yet. |
| F-12 | ported (explosion damage) | `dishonored_sim::world_actors::BreakableState`: `take_damage` = 0x00a26a90 (immune list `m_pImmuneToDamageTypes`, `m_CurHealth >= 0` and damage >= `m_DamageThreshold`, health floored at 0 or 0 for `m_pInstantBreakDamageTypes`, the highest step from `m_NextStep` up whose `m_HealthThreshold` >= health is wanted, positive damage cuts a running step timer to 0.01 s); `tick` = 0x00a3b0d0 / 0x00a3a5f0 (apply the wanted step, `m_NextStep = step + 1`, step timer `m_fStepTimer`; a timer that runs out sets the health to the next threshold and wants that step); `DisSeqEvent_BreakableBroken` output `Num(m_Steps) - step - 1` (0 is "Broken 100%"), originator the breakable, no instigator (`kismet_host::world_events`); the step's AI noise (`m_AINoiseLoudness` / `m_AINoiseContext`, only for a step with `m_pSoundEvent`, 0x00a3a5f0 -> 0x00b14e80) as `SimEvent::Sound` category `Breakable`. Data: `dishonored_assets::world_actors` (the `DishonoredBreakable` family: Movable, NavBlock, PA speakers, batteries; `m_pBreakableTweaks` / `m_pMovableTweaks` / `m_pWhaleOilTweaks` resolved), cached `<map>.worldactors1.json`, `MissionSetup::world_actors`, state `MissionMatch::world_actors` (saved). Damage source: grenade explosions (0x00A02E90 non-player radii, 0x009FEAA0, the explosion's `m_pRadialDamageType`, new `ExplosionConfig::radial_damage_type`, `DisDamageType_Grenade`). Not ported: the blast's line check to the actor (0x00A02B60; the port's collision has no actor identity), sword / projectile / fall / collision-speed damage (no prop hits or rigid bodies), loot drop, chunks, the final step's presentation. NavBlock (`DishonoredBreakableNavBlock`, 14 placed, all in `L_Boyle_Ext_Env`, 5 blocking): the last step sets `m_bIsCurrentlyBlockingNavigation := m_bShouldBlockNavigationWhenDestroyed` (step hook vslot 237, 0x00A305A0), so breaking UNBLOCKS unless that flag is set (no placed instance sets it); `WorldActorsState::nav_blocks` hands the changed `m_NavGuid`s to `KismetRun::set_nav_blocks`, applied in `door_nav` (`NavWorld::set_blocking_guid`). A step with no `m_pStaticMesh` and no `m_aMaterials` takes the actor out (0x00A3A5F0): the blocker is a mover (`level.rs`), emptied by `MissionMatch::place_movers` when `WorldActorsState::gone`. Not modelled: blocked edges are walls, not the original 10,000,000 cost (slot 3 0x00A184D0); the `m_NPCsMovingThru` path abort and the destroy-time edge removal (0x00A27120) of `m_bShouldBlockNavigationWhenDestroyed`; a broken plank stays drawn. Scenario `kismet/boyle_int_bottles_grenade` (8 bottles' BreakableBroken in the blast tick; 1 static pass, 2 unmeasured). |
| F-13 | ported | `dishonored_ai::distraction`: the manager's range update (0x00b52170 / 0x00b52050, brains whose patrol behaviour, the owner of `DisAISubProcessDistractions`, is current), the component tick (0x00b84e90 / 0x00b842f0: cooldown, pulse, the newly-in-range check list), the roll (0x00b83bb0: pawn link and countdown, `m_DistractablePawnTweaks` / `m_NonDistractablePawnTweaks` by tweak lineage, `m_AllowedRoutes`, chances by locomotion modifier index 0/1/4 + suspicion 0 -> Lvl1, 5/6/10 -> Lvl234 (0x00b6eca0), normalized by `max(sum, 1)`, LOS 0x00b81010 when `m_bRequireLOS`), look (0x00b2b150 / 0x00b28550 / 0x00b7da20: output 2, pawn countdown `m_fDistractionCooldownTime`, `m_DistractionTime`, the 90-degree hold 0x00b2b080), animate (`DisBehaviorInteract`, slot Distraction: go-to with exact placement, output 0 on arrival 0x00b84d60 / 0x00b84750 / 0x00b84670, output 1 at the soiree's end 0x00b844f0 or the distracted state's exit 0x00b84810 which also sets the pawn countdown); every output resets the cooldown (0x00b79b80). `DisSeqAct_AIStartDistraction` (0x00b63ed0) and `SeqAct_Toggle` on a distractor (0x00a4c350) run. Data in `AiLevel::distractors` (component through its archetype, `m_DistractionTime` lookup table, the soiree matinee's loop keys, `m_lDistractionSoireeLoops`) and `tweak_distraction_cooldown` (cache key `ailevel4-distract2`). Port limits: the soiree's length is computed from its matinee's loop keys with the distractor's loop counts (0x00b79cf0) instead of the Kismet matinee's completion; the animated variant lacks its In / Out animation lengths (loop only) and its weapon switch takes one tick; `m_bLowerAIAlertness` not applied; a distraction put aside by a higher behaviour is given up. Scenario `kismet/boyle_distractor_railing` (2 static pass, 2 unmeasured). |
| F-14 | ported (core rule; no consumers) | `dishonored_sim::world_actors`: `set_charges` = 0x00c81d70 (empty -> charged restores `m_CurHealth` = `m_Health`, empty = -1; notified: `DisSeqEvent_WhaleOilBattery` output 1 AllChargesDepleted / 0 ChargeUsed; an empty battery that is not recharging leaves its receptacle 0x00c7d190), `use_charge` = 0x00c85170 with the costs of 0x00c731d0 (pawn / ambient animal / watchtower), `plug` = 0x00c7b490 with the attach test 0x00c65820 (a receptacle that allows empty batteries is a recharger: takes empty ones, `m_bRecharging`), `eject` = 0x00c7b6b0 (OnUnplug, instigator the battery), the receptacle's first-tick plug of its `m_pBattery` (0x00c822b0, not notified), initial charges (0x00c850c0). Kismet: `DisSeqAct_RefillWhaleOilBattery` (0x00c82190, `int(m_PercentageCharge * 0.01 * m_NumberOfCharges)`, not notified), `DisSeqAct_PlugWhaleOilBattery` (0x00c7d7c0: pairs Battery / Receptacle, Plug unless a charged battery is held, Unplug ejects, both inputs nothing), events `DisSeqEvent_WhaleOilBattery` / `DisSeqEvent_WhaleOilReceptacle` (U-W10 closed: battery outputs 0 ChargeUsed, 1 AllChargesDepleted, 2 Damaged, 3 Destroyed; receptacle 0 OnPlug, 1 OnUnplug). Not ported: the powered devices (walls of light, arc pylons, watchtowers: no such actors in the port; `use_charge` / `powered` are the API they need), the player carrying / inserting a battery (0x00c7da70, needs the carry system), battery damage (its own TakeDamage 0x00c7d340 with the shield trace 0x00c7b1a0 and the explosion chain timer, vslot 237 0x00c7d660 Damaged / Destroyed), the physical ejection's shield test (the battery always leaves), the empty-tank HUD message. No scenario: the Boyle maps have no receptacles and nothing uses charges; captures queued (charges / plugs as loaded). |
| F-19 | done | PLAYGROUND.md corrected (`+0x2C4` is `m_State`; the lock is bit 0 of `+0x2B0`). |

U-W1 is closed by the data read: `m_DarknessLevels` = 0, 12, 48, 72, 96, 114, 138, 162, 168, 200, 390
(raw int array of `Twk_Pawn_Corvo_Release`). Scenarios (system Chaos): `chaos/adddarkness_sweep`,
`chaos/civilian_kills`, `chaos/guard_kills_sweep`, `chaos/corpse_discovered` (all Static, all pass;
before this port the channels did not exist and the score was a constant 0). No alarm scenario:
the Playground has no alarm bell; the alarm deed is covered by `darkness` unit tests.

Open questions: U-W1 confirm the level table by `session read`; U-W2 the exact kill-credit
conditions of 0x00B73990; U-W3 the lock gate between Use and the opening stage; U-W4 the key
match; U-W5 Windblast vs door forces; U-W7 coin cap; U-W8 the
post-combat save wait; U-W9 `DishonoredNote.bHidden`; U-W11 the witness list a player's
door use builds. Closed by the world-actor port: U-W6 (0x00a26a90 / 0x00a3a5f0, F-12 above),
U-W10 (the event enums, F-14 above). New: U-W13 whether a breakable's step is applied in the
blast's frame or the breakable's next tick (actor order); U-W14 whether `DisBehaviorInteract` ends
on the go-to unlink (0x00b80e80's message to the brain) while the pawn's distracted state plays on;
U-W15 which interruptions end a soiree distraction (the port: a higher behaviour).

---

## 11. Scenarios and captures

New (all Static or Unverified, sources cited in the spec): `inventory/ammo_cap_grenade` (5),
`inventory/ammo_cap_grenade_belt` (5 + 4), `inventory/elixir_cap` (10 / 10),
`inventory/drink_potency` (+40, Unverified: whether regeneration adds within the window). All pass
on the port. Existing ones relevant here: `inventory/pickup_coins`, `inventory/pickup_elixir`,
`inventory/drink_health`, `saveload/blocked_mid_blink`, `saveload/roundtrip`.

Not authored (the runner has no channel and the port has no rule yet); add them with F-1:
- `world/chaos_civilian_kills`: kill N civilians with the sword, record `darkness.score` /
  `darkness.level`; expected score `6 * N`, level 1 at N = 2 (deed 0), level 2 at N = 8.
- `world/chaos_guard_kills_sweep`: kill 1..8 guards; score 6 after 4, 12 after 8 (deed 2).
- `world/chaos_adddarkness_sweep`: Kismet / cheat `AddDarkness` of -50, 11, 12, 47, 48, 389, 390
  from score 0: level 0, 0, 1, 1, 2, 9, 10 and the clamp at 0 for -50.
- `world/door_use`: stand in front of a door, Use twice: state 0 -> 1 or 2 -> 0 (needs a door
  state channel).

Captures requested (see `fixtures/oracle/capture_queue.txt`): the manager's fields, the level
table and deeds as loaded, the cheat-driven score / level mapping through `debugdarkness`, the
door states and locks of Boyle Ext, a door Use, and the engine's disable-save counters.
