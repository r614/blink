# Inventory, takedowns, powers and gadgets of the original executable

How `Dishonored.exe` (GOG build) handles what the player does to the world beyond movement and
the sword: the inventory and pickups, choking and carrying bodies, Possession, Bend Time,
Devouring Swarm, Shadow Kill, the passive powers and bone charms, grenades and Spring Razors.
Recovered with Ghidra (scripts in `research/ghidra/`, output kept out of the repository), the
UnrealScript class layouts, the tweak objects and the INIs; `dishonored_sim` reproduces it.
Findings are in our own words; addresses are virtual addresses in the image. Evidence levels
as elsewhere: CONFIRMED, LIKELY, SPECULATIVE. Movement, Blink, Windblast, parry and regen are in
`PLAYER.md`.

## Data

`dishonored_assets::items_config` reads everything into `GameplayConfig::items`
(`dishonored_ir::items`), with each value's source in the provenance; the cache file is
`gameplay_config.v6.json`.

| What | Source | Values (Normal difficulty) |
|---|---|---|
| Ammo capacities | `Twk_Pawn_Corvo_Release.pAttributeTweaksNormal` `m_*Capacity` (copied into `m_AmmoInfo[].m_AmmoCapacity` by 0x00B56880) | bullets 10, explosive bullets 10, bolts 10, sleep darts 10, incendiary bolts 10, Spring Razors 5, grenades 5, sticky grenades 5 |
| Elixirs | `m_nHealthElixirValue` / `m_nManaElixirValue` / `m_nMax*Elixir` | 40 health, 50 mana, at most 10 each |
| Choke | `Attribute_ChokeTime` + the choke animation; `DisTweaks_PlayerInput.m_fChokeHoldToAutoCarry` | 1.5 s + 5.033 s; 0.2 s |
| Carrying | `Attribute_GroundSpeedCarryingCorpse`, `Attribute_JumpZCarryingCorpse`, `Attribute_ThrowStrength` | 350, 800, 2000 |
| Use reach | `DisTweaks_InteractableInterface.m_fMaxDistance` (class default; no pickup overrides it) | 200 |
| Ranged damage | `Attribute_RangedDamage` of the crossbow / pistol item attributes (through their fallback chains) | 20 / 10 |
| Bolts | `Twk_Proj_Arrow*`: `m_fDamageMultiplier`, `m_fSpeedMultiplier`, `m_fGravityMultiplier`, `m_fLegsAndArmsDamageCap` | bolt 1 / 1 / 1.3 / 15; sleep dart 1.25 / 1 / 0.4 / 25; incendiary 2.5 / 0.2 / 0.5 / 50 |
| Possession | `Twk_Powers_Possess`, `Twk_Possessable_BaseNPC`, `Twk_Possessable_Rat` | cost 60, range 1000 / 2000, cancel cooldown 1.0, intro 1.1 s; people: level 2 only, 15 s, 300 uu/s, not while choked; rats: level 1, 20 / 30 s |
| Bend Time | `Twk_Powers.BendTime.Twk_BendTime` | cost 60; level 1: world 0.15, caster 0.85, 12 s; level 2: world 0, caster 0.9, 8 s; dilation warm-up 0.15 s, cool-down 0.25 s, projectile early wake-up 0.25 s |
| Devouring Swarm | `Twk_DevouringSwarm` + `Twk_RatSwarm.Twk_DevouringSwarm_Lvl1/2` | cost 40, spawn delay 0.65 s, 1500 uu, 30 s, 30 / 60 rats, detection 700, first bite after 2 s, every 0.5 s, 0.4 per bite clamped to 2..5 |
| Rune costs | `m_RuneCosts` of each power tweak; `m_Powers[].m_Levels[].m_RuneCost` (DefaultPlayer.ini) | e.g. Possession 3 / 5, Bend Time 2 / 8, Vitality 1 / 3 |
| Passive powers | `DishonoredPowersComponent.m_Powers` in DefaultPlayer.ini | Vitality: +20 HealthMax; level 2 +20 HealthMax, +20 HealthRegenLimit, HealthRegenInitialDelay 5. Celerity (Agility), BloodThirsty, ShadowKill |
| Bone charms | `Twk_Pawn_Corvo_Release.pCharmListTweaks` `m_Charms` (29), `m_MaxActivatedBoneCharmCount` 3 | attribute + value per charm (Strong Arms ChokeTime -1.5, Undertaker carry speed +38, Swift Shadow sneak speed +100, ...) |
| Spring Razor | `Twk_Inv_SpringRazor.Twk_Inv_SpringRazorPlaced` | arming 0.5 s, detection and detonation cylinders 250 x 250, trigger reaction 0, damage reaction 0.75, damage delay 0.35, non-NPC damage 5, immune-NPC damage 50 |
| Grenades | `Twk_Proj_Grenade` / `Twk_Proj_StickyGrenade` + their `DisTweaks_GrenadeComponent` and `DisTweaks_Explosion`, `DisTweaks_ThrowGrenade` | thrown at 2500 uu/s raised 5 deg; detonation 0.5 s after impact; 50 damage inside 300 uu falling to 0 at 600 (players 150 / 600); attached (sticky) +75 |
| Workshop upgrades | `DisTweaks_Upgrade` (`Twk_Upgrades.*`) `m_Attributes` | Ammo Pouch / Quiver +10 bullets / bolts, Grenade Belt +4 grenades and sticky grenades, Spring Razor Pouch +4, Spring Razor radius x2, Instant Sleep (`SleepDelayReduction` 1000), pistol magazine / reload speed, crossbow range / reload / accuracy, armour 5, silent running. The sim applies them as attribute additions (capacities, sleep delay, razor radius); the others are carried for the HUD |
| Melee reach | DefaultPlayer.ini `m_fMinSpeedRayScale` 400, `m_fRaySpeedScale` 0.005, `m_fRaySpeedScale_Max` 1.5 | see PLAYER.md |

