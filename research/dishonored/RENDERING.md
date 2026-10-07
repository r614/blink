# Level rendering from the original shaders

How the port reproduces the original's level shading. The maths comes from the compiled D3D9
shaders (`RefShaderCache-PC-D3D-SM3.upk`: material shaders; `GlobalShaderCache-PC-D3D-SM3.bin`:
global passes) and their setters in `Dishonored.exe`. Dated findings are in `research/LOG.md`
("Level rendering from the original shaders", "Distance-field shadows", "Bloom parts, DOF
downsample, sky dome, local distance-field shadows").

## Tools

- `tools/package_dump/src/bin/shader_dump.rs <cache> <out> [filter]`: carves the shader blobs
  and lists their constant tables. The global cache names each shader by the type string that
  precedes it (e.g. 52 / 53 `FKuwaPixelShader3` / `FKuwaVertexShader3`, 56 - 63 ArkPp DOF, 64 - 71
  ArkPp radial / motion / box blur, 82 - 87 bloom, 162 - 191 DisFog); `fxc /dumpbin` disassembles.
- `tools/package_dump/src/bin/shader_maps.rs <RefShaderCache> <out> [material] [switch=0|1 ...]`:
  lists the material shader maps: friendly name (the base material, e.g. `generic_PMAT`,
  `Skies_PMAT`), static switches and component masks, the shaders used (type, vertex factory,
  shader number in `shader_dump`'s order) and the uniform expression set behind
  `UniformPixelVector_n` / `UniformPixelScalars_n` / `Texture2D_n`. Layout found by inspection: a
  shader record is `type FName, GUID, SHA (20 B), skip offset, ..., code`; a shader map is
  `0x321, ?, skip offset`, entries `(type FName, GUID, type FName)` (mesh shader maps introduced by
  their vertex factory name), the friendly name `FString`, the `FStaticParameterSet`, then the
  uniform expression arrays (pixel vectors, pixel scalars, 2D textures, then cube textures, vertex
  vectors and vertex scalars: `UniformVertexScalars_n`, the world position offset inputs). Match
  a MIC to its map with its static switch values (`mic_static`). A mesh shader map writes its
  vertex factory's name *after* its shaders (`FMeshMaterialShaderMap::Serialize`), so each
  entry is labelled by the next vertex factory name: the first list is `FLocalVertexFactory`'s
  (static meshes: every light-map policy), the second `FGPUSkinVertexFactory`'s (no texture
  light maps). Until 2026-10-07 the tool labelled each list with the previous name, so the
  skinned list was read as the static one (U-561).
- `level_volume <package>` finds a level's precomputed light volume (see "Light environments").
- `map_dump --props` prints the doors' and usable objects' light environments
  (`DISHONORED_LIGHT_ENV_DEBUG=<actor>` lists the composited lights); `--unlit` lists the
  visible instances without a lightmap.

The cooked materials keep only their parameter and switch expressions; the graphs are stripped,
so every material term below was read from the compiled shader of the matching permutation.

## Static geometry (`map_render.rs`, `lightmap_ext.wgsl`)

- Directional lightmaps (HL2 basis, diffuse and lightmap specular): see LOG.
- The merged batches carry their lightmap in the material (`BakedLightmap`), not as Bevy
  `Lightmap` components, so Bevy's pipeline does not define `LIGHTMAP` for them:
  `apply_pbr_lighting` neither adds `pbr_input.lightmap_light` nor drops the ambient and the
  environment cube's diffuse. Until 2026-10-07 the static level was therefore lit by Bevy's
  placeholder ambient, the environment diffuse and the dynamic lights (only the lightmap specular,
  added outside, came from the lightmaps): the ground was about 4x too dark under the lamps
  (U-584). `lightmap_ext.wgsl` now zeroes `diffuse_occlusion` (no ambient or environment
  diffuse, as the base pass has none: `AmbientColorAndSkyFactor` = 0) and adds
  `lightmap_light x diffuse_color x exposure` itself.
- Distance-field shadows: each lightmapped component has a `ShadowMap2D` (PF_G8 signed distance
  field on UV1) per unbaked light. Those of the dominant directional light, and of the dominant
  point / spot light of each component (`LightGuid` matched with the level's
  `DominantPoint/SpotLightComponent`s), are resampled into two planes of the lightmap pages
  (`LightmapAtlas::shadow`, `shadow_local`). The light pass shades with
  `saturate((d + x) y)^z` (`DistanceFieldParameters`; measured in game: moon (-0.3667, 3.75, 2), bluish spot lights scale 3.125, warm ones 100, always x = -0.5 + 0.5 / y; U-401, U-461). The port swaps
  Bevy's shadow factor of that light for the field: for the directional light by index 0, for the
  local light by finding it in the fragment's cluster by position (batches are split by light,
  `BatchKey::df_light`). Boyle Ext has one dominant point and two dominant spot lights, Boyle Int
  one dominant spot light. `--no-df-shadows` restores Bevy's shadows;
  `DISHONORED_LIGHTMAP_DEBUG=shadow` / `shadow_local` show (s, d, Bevy factor).
