# Rendering area 4: lighting (WIP audit)

Scope: lightmaps and shadow maps, dynamic shadows, light functions, light environments, decals,
lens flares, emissive / glow. Status: lens flares, the caustic light functions and level streaming are implemented (see "Implemented" below); the flicker functions are `LightFlicker` in `lighting.rs`; the rest of the table is open.
lens flares, emissive / glow. Status: characters / arms / ragdolls lit by their light environment,
gameplay decals (contact system and particle decals), ShadowMap1D decoded, dominant point light
shadow behind the original's settings (sections "Characters", "Gameplay decals", "ShadowMap1D"
and "Dominant point light shadow" below). The lens flare and light function notes further down
are from the first round; flicker light functions and the depth-dependent flare occlusion were
done by other branches.

## Class census (retail CookedPCConsole, `package_scan`)

Counts are exports in the 353 `L_*` level packages (all packages in brackets where different).
Per-map coverage is not computed yet (see next steps).

| Feature | Data source (classes) | Count in levels (all) | Port status |
|---|---|---|---|
| Directional lightmaps | LightMapTexture2D | 19036 (19626) | Done: `lightmap_ext.wgsl` (HL2 basis, diffuse + specular) |
| Distance-field shadows | ShadowMap2D / ShadowMapTexture2D | 46798 / 5425 | Done for the dominant directional light and one dominant point/spot per batch (`lightmap_ext.wgsl`) |
| Vertex shadow maps | ShadowMap1D | 15 (25) | Decoded and imported (`DishonoredMap::vertex_shadows`); not drawn: see "ShadowMap1D" |
| Dominant lights | DominantDirectional / DominantSpot / DominantPoint LightComponent | 30 / 169 / 23 | Done (`lighting.rs`); point lights unshadowed by Bevy (cube cost) |
| Static point / spot lights | PointLightComponent / SpotLightComponent (StaticLightCollectionActor 189) | 4300 / 4698 | Baked in lightmaps; composited into light environments |
| Toggleable / movable lights | PointLightToggleable 61, DisPointLightToggleable 66, SpotLightToggleable 14, DisSpotLightToggleable 1, Point/SpotLightMovable 1 each | | Spawned as Bevy lights (`lighting.rs`); toggle via Kismet not audited |
| Sky lights | SkyLightComponent | 43 (53) | Used only through light environments' upper/lower sky |
| Light functions | LightFunction | 124 (126) | Flicker (110): `lighting.rs` `LightFlicker`. Caustics (14): done, `caustics.rs` |
| Light environments | DynamicLightEnvironmentComponent 6789, LightEnvironmentComponent 88, ParticleLightEnvironmentComponent (class) | | Done for doors/usable props (`prop_light.rs`, `dishonored_assets::light_env`); characters and other dynamic props to audit |
| Rat lights (gameplay) | DisRatLightComponent 67, DisTallboyLightComponent (2) | | Not audited |
| Wall of Light | DisWallOfLight 27 | | Not audited (gameplay/emissive) |
| Decals (actors) | DecalActor / DecalComponent | 0 in levels; DecalComponent 7, DishonoredDecalComponent 3 in non-level packages | No placed decal actors in any map. Level decals are translucent static meshes (`decals.rs`, generic_translucent_PMAT) |
| Impact / particle decals | DisContactSub_Decal 14, ParticleModuleTypeDataDecal 337 (357), DisDecalManager | | Not done (gameplay-spawned: blood, impacts) |
| Lens flares | LensFlareSource 23, LensFlareComponent 23, LensFlare 20 | | Done: `lens_flare.rs` |
| Light functions | LightFunction | 124 (126) | Not done; need the owning lights and their function materials |
| Light environments | DynamicLightEnvironmentComponent 6789, LightEnvironmentComponent 88, ParticleLightEnvironmentComponent (class) | | Done: doors/usable props (`prop_light.rs`), characters, the player's arms, ragdolls (`char_light.rs`). Rats, projectiles, the Spring Razor and weapons have none (see "Characters") |
| Rat lights (gameplay) | DisRatLightComponent 67, DisTallboyLightComponent (2) | | Not audited |
| Wall of Light | DisWallOfLight 27 | | Not audited (gameplay/emissive) |
| Decals (actors) | DecalActor / DecalComponent | 0 in levels; DecalComponent 7, DishonoredDecalComponent 3 in non-level packages | No placed decal actors in any map. Level decals are translucent static meshes (`decals.rs`, generic_translucent_PMAT) |
| Impact / particle decals | DisContactSub_Decal 14, ParticleModuleTypeDataDecal 337 (357), DisDecalManager | | Done as flat quads (`effects.rs`, `present_rules.rs`): see "Gameplay decals" |
| Lens flares | LensFlareSource 23, LensFlareComponent 23, LensFlare 20 | | Not done |
| Emissive / glow | material E_ terms; bloom (area 3) | | Partial: emissive switches in materials; door emissive placeholder off |

