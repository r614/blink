# Ranged weapons and gadgets: the core model

Pistol, crossbow (bolt, sleep dart, incendiary bolt), grenade, sticky grenade, Spring Razor; ammunition.
Written 2026-10-08 from the decompiled exe (`oracle index`), the tweaks (`tweaks`), the hook
measurements in HOOK_TRIAGE.md and the port (`crates/dishonored_sim`: `left_hand.rs`, `gadgets.rs`,
`tuning.rs`, `inventory.rs`). Addresses are the GOG exe's (image base 0x00400000). Reload timing is
out of scope here (another agent owns the crossbow 1.583 vs 1.555 s question); only the facts the
damage and ammo rules need are repeated.

Status words: **matches** (rule and port agree, hook- or code-confirmed), **differs** (port bug or
port missing), **open** (not decidable without a capture). Every "differs" below is backed by the
decompiled code; the three that change a number a player sees most (pistol base damage, close-range
band, crossbow stealth / limb rules) also have a scenario that prints the port's value next to the
prediction and is queued for a capture.

## 1. The fire sequence (shared by pistol and crossbow)

1. **Aim point.** The screen centre, or the aim-assist point (0x00C14640 -> 0x00C14460, target
   choice 0x00C14290). The tweaks switch aim assist on for mouse and gamepad
   (`DisTweaks_ProjectileAttack.m_bUseAimAssist_Mouse` true on both Corvo weapons, windows of 10-50
   px per target class, `m_fMaxAimAssistAngle` 45 deg, distance 4000 pistol / 2000 crossbow); what
   gates it at run time (the saved "mouse friction" option, MENUS.md, defaults Off) is open. The port
   has none.
2. **Dispersion offset** (0x00BFFBA0): `r = rand() x d / 32767`, `a = rand() x 2 pi / 32767 - pi`
   (two draws, radius first), offset `(r cos a, r sin a)` in pixels of a 720-line screen, added to the
   aim point and deprojected through the real view (0x00AA2B10 -> 0x006C5180). **matches**
   (`left_hand::dispersion_offset`, `offset_direction`; hook: r / d mean 0.486 and variance 0.0861
   over 150 shots, trace-minus-port angle under 0.1 deg; the port fixes the reference view at 75 deg
   horizontal on 16:9, the original uses the player's actual view: a known difference, PLAYER.md).
3. **Dispersion state** `m_fDispersion` (+0x12C): set to `Attribute_DispersionMax` on equip and after
   every shot (after the shot is drawn, 0x00C03A60 -> slot 109 0x00C1CB80); the weapon tick (slot 94,
   0x00C1CB10) runs only while a round is loaded and moves it toward `Attribute_DispersionMin` at
   `Attribute_DispersionInterpolationSpeed` per second; held at the maximum while sprinting, falling
   or blocking (`m_bMaxDispersionOn*`). Pistol 45 / 15 / 70, crossbow 30 / 0 / 70 px, px/s.
   **matches** (`left_hand::update_dispersion`; hook: 45 -> 15 at 70.0 / s).
4. **Projectile spawn.** An engine actor is spawned and launched (0x00C540A0, slot 233) at
   `speed x direction x m_fSpeedMultiplier`. The spawn point is **the camera position plus the
   undispersed view direction times `m_fBulletSpawnDistance` / `m_fArrowSpawnDistance` (both 150
   uu)** (0x00C2A3E0: `+0x420` of the weapon tweak), the direction is the dispersed one. The crossbow
   (0x00C38230) additionally traces the camera ray and a muzzle-to-aim ray (flags 0x020A28DF) and,
   when they reach the same actor, spawns the bolt **at the muzzle socket flying at the aim point**;
   otherwise at the camera + 150. `m_fDamage` of the projectile is set from
   `(int)Attribute_RangedDamage`, the headshot value from `(int)Attribute_RangedHeadshotMultiplier`.
   Port: spawns at the eye (`left_hand::update`: `eye_position`), straight along the dispersed
   direction. **differs** (finding F2).
5. **Flight.** `DisProjectile` physics (slot 136, 0x00C546B0), every tick: if
   `|m_fGravityMultiplier| > 0.0001`, `v.z += GetGravityZ x m_fGravityMultiplier x dt`; the step is
   `(v.x dt, v.y dt, v.z_old dt + 0.5 a dt^2)` (the old velocity plus half the gain), swept by a
   trace (0x00C4F3A0) that applies hits as it goes. **matches**: `step_projectiles` /
   `step_grenades` use `(old + new) / 2 x dt`, the same step; gravity -1500 (DefaultGravityZ). The
   bullet's tweak gravity multiplier is 1.0 (port 0.0): under 0.1 uu at 3000 uu, irrelevant.
6. **Hit.** On a pawn: damage `slot 239` (0x00C4A310 base, overridden by DisBullet 0x00C29820 and
   Arrow 0x00C503D0), `(int)` of the float, `TakeDamage`, then the type's hit handler (slot 235): a
   bullet pierces, an arrow sticks or pins. On the world: slot 236.

## 2. Pistol (`DisBullet`, `Twk_Projectiles.Twk_Proj_Bullet`)

| Quantity | Source | Value |
|---|---|---|
| Speed | `m_fSpeed` x `m_fSpeedMultiplier` | 100000 x 1 |
| Base damage | `(int)Attribute_RangedDamage` (pistol item, Normal and every difficulty) **plus** `m_fDamage` (0x00C29820: `cvtsi2ss [this+0x27C]; addss [tweak+0xE8]`, bytes checked) | 10 + 20 = 30 |
| Headshot | x `m_fDamageMultiplier_Headshot` when the hit bone's region byte is `DHR_HEAD` (0x00B66A50 over `m_HitRegions_Map`) | x3 |
| Awareness | x `m_fDamageMultiplier_Stealth` unless the pawn's awareness is `InCombat` (5) | 1.0 (no effect) |
| Distance | distance from the bullet's **spawn point** (`m_SpawnLoc`, set by 0x00C1D370 to the actor's location at launch) to the hit: `< m_fCloseRange` 400 -> x `_CloseRange` 1.5; `> m_fLongRange` 1500 -> x `_LongRange` 1.0; between: x1 | 1.5 below 400 |
| Piercing | on a pawn hit with nothing pierced yet, `rand() / 32767 <= m_fPiercingChance` (1.0) keeps the bullet flying and records the pawn (0x00C3A660); the next pawn's damage is x `m_fPostPiercingDamageRatio` 0.5 and stops it | one pawn, then half |
| Other | `m_fAllowedAngularDeviation` 25, `m_fPostMaxRangeAngle` 20 (a bullet past its range, `m_fMaxRange` = weapon range x max(1, Attribute_Range), deviates), `m_SpreadNumShots` 10 / `m_fSpreadAngle` 40 / `m_fSpreadDamage` 15 (explosive and spread ammunition) | not modelled |

