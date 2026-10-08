# Physics, movers and ragdoll: the core model

Scope: what the original's PhysX 2.8 rigid bodies decide in gameplay (thrown and ragdolled NPCs,
corpses, Windblast on props, breakables, KActors), the rules the port must reproduce, and what
can stay presentation-only. Evidence levels: CONFIRMED (read from the decompiled exe / game data
/ the cached original trace), LIKELY, UNVERIFIED (needs a capture; never judged).
Addresses are virtual addresses in the GOG exe (sha256 `70da9613...`). Doors and InterpActor
movers are in world.md section 3 and COLLISION.md; this file only adds what physics changes.

Method note. The original has no per-body channels in any trace: `npc.<id>.x/y/z/health/physics`
only (the pawn Location, which during RigidBody physics is the root body, see 3.6). Everything
below that is a number is either a tweak / class default, a decompiled formula, or read off
the cached original trace of `powers/windblast_guard_l1` (60 Hz, Boyle runway, level 1).

## 1. What the original does with physics

| thing | mechanism | gameplay effect |
|---|---|---|
| NPC thrown by Windblast | `StateNPCMasterThrown` (vtable 0x0110A2B0): animated flight in `PHYS_Flying` (4), then ragdoll (`PHYS_RigidBody` 10), then back to Walking when it comes to rest | position of the NPC, damage from impacts, wall-hit kill (powers.md F-06), where the body ends |
| NPC death | `DishonoredNotify_Ragdoll` in the death animation hands the pawn to PhysX (ANIMATION.md section 8); `m_bRagdollOnDeath` is false for every Boyle tweak | corpse position: the pawn Location follows the root body from then on (chaos / discovery read the pawn Location) |
| Windblast on props | the gather (0x00BF6780) takes every actor in range; a hit calls `TakeDamage` (vslot +0x148) and, for a physics component, a component impulse (vslot +0x1A8) with a fixed vector (3.4) | props fly, rattle, break, make contact noises |
| KActors, physics props | `DishonoredKActor` / `DishonoredMovable` / `DisWhaleOilBattery` (`PHYS_RigidBody`): the ShouldTrace table (COLLISION.md): only the vision line and the crosshair meet them, pawns walk through | cosmetic for the pawn; the Windblast and breakables are the only things that move them in the missions |
| Breakables | `DisSkeletalBreakable` health / threshold / steps (3.5) | Kismet `BreakableBroken` outputs, loot, NPC attraction |
| Corpse carrying and dropping | `StatePlayerCarryCorpse*`: no throw (takedown.rs comment, U-106) | the body drops at the carrier's feet |
| Thrown objects | NPC `DisItemContext_NPCThrowObject` (m_fThrowSpeed, spin) and `DisTweaks_ThrownObjects` (3 skeletal throwables with weight 1 each: metal piece, brick, broken bottle); projectile `m_fMass`, `m_fHitMomentumFactor`, `m_fGravityMultiplier` | belongs to ranged.md / ai.md (projectile flight), not to this file |
| Player Windblast | none: the player pawn path (0x00B5CCD0) passes the momentum to the hit reaction handlers and clears it | nothing |

The port: NPC and player pawns are one `PlayerState` with a capsule; dead bodies fall as a point
(`takedown::step_body`); `crates/dishonored_runtime/src/ragdoll.rs` is a presentation-only Verlet
ragdoll started at the death notify, with the sim's body position authoritative; there is no
prop, KActor or breakable in the sim (world.md section 4).

## 3. The core rules

### 3.1 Thrown-NPC velocity: Velocity = momentum / 2 (CONFIRMED)