## Map package lists

`map_dump <map> --survey` (first line) gives each persistent map's packages; 41 persistent
`L_*_P` maps. Partial output was being collected when the task was stopped.

## Lens flares (dump: LensFlare*.txt, 23 sources)

- Only two templates exist: `LensFlares.Candles_01_LF` (every candle: the `candle_flames_01_prefab` source,
  embedded in about 19 sub-level packages: ArtDealer, Boyle_Int, Bridge_Part1a/1b, Brothel, Distillery,
  Distillery2, Galvani1, Ovrsr_Light, PrsnSewer, Streets1, Streets2, Streetsewer, TowerRtrn_Int, OutsiderDream)
  and `Lighthouse_01_bo.light06_LF` (L_Streetsewer_Block, source `LensFlareSource_9`).
  Placed free sources: Bridge_Part1a_Env (`LensFlareSource_0`, loc -417,-1402,1392), Bridge_Part1a_Script
  (`LensFlareSource_0`, loc -4167,-5125,641), OutsiderDream_Script (`_1`), Streetsewer_Block (`_9`).
- Candles_01_LF: one SourceElement, Size 250, bUseSourceDistance, bNormalizeRadialDistance, one LFMaterial
  (`Materials_ref.FX_materials.LensFlare_PMAT`, BLEND_Additive, Unlit, 48 expressions: flare mask texture,
  dust texture, params L_LensFlare_Power 5, L_Maximum_Opacity 1, Lg_LensFlare_Glowing_Speed 0.5 / Range 0.1,
  Db_Depth_Biased_Alpha_Intensity 0.1), FixedRelativeBoundingBox, ScreenPercentageMap (4 key lookup table).
  Component: DepthPriorityGroup Foreground.
- Status: implemented (see "Implemented"). The distribution tables are decoded with `dishonored_assets::particles::dist`.

## Light functions (dump: LightFunction*.txt, 124 functions in 34 packages)

- Function materials: `Materials_ref.FX_materials.Flickering_light_function_PMAT` (68 uses, spot lights
  mostly), `Hub_MS.Flickering_light_function_WoodStove_PMAT` (28), `Hub_MS.Flickering_light_function_Boiler_PMAT`
  (20), `Vfx_Materials.main_materials.Casutics_LightFunction_PMAT` (14, Scale 600 on 8; Unlit,
  bUsedAsLightFunction, parameters Speed 0.35, colour 0.859, 26 expressions, 2 textures).
- Maps (function count by package family): Pub Night 16 / Day 15 / Dusk 14 / Morning 14 / Assault 12,
  PrsnSewer 5(+2), Flooded_FAssassins 4, Streetsewer lights 4, Flooded_FRefinery(Env2) 3+3, Flooded_FStreets 3,
  Bridge_Part1b 3, Boyle_Ext 3, Out_HighChaos_EmilyAlive 2, Isl_Geom_Slave/Master 2+2, Brothel 2, Ovrsr_Kennel 2,
  and 1 each in Prison, TowerRtrn_Int, Distillery(2), Bridge_Part1a, ArtDealer, MainMenu, Flooded_FGate,
  Ovrsr, Ovrsr_Back, Out_EnvCallistaDead, Galvani1, Isl_HighChaos.
- Every owner is a Spot/Point/DominantPoint component inside a `StaticLightCollectionActor`, i.e. a light the
  importer currently skips as baked (`level.rs::import_light`, `static_lights_skipped`). Whether the engine
  evaluates the function for such a light at run time or only bakes it is the open engine question.
