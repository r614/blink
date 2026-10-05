# Particle effects

Applies to the GOG Definitive Edition install (`GAME_FILES.md`). Presentation only: nothing here
feeds the simulation.

## What the levels contain

Boyle (all `L_Boyle_*` packages): 190 `ParticleSystem`s, 259 `Emitter` actors, 260
`ParticleSystemComponent`s, 848 `ParticleSpriteEmitter`s with 1,501 `ParticleLODLevel`s. Module
counts: Spawn 1,449, Required 838, Lifetime 832, Size 807, ColorOverLife 734,
SizeMultiplyLife 598, Velocity 565, Location 544, Rotation 432, Acceleration 358,
RotationRate 337 (plus smaller counts of SubImageIndex, LocationPrimitive*, Orbit, Color...).
The placed emitters of the exterior (`L_Boyle_Ext_Fx`, 76 components) are mostly light glows
(`Boyle_FX.Lights.Light_small_Yellow_01` x14, `Ps_SmallLights_01` x10, street lamp spots),
chimney smoke (`Boyle_FX.Effects.ChimeySmoke_01` x8), sewer smoke, fireflies, garbage flies,
wind, street paper, river fog, water droplets, crows and fireworks.

Gameplay effects live in `DishonoredGame`: `Vfx_GamePlay.Blink.*` (Blink_Ground_01,
Blink_Fall_01, Blink_Mantle_01), `Vfx_GamePlay.Windblast.*` (Windblast_lvl1/2, impacts),
`Vfx_PhysMat.pistol.Ps_Pistol_Blast` and `Ps_PistolImpact_*`, `Vfx_PhysMat.Blood.*`
(Pfx_PistolBlood_Blast, Pfx_BloodSProjectile_Default, camera blood),
`Vfx_Weapon.Effects.Ps_SwordSparks_Parry`, `Vfx_PhysMat.<Surface>.*` impacts. The contact
system's particle sub-intersections (237) name the physical-material impacts (e.g. Water ->
`Pfx_WaterSplash`, Wood -> `Pfx_wooden_shard`, flare bolts), see AUDIO_FORMAT.md "Contact
system".

## Cooked format

`ParticleSystem`: `Emitters` (array), `WarmupTime`, `UpdateTime_Delta`, LOD settings, bounds.
`ParticleSpriteEmitter`: `EmitterName`, `LODLevels` (LOD 0 used). `ParticleLODLevel`:
`RequiredModule`, `SpawnModule`, `TypeDataModule` (mesh emitters), `Modules` (array),
`bEnabled`, `PeakActiveParticles`.

Distributions are cooked: the `Distribution` object is stripped and the struct keeps the
baked lookup table (`FRawDistributionFloat/Vector`): `Op` (1 none, 2 random, 3 extreme),
`LookupTableNumElements`, `LookupTableChunkSize`, `LookupTable` = [time scale, start time,
entries...], each entry `chunk` floats (value, or min then max for the random ops), and the
duplicated `LookupTableTimeScale` / `StartTime` properties. Sampling: index = (t - start) x
scale, linear between neighbouring entries, random ops lerp min..max with the particle's
random number (extreme picks one end). Example `ChimeySmoke_01`: lifetime random over 2 entries
(6 floats), start size vector random (14 floats), colour over life 2 entries of 3, alpha over
life 21 entries with time scale 20, size-over-life 20 entries.

Modules decoded (`dishonored_assets::particles`, IR `dishonored_ir::particles`):

| Module | Fields |
|---|---|
| Required | Material (-> map material, blend from the base material), ScreenAlignment, bUseLocalSpace, EmitterDuration, EmitterLoops, EmitterDelay, SubImages_Horizontal/Vertical, SpawnRate + BurstList (older layout) |
| Spawn | Rate, RateScale, BurstList {Count, CountLow, Time} |
| Lifetime / Size / Velocity (+ radial) / Rotation / RotationRate | start values |
| Color | StartColor, StartAlpha |
| ColorOverLife | ColorOverLife, AlphaOverLife |
| SizeMultiplyLife | LifeMultiplier, MultiplyX/Y/Z |
| Acceleration | Acceleration |
| Location / LocationPrimitiveSphere / Cylinder | StartLocation; radius (surface only, positive axes, velocity) / radius, height |
| SubImageIndex | SubImageIndex over life |
| TypeDataMesh | Mesh (reported, not drawn) |

Other module classes (Orbit, attractors, collision, VelocityOverLifetime, beams, trails) are
listed per emitter as unsupported.

## Runtime (`crates/dishonored_runtime/src/effects.rs`)

- The level importer decodes the ParticleSystem of every placed `Emitter`
  (`ParticleSystemComponent.Template`, actor x component transform, `bAutoActivate`) plus the
  gameplay systems, with their materials through the same material/texture path as the map.
- CPU simulation per emitter instance: emitter time with delay, duration and loops; spawn rate
  x rate scale accumulated per frame; bursts at fractions of the duration (LIKELY); particle
  start values from the distributions at emitter time; acceleration, velocity, rotation rate
  per frame; colour / alpha / size over the particle's relative age; SubUV frame from
  SubImageIndex; local or world space; warm-up (`WarmupTime`) run at spawn in 1/15 s steps.
  Placed emitters simulate within 9,000 uu of the camera and only while their level is shown.
- Drawing: one dynamic mesh per emitter, camera-facing quads (square or rectangle;
  velocity-aligned for PSA_Velocity), vertex colour x texture, unlit, blend from the material
  (additive, translucent, masked, modulate).
- Gameplay effects from simulation events: sword sparks on block/parry, pistol muzzle blast,
  blood on sword / pistol / bolt damage, Blink ground effect at the arrival, Windblast cone,
  world impacts.

## Approximations and open points

- U-165: which object posts each gameplay effect and at which socket (the muzzle socket of
  the pistol mesh, the Blink marker, the Windblast origin) is not traced; positions are
  pawn-relative offsets.
- Burst `Time` treated as a fraction of `EmitterDuration`; spawn-rate distributions are
  sampled with the emitter time in seconds (UE3 `ParticleEmitterInstance::Tick` convention).
- Rotation units: turns (x 2 pi), as UE3's Rotation/RotationRate modules.
- Sprite size: `Size.x` is the full sprite width.
- Mesh emitters, orbit, collision, light modules and particle lights are not rendered.