`DisTweaks_ComponentHitReactionThrow` (`Twk_Component.Twk_Component_HitReactionThrow_NPC`, config
at tweak +0x8C) holds `m_fEffectiveMassForThrowing` = **2.0**. The helper 0x00BEB6E0 returns
`input / EffectiveMass` (2.0 when the tweak is absent). The throw start (0x00A81860, called from
the state's entry 0x00A86B90) does:

```
v = m_vMomentum / m_fEffectiveMassForThrowing        // 0x00BEB6E0
pawn.AirSpeed-like field (+0x2A4) = |v|
SetPhysics(PHYS_Flying = 4)                          // vslot +0x218
pawn.Velocity (+0x1B4) = v ; pawn.Acceleration (+0x1C0) = constant
```

The state tick (0x00A98DD0) adds the gravity of the physics volume to `Velocity.z` every tick
(`Velocity.z += gravity * dt`, 0x006A1710), so the flight is a ballistic arc with the Flying
physics' fluid drag. Measured on the cached original trace (L1, momentum (1582, 825) =
k 0.879 x (1800, 939)): physics 4 on the damage tick, first step 13.1 uu per tick along the aim
(786 uu/s = 1582 / 2 less the first tick of drag), decaying by 0.99500 per tick (25-tick
ratio 0.88239: `CalcVelocity` 0x006ACD50 with friction `FluidFriction / 2` = 0.15, fluid, no
braking, applies `v (1 - 0.15 dt)` twice: 0.995006; port note 2026-10-08), z rises 6.8 uu on
the first tick (no gravity in the first step: the state adds it after the move) and the arc peaks 55.9 uu above the
start (`(825 / 2)^2 / (2 x 1500) = 56.6`: gravity about 1500 uu/s^2 on this map).

Port: `left_hand::step_windblasts` adds the whole `windblast_momentum` to `velocity` and sets
Falling. That is 2x the original's velocity (the oracle: `physics/windblast_throw_l1_flight`
fails, apex 109.8 against 55.9).

### 3.2 Minimum momentum (CONFIRMED)

The state entry (0x00A86B90) runs `FUN_00BE8710(&m_vMomentum, min2D, minZ)` on the momentum
before 3.1: if the horizontal magnitude is below `m_fMinMomentum2DMagnitude` it is **scaled up** to
it, and `z` is raised to at least `m_fMinMomentumZ`. Class defaults of `StateNPCMasterThrown`:
`m_fMinMomentum2DMagnitude` 800, `m_fMinMomentumZ` 0, and, when the NPC is coming from a stun
(flag +0x68 bit 5), `m_fMinMomentum2DMagnitudeWhenStunned` 3000 and `m_fMinMomentumZWhenStunned`
1500. So a weak push that throws at all leaves at 400 uu/s horizontally, and a stunned NPC
(the stun reaction) is thrown at 1500 uu/s horizontally and 750 uu/s up.

### 3.3 Which reaction a hit gets (thresholds CONFIRMED as data, the consumer UNVERIFIED)

The same tweak holds `m_fStrongHitReactThreshold` 500, `m_fAnimRagdollThreshold` 1000,
`m_fFullRagdollThreshold` 3800, `m_fRagdollMoveEndThreshold` 10, `m_bShouldNotRagdollWhenAlive`
false. They are compared with the magnitude of the momentum (the throw state's own checks read
the tweak flag at 0x00A87340 / 0x00A872E0; the code comparing 500 / 1000 / 3800 was not found in
the decompile, the pawn reaction switch 0x00B5CCD0 -> vslots +0x694..+0x6B0 is the candidate).
Reading: below 500 only the small hit reaction; 500..1000 the strong hit reaction (no throw);
1000..3800 the animated throw (Flying, then ragdoll); above 3800 a full ragdoll at once. The
level 1 Windblast reaches `|momentum|` 1784 at 183 uu: an animated throw, which the trace shows
(physics 4 for 27 ticks, then 10). The Windblast damage type has `m_bCanThrow` and
`m_bForceThrowOnDeath` true, `m_bDramaticSlowmo` true, `KDamageImpulse` 0.
Passive scenario dummies (no controller) go straight to RigidBody (ORACLE.md).

Sweep for the capture: `physics/windblast_throw_d300 / d600 / d800 / d900` put `|momentum|` at
1627 / 1259 / 1044 / 946: the first three should throw, the last should not (if the thresholds
are on `|momentum|` as read).

### 3.4 Windblast numbers per target class (CONFIRMED)

For a target at 3D distance d with blast factor k (powers.md section 3), force F, extra up U0 and
range R, the vector built by 0x00BF6AF0 is `(F x flat aim, z = max(aim.z x F, U0 x (1 - min(d / R, 1))))`,
normalized to a direction `dir` with length `L = sqrt(F^2 + U^2)` (U the raised z), and then:

- **NPC pawn** (vslot +0x37C = 0): `TakeDamage(round(damage x k), momentum = dir x L x k)` - this
  is the measured (2362, -227, 1317) at level 2: `|momentum| = 2712 = 0.879 x 3085`. Then 3.1.
- **Component of a physics prop** (second element of the gather pair): vslot +0x1A8 of the
  component with the impulse `(dir.x x L x k, dir.y x L x k, 0.15 x L x k)` (0x00BE9770: the z is
  a fixed 0.15 of the magnitude, not the raised z). The `AddImpulse` argument layout is
  UNVERIFIED (the call passes the vector, then zeroed position, bone and flags).