- Status: the caustics (14) are implemented; the flicker functions are `LightFlicker` in `lighting.rs`.

## Implemented (lens flares, caustics, level streaming)

Lens flares (`lens_flare.rs`; imported by `dishonored_assets::level::import_lens_flare`: the 23 actors of the
persistent levels, the candle prefab sources included):
- One camera-facing quad per source at the source position, in `SourceDPG` `SDPG_World` (depth tested,
  Dishonored.exe 0x008e5b50), additive. The quad and colour follow the vertex shader of `FLensFlareVertexFactory`
  (RefShaderCache 1332: inputs POSITION, TANGENT, BLENDWEIGHT, TEXCOORD0-2; constants CameraRight, CameraUp,
  LocalToWorld, ViewProjectionMatrix): size = Size x Scaling x AxisScaling x DistMap_Scale (at the camera distance,
  uu), colour / alpha = Color x DistMap_Color, Alpha x DistMap_Alpha, turned by Rotation. The element is evaluated
  at the source distance (0, bUseSourceDistance, 0x008e1dc0).
- The pixel shaders are the material's own, translated by `ue3_shader` (`ue3prog::build_fragment`): the
  `FLensFlareVertexFactory` base pass pixel shader of the `LensFlare_PMAT` shader map that the instance's static
  switches select (2643 plain, 2645 glow, 2647 glow + depth-biased alpha, 2649 mask, 2651 mask + dust; 2644, 2646,
  ... are their `FLocalVertexFactory` twins). Inputs, from the shaders' declarations: TEXCOORD0.xy the quad uv,
  TEXCOORD1 the vertex colour, TEXCOORD2.y the source's radial distance, TEXCOORD5.xyw the screen position (the
  depth-biased alpha reads the scene depth from the depth prepass). Parameters and the mask and dust textures come
  from the material instance's `Ue3Material` link.
- Radial distance, from 0x008e1dc0: sqrt(x^2 + y^2) of the source's screen position, and with
  bNormalizeRadialDistance divided by sqrt(ratio^2 + 1) of min / max, i.e. max(|x|, |y|). LIKELY: the position is in
  normalised device coordinates (its caller, 0x008e5b50 / 0x008e63c0, has no decompilation in the index). The pixel
  shader fades the flare by 1 - L_Radial_Distance_Factor x distance (0.5 for the candles).
- Not applied: `ScreenPercentageMap` (template +0x19c, proxy +0x12c; the constant 0 with time scale 0 in the retail
  templates; its reader is in the undecompiled draw), the occlusion query (the retail path is the depth test plus
  Db_Enable_Depth_Biased_Alpha), reflection elements (none in the retail templates).
- Culled with the template's FixedRelativeBoundingBox, hidden with the actor's bHidden, leaves with its streamed
  sublevel. `--no-lens-flares` turns it off; without the shader caches no flare is drawn.

Caustic light functions (`caustics.rs`, `caustic_light.wgsl`; 14 lights: Boyle Ext, Brothel, flooded Assassins, island):
- Those spot lights are not Bevy lights. Each opaque static mesh section that shares a lighting channel
  (`ULightComponent::AffectsPrimitive`; the lights have {Dynamic, Unnamed_6}) with a light and lies in its radius is
  drawn once more, additively, with the light's UE3 light-pass terms times the function; no per-frame CPU work.
- The function is `FLightFunctionPixelShader` (RefShaderCache 36479) of `Casutics_LightFunction_PMAT`, translated by
  `ue3_shader` and appended to `caustic_light.wgsl` at run time, with its two textures (T_EV_DustPanner_01,
  T_Caustics_2), Speed and CausticColor. It reads the scene depth from the depth prepass (SceneColorTexture alpha,
  uu / 256), rebuilds (x w, y w, w, 1) and multiplies it by ScreenToLight (c6..c9), using the columns x, y and w.
  LIKELY: that matrix is the function's world-to-light transform over LightFunction::Scale (affine, so w = 1) with
  the function's xy being the light's Y and Z axes; GreyscaleFactor = 1.
- `--no-caustics` turns it off (the lights are then the plain Bevy lights).