- Lightmap specular is on at full strength: the base pass's `SpecularOverrideParameter` and
  `DiffuseOverrideParameter` are (0, 0, 0, 1) in the running game (U-320). The term is in the
  lightmap's Bevy light unit, so it takes the view exposure like the lightmap's diffuse term
  (without it, it was ~1000 times too bright, which is why it had been turned off).
  `--no-lightmap-specular` turns it off.
- Lightmap textures are sampled as sRGB: `LightMapTexture2D` has no script defaults and its
  constructor (0x00657DC0, vtable 0x010305A0) sets no `SRGB`, so the `Texture` default
  `SRGB = true` applies (U-322). The live `LightMapScale[0..1]` constants equal the cooked
  scales (w = 1).
- The bake (decoded lightmap pairs, distance-field resampling, page packing) is cached under
  `DISHONORED_CACHE` as `<map>.lightmaps1.bin` (`dishonored_assets::lightmap_cache`; a
  signature of the bake's inputs, bump `FORMAT` when the bake changes;
  `DISHONORED_NO_LIGHTMAP_CACHE` bakes every time). Boyle Ext: 2.1 s baked, 0.2 s cached
  (168 MB file); the other import stages log their times too (`import <map>: packages, actors,
  lightmaps`).

## Diffuse variation (`D2Variation`, `lightmap_ext.wgsl`)