Placed pickups: `dishonored_assets::pickups::extract_pickups` (tool `pickup_dump`, cache
`<map>.pickups1.json`) reads every `DisAbstractItemPickup(Note|AudioLog)`, `DisStatPickup`,
`DisElixirHealth/Mana`, `DisWhaleBoneCharm` and `DishonoredInventoryPickup` of a map's level
packages and resolves its tweak against them. Spawners' `m_pStealablePickup` marks the pickups
an NPC carries (pickpocketing). Boyle Ext (with the street script) has 68: 54 abstract items
(coins, heirlooms, runes, blueprints, the invitation, notes), 7 elixirs, 5 stat pickups (food),
2 bone charms; 6 are carried by NPCs.

## Inventory (`DishonoredInventory`)

- `m_AmmoInfo[eDisAmmoType]` (count, capacity), `m_ElixirCounts[2]`, `m_AbstractItem` (keys,
  notes, coins, runes: `m_pMoneyItem` = `Coins_AbsItm`, `m_pRuneItem` = `Rune_AbsItm`).
- AddAmmo 0x00C052E0: adds `min(n, capacity - count)` and returns that. RemoveAmmo 0x00C05260:
  fails (removes nothing) when fewer than n remain. CONFIRMED.
- Elixir count 0x00C04E10: `count + n` clamped to `[0, m_nMax*Elixir]`. An elixir pickup is
  refused at the maximum (`DisElixirHealth` 0x00A22160). CONFIRMED.
- Drinking (`Dis_ConsumeElixir` -> controller vfunc +0x53C 0x00AAED00 -> 0x00C04FC0), CONFIRMED:
  - only when the controller allows item use (flag 0x40) and health (mana) is below its maximum;
    otherwise nothing is consumed;
  - health: `health = min(HealthMax, health + (int)((1 + Attribute_HealthPotionPotencyBonus) * 40))`;
    mana: `mana += (int)((1 + Attribute_ManaPotionPotencyBonus) * 50)` through `AddMana`
    (0x00AA1AC0 -> `SetMana`, capped);
  - after a health elixir, `rand() / 32768 < Attribute_PotionManaBonusChanceRatio` (Twist of
    Fortune) also gives mana; not modelled (random, U-111).
- Stat pickups (`DisStatPickup`): each ammo amount is rolled once when the level starts
  (`rand() % (max - min + 1) + min`, 0x00C3D4E0); taking it (0x00A367A0) adds
  `(1 + potency) * m_HealthChange` health, the same for mana, then moves the ammo that fits
  (0x00C05440), keeping the rest in the pickup. It can be taken while one of its ammo types is
  not full or it heals / restores mana (0x00A36980). CONFIRMED. The sim rolls with a
  deterministic hash of the pickup id instead of `rand()`; food is consumed, an ammo pickup stays
  while ammo is left (U-111).
- Abstract item pickups give `m_Quantity` of `m_pAbstractItem`.

## Takedowns

### Choke (`StatePlayerMasterChoke`, `DisItemContext_Choke`, `StateNPCMasterBeingChoked`)