Level streaming (`level.rs`): entities of a Kismet-streamed sublevel (`DishonoredMap::streamed_levels`: meshes,
merged batches, lights, emitters, props, flares) carry `SourceLevel(package)`; `apply_streaming` hides them while
`MatchView::hidden_levels` names the package and shows them again when it does not. It does its pass only when the
hidden list, the active map, a level spawn or newly tagged entities changed, over the tagged entities only. Without a
session (`--inspect`) every streamed sublevel is out except those named in `DISHONORED_STREAM_VISIBLE`.

## Needed dumps (to continue; the .bin tools are denied to this agent by the permission classifier)

1. `obj_dump` full expression graph (expression class, inputs, constants, texture refs) of
   `LensFlares.Candles_01_LF`, `Lighthouse_01_bo.light06_LF`, `Materials_ref.FX_materials.LensFlare_PMAT`,
   the three flicker materials and `Casutics_LightFunction_PMAT`, including the raw distribution lookup tables.
2. Engine side (oracle index / Ghidra): how static lights with a `Function` are treated by the lighting build
   (`ULightComponent` function / `HasStaticLighting`) and in the light pass (function shader sampling), the
   lens flare render path (`FLensFlareRenderer`, occlusion and `ScreenPercentageMap` use).

## Next steps (after the dumps)

1. Lens flares: screen-space additive sprites, GPU occlusion by sampling the depth texture (area 1) at the source.
2. Light functions: flicker multiplier on the matching Bevy lights (time-driven from the decoded material), caustic
   function as a projected texture on the Scale-600 point lights.
3. Light environments for characters and dynamic props (`light_env` + `prop_light.wgsl`).
4. Gameplay decals (DisDecalManager), ShadowMap1D, dominant point shadow toggle, `render_fixes.sh` entries
   (L_Pub_Night_P, L_Streetsewer_P, L_Bridge_Part1a_P for flares and functions).

## Graph dumps (decoded so far)

- Lens flare element (identical Size 250 for both templates, bUseSourceDistance, bNormalizeRadialDistance):
  Candles_01_LF: Scaling table [1.3 x4]; Color table [0.5 1.9 1.9 1.5 0.5 1.9 1.5 0.5]; DistMap_Scale table
  [0.25 3.0 0.25 0.25 0.25 3.0 3.0 3.0] (LookupTableTimeScale 1/2048); DistMap_Alpha [0.15 0.25 0.15 0.25]
  (same time scale); ScreenPercentageMap [0 x4] (time scale 0). Material: `LensFlare_PMAT_INST` parent
  `LensFlare_PMAT`: L_Radial_Distance_Factor 0.5, L_LensFlare_Power 10, L_Maximum_Opacity 1, L_Minimum_Opacity 0,
  L_LensFlare_Color (1, 0.7508, 0.4548, 4.0), mask texture `tguichard_LookAt.LensFlare.Txr_LensRoot`,
  dust `Materials_ref.Textures.clouds`. Lighthouse_01_bo.light06_LF: Color [1 5 5 4 1 5 4 1], Alpha [0.25 x4],
  same DistMap tables, material instance `LensFlares.generic_foggy_light_01_INST`.
  Open: the table layout. A vector table of 8 floats is not a multiple of 3, so the header/chunk layout of
  the cooked `FRawDistribution` (LookupTableNumElements / ChunkSize / StartTime / TimeScale and the sample
  order) is needed before the values can be used (U-lensflare-table).
- Light functions: the cooked expression arrays of Flickering / WoodStove / Boiler keep only
  F_Flickering_1_speed (5 / 1 / 0.5) and F_Flickering_2_speed (3 / 0.25 / default 0); Caustics keeps Speed
  0.35 and CausticColor 0.859. The graph bodies are not recoverable from the package; they come from the
  compiled light function shaders in RefShaderCache (not yet located).

## Characters, arms, ragdolls (light environments; `char_light.rs`, `char_light.wgsl`)

Engine side (decompiled): `FUN_005035d0` is the update of a `DynamicLightEnvironmentComponent`
(schedule), 0x00501B70 the static environment (`dishonored_assets::light_env::static_environment`:
the level's static lights that reach the bounds and see them, plus the floor level's precomputed
light volume, as an SH), 0x004E18F0 the blend (the current SH moves linearly to the new target
over the update interval) and 0x004F85C0 the synthesis of the directional light and the SH light
from the SH. Class data: `MyLightEnvironment` of the pawns (`bDynamic`, `bSynthesizeDirectionalLight`,
`bSynthesizeSHLight` on `DishonoredPawn`), `InvisibleUpdateTime` 5, `MinTimeBetweenFullUpdates` 1,
`WorldInfo.bAllowLightEnvSphericalHarmonicLights` true. Shading is the base pass
`TBasePassPixelShader<FSHLightLightMapPolicy>` (shader 2941).

