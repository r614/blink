# Rendering area 4: lighting (WIP audit)

Scope: lightmaps and shadow maps, dynamic shadows, light functions, light environments, decals,
lens flares, emissive / glow. Status: audit started, no code changed yet.

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

## Next steps

1. Per-map coverage: for each class above, `package_scan <dir> --class <C>` and map packages to
   persistent maps with `map_dump <map> --survey` package lists.
2. Light functions: dump the 124 LightFunction objects (owning light, SourceMaterial, Scale);
   decode the function materials from RefShaderCache; implement as a light cookie on the Bevy
   spot/point lights (GPU-side).
3. Lens flares: dump the 23 LensFlareSource actors and their LensFlare templates (elements,
   materials, ray distance, curves); render as screen-space sprites with occlusion query.
4. Light environments for characters and dynamic props (6789 components): reuse
   `light_env` + `prop_light.wgsl` for NPC skinned meshes; check character brightness/contrast
   WorldInfo scales.
5. Gameplay decals: DisContactSub_Decal / DisDecalManager lifetimes and materials; projected
   decal pass.
6. ShadowMap1D (vertex shadow maps) on the 15 level components.
7. Dominant point light shadows: settings toggle (original's shadow quality option).
8. Add the maps that best show these to `scripts/render_fixes.sh`.
