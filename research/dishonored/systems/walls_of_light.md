# Walls of light (`DisWallOfLight`)

Decoded from `Dishonored.exe` with the static index. The port is in
`crates/dishonored_sim/src/walls.rs`; its module doc lists the rules with their addresses. The
data comes from `crates/dishonored_assets/src/world_actors.rs` (`wall_of_light`), and the retail
test is `crates/dishonored_assets/tests/walls_of_light_retail.rs`.

## Placement data (all maps)

24 walls ship, on 12 maps:

- Boyle Int
- Bridge 1c and 2
- Flooded FGate
- the Lighthouse (both chaos variants)
- Streets1 and Streets2
- Tower Return Int and Yard

Every wall shares the same tweak values:

- `m_Damage` 100
- `m_bJib` false
- `m_ActivationTime` and `m_ActivationTimeForPawnEffect` both 0.1 s

Their collision is a 20 uu thick box from the tweak's `m_pStaticMesh`. The tweaks interface's apply
(0x00A29110) puts that mesh on the component (`SetStaticMesh`); a placed component on the Flooded
gate has no mesh of its own. The box is scaled by `DrawScale3D`, typically 330 to 660 uu wide and
390 to 1230 uu high.

Friendly factions come from the tweak, or from the placed `m_FriendlyFactionsOverride`:

| Level | Friendly factions |
|---|---|
| Default | `Faction_Guard_Default` |
| Boyle | `Neutral_Guard` and `Guard_Default` |
| Bridge | Sokolov, his servants and guards |
| Lighthouse | Martin's and Pendleton's armies and Martin |
| Tower | the tower guards and torturers, plus the Regent's safe room |

Each wall is powered by a `DisWhaleOilReceptacle`, resolved as follows:

1. the wall's own `m_pAttachedReceptacle`;
2. otherwise the receptacle listing the wall in `m_aAttachedActors`.

Bridge 2's wall 0 and Tower Int's wall 1 have no battery placed in their receptacle, so they start
unpowered.

## Rules

The `walls.rs` module doc has the decoded detail. In summary:

- **Power.** A wall is placed hidden with no collision.
  - The receptacle's plug and unplug call the wall's consumer callbacks: 0x00A40230 and 0x00A3EE40.
  - Powering on shows the wall and fires output 0. Powering off hides it, removes collision and
    fires output 1.
  - The tick restores `TouchAll` collision while the battery has a charge.
  - While Bend Time stops the world, the wall has no collision, so it can be walked through.
- **Touch** (live pawns).
  - A pawn of a friendly faction passes. On a reversed (rewired) wall the rule inverts: friendlies
    and the player's enemies die, and a player-controlled pawn never does.
  - While the 0.1 s kill effect runs, any pawn that starts touching dies.
  - Each kill costs the battery's pawn charge.
  - NPCs burn to ash. The player takes `TakeDamage(100)`, an energy damage type with no damage
    reduction.
- **Bodies.** A body that starts touching burns to ash at the pawn cost (the touch of a dead
  pawn). A carried body is spared.
- **Overlap tick.**
  - Rats are shocked.
  - Free props (`DishonoredKActor` / `DishonoredKAsset` no pawn holds) are disintegrated.
  - Both cost the battery's ambient-animal charge.
- **Damage, projectiles, Windblast.** See the next section.
- **Kismet.**
  - `DisSeqAct_WallofLightControl` inputs:
    - 0 TurnOff
    - 1 TurnOn
    - 2 Toggle: it passes the current state to the setter, so it is a no-op as decompiled.
    - 3 SwitchPolarity
  - `DisSeqEvent_WallOfLight` outputs:
    - 0 activated
    - 1 deactivated
    - 2 polarity default
    - 3 polarity reversed
    - 4 kill effect on
    - 5 kill effect off

## Damage, projectiles, props and Windblast (decoded 2026-10-09)

Vtable notes. The wall's vslot 82 (`TakeDamage`, 0x00A192B0, shared with `DisProjectile`)
forwards to vslot 232, which is `TakeDamage_Native` (0x00A3EA80). The parameters are the usual
ones: amount, instigator controller, hit location, momentum, damage type, hit info and causer
(at stack +0x44). The `IDisHitReboundInterface` vtable holds only the destructor and the actor
getter. The Windblast calls the actor's vslot 223 (0x00A3BEF0) directly.