Resulting body damage on a guard: **45 inside 400 uu of the spawn point (about 550 from the eye),
30 beyond, 90 / 135 on the head**, 15 for the second pawn of a pierced line. The port
(`CombatTuning::bullet_damage = m_fDamage`, `left_hand::step_projectiles`): 20 / 30 (x1.5 within 400
of the **eye**) / 60 / 90, no piercing. `ItemsConfig.inventory.pistol_ranged_damage` (10) is read and
never used. **differs** (F1, F2, F3, F4-region).

Head test. Original: the bone hit (physics-asset body) is looked up in a per-pawn bone -> region map
(`eDisHitRegion`: NONE, HEAD, TORSO, LEFT_LEG, RIGHT_LEG, LEFT_ARM, RIGHT_ARM). Port:
`point.z > centre + 0.6 x half height` (`step_projectiles`). The boundary is **open**
(`combat/pistol_height_sweep`). Note the existing `combat/pistol_hit` (pitch -2 at 500 uu from eye
height 173.5) hits the port guard at z 164: a headshot in the port.

## 3. Crossbow (`DisProjectile_Arrow`, `Twk_Projectiles.Twk_Proj_Arrow*`)

| | bolt | sleep dart | incendiary bolt |
|---|---|---|---|
| launch speed | 20000 x 1.0 | 20000 x 0.7 | 20000 x 0.2 |
| gravity multiplier | 1.3 | 0.4 | 0.5 |
| `m_fDamageMultiplier` | 1.0 | 1.25 | 2.5 |
| damage (x 20) | 20 | 25 | 50 |
| `m_fLegsAndArmsDamageCap` | 15 | 25 | 50 |
| `m_bKillOnHeadshot` | yes | no | no |
| `m_fDamageMultiplier_Stealth` | 2.0 | 1.0 (class default) | 1.0 (class default) |
| stealth damage type | Arrow | Arrow_Sleep | Arrow_Flare |
| `m_fChanceToBreak` | 0.7 | 1.0 | 0.0 |

