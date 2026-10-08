# Rendering area 1: depth (audit, WIP)

Scope: a scene-depth texture available to materials and every feature that reads it: soft /
depth-biased particles and translucency, SceneDepth material terms, distortion, height fog and fog
volumes, ambient occlusion.

Status: audit only, partial. No code changed. The `git merge main` and
`git merge worktree-agent-a4c9db721993b0d16` steps from the brief were not done (the merge was
refused by the permission system), so this branch is still at `ad302e7`.

## Engine-side findings

- **Ambient occlusion: not used.** `Engine/Config/BaseEngine.ini` `[SystemSettings]`:
  `AmbientOcclusion=False` with the comment "DISHONORED: nsilvagni (20110104) - Ambient occlusion
  is unwanted". The exe still allocates the AO targets (`FUN_0084cd30`: "AmbientOcclusion",
  "AOHistory"), gated on a render-thread system-settings flag (`DAT_01443344`, copied by
  `FUN_005651c0`). The native class list (oracle index `tables/classes.tsv`, 2760 classes) has no
  `AmbientOcclusionEffect` and no `PostProcessEffect` subclasses; Arkane's `ArkPpNode*` chain
  replaces them. Nothing to implement.
- **HeightFog / ExponentialHeightFog / FogVolume*: not in the game.** None of these classes are in
  the exe's native class list. `DefaultEngine.ini` hides the `ActorFactoryFogVolume*` factories.
  The show flag `FogVolumes=True` remains in BaseEngine.ini, but nothing uses it. Fog is
  Arkane's `DisFog` / `DisFogComponent` (+ `DisFogDisplayComponent`), already implemented in
  `ark_post.rs` / `ark_post.wgsl` (full-screen pass that reads the main depth buffer). The open
  points are in U-321: which layers reach the pass (more than 4 layers, `bInteriorFog`,
  `m_IsExclusive`), flag bits, the sun direction source, and the negative opacity above a layer
  top.
- **Distortion: implemented.** `distortion.rs` covers the accumulate and apply passes and reads
  the main depth buffer (`configure_depth_usage` adds `TEXTURE_BINDING`). Settings in
  BaseEngine.ini: `Distortion=True`, `FilteredDistortion=True`, `DropParticleDistortion=False`.
  These are not yet exposed in `dishonored_settings`. Per-permutation terms are still open (U-400 /
  U-401).
- **Scene depth resolve:** `FUN_00849be0` emits the draw event "ResolveSceneDepthTexture". On
  D3D9 SM3, UE3 also keeps depth in the SceneColor alpha; the distortion accumulate shader reads
  `SceneColor.a x 256`.
- **Material expressions:** the cooked exe registers no editor expression classes such as
  `DepthBiasedAlpha`, `SceneDepth` or `DestDepth`. Their use must therefore be read from the
  compiled shaders: `RefShaderCache-PC-D3D-SM3.upk`, and the captured shaders under
  `cache/oracle/shaders/<hash>/ps_*.bin`, 7289 files. A scan of these for `SceneColorTexture`,
  `SceneDepthTexture`, `MinZ_MaxZRatio` and `ScreenPositionScaleBias` was started and did not
  finish.

## Material-side findings

