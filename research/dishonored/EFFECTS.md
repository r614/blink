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

The emitter keeps its LOD's `Modules` as an ordered list in the cooked order
(`ParticleEmitterAsset::modules`): every enabled module of a class the port simulates is one
`dishonored_ir::particles::ParticleModule` variant of its class with what the decoder reads of
it (a distribution the cooked module carries no table for is `None`). The required, spawn and
type-data modules' own fields (material, screen alignment, local space, duration / loops /
delay, SubUV layout, sort mode, rate and bursts, mesh, trail / beam / decal, events) stay fields
of the emitter; BeamTarget / BeamNoise complete the beam type data and MeshMaterial the
emitter's material. A class the port does not simulate is listed in place as
`Unsupported { class }` (the import report, `ParticleEmitterAsset::unsupported`).

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
- The simulation (`dishonored_fx`: `spawn_particle`, `step`, `dispatch_events`) and the sprite
  builder read the module list through the emitter's accessors (`lifetime()`, `velocity()`,
  `collision()`...): the last module of the class, as the flat layout kept it, a distribution
  without a table included (the install has emitters with two modules of a class, e.g. two
  `ParticleModuleLocation` on `Blink_Ground_01`'s Body; the second Location of
  `Flare_Bolts_02`'s Embers carries no table, so that emitter has no start location), and every
  orbit in order (`orbits()`, summed). A new class is one variant, one decoder arm and one
  simulation arm.
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
  0.1 (U-305). The rays texture is not reproduced (depth-biased alpha is: `FxMaterial`). Since 2026-10-08 mesh
  emitters on `Volume_Light_PMAT` use the material decoded from its shaders instead
  (`volume_light.rs`, RENDERING.md "Light beams"; the particle colour is its
  `MeshEmitterVertexColor`); `GlowMaterial` remains for the other untextured bases. Emissive mesh
  materials (`E_Emissive_Color` with intensity > 1, e.g. the fireflies x25) are drawn additively
  in their emissive colour.
- Captures: `fixtures/scripts/effects_boyle.json` (inspect mode, Boyle Ext viewpoints: chimney
  smoke, sewer smoke, street lamp, small lights, fireflies) and
  `fixtures/scripts/effects_gameplay.json` (offline: pistol, Windblast, Blink).

## Particle materials

The party lanterns' system `Boyle_FX.Lights.Ps_SmallLights_01` (11 placed, e.g. at
(-3353, -2173, -74) and (-3669, -1638, -77) by the front-door steps) has a steam emitter on
`Vfx_Smoke.Materials.M_TB_CrouchSteam_INST` and a smoke emitter on
`vfx_characters.Materials.cigarette_smoke_inst`, both on `Vfx_Materials.main_materials.DefaultTranslu_PMAT`
(BLEND_Translucent, unlit). The base pass pixel shaders of the matching permutations
(`shader_maps` + `shader_dump` of RefShaderCache-PC-D3D-SM3, `fxc /dumpbin`; static parameters
from `mic_static`):

- default (2815): rgb = vertex rgb x `D_Diffuse`.rgb x `D_DiffuseColorTweak`.rgb; alpha =
  `O_Opacity`.r x `D_DiffuseColorTweak`.a x vertex alpha (the channel is `O_Opacity_Channel`).
- `D_DisableDiffuse` (35485, the cigarette smoke): rgb = vertex rgb x tweak.rgb; alpha as above.
- `Mo_Enable_Opacity_Motion` + `Mo_Enable_Diffuse_Motion` (35475, the crouch steam): two
  rotating / panning samples of `Motion_SourceTexture01` (uv rotated about the centre by
  0.2 x Time x `Mo_Motion_Speed`, panned by frac(-0.3 T), frac(0.2 T)) and `Motion_SourceTexture02`
  (rotated about the origin by 0.1 T, panned by (0, frac(0.4 T))) multiply into m; `Motion_Mask`
  is sampled at uv + 0.2 m; rgb = vertex rgb x mask.rgb x tweak.rgb, alpha = mask.r x tweak.a x
  vertex alpha.