- Characters (`CharacterRoot`): the environment is recomputed when `now - last update` exceeds
  `rand(0.8, 1.2) x (visible ? 1 : 5) x clamp(distance / (4 x bounds radius), 1, 10)`; the blend
  and the synthesis run on the GPU (previous SH, target SH, start and end time per material), so
  nothing per entity per frame beyond the schedule check.
- The player's arms (`ViewRoot`): `DLEB_ManualOverride` bounds, the pawn's collision cylinder (the
  stealth light environment's: stealth.md section 10).
- Ragdolls: the pawn entity keeps its `CharLight` when it dies or is thrown. The bounds follow the
  bodies (`Ragdoll::center`, the box of the body positions) instead of the bind pose (LIKELY: the
  original's bounds are the posed mesh's).
- The arms' power glow (`viewmodel::arm_glow_apply`) now also drives the lit copies of the arm
  materials (`char_light::CharMaterial`), which are copies of the standard materials.
- With the original-shader level path (`ue3_level.rs`) nothing changes for characters: that path
  only draws a level's static mesh sections; characters keep Bevy's dominant / toggleable lights
  (`lighting.rs`, the UE3 falloff patch) added to the SH light. `DISHONORED_LEGACY_CHARACTER_LIGHT=1`
  restores Bevy's lighting on characters.
- No light environment in the class defaults (checked with `obj_dump`): `DisGameCrowdAgentSkeletalRat`,
  `DisProjectile_Grenade`, `DisProjectile_Arrow` (their `Components` array holds the skeletal mesh
  component only). Rats, projectiles and the Spring Razor therefore stay on Bevy's lighting; held
  weapons and accessories keep the standard material.
- dishonored-bevy's `world_light.rs` (CPU estimate of irradiance from nearby lights and sun, a
  slot per actor, smoothing) was read and not used: it approximates the environment by hand where
  the decompiled engine gives the SH, the schedule and the blend. Its light volume weight
  `(1 - d^2 / r^2) / r^2` is the same form as `light_env::VolumeSample`'s.

## Gameplay decals

### Decoded (oracle index + obj_dump of the retail packages)

- `DisDecalManager` (DishonoredGame, 0x290 bytes; `DecalManager` Engine 0x284): const
  `MaxDecalCount` = 10; `m_DecalActorPool` (`DishonoredDecalActor`s); class defaults inherited
  from `DecalManager`: `DecalLifeSpan` 10, `DecalDepthBias` -6e-5, `DecalBlendRange` (89.5, 180),
  `MaxActiveDecals` 0 (no cap on the stock `SpawnDecal` path: `FUN_00504d00` recycles the oldest
  active decal only when `0 < MaxActiveDecals <= ActiveDecals.Num()`).
- The pool, `FUN_00bc6480`: with ten live decals, the first in pool order whose
  `PrimitiveComponent.LastRenderTime` is more than 0.2 s older than `WorldInfo.TimeSeconds` is
  destroyed; if none is, no decal is made. A new decal is added to the manager's `ActiveDecals`
  with `LifetimeRemaining` = `m_fLifetime`. LIKELY: the stock manager counts it down and releases
  the decal at 0 (`ActiveDecalInfo` has no other field; the tick that does it was not located:
  `OnDecalFinished` is vslot 232). `FUN_009e3f70` (vslot 169) is a second expiry test, "done and not
  rendered for `m_fLifetime`"; its caller was not found.
- `DisContactSub_Decal.m_DecalInfo` = `DishonoredDecalInfo` (0x48 bytes): `m_pMaterial` +0,
  `m_Materials` +4, `m_BoneNames` +0x10, `m_Behaviour` (byte) +0x1c, `m_MinWidth` / `m_MinHeight` /
  `m_MaxWidth` / `m_MaxHeight` +0x20.., `m_bLoop` +0x30, `m_fTimeStep` +0x34, `m_fWidthStep` +0x38,
  `m_fHeightStep` +0x3c, `m_fLifetime` +0x40, `m_fMinVelocity` +0x44.