- **Depth-biased alpha (soft particles):** the switch is `Db_Enable_Depth_Biased_Alpha` and the
  scalar is `Db_Depth_Biased_Alpha_Intensity`. They are in the `DefaultAdditive_PMAT` family's
  uniform expression set (LOG "Distortion pass": it appears in the shader map of shader 36094).
  EFFECTS.md line 97 has one instance value of 0.05. Not implemented (EFFECTS.md: "Not
  reproduced: depth-biased alpha"). Particles render as unlit `StandardMaterial` in
  `effects.rs` (~line 256), with no depth fade.
- **Water depth opacity / fake fog:** `ark_water.wgsl` already contains the scene-depth path
  under `#ifdef DEPTH_PREPASS` (thickness alpha, `F_Enable_Fake_Fog`). No camera has
  `DepthPrepass`, so the path never runs (U-764). `water.rs` also leaves
  `D_Enable_Opacity_over_Depth` undecoded (`UNDECODED_OFF`). Water belongs to area 2; the depth
  texture it needs belongs to this area.
- **Bloom parts** (`bloom.rs`) are depth-tested against the main depth buffer. This already works.

## Audit table

| Feature | Data source | Maps using it | Status |
|---|---|---|---|
| Ambient occlusion | BaseEngine.ini `AmbientOcclusion=False`; no AO effect class | none | not used by the original; nothing to do |
| HeightFog / ExponentialHeightFog | not in the exe's class list | none | not in the game |
| Fog volumes (FogVolume*DensityInfo) | not in the exe's class list; factories hidden in DefaultEngine.ini | none | not in the game |
| DisFog height fog | `DisFog` actors, `FDisFogPixelShader*` | Boyle Ext confirmed (3 layers); per-map count TODO | implemented (`ark_post`); U-321 open |
| Distortion | `Dist_Enable_Distortion` materials, global shader 237 | 12 emitters in the decoded set; per-map TODO | implemented (`distortion.rs`); no settings toggle yet |
| Depth-biased alpha (soft particles) | `Db_Enable_Depth_Biased_Alpha`, `Db_Depth_Biased_Alpha_Intensity` | TODO (count MICs with the switch on, per map) | missing |
| Water depth opacity / fake fog | `river_main_PMAT` `O_*`, `F_*`, `D_Enable_Opacity_over_Depth` | maps with river_main_PMAT water (Boyle, canals, ...); TODO count | shader written, inactive (no depth prepass) |
| Other SceneDepth / DestDepth / PixelDepth terms | compiled shaders only | TODO (shader scan) | unknown |
| Bloom-part depth test | main depth buffer | all maps with bloom parts | implemented |

## Next steps

1. Merge `main` and `worktree-agent-a4c9db721993b0d16`. The merge was refused here, so this needs
   the user or coordinator.
2. Find which compiled pixel shaders read scene depth. Scan `cache/oracle/shaders/<hash>/ps_*.bin`
   and the RefShaderCache with `shader_dump`, filtering on `SceneColorTexture` or
   `SceneDepthTexture` together with `MinZ_MaxZRatio`. Group the hits by base material (with
   `shader_maps`) to get every depth-reading material family. Expect `DefaultAdditive_PMAT` and the
   other particle bases (depth-biased alpha) and `river_main_PMAT`.
3. Count per map the MICs with `Db_Enable_Depth_Biased_Alpha` on, and the water instances. Use
   `mic_static` / `obj_dump` over all `L_*_P` packages, or the oracle index.
4. Decode the depth-biased alpha formula from one compiled permutation, for example the
   mesh-emitter permutation of 36094 (`fxc /dumpbin`): the scale between the
   `Db_Depth_Biased_Alpha_Intensity` uniform and the depth delta, and the use of
   `MinZ_MaxZRatio`.
5. Make depth available to translucent draws without a CPU cost. Option (a): add `DepthPrepass`
   to the game camera, which Bevy runs on the GPU for opaque geometry, then sample it with
   `prepass_utils::prepass_depth`; this makes `ark_water`'s `DEPTH_PREPASS` path active at once.
   Option (b): copy the main depth after the opaque pass. Compare the costs of (a) and (b) on
   CPU-bound maps.
6. Particles: replace the unlit `StandardMaterial` in `effects.rs` with a small particle material,
   or a `MaterialExtension`, that applies depth-biased alpha from the decoded intensity (soft
   fade), for materials with the switch on.
7. Settings: expose `Distortion` and `DropParticleDistortion` from BaseEngine.ini in
   `dishonored_settings`, honouring the original's video options.
8. Add the views that best show these features to `scripts/render_fixes.sh`: Boyle Ext river (water
   thickness), muzzle smoke / steam particles meeting geometry, Flooded district fog.
