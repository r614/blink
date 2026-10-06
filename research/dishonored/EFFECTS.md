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
`LookupTableNumElements` (values per sample: 1, or 2 = min and max), `LookupTableChunkSize`
(floats per sample), `LookupTable` = [min, max, samples...], `LookupTableTimeScale`,
`LookupTableStartTime`. Tagged properties are deltas against the module's class default, so
omitted fields come from `Engine.Default__ParticleModule<X>` (e.g. `StartRotation` defaults to
Op 2, 2 elements, chunk 2; `AlphaOverLife` to Op 1, 1, 1). Sampling: index = (t - start) x time
scale (0 = constant), linear between neighbouring samples; random ops lerp min..max with the
particle's random number (extreme picks one end). Example `ChimeySmoke_01`: spawn rate 1.3/s,
lifetime 6..7 s, start size 300..600, velocity (-25..25, -25..25, 20..55), colour over life
~0.1 grey, alpha over life 21 samples at time scale 20 (0 -> 0.69 -> 0), size over life 20
samples. Emitters without a Lifetime module keep their particles forever (light glows: one
burst particle).

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
| TypeDataMesh | Mesh, bOverrideMaterial (drawn per particle) |
| VelocityOverLifetime | VelOverLife, Absolute |
| Orbit | OffsetAmount, RotationAmount, RotationRateAmount (several modules sum) |
| AttractorPoint / AttractorLine | Position, Range, Strength, StrengthByDistance, bAffectBaseVelocity, bUseWorldSpacePosition / EndPoint0, EndPoint1, Range, Strength |
| Collision | DampingFactor, MaxCollisions, CollisionCompletionOption |
| TypeDataTrail2 / TypeDataRibbon / TypeDataAnimTrail | strip in spawn order |
| TypeDataBeam2, BeamTarget, BeamNoise | Distance, TextureTile; Target; NoiseRange, NoiseTessellation |

Other module classes are listed per emitter as unsupported.

## Runtime (`crates/dishonored_runtime/src/effects.rs`)

- The level importer decodes the ParticleSystem of every placed `Emitter`
  (`ParticleSystemComponent.Template`, actor x component transform, `bAutoActivate`) plus the
  gameplay systems, with their materials through the same material/texture path as the map.
- CPU simulation per emitter instance: emitter time with delay, duration and loops; spawn rate
  x rate scale accumulated per frame; bursts at fractions of the duration (`BurstList.Time` is
  0..1 of the emitter duration, the UE3 convention; LIKELY); particle start values from the
  distributions at emitter time; acceleration into the base velocity (UE3 `BaseVelocity`),
  then per frame velocity over life (absolute or scaling), point and line attractors, drag;
  collision against the level collision (reflection x damping, completion: kill, freeze, halt,
  freeze translation / rotation); rotation rate; colour / alpha / size over the particle's
  relative age; SubUV frame from SubImageIndex; orbit offsets added at render time (rotation
  and rate in turns); local or world space; warm-up (`WarmupTime`) in 1/15 s steps. Placed
  emitters simulate within 9,000 uu of the camera and only while their level is shown.
- Drawing: one dynamic mesh per emitter, camera-facing quads (square or rectangle;
  velocity-aligned for PSA_Velocity; locked to an emitter axis or rotating about one with
  `ParticleModuleOrientationAxisLock`), vertex colour x texture, unlit, blend from the material
  (additive, translucent, masked, modulate; additive is not fogged). Trail / ribbon emitters
  draw their particles as one camera-facing strip; beams run from the emitter origin along X
  by `Distance` (or to the beam target) with noise points.
- Mesh emitters (`TypeDataMesh`, e.g. street-lamp cones): one entity per particle with the
  mesh scaled by the particle size and turned by `MeshRotation` / `MeshRotationRate`, colour
  and alpha on a per-particle material copy.