- **Projectile**: no damage; it reacts by its tweak's `m_WindblastBehavior` (powers.md F-07).
- Prop `TakeDamage` applies an impulse only when the damage type's `KDamageImpulse` > 0
  (0x00785FE0): Windblast's is 0, so props move by the component impulse only.

### 3.5 Breakables (CONFIRMED; answers world.md U-W6)

`DisSkeletalBreakable` damage (vslot +0x3A4, 0x00A283A0), after the instigator bookkeeping:

```
if not immune(damageType) and (vulnerable list empty or damageType in it):
    if m_CurHealth > 0:
        if damageType == (one special type, address 0x013392C0, class not resolved) or damage >= m_DamageThreshold:
            m_CurHealth = max(m_CurHealth - damage, 0)
            update steps                                   // vslot +0x3A8
```

`m_DamageThreshold` defaults to 5, `m_Health` -1 (unbreakable: `m_CurHealth` never positive).
The step update (0x00A284B0) scans the break-step table from the top: the new wanted step is the
highest step whose health fraction value is `>=` the current health (`steps[i] >= m_CurHealth`),
it plays the steps from `m_NextStep` up to that one (vslot +0x3AC) and sets `m_NextStep = wanted + 1`.
The three Kismet "Broken 100 / 66 / 33 %" outputs fire as steps are reached.

### 3.6 The pawn Location during a ragdoll (CONFIRMED from the trace)

On the physics switch 4 -> 10 (tick 52 -> 53) the guard's Location jumps 151 uu along the throw
and **89 uu up** in one tick (z -178.2 -> -89.3): the pawn Location is the root body of the
ragdoll (`Root_jnt`, the pelvis box), not the capsule centre. Chaos, vision and "who sees the
body" read the Location, so for a corpse that is the pelvis position, about 89 uu above the
standing capsule centre at the hand-over and settling to the body's rest height.

### 3.7 End of an alive ragdoll (CONFIRMED from the trace, mechanism LIKELY)

The trace: RigidBody from tick 52 to 119, speed falling from 1090 to 14 uu/s by tick 115, then
physics Walking (1) at tick 120 at the ragdoll's final place, and the guard resumes patrolling.
`m_fRagdollMoveEndThreshold` = 10 uu/s is the threshold (UNVERIFIED which speed is compared).

### 3.8 Impact damage on a ragdolling NPC (CONFIRMED observed, rule UNVERIFIED)