(Launch speed is `m_fArrowFireSpeed` 20000 x `max(1, Attribute_Range)` x the projectile's
`m_fSpeedMultiplier`; INVENTORY.md's sleep-dart speed "1" is wrong, the data and the port use 0.7.)

**Damage rule** (0x00C503D0 after the base 0x00C4A310):
1. base = `m_Damage` (int, `(int)Attribute_RangedDamage` = 20) x `m_fDamageMultiplier`;
2. head region with `m_bKillOnHeadshot`: damage 99999 when the pawn answers the instakill
   interface (0x00B0FAC0); without it, x `m_fHeadshotMultiplier` (`(int)Attribute_RangedHeadshotMultiplier`
   = `(int)1.2` = 1);
3. region HEAD or TORSO: if the pawn's awareness (0x00B0C1E0 -> 0x00B5E250) is Unaware (0), Surprised
   (2), Suspicious (3) or Fearful (4), damage x `m_fDamageMultiplier_Stealth` 2.0 and the stealth
   damage type; AwareOfPlayer, InCombat, Begging and Choked get the plain value;
4. any other region (legs, arms, none): damage capped at `m_fLegsAndArmsDamageCap`;
5. a further bonus attribute of the shooter (`DAT_0145EAC0`, probably the Bloodthirsty power) adds a
   fraction of the damage, and the `m_pDamageType_BloodThirsty2` type follows (not decoded further).

So an idle guard's torso takes **40** from a bolt (sleep dart and flare: the stealth factor is 1.0), its legs **15**, its head dies; an alerted guard's torso 20.
Port: 20 everywhere, kill on the head, no limbs, `bolt_stealth_mult` and `bolt_limb_damage_cap`
parsed and unused. **differs** (F4). Sleep darts and bolts that put a pawn to sleep go through the
same `TakeDamage` with the Arrow_Sleep type (AI.md "Sleep darts"); the port applies the sleep timer
and no damage (`step_projectiles`): **open** whether the 25 reaches the health.

**Arrow behaviour not in the port:** a bolt that hits a pawn pins or sticks (`m_fArrowSinkStruckDistance`
25 / 20 for bolts, 3 for darts; stuck in a live pawn for `m_fStuckInAlivePawnVanishTime` 10 s, with a
dead body it pins the limb at `m_fSpeedModifierWhenPinning` 0.1); bolts that stop in the environment,
a body or the air can be picked up again (`m_bCanRecoverAmmoIn*`, `m_pStatPickup_Single` Bolt_Ammo)
with `m_fChanceToBreak` bolt 0.7, dart 1.0, flare 0.0. The port deletes every bolt at its first
contact and has no bolt pickup. **differs** (F9).

**Crossbow Range upgrade** (`Twk_Upgrade_Crossbow_Range`: `Attribute_Range` +2.0): scales the launch
speed (and, through 0x00C072D0, the weapon range) by `max(1, Attribute_Range)`. The port keeps the
upgrade for the HUD only. **differs** (F5).

Fire timing (shot -> recovery 0.398, fast-reload shot 0.4167) and the reload stages: PLAYER.md, out of
scope.

## 4. Ammunition, magazines, upgrades

- Capacities (Normal): bullets 10, bolts 10, sleep darts 10, incendiary bolts 10, Spring Razors 5,
  grenades 5, sticky grenades 5 (`Twk_Pawn_Corvo_Release.pAttributeTweaksNormal` `m_*Capacity`, copied
  by 0x00B56880). Workshop upgrades add to the capacity attributes (+10 / +10, grenade belt +4 / +4,
  razor pouch +4). **matches** (`combat/ammo_capacity`: `FullAmmo` gives 5 / 5 / 5 / 10 / 10).
  Whether the chambered round is inside the 10 (the port holds `loaded 1` plus `reserve 10`) is **open**.
- Magazine: `1 + (int)Attribute_MagazineBonus` rounds, capped by the ammo held (0x00C20DB0 /
  0x00C28C50). The crossbow holds one. **matches** (`left_hand::load_magazine`).
- Upgrades act on their `m_pHostItemType` only: `Pistol_Accuracy1` max / min dispersion -10;
  `Pistol_Accuracy2` max -15, min -3, speed +20; `Crossbow_Accuracy` max -10, speed +40; magazine
  +1 / +2 / +3; reload speed (Attribute_ReloadSpeed, Attribute_FastReload). **matches**
  (`Dispersion::of`, `gun_reload`). Missing: `Attribute_Range` (F5).

## 5. Grenades and sticky grenades

**Throw** (0x00C38F70): `DishonoredNotify_FireProjectile` at 0.300 s of the throw clip (exit 0.680).
Speed `m_fThrowSpeed` 2500, the aim direction rotated up by `m_fThrowAngle` 5 deg about the
horizontal right axis, **not** added to the thrower's velocity (a drop uses
`velocity . dir + m_fDropSpeed` 800 instead). **matches** (`gadgets::throw_velocity`, no pawn
velocity). The projectile starts at the hand (`m_AlignmentBoneName`, after a line check from the eye
to it), not at the eye (port: `eye_position`): **differs** (F7), size of the offset open.