- The fire function, `FUN_00bcf000` (vslot 73): fires only if `m_Materials` is non-empty
  (`m_DecalInfo + 8 > 0`) and, when the contact has a velocity, `m_fMinVelocity^2 < |v|^2`; takes a
  pooled `DishonoredDecalActor`, orients it from `FindBetween((-1, 0, 0), normal)`, sets
  `DecalRotation` to the signed angle (about the normal) between a reference axis and the
  velocity projected on the surface. The reference axis comes from `FUN_00401fd0` (a rotation
  of a vector about an axis) whose arguments are not recovered: the port aligns the texture axis
  with the projected velocity itself (LIKELY).
- The decal component (`DishonoredDecalComponent`, ctor `FUN_00a0c3c0`): a random entry of
  `m_Materials` (`m_pMaterial` only when the list is empty), `Width` / `Height` = `m_MaxWidth` /
  `m_MaxHeight`, growth state starts at `m_MinWidth` / `m_MinHeight`. The tick `FUN_00a01000` grows
  (`FUN_009fb520`: every `m_fTimeStep` add `m_fWidthStep` / `m_fHeightStep`, to the maximum; looping
  restarts at the minimum; the material gets a UV scale / offset parameter of
  `Width / RuntimeWidth`) only for `m_Behaviour` = 1. Every retail sub has `m_Behaviour` 0 (the
  property is never set): done at once, the decal is its maximum size from the first frame.