generic_PMAT with `D2_Enable_Diffuse_Variation` (base pass 15243 with the mask on UV0, 28053
with `D2_Use_UV_coordinate_Index_1`):
`diffuse = lerp(D_Diffuse_color x D, D2(UV0 x D2_Diffuse_variation_Tilling),
D2_Diffuse_variation_mask(uv).ch x D2_Variation_Mask_Power)` (a multiply, not a power; not
saturated; the channel is the static mask `D2_Diffuse_Variation_mask_channel`). With
`D2_Use_UV_coordinate_Index_1` the mask is read through texture coordinate 1, the meshes'
lightmap channel (`LightMapCoordinateIndex` 1, e.g. Boyle's `ext_boyle_ground_garden_03` with
the per-mesh mask `ext_boyle_ground_garden_m`): the port batches such components alone and maps
the atlas UV back with their lightmap rect. The garden gravel was near-black without it (the
dirt variation is not tinted by `D_Diffuse_color`). The same mask value `m` drives the normal
and specular variations (base pass 28116 of `gravel_02_garden_inst`):
`n = normalize(lerp(N(UV0), N2(UV0 x N2_NormalMap_variation_Tilling), m))` with
`N2_Enable_NormalMap_Variation` (the port lerps the raw texels and maps them through Bevy's
tangent frame, then derives the lightmap's tangent-space normal from it as before), and the
lightmap specular colour `lerp(S, S2(UV0 x S2_Specular_variation_Tilling)[S2 mask], m)` with
`S2_Enable_Specular_Variation`. Not rendered: the `Multiply` / `variation_color` /
`UV_coordinate_Index_2` permutations (U-583).

## Point and spot light falloff (`lighting.rs`)

The light pass (`TLightPixelShader<FPointLightPolicy / FSpotLightPolicy>`, e.g. 34985 / 34987
with vertex shader 16): the vertex shader passes `(LightPosition - P) x InvRadius`, the pixel
shader computes `x = max(1 - |v|^2, 0)`, `x^LightColorAndFalloffExponent.w` (0 below 1e-6), times
N.L, `LightColor` (= colour x brightness) and, for spots,
`saturate((cos - cos outer) / (cos inner - cos outer))^2` (the same form as Bevy's cone). There is
no inverse square and no 1/pi. The port patches `bevy_pbr`'s `pbr_lighting.wgsl` (embedded
source rewritten and reloaded before the first frame, `lighting::Ue3FalloffPlugin`) so a point or
spot light with a non-zero `radius` uses that falloff, the exponent travelling in the radius as
`(E + 1) / 1000`; `intensity = 4 pi^2 x Brightness x U` cancels Bevy's `1/4pi` and Lambert `1/pi`.
Boyle's lights use exponents 2 to 6. The old R/4 match was ~3x too dim at half the radius.
`LightColor` (an `FColor`) is linearised as `(c / 255)^2.2` (UE3's `FLinearColor(FColor)`):
measured, the moon's (164, 209, 255) x 0.25 reaches the light pass as (0.0947, 0.1614, 0.25);
the sRGB curve gave 0.371 instead of 0.379 for the red. Fog colours use the same conversion.
The shading of the light pass (generic_PMAT, e.g. 15166) is
`(saturate(N.L) x diffuse + S x pow(saturate(R.L), SP) x (SP + 8) / (8 pi)) x LightColor x falloff`
(each lobe 0 below 1e-6; R = reflect(-V, N); no N.L on the specular; no Burley term): the same
rewrite replaces Bevy's BRDF for these lights and every directional light (`ue3_light`, U-581).
The material terms travel in Bevy's: S = 2 x reflectance (`build_material` sets reflectance 0.5
and the specular texture x colour as the tint; 0 without `S_Enable_Specular`), SP from the
perceptual roughness `(2 / (SP + 2))^(1/4)`.

## Camera

- The FOV (`DefaultCamera.ini` `m_fDefaultFOV` 75, the options' `PSI_GraphicsPC_FOV`) is
  horizontal, used as is at 16:9 (UE3's `FPerspectiveMatrix`; Dishonored's `LocalPlayer` has no
  `AspectRatioAxisConstraint`). The capture probe reads 75 from `CameraCache.POV.FOV`, and the
  port's renders at a horizontal 75 degrees match the reference captures' framing at every
  viewpoint. The game camera already applied it (`viewmodel::apply_fov`); the `--inspect` free
  camera (fidelity captures) kept Bevy's 75 degrees *vertical* (107 horizontal), so every pair
  looked wider. It now runs `apply_fov` too.
- Placement: both the probe (`measure views`, which moves the pawn until
  `CameraCache.POV.Location` is on the viewpoint) and the port's `camera` steps put the eye on
  the viewpoint; the pawn location is 64 uu lower for the probe's first guess only.

## Moon and sun (`sky_disc.rs`, `sky_disc.wgsl`)

- `Vfx_Materials.main_materials.Moon_PMAT` (translucent, lit model but no diffuse input; its
  base pass 34984): emissive `Tex#0.r x Moon_Color.rgb x a`, opacity
  `saturate(Cloud((u x 0.15 + Cloud_Speed x Time, v)).r) x Tex#0.g`. `Tex#0` is not a
  parameter, so it survives cooking only in the material's `FMaterial::UniformExpressionTextures`
  (native data after the tags: compile errors, texture dependency map, max dependency length,
  GUID, user texcoord count, then the texture array; `material_chain::uniform_expression_textures`):
  `Vfx_Materials.Textures.Moon`, red everywhere and the disc and halo in green, so the disc covers
  about 0.4 of the mesh's UV square. Boyle's `Moon_Boyle_INST`: `Cloud` = `MoonClouds`,
  `Cloud_Speed` -0.015, `Moon_Color` (0.95, 0.95, 1) x 2. The port drew the `Cloud` texture
  additively over the whole `Stm_SunDome` mesh, a disc about 2.4 times too wide.
- `Vfx_Materials.Material.Sun_PMAT` (additive, unlit; 1668): `m = max(1 - 2 |uv - 0.5|, 0)`,
  `m^SphereMask_Power x SunIntensity x Color.rgb` (the colour's alpha is unused; defaults
  2, 2, (1, 0.94, 0.74)). No Boyle mesh uses it.
- Not rendered: the bright, cloud-lit glow around the original's moon (U-580).

## Doors and usable objects (`usable_props.rs`, `level_props.rs`)

`DisDoor` and `DishonoredUsableObject` (subclasses `DisWaterSource`, `DisAlarmBell`,
`DisAudioLogPlayer`, `DishonoredDynamicUsableObject`) are `DisSkeletalBreakable`s: their visible
part is `SkeletalMeshComponent0`; the `StaticUsablePart` static mesh component is empty on Boyle.
The level importer read only static mesh components, so none of them was drawn: the `street`
viewpoint looked through the empty frame of the Garden-to-Ballroom transition door
(`L_Boyle_Ext_P:DishonoredUsableObject_7`, `double_doors.DBdoor_big_rich_ext_metal_sk`). Boyle Ext
has 8 `DisDoor`s (all in `L_Boyle_Ext_Audio`), 4 transition doors, and lockers, bins, hatches,
sinks, a chest, alarm panels and bells: 39 props on 19 skeletal meshes.

- Import: the actor x component transform, the mesh and the component's `AnimSets` (resolved to
  the package exporting them), `bHidden` / `HiddenGame`; for doors `m_InitialDoorState` and the
  `m_TransitionAnimName` of the tweak's four stages (`m_pDoorTweaks`, `DisTweaks_Door`
  `m_DoorOpenCW` .. `m_DoorCloseCCW`, resolved through the tweak fallback chain).
