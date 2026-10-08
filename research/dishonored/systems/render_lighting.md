# Rendering area 4: lighting (WIP audit)

Scope: lightmaps and shadow maps, dynamic shadows, light functions, light environments, decals,
lens flares, emissive / glow. Status: audit extended with lens flare and light function dumps; no code changed (blocked on the dumps listed under Needed dumps).

## Class census (retail CookedPCConsole, `package_scan`)

Counts are exports in the 353 `L_*` level packages (all packages in brackets where different).
Per-map coverage is not computed yet (see next steps).

| Feature | Data source (classes) | Count in levels (all) | Port status |
|---|---|---|---|
| Directional lightmaps | LightMapTexture2D | 19036 (19626) | Done: `lightmap_ext.wgsl` (HL2 basis, diffuse + specular) |
| Distance-field shadows | ShadowMap2D / ShadowMapTexture2D | 46798 / 5425 | Done for the dominant directional light and one dominant point/spot per batch (`lightmap_ext.wgsl`) |
| Vertex shadow maps | ShadowMap1D | 15 (25) | Not done (unaudited which meshes) |
| Dominant lights | DominantDirectional / DominantSpot / DominantPoint LightComponent | 30 / 169 / 23 | Done (`lighting.rs`); point lights unshadowed by Bevy (cube cost) |
| Static point / spot lights | PointLightComponent / SpotLightComponent (StaticLightCollectionActor 189) | 4300 / 4698 | Baked in lightmaps; composited into light environments |
| Toggleable / movable lights | PointLightToggleable 61, DisPointLightToggleable 66, SpotLightToggleable 14, DisSpotLightToggleable 1, Point/SpotLightMovable 1 each | | Spawned as Bevy lights (`lighting.rs`); toggle via Kismet not audited |
| Sky lights | SkyLightComponent | 43 (53) | Used only through light environments' upper/lower sky |
| Light functions | LightFunction | 124 (126) | Not done; need the owning lights and their function materials |
| Light environments | DynamicLightEnvironmentComponent 6789, LightEnvironmentComponent 88, ParticleLightEnvironmentComponent (class) | | Done for doors/usable props (`prop_light.rs`, `dishonored_assets::light_env`); characters and other dynamic props to audit |
| Rat lights (gameplay) | DisRatLightComponent 67, DisTallboyLightComponent (2) | | Not audited |
| Wall of Light | DisWallOfLight 27 | | Not audited (gameplay/emissive) |
| Decals (actors) | DecalActor / DecalComponent | 0 in levels; DecalComponent 7, DishonoredDecalComponent 3 in non-level packages | No placed decal actors in any map. Level decals are translucent static meshes (`decals.rs`, generic_translucent_PMAT) |
| Impact / particle decals | DisContactSub_Decal 14, ParticleModuleTypeDataDecal 337 (357), DisDecalManager | | Not done (gameplay-spawned: blood, impacts) |
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
- Status: not implemented. The distribution lookup tables (Scaling 4, Color 8, DistMap_Scale 8,
  DistMap_Alpha 4, ScreenPercentageMap 4 entries) and the material's texture/switch wiring are summarised
  by obj_dump only as "[N elems]", so the values are not yet available.

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
- Status: not implemented.

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