- Retail table (`Dis_ContactSystem`): 13 pairs carry a decal sub. Only the subs with `m_Materials`
  can fire: Bullet x Environment (`Bullet_Impact_Metal_01_INST`, 8 x 8 uu, 10 s), Corpse /
  Limb / BreakingBones x Environment (`blood_splatter_decal_INST`, 150 x 150, 10 / 10 / 15 s, speed
  above 750 uu/s), PistolBlast x Environment (`Bullet_Impact_Metal_01_INST`, 7 x 7, 7 s). The
  subs for Arrow, Arrow_Sleep and the Bullet x Env_Metal / Env_Wood / Env_Glass pairs carry
  `m_pMaterial` only and therefore never fire (as decoded). Bodies hitting the ground are not
  simulated (the sim's body contact has speed 0), so the blood subs are not triggered; the bullet
  impact is.

### Port

- `dishonored_audio::contact::ContactDecal` (+ `ContactSystem::decal` / `decals`, `MAX_DECAL_COUNT`,
  `DECAL_EVICT_AFTER`); `present_rules::contact_decal` turns a projectile impact into
  `FxRequest::Decal` (the struck surface's normal from the collision world, the launch direction
  as the texture axis); `effects.rs` makes the quad (`decal_mesh`), keeps the pool of 10 with the
  0.2 s eviction rule (`ContactDecal` component, `age_decals`, `ViewVisibility` for
  `LastRenderTime`) and ends a decal after `m_fLifetime`.
- The decal materials (`GAMEPLAY_DECAL_MATERIALS`) are imported with every level
  (`ImportOptions::extra_materials` -> `DishonoredMap::named_materials`).
- APPROXIMATION: a flat quad drawn as a sprite of the material, not the decal shader
  (`decals_fx_PMAT`: an unlit translucent material, `D_Diffuse`, `M_Mask`, `D_DiffuseBrightness`)
  and not the clipped receiver triangles of `DecalComponent` (FieldOfView 80, FarPlane 300,
  BlendRange 89.5..180 degrees fade). The quad sits 0.25 uu above the surface.
- Particle decals: `ParticleModuleTypeDataDecal` (6 in DishonoredGame, 337 in all packages:
  copies of the same systems in the level script packages; 8 materials: `Burn_Decal_INST` 75,
  `blood_gibs_splatter_inst` 72, `blood_splatter_decal_INST` 69, `Vomit_Splatter_01_Inst` 64,
  `RK_splatter_decal_INST` 33, `m_decal_glow` 33, `Burnt_Blast_01_inst` 7, a few others) is a decal
  emitter. `Pfx_BloodMotion` shows the pattern: a drop emitter with `ParticleModuleCollision`
  and an `ParticleModuleEventGenerator` event (`EPET_Collision`, `FirstTimeOnly`, `CustomName`
  "Blood_Decal"), and a decal emitter with `ParticleModuleEventReceiverSpawn` for that name
  (`SpawnCount` 1), size 40..80, `Lifetime` 5..7 s and an alpha-over-life fade-out. The importer
  reads the type data, the generator's events and the receiver (`ParticleEmitterAsset::{decal,
  events_out, events_in}`); the simulation (`present_rules`) reports collision / spawn / death
  events and `dispatch_events` spawns the receivers' particles at them with the surface normal;
  `build_mesh` draws a decal emitter's particles as quads lying on that surface, turned by the
  particle's rotation (adapted from dishonored-bevy, NOTICE). A decal emitter no event feeds is
  placed by a ray along the particle's velocity (down when it has none; LIKELY) and has no decal
  if nothing is within 120 uu.
- Setting: `Graphics::dynamic_decals` (BaseEngine.ini `DynamicDecals` / `bDynamicDecalsEnabled`
  True; not in the video menu) gates the contact decals; `--no-decals` turns them off. The particle
  decals stay with the particle effects (`--no-fx`).

## ShadowMap1D

15 exports (25 counting the non-level packages): `L_Flooded_FRefinery_Void` 1, `L_OutsiderDream_FX`
3, `L_Out_EnvLabPiero` 1, `L_Out_EnvLabPieroAndSokolov` 1, `L_PrsnSewer_FX` 1, `L_Pub_Assault_FX` /
`_Dusk_FX` / `_Morning_FX` 1 each, `L_Streets1_Block` 3, `L_Streets2_Block` 1, `L_Tower_Fx` 1;
all but one on `InterpActor`s (matinee movers), one on a `StaticMeshCollectionActor` mesh.
Layout (`export_hex` of the retail exports, 788 bytes each = 189 vertices): no tagged
properties; `u32 count`, `count` x `f32` (the dominant light's visibility of each vertex of the LOD,
0 shadowed .. 1 lit; values such as 0.944, 0.407, 1.0, 0.0), then the light's `FGuid`
(`ShadowMap2D` keeps it as a property). Components reference them in their LOD's
`ShadowVertexBuffers`, beside a vertex lightmap (`FLightMap1D`, still undecoded: the importer counts
them in `lightmaps_vertex`).

Port: `lightmap::read_shadow_map_1d` reads them (strictly: the stream must end at the export's end);
the importer stores them as `DishonoredMap::vertex_shadows` (instance, light guid, samples). NOT
DRAWN: the merged batches of `map_render` and the shared per-section meshes of `ue3_level` have no
per-instance vertex data, and these components are lit by a vertex lightmap Blink does not decode,
so the factor would multiply a light the component does not yet receive. Next step: decode
`FLightMap1D` (`DirectionalSamples` bulk, two scale vectors, `SimpleSamples` bulk) with the
vertex shadow, give those instances their own mesh with the samples as an attribute, and
multiply the dominant light of `lightmap_ext.wgsl` by it where `df_shadow` is not set.

## Dominant point light shadow

The first-round decision stands (lighting.rs, `RenderFeatures::point_shadows`): the original's
`DynamicShadows` and `LightEnvironmentShadows` ([SystemSettings], both True; no video menu entry;
`dishonored_settings::Graphics::{dynamic_shadows, light_environment_shadows}`) gate the dominant
point light's Bevy cube shadow map (six scene passes, so only the dominant light has one);
`--no-point-shadows` / `DISHONORED_NO_POINT_SHADOWS` turn it off. Maps with dominant point lights:
`L_Flooded_FRefinery_P` (6), `L_Flooded_FAssassins_P` (2), `L_Out_HighChaos_emilyDead_P` (3),
`L_Boyle_Ext_P`, `L_Brothel_P`, `L_Out_P`, `L_Streetsewer_P` (1 each). With the original-shader
level path the dominant point / spot lights' passes (`ue3_level.rs`) use the distance fields of
the level geometry; the Bevy cube shadow map then shadows characters and dynamic props only
(sections drawn with the original shaders are not lit by Bevy's lights).