Classes named by their `StaticClass` registrations (the name pointer at +0x10):

| Global | Class |
|---|---|
| `DAT_0145F1A8` | `StateNPCMasterDead_Electrocute` |
| `DAT_0145F19C` | `StateNPCMasterDead_BeingCarried` |
| `DAT_0145ECDC` | `DishonoredDamageType_FastHit` |
| `DAT_0145EE04` | `DisWallOfLight` |
| `DAT_0146672C` | `DishonoredNPCPawn` |
| `DAT_0145EF34` | `DishonoredMovable` |
| `DAT_0145EDC8` | `DishonoredKActor` |
| `DAT_0145EDCC` | `DishonoredKAsset` |
| `DAT_0145EE58` | `DisPickup_Base` |

`FUN_00B49000` reads `DishonoredPawn.m_bIsOfficiallyDead` (+0x508, bit 10).

### Damage (`TakeDamage_Native`, 0x00A3EA80)

The wall needs a receptacle whose battery has a charge, and `m_bEnabled`. Then:

- A causer that is not a pawn is disintegrated (0x00A3E7D0):
  - a charge at the ambient-animal cost; with none, nothing happens;
  - the shock sound, an AI noise of the causer's instigator, and `m_ShockParticleSystemTemplate`
    at the hit;
  - the kill effect;
  - then the causer is destroyed. An NPC body is burnt instead (0.01 s).