**Fuse** (`DisGrenadeComponent`, tick 0x00C5A2A0, delay 0x00C58210): `m_fDetonationDelay` 3.0 plus
`Attribute_GrenadeExplosionDelay` for the player's grenades; the timer runs at the actor's time
dilation, is held above `m_fMinDetonationDelayAfterHit` (0.5, but the hit handler 0x00C5A390 zeroes
it) and explodes at zero. A plain grenade is armed from its first tick (hook: exactly 3.0 s whatever it
hits); a sticky grenade by its first impact (not measured). A grenade that takes damage explodes after
`m_fExplosionDelayOnTakeDamage` 0.2 (not in the port, **open**). **matches** (`gadgets::step_grenades`;
`combat/grenade_fuse`, `grenade_bounce`: fuse 3.0 s).

**Bounce** (grenade slots 235 / 236 -> 0x00C5D940, sticky 0x00C5C280). With the surface normal `n` and
velocity `v` at the hit:

- hit on a pawn: `e = m_fBodyBounciness` 0.1, `d = m_fBodyBounceDamping` 0.05;
- hit on the world, `n.z > 0.0001`: at rest if `-v.z < m_fRigidBodyZMagThreshold` (150) `x n.z`,
  else `e = m_fBounciness x (1 - n.z) + m_fFloorBounciness x n.z` (0.3 / 0.65),
  `d = m_fBounceDamping x (1 - n.z) + m_fFloorBounceDamping x n.z` (0.1 / 0.4); `n.z <= 0.0001`
  (wall, ceiling): `e` 0.3, `d` 0.1;
- `v' = (v - (1 + e)(v . n) n) x d`.

Example, floor, `v = (2266, 0, -1302)`: the original keeps `(906, 0, 338)`; the port, which reflects
`v - 2 (v . n) n` and scales the **whole vector** by 0.65 (floor) / 0.3 (wall) and stops below 20 uu/s,
gives `(1473, 0, 846)`. A pawn hit in the port is `-v x 0.1`. **differs** (F6).
`combat/grenade_bounce` prints the port's positions at 0.5, 1.0, 2.0 s and at rest for the capture.

**Sticky grenade** (0x00C5C280): arms (0x00C5A390), then on the hit either bounces with its own
`m_fBounciness` 0.3 / `m_fBounceDamping` 0.1 (same law) or, when the hit flag allows, stops, becomes a
based actor on the hit pawn / mesh and sits `m_fSinkDistance` 5 uu back along its velocity from the
mesh hit point. The port sticks at `hit.point + n x 2` on any world hit and stores the offset on a
pawn. **differs, small** (F6); the stick rule's flag is **open**.

**Explosion** (`DisExplosion`, tweak `DisTweaks_Explosion` of the grenade): per victim 0x00A02E90:
- radii: players `m_fPlayerFullDamageRadius` 150 / `m_fPlayerDamageRadius` 600, everything else 300 /
  600; `d` = distance from the blast to the pawn (the pawn's reference point);
- damage (0x009FEAA0): `Attribute_RadialDamage` (50, all difficulties) x `(1 - (d - full) / (outer - full))`
  clamped to 0..50, rounded to nearest (`ROUND(2x + 0.5) >> 1`);
- line check (0x00A02B60): a MultiLineCheck (flags 0x080A2096) from the blast to the pawn; any blocker
  that is not one of a list of ignorable actors (the pawn's own, its owner, several volume / effect
  classes) zeroes the damage; skipped when `m_bNoLineCheckToPotentialHits`;
- momentum (0x009FAFA0): linear from `m_fRadialMomentum` 1500 (up: `m_fExtraUpForce` 500) at the full
  radius to 0 at the outer radius, handed to `TakeDamage`; the player's `TakeDamage` discards it (U-051),
  NPCs ragdoll;
- sticky: `m_fAttachedDamage` 75 (`m_fAttachedMomentum` 1500) to the pawn it is stuck to, its own type.

**matches**: `gadgets::explosion_damage` (linear between the radii, `round`), the ray to the pawn
(the port blocks on any solid before the pawn's centre minus the capsule radius, the original on any
blocker of the check; equivalent for static geometry, **open** for props); `combat/grenade_explosion_falloff`
shows the port's curve (50 inside 300, 37 at 357, 22 at 457, 6 at 556, 0 at 656) against the formula.
Differences: the port's momentum factor `1 - d / outer` is not the original's (full momentum up to the
full radius) and it pushes players (a multiplayer extension; NPC knock-back applies a velocity here and
a ragdoll there); the sticky's attached damage is added to the radial one (the original applies it as
its own `TakeDamage`; the sum is what the health sees, **open** whether the radial part also hits it).