- Material graphs: cooked materials keep only their parameter expressions (the cooker strips
  every other node; e.g. `Volume_Light_PMAT` keeps Fresnel_exponent 5, LightIntensity 2,
  SphereMask_Power 2, Ray_Speed, Db_Depth_Biased_Alpha_Intensity 0.05, StartOffset 128,
  Distance, Color, ColorAlert, the window_rays mask and three static switches), so the graph is
  rebuilt from those names (APPROXIMATION): textured sprites tint by `Color` and scale by the
  intensity / opacity scalars; untextured sprites get a radial falloff `(1 - r)^SphereMask_Power`;
  untextured mesh emitters (volumetric light cones, glow spheres) use `GlowMaterial`
  (`volume_glow.wgsl`): additive Color x LightIntensity x particle colour, faded toward the
  silhouette by |N.V|^Fresnel_exponent, away from the particle by the sphere mask
  (SphereMask_Power over Distance x mesh radius) and near the camera over StartOffset, scaled by
  0.1 (U-305). Depth-biased alpha and the rays texture are not reproduced. Emissive mesh
  materials (`E_Emissive_Color` with intensity > 1, e.g. the fireflies x25) are drawn additively
  in their emissive colour.
- Captures: `fixtures/scripts/effects_boyle.json` (inspect mode, Boyle Ext viewpoints: chimney
  smoke, sewer smoke, street lamp, small lights, fireflies) and
  `fixtures/scripts/effects_gameplay.json` (offline: pistol, Windblast, Blink).

## Gameplay effects (U-165 resolved)

From the tweaks (`tweaks` tool):

| Effect | Source | System, socket |
|---|---|---|
| Player pistol muzzle | `Twk_Inv_PistolCorvo` `DisTweaks_FirePistol` | `Vfx_Weapon.Effects.pistol.Ps_PlayerPistol_StdMuzzle_01` at `m_MuzzleSocketName` `Spitfire` |
| Third-person pistol muzzle | pawn-specific `DisTweaks_NPCFireGun` | `Vfx_Weapon.Effects.Ps_PistolMuzzle` at `Spitfire` |
| Blink | `Twk_Powers.Blink.Twk_Blink` | `m_pGroundPS` Blink_Ground_01, `m_pMantlePS` Blink_Mantle_01, `m_pLowFallPS` / `m_pHighFallPS` Blink_Fall_01 |
| Windblast | `Twk_WindBlast` levels | `m_EffectTemplate` Windblast_lvl1 / Windblast_lvl2 (`m_HandEffectSocketName` LeftHandWpn on level 2, no hand template), underwater Windblast_UndWater_lvl1 |
| Shadow Kill | `Twk_Pawn_DefaultNPC` | `m_ShadowKillFXs`: `Vfx_GamePlay.ShadowKill.ps_SK_Leg_Arms` on 10 bones (upper/lower legs and arms, spine_1, head), `m_fShadowKillDelay` 0.1 s, fiery material `Shadow_kill_inst` 2 s, sound `Snd_Power_Shadow_Kill` |
| Impacts, blood | contact system particle slots | striker (Bullet / Arrow / Sword) x surface or body |

The `Spitfire` socket is a `SkeletalMeshSocket` of the weapon mesh (third-person
`Wpn_PlyGunElite`: bone Root_jnt, (5.25, 0.99, -46.47), pitch -90 deg: fires along the bone's -Z).
The runtime computes it from the animated third-person pistol each frame
(`EffectSockets::muzzle`); the first-person shot (no third-person pawn drawn) uses the eye
offset. The Shadow Kill bones come from the animated corpse (`EffectSockets::bones`).
Remaining approximations: the Windblast origin (pawn eye height along the view), the Blink
arrival height (80 uu below the pawn centre), the underwater test (not wired), the fiery
material.

## Approximations and open points

- Burst `Time` as a fraction of `EmitterDuration`; spawn-rate distributions sampled with the
  emitter time in seconds (UE3 `ParticleEmitterInstance::Tick` convention).
- Rotation units: turns (x 2 pi), as UE3's Rotation/RotationRate modules.
- Sprite size: `Size.x` is the full sprite width.
- Light modules and particle lights are not rendered; collision uses the static level
  collision only (no pawns).