- A pawn causer with a `DishonoredDamageType_FastHit` damage type (the sword's) only shocks the
  wall (0x00A3BCE0).

### Projectiles (0x00C4F3A0)

The contact processor flags a hit actor that is a `DisWallOfLight`. Whatever the damage, it calls
the wall's `TakeDamage` with the projectile as the damage causer. It then skips the impact effects
(0x00BE61A0) and destroys the projectile (0x00BBAFA0) instead of the hit-world or hit-pawn slots
(236 / 235). So nothing sticks, bounces or bursts.

This covers bolts, darts, bullets (`DisBullet`) and grenades. A grenade's bounces are slots 235 and
236 (0x00C5E2E0, 0x00C5E340 -> 0x00C5D940), which the wall bypasses, so a grenade does not
explode.

The projectile checks meet the wall only while it collides. `ShouldTrace` (0x00A21B30) answers:

- the projectile check (0x2000000 with `TRACE_Movers`);
- the melee check (0x4000000), always;
- nothing while time is stopped.

### Kill effect (0x00A3C010)

With `m_ActivationTime > 0`:

1. The activation timer is set: `m_ActivationTimeForPawnEffect` for a pawn's kill, else
   `m_ActivationTime`.
2. Every live pawn in the wall's `Touching` list is killed by 0x00A3B940. There is no faction
   test and no player-controlled test.
3. A kill sets the pawn timer (unless the tweak jibs) and moves the effect to that pawn.
4. The beams fire to the effect's point.

Output 4 fires in any case.

### The kill (0x00A3B940)

- It skips a pawn in `StateNPCMasterDead_Electrocute`. This was the unresolved special-class
  exemption.
- It takes a charge at the pawn cost and plays the shock sound.
- An NPC is burnt or jibbed. Any other pawn takes `TakeDamage(m_Damage)`.

### Touch by a dead pawn (0x00A3E340)

A dead pawn (`m_bIsOfficiallyDead`, not the player's) counts as "friendly" when the wall is
reversed. So `friendly == m_bReversed` holds and it burns, at the pawn cost.

The exception is a body in `StateNPCMasterDead_BeingCarried`: it counts as the opposite, so a body
carried through the wall is spared.

The earlier port burnt bodies from the overlap tick at the ambient-animal cost. That was wrong: the
tick only takes the classes below.

### Overlap tick (0x00A3EEE0)

The tick gathers the octree overlaps of the wall's box. It then runs the collision component's
point check with the other's box (flags 0x22086).

| What overlaps | What happens |
|---|---|
| A live rat (`DisGameCrowdAgentSkeletalRat`) | It is shocked (0x00A3E630) |
| A `DishonoredKActor` or `DishonoredKAsset` | It is disintegrated, unless its `Base` is a pawn (held) or it is a `DisPickup_Base` whose tweak does not allow it |

The disintegrated case covers thrown and dropped props.

### Windblast (0x00A3BEF0)

The gather (0x00BF6780) takes the colliding actors in range. When the front passes the wall's
bounds centre (0x00BF6AF0, after its sight check 0x00BEDC90), it calls the wall's vslot 223
instead of a push. The wall:

1. takes the blast factor at the centre (0x00BE8570, `windblast_factor`);
2. at `m_fWindblastMinimumForce` (0.3) or more, shocks at the centre (0x00A3BCE0);
3. with a receptacle, takes a charge at the ambient-animal cost;
4. answers 1, so it is never pushed.

Only a colliding wall is gathered: a powered wall while time runs.

### Shock (0x00A3BCE0)

- The shock sound at the point.
- An AI noise (`DisNoiseMakerInterface`, 4 / 9).
- With `m_ActivationTime > 0`:
  - if no timer runs, the activation material and the start-kill-effect sound;
  - the timer set to `m_ActivationTime`;
  - the shock particles;
  - the beams to the point.
- No Kismet output.

So for 0.1 s after a strong Windblast, a pawn walking in dies.

## Presentation (decoded 2026-10-09)

### The mesh

Every placed wall's mesh component is `HiddenGame`, through its archetype. The tweak's
`m_pStaticMesh` is `Vfx_GamePlay.WallLight.walllight_visible_in_editor`. The mesh only collides:

- the Flooded gate's collision comes from the tweak mesh, which the asset extraction already
  handles;
- the mesh is never drawn.

### What is seen

| Effect | Behaviour |
|---|---|
| `m_SparkEffects` (placed `Emitter`s) | Deactivated at placement (0x00A28D80), activated while powered (0x00A3EB10), deactivated when powered off (0x00A3ECA0) |
| `m_Beams` (placed `Emitter`s with beam emitters) | Activated by the kill effect and the shock (0x00A30CA0). Their beam end is the effect point (0x00A29240, `SetBeamTargetPoint` on `FParticleBeam2EmitterInstance`). Deactivated when the timer runs out (0x00A28F80) |
| `m_CrossSparkEffect` | Template `m_CrossSparkEffectTemplate` (`Vfx_GamePlay.WallLight.Lightnings3_random`, set at placement); see below |
| Shock particles | `m_ShockParticleSystemTemplate` (`Vfx_GamePlay.WallLight.ps_shock_electrified`) at the shock point, with the wall's rotation |

The cross spark: every `rand()` between `m_fMinSparkInterval` and `m_fMaxSparkInterval` (2.0 to
3.5 s in `WallOfLight_twk`), the tick:

1. picks two different spark emitters at random;
2. puts the component on the first, with its beam end on the second;
3. activates it for `m_fSparkDuration` (0.17 s), with `m_bDisableSystemBetweenSparks`.

The sounds:

| Moment | Sounds |
|---|---|
| Powered on | `m_ActivateSound`, `m_AmbientStart` |
| Powered off | `m_DisactivateSound`, `m_AmbientStop` |
| Kill effect start / stop | `m_pStartKillEffectSound` / `m_pStopKillEffectSound` |

## Port status

| Part | Port |
|---|---|
| Damage, projectiles, kill effect, dead-pawn touch, props, Windblast | `dishonored_sim::walls`: `apply_hits`, `projectile_hit`, `step`; the projectile steps in `left_hand`, `gadgets` and `launchers`; the gather and front in `left_hand::step_windblasts` |
| Projectile and melee blocking | The mission layers the colliding walls into the world (`walls::layer`, projectile and melee channels). The level's static collision no longer holds the wall (`level.rs`, cache `collision22`) |
| Presentation | `dishonored_runtime::walls_of_light` (mesh hidden, spark emitters, beams, cross spark) and `effects::gameplay_effects` (shock particles) |
| Data | `WallOfLightDef` carries the presentation data and the Windblast threshold (cache `worldactors4`) |

## Open

- **The sword's shock** (a `FastHit` pawn causer). The sim's sword meets no actors but pawns.
- **AI noise.** The disintegration's and the shock's AI noise (`DisNoiseMakerInterface`, 4 / 9)
  is not mapped to the sim's noises.
- **Explosions.** Radius damage reaching a wall (a causer that is not a pawn) is not ported.
- **Sounds.** The wall's sounds are not played by the runtime's audio.
- **Possession.** The possessed-creature branches of the touch filter are not ported.
- **Collision audit.** `harness collision` compares the static collision only, and the wall is no
  longer in it. Boyle Int's wall answers there only while powered.