## 6. Spring Razor (`DisGadget_SpringRazorPlaced`, `Twk_Inv_SpringRazorPlaced`)

Place at 0.200 s of `Gadgets_SpringRazor_Deploy` (exit 0.367). The placed gadget: arming
`m_fArmingTime` 0.5; then (0x00C2D650) the detection cylinder (radius / height 250 x 250, along the
surface normal, sensor `m_fSensorOffsetDistance` 10 off the surface) against the sample points of every
`DisSpringRazorTargetInterface` pawn (NPCs only), with a clear line; trigger reaction `m_fTriggerReactionTime`
0 (0.75 `m_fDamageReactionTime` when set off by damage, e.g. an explosion); detonation; after
`m_fDamageActorDelayTime` 0.35 (0x00C36350) everything in the detonation cylinder (250 x 250, radius scaled
by `Attribute_SpringRazorRadiusBonus` via 0x00C2D360, `m_fMaxDamageDistance` 900) is hit through the same
sample-point test: NPCs lose bones and die (`m_nMinNumBonesToSever` 2 / 3, `m_fImpulseForce` 800), the
`m_ImmuneNPCDamage` (50) applies to NPCs flagged immune (which, not decoded), other pawns take
`m_NonNPCDamage` (5). The port: the same timers (**matches**: `combat/spring_razor_radius` port trigger
0.5 s after placing), sample points = pawn centre and +-1/4 of the height instead of the NPC's own points
(bones through the interface at +0x14; three capsule points when it has none), every NPC killed
(`kill`), no immune NPCs. The cylinder orientation, which of the 240 / 262 uu guards die and the placement
point are **open**.

## 7. Noise and other side effects

`DisTweaks_FirePistol.m_AINoiseLoudnessWhenFiring` = `EAINoiseLoudness4` (6000 uu in `DefaultAI.ini`,
AI.md); the explosion `m_AINoiseLoudness` also Loudness4. The port emits a 4000 radius sound for the
shot and the blast, 500 for a bolt and 300 for a throw (invented values; the crossbow and throw tweaks
carry no loudness field, a projectile may make its noise on impact through its noise-maker interface).
**open** (F11); the hearing check belongs to the AI doc.

## 8. Rule / port matrix

| Rule | Original | Port | |
|---|---|---|---|
| dispersion draw, state, bounds | 0x00BFFBA0, 0x00C1CB10 | `left_hand` | matches (hook) |
| deprojection of the offset | 0x00AA2B10 | `offset_direction` (fixed 75 deg x 16:9) | matches, known diff |
| projectile step | 0x00C546B0 | `step_projectiles`, `step_grenades` | matches |
| launch speed bolt / dart / flare | 20000 x 1.0 / 0.7 / 0.2 | same | matches (`crossbow_trajectory`) |
| throw velocity | 2500, +5 deg, not +pawn | same | matches |
| fuse | 3.0 s armed at the throw | same | matches (hook) |
| explosion falloff / rounding | 0x009FEAA0 | `explosion_damage` | matches |
| pistol base damage | 10 + 20 | 20 | **differs** F1 |
| spawn point (+150), muzzle, range band origin | 0x00C2A3E0 / 0x00C38230 | eye | **differs** F2 |
| bullet piercing | 0x00C3A660 | none | **differs** F3 |
| crossbow stealth x2 / limb cap / regions | 0x00C503D0 | none | **differs** F4 |
| Attribute_Range | 0x00C38230 / 0x00C072D0 | none | **differs** F5 |
| grenade bounce law | 0x00C5D940 | `step_grenades` | **differs** F6 |
| grenade start at the hand | 0x00C38F70 | eye | **differs** F7 |
| aim assist | 0x00C14460 | none | **differs** F8 (gate open) |
| bolt recovery, pinning | arrow hit handler 0x00C4FEA0 | none | **differs** F9 |
| razor sample points, immune NPCs | 0x00C36350 | simplified | **differs** F10 |
| AI noise radii | tweaks / DefaultAI | invented radii | open F11 |

## 9. Scenarios (`fixtures/oracle/scenarios/combat/`)

All use the Playground lane (the player at (-5600, 0, 96), eye z 173.5, guards at centre z 92, passive,
port start health 70), one guard per shot on its own bearing so a shot never meets a neighbour. They
are queued in `fixtures/oracle/capture_queue.txt`; the originals' guards will have their own health, so
the checks use the health **lost** (`DropFrom`), never the final value.