The L1 trace loses 10 health at tick 75 and 12 at tick 79 (33 -> 23 -> 11) with the guard
hitting something near y -4175 (`DisDamageType_RagdollImpact`, a child of `DisDamageType_Impact`).
The only decoded rule is the Windblast wall hit (0x00B529C0, powers.md F-06: contact normal
wall-like, pawn speed above the tweak's +0x1E0, damage +0x1E8). The 10 and 12 here are not that
rule's 100; the speed-to-damage function of the ragdoll contact is not decoded.

### 3.9 The ragdoll impulse (model CONFIRMED, the unit scale UNVERIFIED)

When the throw state starts as a full ragdoll (`m_bRagdoll`), or after the Flying phase, the tick
(0x00A98DD0) applies `J = |v| ` (where `v = momentum / 2` by 0x00BEB6E0 when `m_bUseMomemtumAsForce`,
else the pawn's current `Velocity`) along `normalize(v)`:
`FUN_00BF0D30` / `FUN_00BF0B50`: the mesh's body list is fetched (vslot +0x23C), `J / N` is
computed for the N bodies (`N` = number of entries), and every non-fixed body (flag 0x80 clear)
receives two PhysX impulses: the horizontal part `dir.xy x J / N` in force mode 3
(`NX_SMOOTH_IMPULSE`, LIKELY: the mode ints match the PhysX 2.8 `NxForceMode` enum) and the vertical part `dir.z x J / N` in mode 1 (`NX_IMPULSE`). So the
**centre of mass receives `J` in momentum units, shared equally by the bodies, and each body's
speed change is `J / (N x its own mass)` times the PhysX scale**. The total momentum is
independent of the individual masses; a body's speed is not.

Measured: the level 2 hit on a Passive guard peaks at 1005 uu/s horizontally from a horizontal
`momentum / 2` of 1184 (ratio 0.85); in the level 1 trace the ragdoll phase peaks at 1090 uu/s
for a horizontal `momentum / 2` of 791 while entering with the Flying velocity (about 690
horizontally) - the two do not reduce to one ratio with the information in the traces (the
measured speed is the pelvis, which rotates and bounces; the masses are in the physics asset:
`MassScale` 1.5..2.0 per body, `PhysicalMaterial` density 1.0). Capture P-5 in the queue
(per-body velocities and masses) is what decodes it.

Material constants of the ragdoll (physics asset `Npc_EliteGuard.Physics.Skm_EliteGuard_Physics`,
18 bodies, 12 capsules, 6 boxes): body material `RGDL_Body` (angular damping 2,
linear 1), limbs `RGDL_Limb` (linear damping 1), head `RGDL_Head` (linear damping 1); the
engine defaults friction 0.7, restitution 0.3, density 1.0. The port's `DAMPING` 0.985 per step
and `FRICTION` 0.6 are visual guesses.

## 4. Port status per rule

| rule | port | verdict |
|---|---|---|
| 3.1 velocity = momentum / 2, Flying with gravity + drag | `velocity += momentum`, then Falling | **bug** (2x; the wrong physics) |
| 3.2 minimum momentum 800 / 3000 | none | **missing** |
| 3.3 reaction class by `|momentum|` | any hit throws | **missing**, consumer UNVERIFIED |
| 3.4 prop impulse `(dir.xy, 0.15)` | no props | missing (out of scope, see 7) |
| 3.5 breakable threshold / steps | no breakables | missing (world.md section 4) |
| 3.6 Location = root body | the sim keeps the capsule; the presentation ragdoll moves bones only | **known difference**, matters for discovery |
| 3.7 end of an alive ragdoll | Falling -> Walking when landed | LIKELY close, timing unmeasured |
| 3.8 impact damage 10 / 12 | none (except the wall hit) | **missing**, UNVERIFIED |
| 3.9 ragdoll impulse and masses | presentation Verlet, not driven by the momentum | presentation-only |

## 5. Verification of the ranged porter's `matrix_origin` finding

`dishonored_assets::npc_anim::matrix_origin` (the capsule centre of a physics body), Raw branch:
reads `b[48..52], b[52..56], b[56..60]` = floats 12, 13, 14 of the 64 cooked bytes. The
cooked `Matrix` stores each plane as (W, X, Y, Z) - `matrix_rows`, below it in the same file,
already says so and reads floats (1, 2, 3), (5, 6, 7), (9, 10, 11), (13, 14, 15). Checked on
`Skm_EliteGuard_Physics.RB_BodySetup_159` (`spine_3_jnt`, one capsule, Radius 14.75, Length 17.0):
floats 11..15 of the TM bytes are 1.0, 7.25, 2.8e-13, 1.9e-9, 0 - the W plane is `(1, 7.25, ~0, ~0)`,
so the capsule centre is (7.25, 0, 0) and `matrix_origin` returns (1.0, 7.25, ~0): **CONFIRMED, one float
off**. The fix is `Some([f(13), f(14), f(15)])` (or reuse `matrix_rows(v)?[3]`). The current impact
is nil: `PhysBody.center` and `.length` are written by `read_physics_asset` but nothing reads them
(`ragdoll.rs` builds its bodies from the bone positions; `HitBody.center` in `hit_regions` comes from
`matrix_rows`, which is right). It becomes a bug the moment a sim ragdoll uses the capsule centres.

## 6. Findings

| id | class | finding | fix for the port |
|---|---|---|---|
| P-1 | **port bug** | A Windblasted NPC leaves with the whole momentum as velocity (about 1580 uu/s at level 1 against 790); the original divides by `m_fEffectiveMassForThrowing` 2.0 (0x00BEB6E0 / 0x00A81860). `physics/windblast_throw_l1_flight` fails (apex 109.8 vs 55.9) | `velocity = momentum / throw_effective_mass` with the tweak value (load `Twk_Component_HitReactionThrow_NPC.m_ComponentConfig.m_fEffectiveMassForThrowing`) |
| P-2 | **port missing** | Flying phase: the original NPC is `PHYS_Flying` for about 27 ticks (0.45 s), gravity added by hand, drag about 0.55 %/tick, no ground friction, then RigidBody; the port uses Falling | add a throw mode: velocity integrated with gravity + `FluidFriction 0.3 x dt` drag, ends after the timer (3.7: cause UNVERIFIED, candidates `m_fBodyAtRestTestTimer` 0.5, anim end) |
| P-3 | **port missing** | Minimum momentum clamp (800 horizontal, 3000 / 1500 when stunned) before P-1 | apply 3.2 to the momentum, then P-1 |
| P-4 | **port missing, UNVERIFIED consumer** | Reaction class by `|momentum|` against 500 / 1000 / 3800 | capture `physics/windblast_throw_d*`, then gate the throw on the class |
| P-5 | **UNVERIFIED** | Ragdoll velocity per momentum (1005 uu/s at L2, 1090 at L1): the impulse model is decoded (3.9), the masses and scale are not | capture P-5; the sim only needs the pawn Location path: fit `speed = c x J_h` or model bodies |
| P-6 | **known difference, matters** | The pawn Location is the root body during RigidBody (89 uu above the capsule centre at the switch); corpse placement for discovery uses it | for a corpse use the ragdoll root's rest position (see 7) |
| P-7 | **port missing** | Ragdoll impact damage (10 and 12 at the L1 trace's two impacts), `DisDamageType_RagdollImpact` | capture the damage path (queue P-8), then the speed-to-damage rule |
| P-8 | **cross-system (powers)** | Windblast timing: the original's damage lands 20 ticks after the press (cached L1 trace: press tick 5, health drop tick 25; ORACLE.md: 19 ticks at L2); the port's lands 9 ticks after (tick 14) | the cast is driven by the `DisNotify_WindBlast` animation notify, not the 0.1 s timer: 0.33 s from the press; see powers.md |
| P-9 | **port bug (latent)** | `matrix_origin` off by one float (section 5) | read floats 13..15 |
| P-10 | **decoded, not ported** | Breakable damage rule (3.5) | implement in the breakable actor (world.md section 4) |
| P-11 | **oracle limitation** | No per-body or velocity channel for NPCs in the original trace; ragdolls fall through the Playground (U-806) | ragdoll scenarios must use real maps (Boyle); a capture with body velocities (P-5) |

## 6a. Port status (2026-10-08)

Decoded during the port (static index, re-read):

- **The end of the Flying phase (P-2).** The throw state's enter (vslot +0x124, 0x00A9FAA0)
  registers the callback 0x00A7CBA0 for pawn event 5; the callback sets `m_bRagdoll` and
  `m_bImpulseNextFrame` (the next tick applies the ragdoll impulse from the current Velocity).
  Event 5 is fired by the NPC pawn's ragdoll start (vslot +0x580, 0x00B6FBB0), which
  `DishonoredNotify_Ragdoll` calls (0x00A8CFE0). The Windblast throw plays
  `Npc_EliteGuard_Generic_as` `Generic_WindBlast_{Front, Left, Right, Back}` (action 7 with
  `m_bWindblast`), whose ragdoll notifies are at 0.4550 / 0.3900 / 0.4483 / 0.4179 s. The L1
  trace: hit at tick 25, physics 10 at tick 52 = the 28th animation update (28 / 60 >= 0.455 >
  27 / 60). CONFIRMED. Which of 1 / 2 is Left / Right (`ECardinalDirection` not in the index:
  the throw start turns the pawn by +90 / -90 for 1 / 2) is LIKELY.
- **The cast delay (P-8).** `DisNotify_WindBlast` (0x00A96AE0 -> 0x00C44E10) starts the blast
  (0x00BF9570, origin and direction) directly; the power's warmup (0.1 s) only leads to the fire
  mode 0x00BF9720, which waits while the cast action plays. The notify is at 0.27 s in
  `Ply_Powers_as` `Powers_Cast_Windblast`. CONFIRMED.
- **The reaction class (P-4).** The damage path 0x00B5CCD0 gets its reaction from 0x00B57E60 ->
  pawn vslot +0x6C8 (0x00B57F00): the damage type's `m_ExpectedHitReaction` when set (Windblast
  leaves it 0), else 4 or 3 by a flag of the pawn's tweak (+0x100 -> +0x90 bit 1); 4 calls vslot
  +0x694 (the NPC hit react, 0x00B733F0). No read of the 500 / 1000 / 3800 thresholds was found
  in the decompile (a scan for `+0x118` pointer reads at config offsets 0x90..0x9C finds none).
  Not ported.
- **The rest position (P-6).** The L1 trace weakens the pelvis model: after the impacts the
  guard's Location sits at z -210.00 for ticks 79..84 and settles at -210.18 (standing capsule
  centre -210.09), i.e. at the capsule-centre height, not at a lying pelvis. The +89 uu jump at
  the switch is not explained. The sim's bodies already rest at the capsule-centre height. Not
  ported.

| id | status | where |
|---|---|---|
| P-1 | **ported** | `movement::start_throw` (velocity = clamped momentum / `ThrowConfig::effective_mass`), called by `left_hand::step_windblasts` instead of adding the momentum |
| P-2 | **ported** | `movement::flying` (`MoveMode::Falling` with `PlayerState.thrown` active): `CalcVelocity` friction `FluidFriction / 2`, fluid, `AirSpeed` cap, sliding move, gravity after the move; ends at the direction's ragdoll notify (`movement::throw_direction`), then the falling move stands for the ragdoll |
| P-3 | **ported** (the stunned pair loaded, unused: the sim has no stun) | `movement::clamp_throw_momentum` |
| P-4 | left open | consumer not found (above); thresholds loaded as data only |
| P-5 | left open | needs the capture |
| P-6 | left open | evidence against the offset model (above) |
| P-7 | left open | the ragdoll-impact damage rule is not in 0x00B529C0 / 0x0078F7D0 nor in script; capture P-8 of the queue |
| P-8 | **ported** | `CombatTuning::windblast_start_delay` (the cast notify, loaded) replaces the 0.1 s warmup in `step_world` and `windblast_lockout` |
| P-9 | **ported** | `npc_anim::matrix_origin` = `matrix_rows(..)[3]`; test `anim_retail::elite_guard_physics_asset` (spine_3 capsule centre (7.25, 0, 0)) |
| P-10 | left open | no breakables in the sim |

Scenarios (`oracle scenario --system physics --system powers`): before 25 pass / 1 fail
(`windblast_throw_l1_flight` apex 109.8); after 26 pass / 0 fail. `windblast_throw_l1_flight`
now also judges the damage time (port tick 24, original 25) and the flight drift over ticks
26..36. `powers/windblast_fan_l2`'s front timing became 0.27 + 0.4 s. Side effect: the guard of
`powers/windblast_guard_l1` now lands about 700 uu out instead of 1100 and, as the sim's NPC
gets up on landing (no ragdoll rest, section 3.7), comes back and hits the player at tick 122;
the original rests until tick 117.

## 7. Recommendation: what the sim must model

Must be in the sim (gameplay depends on it, rules are small and decoded):

1. The thrown-NPC velocity and flight (P-1, P-2, P-3): Velocity = momentum / 2 with the 800 minimum,
   Flying with gravity and drag for the throw duration. This decides where a Windblasted NPC
   lands, whether it reaches a ledge / wall / water / spikes, and therefore deaths, chaos and
   discovery. It is a few lines in `left_hand` and the NPC mover.
2. The reaction class by `|momentum|` (P-4), once captured: below 1000 no throw.
3. A body's rest position as the pawn Location (P-6): the corpse position the stealth / chaos code
   uses should be the root-body rest position, not the capsule. A fixed offset (the pelvis
   about 89 uu above the capsule centre at the switch, then lying: the root's rest height) is a
   good first model; the Verlet ragdoll's root at rest is the existing presentation and can be
   used to place the sim body once it settles (it is deterministic).
4. The wall-hit / impact damage on thrown NPCs (powers.md F-06 done; P-7 to follow).
5. The breakable rule (P-10) when breakables are added: threshold, health, steps, events.

Can stay presentation-only: the ragdoll's limb motion, constraint limits, per-body masses,
friction and restitution (nothing in the sim reads them except the root body's rest), the
physics-prop flight after a Windblast (props are not blockers for pawns and AI: COLLISION.md; their
only effects are noise on contact and Kismet touches), prop-vs-prop collisions, and the thrown
object visuals. If props are added later, give them a bounded model: the 3.4 impulse divided by
the body's mass, gravity, a floor, and a sleep timer; no joint simulation.

Scenarios: `fixtures/oracle/scenarios/physics/` (`windblast_throw_l1_flight`, judged against the
cached original trace; `windblast_throw_d300 / d600 / d800 / d900`, the reaction-class sweep, in
`capture_queue.txt`). Run on 2026-10-08: the four sweep specs pass their static damage checks
(16 / 13 / 11 / 10) and report the physics class as unmeasured (the port has no `npc.*.physics`
channel); `windblast_throw_l1_flight` fails with the port's apex at 109.8 against 55.9 (P-1).