- Enter 0x00A7AA00: `m_fRequiredTime = m_fTimer = Attribute_ChokeTime + length of anim state
  0x5f` (`FUN_00c4de20`); the victim enters `StateNPCMasterBeingChoked`. The anim is
  `Sword_Choke_Loop_Master` (5.033 s, `Ply_Sword_Choke_as`), LIKELY (U-101): 6.53 s in total,
  4.03 s with Strong Arms.
- Tick 0x00A71100 by `eChokeState`, CONFIRMED:
  - In (`Sword_Choke_In` 0.667 s) and Loop: the timer runs down. Whenever the victim is no
    longer in the being-choked state the choke is cancelled (0x00A71090, state Cancel).
  - Loop: if the choke button (`DishonoredPlayerController.m_bChokeButton`, +0x62F) is released
    the choke is lost (Lose: the victim breaks free); when the timer reaches 0 it is won (Win:
    knocked out, `DisDamageType_Choke`).
  - Win: holding Use (`m_bUseButton`, +0x633) runs `m_fHoldTimeForAutoCarry` (0.2 s) down, else
    it is reset; at 0 the body is lifted (`Sword_Choke_CarryCorpse`).
- Exit times from the animations: Win 2.470 s, Lose 2.229 s, Cancel 1.457 s, Choke->carry 1.618 s.
- The NPC awareness table refuses Possession of a choked NPC (`m_AwarenessSettings[Choked]`).
- Range and angle: `DisTweaks_Choke` sets none (fallback-only tweak); the sim uses the back
  assassination context's geometry (U-103).
- The sim maps the choke button to Use.

### Bodies (`StatePlayerCarryCorpseIdle`, `StatePlayerGrabCorpse`)

- Lift: `Empty_CarryCorpse_In` (`DisNotify_AcquireCorpse` 0.056 s, `EndLiftCorpse` 0.967 s).
  Drop: `DropLow` (`ReleaseCorpse` 0.5 s, end 1.1 s), `DropHigh` 0.267 / 0.856, `DropNoMove`
  0.173 / 0.421 (`DropCheckTweaks` choose). CONFIRMED (data).
- While carrying, the attribute update (0x00B56880) uses `Attribute_GroundSpeedCarryingCorpse`
  (350) for `GroundSpeed` and the jump uses `Attribute_JumpZCarryingCorpse` (0x00A75EE0,
  0x00A60010). `Attribute_GroundSpeedBonusWhileCarrying` scales the carry state's speed
  (0x00AA2EC0). CONFIRMED.
- The sim: Use lifts the body under the crosshair (dead or unconscious), Use drops it in
  front, attack throws it (speed = `ThrowStrength` x 0.25, U-106); only Blink is usable while
  carrying; the sword is sheathed.

### Sleep darts and knocked-out NPCs

- `DisDamageType_Arrow_Sleep` (DefaultWeapon.ini): `m_bPutToSleep`, `m_fTimeBeforeSleep` 1.5 s,
  `m_bInstantSleepForUnawareNPCs`, `m_bSlowDeathOnly` (the "death" is sleep). The sim puts an NPC
  to sleep 1.5 s after the hit, without damage (instant for unaware NPCs needs the AI, U-105).
- Knocked-out (choked, slept) NPCs stay down; damaging one kills it (`m_bKillSleeper`).
  Spawners with `m_bTreatAsKnockedOut` start unconscious.
- Hooks: `SimEvent::KnockedOut`, `ChokeStart` / `ChokeEnd`, `BodyLifted` / `BodyDropped`;
  `PlayerState::incapacitated()` and `is_body()`. Missions count knock-outs apart from kills
  (`MissionStats::knocked_out`; chaos counts kills only).

## Powers

- Cost (`DishonoredActivePowerComponent`): usable when mana >= `m_ManaCost` (vfunc +0x18C);
  the spend (0x00AA93E0) is skipped when `rand` is below `Attribute_ChanceUseNoManaForSpell`.
  CONFIRMED.