| Scenario | Rule | Port prints | Prediction (original) |
|---|---|---|---|
| `pistol_damage_range` | close band from the spawn point, long range, point blank | 30 / 20 / 20 / 20 / 20 / 30 at 250 / 500 / 600 / 1200 / 1700 / 120 | 45 / 45 / 30 / 30 / 30 / 45 (or 0 at 120) |
| `pistol_height_sweep` | head region boundary | 20 below aim z 145, 60 above | 30 body, 90 head, boundary open |
| `pistol_pierce` | pierce + 0.5 | first 20, second 0 (check fails: Static) | 30, 15, 0 |
| `crossbow_damage_regions` | stealth x2, limb cap, head kill | 70 (dead), 20, 20, 20 | 70 (dead), 40, 40 or 15, 15 |
| `crossbow_trajectory` | launch speed, drop, flight time | 20001 / 14000 / 4000 (passes, Static) | trajectory by capture |
| `grenade_bounce` | bounce law, rest position, fuse | x at 0.5 / 1.0 / 2.0 s: -4755 / -4019 / -2921, blast at x -2373 | by capture |
| `grenade_explosion_falloff` | radial curve (sticky on the floor, ring of six guards) | 22 / 0 / 50 / 37 / 50 / 6 | 24 / 0 / 50 / 40 / 50 / 7 from the nominal impact |
| `spring_razor_radius` | trigger time, which guards die (razor under the player, ring 120-310) | trigger 0.5 s after placing; 120 / 200 / 240 die, 262 / 310 live | by capture |
| `ammo_capacity` | capacities | 5 / 5 / 5 / 10 / 10 pass | static from the tweaks |

The pre-existing `pistol_hit`, `crossbow_hit`, `grenade_fuse`, `spring_razor` are queued for their
`Unmeasured` damage values too.

## 10. Findings

Each: diagnosis, evidence, the fix for the port. "Port bug" = the port's rule is wrong; "port missing" =
a rule is absent.

**F1 Pistol base damage (port bug, high confidence).** The bullet's damage is the **sum** of the
projectile's `m_Damage` (`(int)Attribute_RangedDamage` of the pistol item, 10) and the tweak's `m_fDamage`
(20) (0x00C29820; `cvtsi2ss [ebx+0x27C]; addss xmm0,[eax+0xE8]`). The port uses `m_fDamage` alone
(`tuning.rs:314`). Fix: `bullet_damage = trunc(pistol Attribute_RangedDamage with upgrades) + m_fDamage`
(`GameplayConfig.items.inventory.pistol_ranged_damage` already holds the 10); keep x3 headshot. Result
30 / 45 / 90 / 135. Confirm with `pistol_damage_range` (the guard's health lost; difficulty or NPC
armour scaling in `TakeDamage` 0x00B5CCD0 is not excluded).

**F2 Spawn point and range-band origin (port bug).** Bullet and bolt start 150 uu in front of the camera
along the undispersed view (`m_fBulletSpawnDistance` / `m_fArrowSpawnDistance`); close and long-range
distances are measured from that point (strict `< 400`). Port: the eye, `dist <= 400`. Effect: the 1.5x
band is 150 uu too short; a shot at a target under 150 uu is a capture question (`pistol_damage_range`,
guard 120 uu away). Fix: spawn at `eye + view_dir x 150`, fly along the dispersed direction, measure
from the spawn point. The crossbow's muzzle-socket spawn (aimed at the camera ray's hit point when the
muzzle has a clear line to the same actor) needs the socket offset; measure it from the first projectile
position (`crossbow_trajectory` capture).

**F3 Bullet piercing (port missing).** `m_fPiercingChance` 1.0: the first pawn hit never stops the bullet;
the second takes x0.5 (`m_fPostPiercingDamageRatio`) and stops it (0x00C3A660, 0x00C29820, sweep
recursion up to 10 in 0x00C4F3A0). Fix: `ProjectileState` gains `pierced: Option<PlayerId>`; on the first
pawn hit apply damage, record, continue the sweep; on the second apply x0.5 and end; consume one
`rand()` draw per first hit (`rand() <= chance`, always true at 1.0, but the draw advances the stream).
Check: `pistol_pierce` fails in the port today.

**F4 Crossbow damage rules (port missing).** Stealth x2 for head / torso hits on Unaware / Surprised /
Suspicious / Fearful pawns, limb cap (bolt 15, dart 25, flare 50), region classification from the
pawn's bone map. The sim has head and body only. Fix: add a hit-region function (`Head`, `Torso`, `Limb`)
with boundaries taken from the `pistol_height_sweep` capture (and lateral for arms), the awareness
mapping from the AI crate (`unaware_of` generalised to the four stealth states), then
`dmg = base x (2 if stealth) ` for head / torso, `min(base, cap)` for limbs. The same region function
fixes the pistol head test (F1 section 2).