- Render: skinned like the cutscene props (`characters::upload`), at the placed transform times
  the Kismet's delta for the actor (`MissionView::movers`).
- Doors follow their `EDisDoorState` (0 closed, 1 opened CW, 2 opened CCW): the level's, or the
  Kismet's (`DisSeqAct_Door`, `MissionView::doors`). A change plays the stage animation of the new
  state (open CW / CCW) or of the direction it was open in (close CW / CCW); an open door rests on
  its open animation's last frame, a closed door in the reference pose. Which stage belongs to
  which state is read from the stage names (U-560); the double doors' tweak names the opposite
  direction's animation (`m_DoorOpenCW` plays `Player_Open_Ccw`).
- `--inspect` has no mission: every prop shows where and as the level placed it.
- Lit by their light environment (next section), not by Bevy's ambient and environment light
  (whose Fresnel reflection of the cube made the bluish band on the street door's seam).

## Light environments (`dishonored_assets::light_env`, `prop_light.rs`)

The doors and usable objects carry a `DynamicLightEnvironmentComponent` (class defaults:
`bSynthesizeDirectionalLight`, no `bSynthesizeSHLight`, `bUseBooleanEnvironmentShadowing`,
`NumVolumeVisibilitySamples` 1, `BouncedLightingFactor` 1, `AmbientGlow` 0; the doors set
`bDynamic` false). Decoded from `FDynamicLightEnvironmentState` (state 0x640 bytes, constructor
0x00504FD0; full update 0x005034D0: owner 0x004FEB30, dominant light 0x00500510, static environment
0x00501B70, dynamic environment 0x00503140, light list 0x004F85C0):

- SH: 9 coefficients per channel (padded to 12). `SHBasisFunction` (0x00450860) is the real SH
  with the Condon-Shortley phase, normalisation constants at 0x0126B100 (0.282095, 0.488603 x 3,
  0.182091, 0.364183, 0.630783, 0.364183, 0.182091). Static initialisers (0x00F67460 ..):
  `AmbientFunction` = (1 / (2 sqrt(pi)), 0, ...), `UpperSkyFunction` = (1 / sqrt(pi), 0,
  sqrt(3 / pi) / 2), `LowerSkyFunction` the same with -sqrt(3 / pi) / 2, and 2 sqrt(pi) (the
  doubles are widened floats).
- Static environment: for each enabled static light (the world's static light list; dominant
  lights only with `bForceCompositeAllLights`) whose channels overlap the owner component's,
  with `CompositeDynamic` read as `Dynamic` and `Dynamic` cleared (0x004DB950), that reaches the
  bounds (`AffectsBounds`: point sphere test 0x00530330, spot cone test 0x005499E0) and is visible
  (0x004FF270: with `CastShadows` and `CastStaticShadows`, one line check from the bounds sphere
  point facing the light to the light; visibility 0 skips it): `+= Y(L) x GetDirectIntensity`
  (0x004FF710). `GetDirectIntensity` = `Brightness x FLinearColor(LightColor)` (0x0050D700) x
  `max(1 - d^2/R^2, 0)^FalloffExponent` for points (0x005306F0) x the spot cone
  `saturate((cos - cos outer) / (cos inner - cos outer))^2` (0x00549EE0; inner clamped to
  [0, 89] deg, outer to [inner + 0.001 rad, 1.554343 rad]); directional lights from `-axis`, sky
  lights from straight up (`GetPosition` (0, 0, 1, 0), 0x00547370). The bounce
  (`LightEnv_BouncedLightBrightness`, 0 by default) adds `Y(-L)` into the second environment.
  Static lights keep CompositeDynamic from `Default__LightComponent` (the `PointLight` /
  `SpotLight` templates only clear Dynamic): they light props although they are not in the
  props' `Dynamic` channel.
- Precomputed light volume: a trace down from the bounds origin finds the floor's level; its
  `PrecomputedLightVolume` (ULevel +0x2B0; serialized near the end of the `Level` export:
  `bInitialized`, bounds `FBox`, a dword, then 33-byte samples, then 28 bytes) is interpolated
  at the origin (0x0053DFA0): samples with d < radius, weight `(1 - d / radius) / radius`, each
  sample's SH (0x00529710) = `Y(indirect dir) x IndirectRadiance + Y(environment dir) x
  EnvironmentRadiance + AmbientFunction x AmbientRadiance` (directions as (theta, phi) bytes,
  theta = b / 255 pi, phi = b / 255 2 pi - pi, tables 0x01278980 / 0x01279180; radiances RGBE
  `FColor`s, 0x00403270 = `ldexp(1/255, e - 128)`; with `bIsCharacterLightEnvironment` the
  WorldInfo's character brightness / contrast scales apply, 1 otherwise); the sum normalised by
  the total weight is added. Boyle: `L_Boyle_Ext_P` 332 grid samples (radius 3464),
  `L_Boyle_Ext_Env` 16425 surface samples (radius 283), `L_BoyleStreet_Ext_Script` 172.
- Light list (0x004F85C0): a directional light along `(-L3, -L1, L2)` of the luminance SH
  (0.3, 0.59, 0.11), coloured by the projection `max(T.Y / Y.Y, 0)` (0x004E1300) and subtracted
  (0x004E1480); then the upper and the lower sky light, the projections of the remainder on the
  sky functions, each subtracted in turn (0x004E16E0): `SkyLightComponent` Brightness /
  LightColor and LowerBrightness / LowerColor (+0x100 / +0x104, +0x1A8 / +0x1AC).
- Drawn (skinned base pass `FDirectionalLightLightMapPolicy` with the sky light, generic_PMAT
  15153): `(saturate(N.L) x diffuse + S x pow(saturate(R.L), SP) x (SP + 8) / (8 pi)) x
  LightColor + diffuse x (UpperSky x ((1 + N.Z) / 2)^2 + LowerSky x ((1 - N.Z) / 2)^2)`. The
  dominant and toggleable lights add their light passes (Bevy lights).

Example: the street door (`DishonoredUsableObject_7`) gets one lamp (`PointLight_183_LC`) and the
Env level's volume: directional (0.36, 0.33, 0.26) from (-0.96, 0.06, -0.28), upper sky
(0.126, 0.113, 0.077). Approximations: U-620.

Level actors the Kismet moves (matinees, `SeqAct_AttachToActor`, teleports) are replicated as
`MissionView::movers` (actor, placed location, rigid delta = current x placed^-1, net protocol 6);
`InterpActor` meshes outside a playing matinee follow them (`level_props::follow_movers`), so a
mover stays where a matinee left it.

## Translucent decals (`decals.rs`, `ark_translucent.wgsl`)

Boyle's ground decals are static meshes a few units over the ground with translucent materials of
`Materials_ref.main_materials.generic_translucent_PMAT` (no `DecalActor` on Boyle): the puddles
`ext_boyle_ground_01.ext_boyle_puddles_decals_01` (`towerEmpress_ground.puddles_decals_01_inst`),
street splotches, graffiti, confetti. Shader maps of the base (`shader_maps ... generic_translucent`,
8 permutations; the puddles' is the one with `O_Opacity_Mask_Channel` = G, `C_Cubemap_Mask_Channel`
= B; base pass with the directional lightmap 9347, without lightmap 9353, light passes 9362 point
/ 9366 directional):

- `FLocalVertexFactory` has every light-map policy (the earlier reading took the skinned vertex
  factory's list for it, see "Tools"): a lightmapped decal (the Boyle ones all are) draws its
  base pass with its own directional lightmap, like the opaque base pass's diffuse:
  `diffuse x LightMap[0] x sum_i LightMap[1]_i x max(saturate(n.B_i)^2, 1e-6)` with n the normal
  map's tangent-space normal; the lightmap specular is multiplied by `SpecularOverrideParameter.xyz`
  (0). The dominant / toggleable lights add light passes (the distance-field shadow variants
  exist too).
- diffuse = `D` [x `D_Diffuse_color.rgb x a` with `D_Enable_Diffuse_color`] + SceneReflection(R) x
  `C_Cubemap_Mask[ch]` [x `C_Cubemap_color.rgb x a` with `C_Enable_Cubemap_Color`]; R reflects the
  view about the normal-mapped normal; the reflection is part of the diffuse, so it is lit.
- `F_Enable_Fresnel` (all Boyle maps have `F_Invert_Fresnel_result`): f = (1 - saturate(N.V))^
  `F_Fresnel_falloff_exponent` (0 when 1 - N.V <= 1e-6); diffuse = lerp(diffuse, `F_Fresnel_Color.rgb
  x a`, f); alpha = saturate(alpha + f).
- base pass: diffuse x `AmbientColorAndSkyFactor` (+ `E` x `E_Emissive_Color.rgb x a`); each light
  pass: diffuse x saturate(N.L) x `LightColorAndFalloffExponent` (the specular term is multiplied by
  `SpecularOverrideParameter.xyz`, 0 in game).
- alpha = clamp(`O_Opacity_mask[ch]`, `O_Minimum_Opacity`, `O_Maximum_Opacity`).

The port draws them with an extended `StandardMaterial`: Bevy's direct lighting with reflectance,
environment and ambient light removed, plus the component's directional lightmap (the atlas
planes and its rect, one material per lightmap region; tangent frame from the UV derivatives as
in `lightmap_ext.wgsl`) and the base pass ambient (`decals::BASE_PASS_AMBIENT`:
`AmbientColorAndSkyFactor` reads (0, 0, 0, 1) in every base pass drawn on Boyle Ext, measured with
`probe_launch measure psconst`, U-562); not a Bevy shadow receiver: the dominant directional light
takes the component's distance field. Before, Bevy's environment specular (bright at the ground's
grazing angles) and the placeholder ambient made the puddles pale patches, and then, without the
lightmap, nearly black ones over every path.
## Diffuse colour (`map_render::diffuse_color_terms`)

Every base material that exposes `D_Diffuse_color` multiplies its colour by its alpha in the
base-pass pixel shader (`mul r.xyz, cN, cN.w`): `generic_PMAT`, `generic_masked_PMAT`,
`generic_masked_SingleSided_PMAT`, `generic_translucent_PMAT`, `generic_translucent_2sided_PMAT`,
`vertexAnim_PMAT`, `vertexAnim_2Sided_PMAT`, `spline_PMAT`, `bugs_PMAT`, `water_slide_PMAT`,
`DefaultMesh_PMAT`, `DefaultMesh_2_Sided_PMAT`, `Default_Simple_Translucent_PMAT`, `Skies_PMAT`
(one directional-lightmap or unlit base pass of each, e.g. shaders 11429, 38250, 3315, 1734).
Only `FogSheet_PMAT` reads the whole vector (`mov r0, cN`). So diffuse = texture x rgb x a: the
Boyle shrubs use a = 1.5, grass and flowers 2. The port multiplies for those bases; fog sheets and
undecoded unlit translucent bases keep the alpha as opacity.

## Foliage (`vertexAnim_PMAT`)

Boyle Ext's hedges, shrubs and flowers are lightmapped `StaticMeshCollectionActor` components (no
foliage actors, see PACKAGE_FORMAT.md), drawn by the ordinary static path: masked (clip
`O_Opacity_mask.r - 0.3333`, `texkill`), single-sided except `*_2_sided_inst`
(`generic_masked_PMAT`), no collision (`CollideActors` false). The directional-lightmap pixel
shader of `vertexAnim_PMAT` without a normal map (1743) uses the flat-normal diffuse (`dir . 1/3`)
and a specular lobe on the reflected view vector with power 16 (`SpecularOverrideParameter`).

Wind (the permutation without `Va_Enable_Complex_vertex_anim` / `Va_Enable_Rotation_About_Axis`,
vertex shaders 7327 / 7358): with z the vertex's world height, every world axis gets

    offset = Va_Radius_spreading x sin(2 pi x frac(z x Va_Distorsion_strenght + Time x Va_Vertex_Anim_Speed + 0.5) - pi)

(`UniformVertexScalars_0` = (`Va_Distorsion_strenght`, `Va_Vertex_Anim_Speed`, Time, Time x speed),
`_1.x` = `Va_Radius_spreading`). On Boyle the amplitude is at most 1 uu (shrubs: strength 1,
speed 0.2, radius 1; flowers: 0.002, 0.2, 0.5). The port does not draw it (U-570).

## Skeletal scenery

Placed `SkeletalMeshComponent`s of doors, usable objects (lockers, bins, hatches, chests, Wall of
Light panels), sinks, alarm bells, the climbable chain and the Kismet-free `SkeletalMeshActor`s
(sewer gate, bridge) are imported as render-only static instances in their reference pose (42 on
Boyle Ext). They have no lightmaps (the original lights them through a
`DynamicLightEnvironmentComponent`), so the port lights them with its dynamic lights only (U-571).
Since the usable props moved to `usable_props` (animated, lit by their light environment, see
"Light environments"), only the climbable chain and the unscripted `SkeletalMeshActor`s remain
here.

`O_Enable_full_opacity` (the Boyle balloons, `balloons_01_vertexAnim_inst`): the masked base
compiles without its opacity mask (no `texkill` in 1815 / 1831), so such materials draw opaque;
the port had clipped the balloons' panels into see-through holes.

## Sky dome (`sky.rs`, `sky.wgsl`)

`Skies.Skies_PMAT` (unlit, base pass `TBasePassPixelShaderFNoLightMapPolicyNoSkyLightFALSEFALSE`):

- clouds: `C_Clouds_Texture` / `C2_Clouds_2_Texture` at UV0 turned about (0.5, 0.5) by
  `Speed x Time x 0.1` rad;
- base: `G_Use_Gradiant_Sky` ? `Top + (Bottom - Top) x |UV0.y + clouds.r x Cloud_Distortion|^
  G_Gradiant_Power` : `D_Diffuse((U_Offset - UV1.x, UV1.y) + clouds.r x Cloud_Distortion)`; times
  `D_Diffuse_color.rgb x a`;
- each cloud layer `c + clouds x Visibility x (Color.rgb x a x clouds - c)`;
- `max(|c|, 1e-6)^sky_contrast`;
- `Enable_Storm`: `+ Storm(Storm_Offset + UV1.x, UV1.y) x Storm_Color.rgb x (Storm_Intensity +
  Storm_Min_Intensity)`.

The vertex shader passes UV1 swizzled (`o1.zw = uv1.yx`). Decoded from the shader maps of Boyle
Ext's `Boyle_2_inst` and Boyle Int's `Boyle_01_inst`; with `H_Enable_Horizon` the contrast result
is saturated and a horizon colour is blended by an unnamed noise texture (`Tex#1`), not rendered
(U-520, with `TP_Enable_Top_Texture`, the storm / gradient masks and `G_Grandiant_UV2`).
Moon and sun: see "Moon and sun" above.

## Bloom parts (`bloom.rs`, `bloom.wgsl`)

There is no scene-threshold bloom. 0x009251A0 (events "BloomParts", "Draw blooming primitives",
"Bloom reduction and blur", "Bloom compose"):

1. primitives with bloom relevance are drawn with `TBloomPartMeshPixelShader<FBloomPartMeshPolicy>`
   into a scene-size LogLuv RGBA8 target; for the generic bases (`generic_PMAT`,
   `generic_masked_SingleSided_PMAT`) with `B_Enable_Bloom` the colour is `E_Emissive (masked
   channel) [x E_Emissive_Color.rgb x a with E_Enable_Emissive_color] x B_Bloom_Intensity` (0
   without `E_Enable_Emissive`); other bases with bloom shaders (`Flickering_light_PMAT`,
   `Oil_PMAT`, `Glass_Parrallax_PMAT`, `Blink*`, `Sun_Glare_PMAT`, `Skies_PMAT` with bloom) are
   not decoded (U-521);
2. `FBloomDownSamplePixelShader` (84) at quarter size: 4 taps at +-1/4 quarter-size pixel,
   decoded, averaged, `x gBloomTint.rgb + gBloomTint.w`, at least 0, with `gBloomTint =
   (m_Tint.rgb x m_Scale, -m_Threshold x m_Scale)` from `ArkPpBloomParameters` (0x0090DE60);
3. "ArkGaussian" (0x0091FE50): `TBloomBlurPixelShader` (82 / 83) along X then Y: taps 0,
   +-1.3973, +-3.3973 texels, weights 0.2416, 0.2643, 0.1149;
4. added after the fog in the DisFog pass (`sBloomParts`), or by `FBloomComposePixelShader`
   (scene + bloom) when there is no fog pass.

The port draws the parts as world-space triangles collected at level spawn (one batch per
material), depth-tested against the scene depth, into an RGBA16F target. `--no-bloom` disables.

## DisFog and ArkPp (`ark_post.rs`, `ark_post.wgsl`, `ark_downsample.wgsl`)

Three passes in the original's order: DisFog (+ bloom parts); the DOF's quarter-size copy (two
`FArkPpDofDownsamplePS` passes, each one bilinear tap at the shared corner of a 2x2 block,
0x00922210; `gResolutionParams` = (1/w, 1/h, 36, 42), 0x0090EAB0); then `FArkPpDofUber_*PS`:
the far blur blends to that copy sampled bilinearly at the pixel's uv
(`saturate((depth - Focus) / InFocusRadius) x FarBlurAmount`), the colour LUT maths and grain.
`FArkPpBlurBoxBlurPixelShader` (68) is a stub that outputs magenta; the Kuwahara shaders (52 -
55: the mean of the least-variance quadrant of a 3x3 / 5x5 window) belong to the
`UArkPpNodeKuwa` post node and are not used by the DOF path (whether a level's post chain uses
the node is U-522).

Checked against the running game (2026-10-06): `LocalPlayer.m_CurrentArkPpSettings` (+0x124,
132 bytes; `m_LevelArkPpSettings` at +0xA0 holds the struct defaults) carries Boyle Ext's
cooked values unchanged (focus 5500, far blur 0.2, CrMgYb shadows (0, 0.03, 0.04) / midtones
(0, 0.3, 0.4), exposure 0.3, gamma 0.95, grain 3, contrast 0.05, bloom tint (1, 0.953, 0.723)
x 0.4), so the port builds its uniform from the right data. The LUT bake (57) and uber (59 / 61)
disassemble as the port computes them (LUT coordinate `sqrt(c)`, squared in the bake, exposure
after the clamp). These passes are drawn with `DrawPrimitiveUP`, which the probe now hooks
(slots 83 / 84), so their live constants can be read with `probe_launch measure consts
gExposure_GammaAdjust 5` (not run yet: the display gamma that reaches the LUT is U-582).