- Only `DisDLC06ActivePowerComponent_Transversal` (Daud's Blink, DLC) suspends mana
  regeneration (the class of 0x00AA7A20's check, StaticClass record 0x01398D30). No base-game
  power does. CONFIRMED (U-050 resolved).
- Cast animations (`Ply_Powers_as`): `DisNotify_BendTime` 0.140 s (exit 1.295),
  `DisNotify_DevouringSwarm` 0.165 s (exit 1.679), `DisNotify_Possession` 0.172 s of
  `Possession_Out` (exit 1.117).

### Possession (`DishonoredActivePowerComponent_Possess`)

- Stages 0x00BFD6D0: inactive 0x00BFB320, targeting 0x00BFD300, pre-possess 0x00BFCD40,
  active 0x00BFB390, cooldown 0x00BED880. CONFIRMED.
- Start 0x00BFB6C0: `m_fPossessionTimer = possessable.m_Levels[level].m_fPossessDuration *
  (1 + Attribute_SpellStrength)` (+ `Attribute_WhiteRatPossessDurationBonus` for white rats).
- Active: the timer runs except in cinematic mode; possession ends when it runs out or the host
  is gone; using the power again requests the end (0x00BFB520); a cancel outside the active
  stage goes to cooldown `m_fCancelCooldown`. End 0x00BF9330 releases the host and leaves the
  possessor at the host (`DUET_UpFromFeet`).
- The host moves at `m_fGroundSpeedWhilePossessed` (`Modifier_PossessGroundSpeed`,
  0x00A61770) and cannot sprint (`m_bCanSprintWhilePossessed` false).
- The sim: hold to target (U-108 for the scoring), release to pay and travel in over
  `m_IntroDuration`; the possessor's input drives the host; the possessor steps out behind the
  host; rat pawns are possessed the same way (see "Rats").

### Bend Time (`DishonoredActivePowerComponent_BendTime`)

- Start 0x00BF5190 / 0x00BF8810: `m_fWorldDilation`, `m_fSelfDilation` from the level,
  `m_fMaxDuration = m_TimeDuration * (1 + SpellStrength)`, start time; world flag +0x16C set.
- Tick 0x00BFBE60 in real time: post-effect blend in/out; at `max - m_fTimeDilationCooldownTime`
  the dilation is blended back to 1 over the cool-down (0x009FA0D0); at
  `max - m_fProjectileEarlyWakeupTime` projectiles wake up (0x00BF36F0); past the maximum it
  ends (0x00BFA200). Using it again (0x00BF8950) moves the start so only the cool-down remains.
- `DisGrenadeComponent` runs its timer at the actor's `CustomTimeDilation` (0x00C5A2A0).
- The sim scales each pawn's tick: the caster by its own dilation, everything else (pawns,
  projectiles, grenades, traps, swarms, Windblast fronts) by the world dilation, blended in
  linearly over the warm-up and out over the cool-down (U-109). Stopped pawns do not run.

### Devouring Swarm, Shadow Kill

- Swarm: summoned along the view up to `m_fMaxDistance`, `m_fRatSwarmSpawnDelay` later; rats go
  for the nearest pawn but the summoner within the detection radius, eat bodies
  (`m_fEatStartupDelayAfterKill`, `m_fEatStartupDuration`, `m_fEatPerLimbDuration`; 4 limbs per
  body remains SPECULATIVE).
- Rats (U-104 resolved except eating): the crowd agent archetype `RatSwarm.CrowdAgent_SmallRat`
  (`DisGameCrowdAgentSkeletalRat`) has `MaxSpeed` 350, `Health` 1, tweak `Twk_RatSwarm.Twk_Rat`
  (`m_fAttackDistance` 120). The attack group update (0x00C891E0): once the group has been on its
  target for `m_fInitialDelayBeforeDealingDamage` (no delay when the target is the player), every
  `m_fDamageTimeInterval` (accumulated) a random rat of the group bites for
  `(int)clamp(count x m_fDamagePerBite, m_fMinDamage, m_fMaxDamage)` with `DishonoredDamageType_Bite`
  and the target gets `Modifier_RatAttackSpeedModifier` (`-m_fPawnSlowdownPercent` % of the base
  `Attribute_GroundSpeed` / `GroundSpeedSprint` for `m_fPawnSlowdownDuration`: 40 % / 1 s in the
  class defaults); a target pawn stomping rats removes them from the group. Rats attack the player
  within `m_fStandUpRatSwarmAttackDistance` 200, sneaking `m_fCrouchedRatSwarmAttackDistance` 60
  (`Twk_Pawn_Corvo_Release`). `Attribute_RatsAttackRadiusReduction` (a charm) divides a query radius
  (0x00C87B20).
- The sim: the Devouring Swarm stays one aggregate (decoded damage, slowdown, reach, player rule);
  individual rats are `PawnKind::Rat` pawns (`dishonored_sim::rats`): small box (15 x 10), 350 uu/s,
  no sword, left hand or jump, health 1, driven by a brain or a possessor; a rat attacks while its
  input holds attack and a pawn is in reach in front of it, and the attacks on one pawn form a group
  with the rule above (`DisTweaks_RatSwarm` class-default values). Hooks for `dishonored_ai`:
  `rats::spawn_rat`, `SimEvent::RatSpawned` / `RatBite`, `PlayerState::rat_attack`.
- Rat possession (Possession level 1, `Twk_Possessable_Rat`, 20 / 30 s): rat pawns are possessable;
  aiming at a Devouring Swarm possesses one of its rats, which leaves the swarm as a rat pawn.
- Shadow Kill 0x00A87B60: level 2 always; level 1 when the victim's awareness is below 1 or it was
  out (the parameters of the call; never for bites); the corpse burns to ash
  (`DisSeqAct_BodyShadowKill`, `m_fShadowKillFadeToBlackDuration` 5). The sim reads the AI's
  `PlayerState::ai_unaware` hook (U-110 resolved, needs `dishonored_ai` to write it).
- Sleep darts: `m_bInstantSleepForUnawareNPCs`: an NPC whose `ai_unaware` hook is set sleeps at once
  (U-105 resolved on the sim side).

### Passive powers and bone charms

- `m_Powers` modifiers: `AddVal`, `SetVal`, `AddBasePercent` on named attributes. The sim
  applies the current level's list (U-100).
- Charms add their value to their attribute; the sim applies the ones that act on simulated
  attributes (health/mana maxima, regen, potion potency, choke time, carry/sneak/sprint speeds,
  sword speed factor, falling damage, throw strength, Falling Star's drop-assassination bonus
  0x00ABBD10). Not yet applied: Clockwork Malfunction (grenade delay), the Spring Razor radius
  bonus, the random ones (Twist of Fortune, Scavenger) and the rat/plague/water charms.
- Rune purchase: `m_RuneCosts` / `m_RuneCost` of the next level.

## Gadgets

- Crossbow ammo and damage: the item fires the projectile with `(int)Attribute_RangedDamage` and
  `(int)Attribute_RangedHeadshotMultiplier` (0x00C257A0, 0x00C25D80); the projectile's damage is
  that times its `m_fDamageMultiplier` (0x00C4A310). Bolts 20, sleep darts 25, incendiary 50
  (U-049 resolved: was 35 in the sim).
- Incendiary bolts (`DisDamageType_Arrow_Flare`): `m_bCannotKillNPC`, `m_bPutToFire`; the NPC is
  immolated after `m_fNPCImmolationTime` (0.5 s) and burns to death in the sim (U-107).
- Grenades (`DisGrenadeComponent`): armed by the first impact (+0x64 bit 1); the fuse
  (`m_fDetonationDelay` + `Attribute_GrenadeExplosionDelay` for the player, 0x00C58210) cannot run
  below `m_fMinDetonationDelayAfterHit` before exploding (0x00C5A2A0); the cooked fuse is 0, so a
  grenade explodes 0.5 s after it first hits something (LIKELY, U-102). Explosion: radial
  damage with a line check and linear falloff between the full and the outer radius (players
  use the player radii); a sticky grenade adds `m_fAttachedDamage` to the pawn it sticks to;
  explosions set off Spring Razors.
- Spring Razor (`DisGadget_SpringRazorPlaced` 0x00C39C10): arming timer, then detection of
  `DisSpringRazorTargetInterface` pawns (NPCs only; the player never triggers it) inside the
  detection cylinder with a clear line (0x00C2D650) -> `m_fTriggerReactionTime` (or
  `m_fDamageReactionTime` when damaged, 0x00C31F80) -> detonation (0x00C2D970) ->
  `m_fDamageActorDelayTime` -> damage in the detonation cylinder (0x00C36350): NPCs are cut down,
  other pawns take `m_NonNPCDamage` (5). Radii scale with `Attribute_SpringRazorRadiusBonus`
  (0x00C2D360). CONFIRMED.
- Deploy timings: grenade `FireProjectile` at 0.300 s (exit 0.680), Spring Razor placed at
  0.200 s (`Gadgets_SpringRazor_Deploy`, 0.367 s).

## Multiplayer rules (not in the original)

The original has no player-versus-player: choke, sleep darts, Possession and Spring Razor
triggers only ever act on NPCs, and the sim keeps it that way (players are not choked,
possessed, slept or detected by razors; razors and grenades still damage players by the
non-NPC rules). Inventory and power levels are per pawn; new modes grant them (the default
player has Blink, Windblast and Dark Vision at level 2 as before).

## Replication

`WorldState` gains pickups, held abstract items, traps, Bend Time casts and swarms (snapshot
field `extras`, protocol version 2); `PlayerState` gains the inventory, condition, choke,
carry, possession and time-scale fields. All are in the state hash.