**F5 `Attribute_Range` (port missing).** Crossbow launch speed (and range) x `max(1, Attribute_Range)`;
`Crossbow_Range` adds 2.0. Fix: apply in `CombatTuning`/`left_hand::update` (the host-restricted
attribute helper already exists). The attribute's base on the crossbow is open (queue: `session dump
class=DisWepCrossbow filter=Attribute`).

**F6 Grenade bounce (port bug).** Implement 0x00C5D940: `v' = (v - (1 + e)(v . n) n) x d` with `e`, `d`
blended by `n.z` (floor / wall), pawn values 0.1 / 0.05, at rest when `-v.z < 150 n.z`; the sticky's own
0.3 / 0.1 and the 5 uu sink. Remove the `< 20 uu/s` stop and the whole-vector scaling. Capture:
`grenade_bounce`.

**F7 Throw origin (port bug, size open).** The thrown grenade starts at the item's alignment bone with a
line check from the eye (0x00C38F70). Capture the first projectile position, fix as an eye-relative
offset (rotated by the view) in `left_hand::update`.

**F8 Aim assist (port missing, gate open).** Corvo's pistol and crossbow tweaks enable it for the mouse;
the target choice is 0x00C14290 with `m_AimAssistChoiceInfo` (angle 45 deg, weight 10000 / 1 per
distance, 4000 / 2000 uu). Decide with a capture whether it is active on the default mouse settings
(shot at a guard 1.5 deg off the crosshair).

**F9 Bolt recovery, pinning (port missing).** Stuck bolts, 10 s vanish in a live pawn, pickups with
`m_fChanceToBreak`. Fix in `step_projectiles` (stuck state, `AmmoType::Bolt` pickup) once the pickup
system takes dynamic pickups.

**F10 Spring Razor details (port simplified).** NPC sample points, `m_ImmuneNPCDamage` 50, bone
severing (visual), damage-triggered reaction 0.75 s is present. Needs the immune flag's source
(0x00C35D60) before a fix.

**F11 AI noise radii (open).** 6000 uu Loudness4 for the pistol and the blast against the port's 4000;
crossbow 500 and throw 300 have no tweak behind them. Belongs to the AI hearing check.

**F12 Docs.** INVENTORY.md "sleep dart speed 1" should be 0.7; its "(0x00C257A0, 0x00C25D80)" are the NPC
fire functions (`DisItemContext_NPCFireGun`); the player's are 0x00C2A3E0 (pistol) and 0x00C38230 (crossbow).

### Port status (2026-10-08)

