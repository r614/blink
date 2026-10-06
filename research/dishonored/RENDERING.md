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
  uniform expression arrays (pixel vectors, pixel scalars, 2D textures). Match a MIC to its map
  with its static switch values (`mic_static`).

The cooked materials keep only their parameter and switch expressions; the graphs are stripped,
so every material term below was read from the compiled shader of the matching permutation.

## Static geometry (`map_render.rs`, `lightmap_ext.wgsl`)

- Directional lightmaps (HL2 basis, diffuse and lightmap specular): see LOG.
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
= B; base pass 9353, light passes 9362 point / 9366 directional):

- Only `FNoLightMapPolicy`, `FDirectionalLightLightMapPolicy` and `FSHLightLightMapPolicy` base
  passes and `FNoStaticShadowingPolicy` light passes exist for `FLocalVertexFactory`: translucent
  static meshes have no lightmap and are not shadowed.
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
environment and ambient light removed, plus the base pass ambient (`decals::BASE_PASS_AMBIENT`,
not measured: U-562), no lightmap, not a shadow receiver. Before, Bevy's environment specular
(bright at the ground's grazing angles) and the placeholder ambient made the puddles pale patches.
Static lights light non-lightmapped translucency dynamically in UE3; the port has only the
dominant and toggleable lights (U-561).

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
Moon and sun materials keep the old approximation.

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