`M_TB_CrouchSteam_INST` sets no texture: the base's defaults apply (`Motion_Mask` =
`Vfx_Smoke.Textures.FXT_Smoke_01`, both sources `FXT_Smoke_Tile_01`, `D_Diffuse` / `O_Opacity`
= `Vfx_PhysMat.Textures.Smoke01_d`). The port took the sprite's alpha from the `D_Diffuse`
texture's own alpha (opaque) and tinted by `Color` (absent), so the steam and smoke drew as dark
grey squares (`D_DiffuseColorTweak` (0.21, 0.20, 0.16)). Now the importer puts the permutation's
colour source (D_Diffuse / white / Motion_Mask) with its opacity source in alpha into the
diffuse slot, and the sprite material tints by `D_DiffuseColorTweak` (alpha into the opacity).
APPROXIMATION: the motion distortion of the mask (up to 0.2 uv) is not animated.

### Every particle base

The same method over every particle base material the gameplay effects use (118 materials
in DishonoredGame's particle systems, grouped by base; `crates/dishonored_assets/src/particle_material.rs`
holds the recipes, each citing its shaders):

| Base | Blend | Colour | Opacity |
|---|---|---|---|
| DefaultTranslu / Translu2sided / TransluFOG, DefaultAdditive / _2Sided / _UV / _OutBendTime | translucent / additive | `D_Diffuse` (one channel as grey with `D_Enable_Diffuse_Channel`; white with `D_DisableDiffuse` / `D_Disable_Diffuse`; `Motion_Mask` with motion) x `D_DiffuseColorTweak`.rgb | `O_Opacity[O_Opacity_Channel]` (alpha with `O_EnableOpacityAlpha`, `Motion_Mask.r` with motion) x tweak.a; additive: rgb x alpha |
| Default_Simple_Translucent (37612..37620) | translucent | `D_Diffuse` (or `D_Diffuse_color` x a) | vertex alpha only; x `MA_MaskTexture[MA_Mask_Channel]` with `MA_Use_Masked_Alpha` |
| HeightMapAnim (6086..6107; blood anims) | translucent | `D_Diffuse` x `D_Diffuse_Brightness` (`DiffuseColor` with `B_Enable_Blood`) | `saturate((O_OpacityRGB_R_is_default[R / G Use_Green / B Use_Blue] - vertex alpha) x Mask_Multiply)` |
| SubUV_HeightMap (5807, 5820, 5872; blood splash / spray) | masked | two sub-images of `D_diffuse_texture` x brightness (+ cubemap) | clip where `saturate((H - vertex alpha) x Mask_Multiply)` < 1/3, H = `H_HightMap_RGB_R_is_default` channel as above |
| HeavySmoke | translucent | `D_DiffuseAlpha` | `O_Opacity[channel]` |
| Water_Translu / Water_Additive | translucent / additive | `R_Reflexion2d` x tweak.rgb x a | `Alpha[Alpha_Mask]` |
| WaterSplash | additive | `Tex#0` x `Brightness` | `saturate((Shape.r - vertex alpha) x Max_Opacity)` |
| Fire_Explosion (_02 / _03 / _Oil) | translucent | `F_Fire_Color_Texture` x `F_Fire_Color` | `D_Diffuse.b` |
| Look_Up_Fire (grenade fire) | translucent | gradient lookup (APPROXIMATION: `Tex#0` x `lookup_color`) | smoothstep of `Diffuse.r` (APPROXIMATION: `Diffuse.r`) |
| Volume_Light_translu | translucent | `Color` x a | `A_Alpha.r` x `Opacity` |
| Glow_PMAT | additive | `G_GlowColor` | procedural `max(1 - 2r, 0)^G_GlowPower` |
| test_look_up (36313 / 36315; the pistol muzzle's smoke) | additive | vertex red x lerp(`diffuse_color2`, `diffuse_color`, vertex green); vertex blue unused | `smoothstep(saturate((Diffuse[alpha_channel] - (1 - vertex alpha)) / vertex alpha))` |

The blood squares: `Blood_Splash_INST` / `Blood_Spray*_INST` are SubUV_HeightMap materials with
an opaque DXT1 `D_diffuse_texture`; the port alpha-tested that alpha, so every sprite passed as
a square. The importer now bakes the colour source with the opacity channel in alpha
(`MaterialParams::particle` keeps the constants), the sprite takes a random 2 x 2 sub-image
(`PSUVIM_Random`, `ParticleEmitterAsset::sub_random`). The eroding materials (height-map blood,
water splash) bake `saturate(H x multiplier)` and fade by `1 - vertex alpha` (APPROXIMATION:
the original's shape shrinks instead). Bases without a recipe that are translucent, additive or
masked take the first conventional opacity parameter (`OPACITY_PARAMS`). Checks:
`dishonored_assets` test `particle_material` (recipes) and retail
`gameplay_particle_sprites_have_an_opacity` (no gameplay sprite keeps an opaque alpha).

The pistol smoke's cyan cloud (2026-10-12): the first-person muzzle `Ps_PlayerPistol_StdMuzzle_01`
has two smoke emitters, `fast_smoke` (burst 3, life 0.35..0.5 s, size 100..125 uu, x 0.08..2.8 over
life) and `old_smoke` (burst 5, life 1.5..2.5 s, size 100..200 uu, x 0..2.6), both camera-facing
additive sprites, unsorted, on `tguichard_test_test.smoke_look_up.smoky_lookup` (Startup; base
`test_look_up`, `BLEND_Additive`, `alpha_channel` = G by `mic_static`, `Diffuse` =
`Vfx_Smoke.Textures.FXT_Steam_01`, an opaque grey DXT1; `diffuse_color` (0.406, 0.408, 0.368),
`diffuse_color2` (1, 0.838, 0.401); `lookup_color` and the switches `Enable_colored_lookup` /
`Enable_Distortion` are set but no compiled permutation reads them). The base had no recipe, so the
port drew the opaque steam texture additively times the particle colour. That colour is not a
colour: its ColorOverLife runs (0, 0, 1) -> (1, 0, 1) -> (0.4, 0.97, 1) -> (0, 1, 1) with the
alpha 1 -> 0 (red the glow's strength, green the lookup position, blue a constant 1 the shader
never reads), so the sprites turned cyan (G = B = 1, R -> 0) and, 200..560 uu wide at the gun,
filled the screen for the old smoke's 2 s. The base's two shader maps (`shader_maps ...
test_look_up`: particle vertex factory 36313 for `alpha_channel` R, 36315 for G; 36314 / 36316
the static-mesh twins; uniforms `SelectionColor`, `diffuse_color2` (c6), `diffuse_color` (c7),
`Texture2D_0` = `Diffuse`) compute, with v1 the particle colour (vertex shader 1 passes
`TEXCOORD1` through, as for the other bases): `t = saturate((H - (1 - v1.w)) / v1.w)`,
`rgb = t^2 (3 - 2t) x v1.r x lerp(c6, c7, v1.g)`, alpha 0. Now the recipe bakes `Diffuse[G]` as
the sprite's alpha, the sprite shader takes the look-up smoothstep without the vertex-alpha factor
(`ParticleShading::erode_bare`, `particle_soft.wgsl` soft.w) and the sprite builder turns the
particle colour into `R x lerp(diffuse_color2, diffuse_color, G)` (`ParticleShading::vertex_lerp`,
`effects::lookup_colour`). The smoke is now a short warm glow at the muzzle fading to a faint grey
haze (red < 0.05 after a third of its life). Its only users in the gameplay set are these two
emitters (the level script packages that name `smoky_lookup` cook the same muzzle). CONFIRMED from
the disassembly; mesh emitters on the base (36314 / 36316) are not in the data and the mesh path
does not apply the lookup.

Particles without a Lifetime module live until their emitter is destroyed; an emitter that has
played its `EmitterLoops` is destroyed with them (the pistol muzzle's five sprite emitters
stayed forever before). One-shot instances whose emitters loop forever are cut after 12 s.

## Gameplay effects (U-165 resolved)

From the tweaks (`tweaks` tool):

| Effect | Source | System, socket |
|---|---|---|
| Player pistol muzzle | `Twk_Inv_PistolCorvo` `DisTweaks_FirePistol` | `Vfx_Weapon.Effects.pistol.Ps_PlayerPistol_StdMuzzle_01` at `m_MuzzleSocketName` `Spitfire` |
| Third-person pistol muzzle | pawn-specific `DisTweaks_NPCFireGun` | `Vfx_Weapon.Effects.Ps_PistolMuzzle` at `Spitfire` |
| Blink target indicator | `Twk_Powers.Blink.Twk_Blink` | `m_pGroundPS` Blink_Ground_01, `m_pMantlePS` Blink_Mantle_01, `m_pLowFallPS` / `m_pHighFallPS` Blink_Fall_01 (indicator sizes `m_fTargetingGroundIndicatorSize` 80, `m_fTargetingGoalIndicatorSize` 130); every emitter loops forever with `bKillOnDeactivate` |
| Possession | power class / Startup | Possession_HandCast_01 at the hand, PossesTarget_Human_01 / PossesTarget_Rats_01 on the host |
| Devouring Swarm | power class | Ps_RSwarm_HandCast_01, RSwarm_Summoning_lvl1 / lvl2 at the swarm |
| Dark Vision | `Twk_DarkVision` | `m_AINoiseVisualEffect` Ps_DVision_Sounds_Base01, NPC `m_VisionConePatrolTemplate` Ps_DVision_VCone_Blue_01, grade `m_StaticUberAdjustement`, souls `DarkVision_Souls_INST` (`m_LowerColor` / `m_UpperColor`) |
| Grenade, Spring Razor | Startup | `Vfx_Weapon.Effects.Grenade.ps_grenade`, `Ps_Grenade_Trail`; `SpringRazor.Ps_SpringRazor` |
| Projectile trails, crossbow muzzle | Startup | `Trails.Ps_Pistol_StdTracer_01`, `Trails.StdBolt_Trail`, `Ps_CrossBowMuzzle` |
| Windblast | `Twk_WindBlast` levels | `m_EffectTemplate` Windblast_lvl1 / Windblast_lvl2 (`m_HandEffectSocketName` LeftHandWpn on level 2, no hand template), underwater Windblast_UndWater_lvl1 |
| Shadow Kill | `Twk_Pawn_DefaultNPC` | `m_ShadowKillFXs`: `Vfx_GamePlay.ShadowKill.ps_SK_Leg_Arms` on 10 bones (upper/lower legs and arms, spine_1, head), `m_fShadowKillDelay` 0.1 s, fiery material `Shadow_kill_inst` 2 s, sound `Snd_Power_Shadow_Kill` |
| Impacts, blood | contact system particle slots | striker (Bullet / Arrow / Sword) x surface or body |

The `Spitfire` socket is a `SkeletalMeshSocket` of the weapon mesh (third-person
`Wpn_PlyGunElite`: bone Root_jnt, (5.25, 0.99, -46.47), pitch -90 deg: fires along the bone's -Z).
The runtime computes it from the animated third-person pistol each frame
(`EffectSockets::muzzle`); the first-person shot (no third-person pawn drawn) uses the eye
offset. The Shadow Kill bones come from the animated corpse (`EffectSockets::bones`).
The Blink systems are the targeting indicator, not an arrival effect: while the local player
aims (`left.blink_target`) one instance stands on the floor under the target (Ground) or at a
target in the air (Fall) and follows it; release, cancel or travel kills it
(`effects::blink_indicator`). The port used to spawn them as one-shots at the arrival, picked
Fall because the pawn is Falling for a tick there, and their endless emitters left a glowing orb
at every destination. Selection: Mantle when the Blink edge finder (`try_blink_mantle`, the
search the arrival runs) finds a ledge from the target, Ground over a floor within the
collision height, Fall otherwise. The pawn also has `BlinkTargetingGoal` (StaticMeshComponent,
hidden in the defaults, its mesh set natively; `BlinkTarget_PMAT` with `IndicatorType` is in
RefShaderCache but in no package) for the goal indicator sizes 130 / 80 (DefaultPower.ini): not
drawn. The pistol tracer `Ps_Pistol_StdTracer_01` is a `TypeDataBeam2` emitter with
`PEB2STM_UserSet` absolute target (one 0.1 s loop): spawned at the shot's impact from the muzzle
to the hit (`effects::spawn_beam`), as the bullet hits in its first tick.

World models (`world_models.rs`, from the tweaks): grenade `Grenade.Grenade`, sticky
`StickyBomb.StickyBomb_world_SKM`, bolts `Crossbows.bolt_01` / `Bolt_Sleep` / `Bolt_Flare`, bullet
`Wpn_Bullet.skm_bullet`, Spring Razor `SpringRazor.SpringRazor`, rats
`Npc_SmallRat.Mesh.Npc_SmallRat`; bind pose (their animations not played yet).

Windblast: `WindBlast_Swirls_02_inst` (DefaultAdditive, `D_Disable_Diffuse`, opacity
`WindBlast_Swirls_01_m`.r) through the recipe, the wave mesh `WindBlast_Wave_02` with
`WindWave_Disto_02_inst` in the distortion pass. Depth-biased alpha
(`Db_Enable_Depth_Biased_Alpha`, soft particles) fades them by the scene depth (`particle_soft.wgsl`,
systems/render_depth.md). Trails and Dark Vision's cones follow their object
(`effects::attached_effects`); a trail whose projectile is gone fades out.
Remaining approximations: the Windblast origin (pawn eye height along the view), the
underwater test (not wired), the fiery material, cast effects at the eye instead of the hand
socket.

## Dark Vision

`dark_vision.rs`: the static uber adjustment of `Twk_DarkVision` (colour balance shadows
(0.5, 0.1, -0.2), midtones / highlights (0.1, 0, 0), opacity 1, pre-desaturation 1,
post-desaturation 0.5, exposure 0.1) blended into the ArkPp grade by the power's fade (0.1 s in,
0.2 s out), and the revealed pawns redrawn with `m_pSoulMaterial` =
`AltScreen_Effects.SAVED.DarkVision_Souls_INST` (parent `DarkVision_PMAT`, BLEND opaque,
unlit, `bUsedWithArkPostProcess`). Its shader map (RefShaderCache) has a soul pass,
`TSoulPartMeshPixelShader<FSoulPartMeshPolicy>` (35003; skinned vertex shader 35012), and the
ArkPp mesh passes (35005 gamma / 35006 linear, the same terms):

- in front of the scene (scene depth at the pixel - (w - 5) >= 0): `Full_Color`, alpha
  `0.7 x (0.5 + 0.5 (n_view.z + 1)^2)`;
- hidden: `Behind_color + 0.3 x Tex#1.g` at `uv + frac(-0.05 t) + 0.4 x Tex#0.rg` (Tex#0 at
  `uv + frac(0.05 t)`), alpha 0.8;
- both x `1 - smoothstep(vNearFarParams.x, .y, w)`.

`DarkVision_Souls_INST`: `Full_Color` (1, 0.947, 0), `Behind_color` (0.859, 0.810, 0);
`vNearFarParams` default (1800, 2000), the power's `m_fFadeStartAtDistance` / `m_fDistance` are
(1700, 2000) (used, LIKELY set by the power). The port draws two passes with the two depth tests
(`SoulFrontMaterial`, `SoulBehindMaterial`), alpha blended. The items / mechanics instances
(`m_pItemMaterial`, `m_pMechanicMaterial`, level 2) are not applied (the port reveals pawns only).

The screen materials (`DisDarkVisionPpController` on `Test_PPG`'s `ArkPpNodeMaterial_9`:
`PPG_EyeLidFinal_Mat`, `PPG_DarkVisionFinal_Mat`) have no shader map in RefShaderCache or any
package (no other shader cache in the install), so their pass cannot be decoded from shaders;
only the tweak's grade is applied. The debug capsules stay behind F3.

## Power screen effects (the post-process chain)

`Startup` `AltScreen_Effects.PostProcessChain.Test_PPG` is the game's chain: switches
`BendTimeSwitch` / `BendTimeUnderWaterSwitch` / `BlinkSwitch` / `PossessionSwitch` /
`AdrenalineSwitch` / `KOSwitch` / `LensSwitch` select vector-field materials
(`PPG_BendTimeVectors_Std_INST`, `_UnderWater_INST`, `PPG_BlinkVectors_*`,
`PPG_PossessionVectors`, `POSSESSION_IN_INST` / `OUT_INST`, `PPG_AdrenalineVectors_INST`,
`PPG_KOVectors`, `PPG_LensVectors`) feeding `ArkPpNodeBlur.m_VectorField` (a directional blur of
the scene along the field), with `DisPostProcessManager.PpBridge` driving the switches
(`m_BendTimeIntensity`, `m_fPostEffectWarmupTime` / `CooldownTime` 2 s in DefaultPower.ini). None
of the `PPG_*` materials has a shader map in RefShaderCache; their shaders were captured from
the running game (oracle shader capture `oracle/shaders/70da96138fe2171c`, `effects/*.json`:
the shaders drawn only while the effect is on, their constants and the post manager's channels
per tick). Decoded and ported (`power_pp.rs`, `ark_post.wgsl`):

- Bend Time: vector field `0xbbe665d07e36986b` (`PPG_BendTimeVectors_Std_INST`; its
  `UniformExpressionTextures` read from the material's native data: `Tex#0` = `water2`,
  `Tex#1` = `fxt_eyelid_gradient`): `50 x vBendTimeOpacity x (1 - eyelid.r) x (0.15 x edge x
  (n1 + n2) - (2 uv - 1))`, n = water2.rg x 2 - 1 at `uv + frac(T (0.2, -0.1))` and
  `uv + frac(T (-0.15, 0.3))` (pans measured from the constants: per world second at the 0.85
  dilation), edge = saturate(10 - |10 (2 uv - 1)|). `ArkPpNodeBlur` passes `0xbd9a42b61794aabb`
  / `0x6095a5daa9109b1d`: 4 taps at `uv + k x field x gVectorFieldScales` (k = 0..3), scales
  `1 / (16 x size)` then `1 / (4 x size)` (captured (4.88e-5, 8.68e-5) and (1.95e-4, 3.47e-4) at
  1280 x 720), the second on the first's output. Intensity `bt` (= vBendTimeOpacity): +0.0333 per
  60 Hz frame from the cast (0.5 s, the tweak's `m_fPostEffectWarmupTime`), 1 while it lasts, down
  over `m_fPostEffectCooldownTime`. The port evaluates the field per tap inside the uber pass
  (same values at pixel centres as the original's field target).
- Blink: its own field `0x0b36c71ce1512549` (`PPG_BlinkVectors`, `Tex#0` =
  `fxt_eyelid_gradient` from the native data), with d = uv - 0.5, rho = |(1.975 d.x, 2 d.y)|:
  rg = 240 y (1 - saturate(gradient.r + 0.5 - y)) d (radial blur), ba = 0.15 x (1 - rho^3) d (lens
  bulge, the blur's second pass starts at uv + ba). x / y (`UniformPixelVector_1.xy`, the
  `blink` / `blinkg` channels; `Bend Time`'s `bt` stays 0 during a Blink) from
  `blink_lens_timeline.json`: aiming, x wobbles up to 1.3 x then rests at
  `m_fWarmupDistorionMinStrength` x `m_fWarmupWobbleMaxStrength` (0.35 x 0.2 = 0.07; the captured
  shape is replayed relative to that rest); travelling, x and y grow to
  `m_fMoveDistortionMaxStrength` 1.5 / `m_fMoveBlurMaxStrength` 2.5 by
  `m_fMoveReachMaxAtPercentage` 0.8 of the distance (cubic fit of the samples); after, they decay
  with a 0.24 s time constant, x with `m_CoolDownWobbleCount` 10 wobbles over `m_fCooldownTime`.
  The residual 0.0075 the capture keeps while Blink stays selected is not reproduced.
- Dark Vision: the eyelid pass `0x3c1c89ab574a7626` (`PPG_EyeLidFinal_Mat` /
  `PPG_DarkVisionFinal_Mat`, texture `fxt_eyelid_gradient`): `lerp(EyeLidColor,
  gradient.g x lerp(scene, souls, souls.a), saturate(gradient.r - (1 - 2 Alpha)))`. The souls are
  rendered into their own target in the original and blended here; the port's soul passes blend
  into the scene directly (the same lerp). `darkvision_eyelid_timeline.json`: the colour is
  `EyeLidColor` in linear space (0.1275, 0.0892, 0.0582); Alpha is the power's fade (0 -> 1 over
  `m_PowerFadeInDuration`, 1 for the duration, the fade-out remaining over
  `m_PowerFadeOutDuration`, one frame late), the pass drawn only while the power is on.
- Possession: no screen pass among its active-only shaders (the possession travel's distortion
  meshes and particles only); the `pos*` channels rise ~1 s after the cast (`pos` +0.18 / s,
  `posg` +0.55 / s). Not ported.
- The arm glow is not among the captured shaders.

## Distortion (U-360 resolved, U-400 open)

Materials with `bUsesDistortion` (`DefaultAdditive_PMAT` and the other particle bases; the
instances switch it on with `Dist_Enable_Distortion`, and `D_Disable_Diffuse` leaves only the
distortion, e.g. the pistol muzzle's `Vfx_GamePlay.Windblast.WindWave_Disto_01_inst`:
`Dist_Distortion` = `Assassins_Teleport_01`, `Dist_Distortion_Intensity` -100, `O_Opacity` =
`Wind_Swirl_01`, UV scrolling (0.1, -0.5), vertex colour on) are drawn a second time after
translucency, as UE3 does:

1. Accumulate (the material's distortion pixel shader; 133 such shaders in
   `RefShaderCache-PC-D3D-SM3.upk`, all with the same tail, e.g. 36064/36065 for the
   `DefaultAdditive_PMAT` family with the sprite and mesh vertex factories): D = the material's
   `Distortion` input in 1/255 screen units. For the Arkane bases it is a scalar (both offset
   components equal): `Dist_Distortion.r` (at the UV-distorted / scrolled coordinate) x a uniform
   (the intensity) x vertex colour alpha x `O_Opacity.r`. Pixels with |D|^2 < 0.1 are discarded;
   the scene depth (scene colour alpha x 256 in the original) at `uv + (Dx, -Dy) / 255` is
   compared with the pixel's depth and D is zeroed when the scene is in front; then
   E = clamp(4 D / 255, -1, 1) is added into an RGBA8 target, max(E, 0) in RG and max(-E, 0) in BA.
2. Apply (global cache shader 237, `AccumulatedDistortionTexture`, `SceneColorTexture`,
   `SceneColorRect`): the scene colour at `uv + (RG - BA) x (0.25, -0.25)`, or at `uv` when that
   coordinate leaves `SceneColorRect`.

Port: `distortion.rs` (`DistortionMaterial::decode`, the per-frame `DistortionFrame` of
distorting triangles built by `effects::simulate` from the same particles, sprite and mesh
emitters) and `distortion.wgsl` / `distortion_apply.wgsl` in the Core3d `EarlyPostProcess`
set, before the ArkPp pass. The offset target is RGBA16F; the apply pass saturates the sum at 1
like the RGBA8 target.

The material terms come from the shader maps' uniform expressions (`FUniformExpressionSet` in
the RefShaderCache: per material, arrays of pixel vector and scalar expressions serialized as
type FName + payload, `FMaterialUniformExpressionScalarParameter` = name, default;
`FoldedMath` = A, B, op byte; `Constant` = FLinearColor + value type byte; `AppendVector` =
A, B, component count; `Periodic` = frac; ...). For the mesh-emitter permutation matching
shader 36094: `UniformPixelVector_1` = `D_DiffuseColorTweak`, `_2` = `MeshEmitterVertexColor`
(the particle colour), `_3` = (frac(Time x `Dist_Distortion_Speed`), same) the distortion pan;
scalars `U_Scrolling_Speed` x Time and `V_Scrolling_Speed` x Time scroll the `O_Opacity`
sample; `Dist_Distortion_Intensity` multiplies the `Dist_Distortion` sample. So D =
`O_Opacity.r`(scrolled) x `D_DiffuseColorTweak.a` x `MeshEmitterVertexColor.a` x vertex colour
x `Dist_Distortion.r`(panned) x intensity, which the port evaluates (no static-mesh vertex
colours are imported, so that factor is 1). U-400 keeps the per-permutation details open.

## Approximations and open points

- Bursts at emitter time 0 fire on the first cycle (until 2026-10-06 they only fired from the
  second loop on, so one-shot effects lost their time-0 bursts, e.g. the muzzle shockwave).

- Burst `Time` as a fraction of `EmitterDuration`; spawn-rate distributions sampled with the
  emitter time in seconds (UE3 `ParticleEmitterInstance::Tick` convention).
- Rotation units: turns (x 2 pi), as UE3's Rotation/RotationRate modules.
- Sprite size: `Size.x` is the full sprite width.
- Light modules and particle lights are not rendered; collision uses the static level
  collision only (no pawns).