| Finding | Status | Where |
|---|---|---|
| F1 | **ported**: `bullet_damage = trunc(pistol Attribute_RangedDamage) + m_fDamage` = 30. Capture: 45 at 300 uu (close band), matches | `tuning.rs` `CombatTuning::from_config` |
| F2 | **ported**: bullet spawns at camera + `m_fBulletSpawnDistance` along the undispersed view, the crossbow (0x00C38230 re-read) at camera + `m_fArrowSpawnDistance` along the **dispersed** direction it flies; `ProjectileState.spawn`; bands from the spawn point, close strictly `<`, long strictly `>` (`m_fLongRange` / `_LongRange`), stealth factor unless in combat (`bullet_damage`). **Left open:** the bolt's muzzle-socket spawn when the camera and muzzle rays reach the same actor (socket offset needs a capture) | `left_hand.rs` `fire_gun`, `bullet_damage`, `step_projectiles` |
| F3 | **ported**: on a bullet's first pawn hit, one draw of the shooter's stream `<= m_fPiercingChance` keeps the sweep going past the pawn (`pierced`); the next pawn takes x`m_fPostPiercingDamageRatio`. `pistol_pierce` now passes (30 / 15 / 0) | `step_projectiles` |
| F4 | **ported**: `arrow_damage` (head kill or x`(int)Attribute_RangedHeadshotMultiplier`; head / torso x`m_fDamageMultiplier_Stealth` when the AI reports the pawn unaware; other regions capped). Regions: `hit_regions.rs` classifies the shot against the elite guard's `Skm_EliteGuard_Physics` bodies in the bind pose (loaded by `npc_anim::load_hit_bodies` into `items.hit_bodies`, fitted to the pawn's height and yaw), bone -> region by name (`HitRegion::of_bone`: neck = head, the asset has no head body; the coat panels = None, so capped). The pawn's `m_HitRegions_Map` source was not found: the name mapping is the port's reading. Capture: 40 on an idle guard's torso, matches. Passive scenario dummies have no brain, so `ai_unaware` is false and they take 20 | `left_hand.rs`, `hit_regions.rs`, `dishonored_assets::npc_anim` |
| F5 | **ported**: crossbow launch speed x `max(1, Attribute_Range)` (no attribute tweak carries a Range base: 0, +2 with `Crossbow_Range`). The range part (0x00C072D0) has no counterpart in the sim | `crossbow_range_scale` |
| F6 | **ported**: 0x00C5D940 law (wall / floor pair blended by `n.z`, pawn pair, rest when `-v.z < m_fRigidBodyZMagThreshold n.z`), the 20 uu/s stop removed; the sticky grenade on a pawn sits `m_fSinkDistance` back along its velocity. The sticky's stick-or-bounce flag stays open (it sticks) | `gadgets.rs` `bounce`, `world_bounce`, `step_grenades` |
| F7 | **left open**: the hand offset is not in the data (capture the first projectile position); the throw still starts at the eye | |
| F8 | left open (gate) | |
| F9 | **left open**: the hit handler 0x00C4FEA0 sticks / pins (velocity x`m_fSpeedModifierWhenPinning` on a dead pawn's bone, vanish timer `m_fStuckInAlivePawnVanishTime`), but where `m_fChanceToBreak` and the recovery pickup are applied was not found (base `DisProjectile_Arrow` slots 246-248 not decoded); pinning needs ragdoll bodies the sim does not have | |
| F10 | **ported** (0x00C36350, 0x00C35D60, 0x00C2D650, 0x00BC0280): cylinders from the trap's location along its normal to the height, `0 < h < H` and `r^2 < R^2` strict; a pawn without the razor interface offers its centre and +- 2/3 of its half height (NPCs answer through the interface: not decoded, the same points stand in); `m_fMaxDamageDistance` is handed to the damage, not a cull. Correction: the damage rule is the reverse of section 6: an NPC whose interface accepts the cut takes `m_NonNPCDamage[difficulty]` and is severed (the capture's 5 then death), a pawn **without** the interface (the player) takes `m_ImmuneNPCDamage` 50. Which NPCs refuse the cut is not decoded (none) | `gadgets.rs` `step_traps` |
| F11 | **ported**: pistol `m_AINoiseLoudnessWhenFiring` and explosion `m_AINoiseLoudness` read by name (`EAINoiseLoudness4` -> 6000); the crossbow's and the throw's invented 500 / 300 noises removed (their tweaks have no loudness). An impact noise of the projectiles' noise-maker interface is not decoded | `gameplay.rs`, `items_config.rs`, `fire_gun`, `explode` |
| F12 | **done**: INVENTORY.md sleep-dart speed 0.7, fire functions, Spring Razor damage | |

**F13 Bullet blast (new, left open).** The capture shows 15 `DishonoredDamageType_BulletBlast`
besides the 45. Decoded (0x00C36E50, called from the bullet's tick 0x00C3A500 on its first ticked
step, and from its hit handlers when not yet `m_bBlasted`): origin = `m_ShotsLocation` (the
muzzle socket, set by the fire 0x00C2A3E0) + `m_ShotsDirection` x `m_fSpreadDistanceFromBullet`
(-200); every pawn but the pierced one within `m_fCloseRange` (400) of it whose bounding-box
centre gets a positive Windblast-style factor (0x00BE8570, half angle `m_fSpreadAngle / 2` = 20
deg) and is the first thing a trace from the origin meets takes `ROUND(m_fSpreadDamage)` = 15
(not scaled by the factor) with momentum x`m_fSpreadForce`, at most `m_SpreadNumShots` (10)
pawns; the remaining shots are spawned as spread pellets (0x00C21BA0). Not ported: the muzzle
socket offset is unknown and the trace does not exclude the shooter, so whether the shooter or
the target took the measured 15 needs the capture's per-pawn damage.

The `crossbow_hit` trail check now fails: a bolt spawned 150 uu ahead reaches the guard 500 uu
away in its launch tick and is never in a snapshot, so the presentation (`attached_want`) never
starts its trail. The trail is attached at launch in the original; the presentation needs to
start (and at once stop) it from `SimEvent::Fired` for a projectile that ended the same tick.

## 11. Open questions and captures

- Pistol and crossbow damage on a guard (F1, F2, F4): the queued scenarios; each is a health-lost read.
- Hit-region boundary (`pistol_height_sweep`), and whether a body shot at 120 uu connects.
- Bolt, dart, flare flight: per-tick location (`crossbow_trajectory`); the muzzle offset (F2).
- Grenade rest and bounce positions (`grenade_bounce`); the blast position and the guards' losses
  (`grenade_explosion_falloff`); which razor guards die and the trigger time (`spring_razor_radius`).
- Attributes read by name: pistol / crossbow `Attribute_RangedDamage`, `Attribute_Range` (queue
  `session dump` lines).
- Aim-assist gate; sleep-dart health effect; `m_fExplosionDelayOnTakeDamage` wiring; instakill
  immunities (0x00B0FAC0).
