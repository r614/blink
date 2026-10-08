# Rendering area 1: depth (audit, WIP)

Scope: a scene-depth texture available to materials and every feature that reads it: soft /
depth-biased particles and translucency, SceneDepth material terms, distortion, height fog and fog
volumes, ambient occlusion.

Status: audit plus the first implementation step. The branch now has main and
worktree-agent-a4c9db721993b0d16 merged (LOG.md / UNKNOWNS.md conflicts resolved by keeping both
sides).

## Implemented (second pass)

- **Depth prepass on the game and inspect cameras** (`lib.rs setup_world`: Bevy `DepthPrepass`;
  `DISHONORED_NODEPTH=1` turns it off to measure cost). This activates the already-written
  `#ifdef DEPTH_PREPASS` paths of `ark_water.wgsl` (thickness opacity, `F_Enable_Fake_Fog`),
  `volume_light.wgsl` (Db fade) and `water_foam.wgsl`, via `prepass_utils::prepass_depth` +
  `depth_ndc_to_view_z`. Option (a) of the plan was chosen because Bevy has no depth copy before
  the transparent pass and these shaders were written against the prepass. Cost: a GPU depth-only
  pass of the opaque geometry, plus the render-world queueing of the opaque meshes into the
  prepass phase (CPU, per visible mesh). Measure with and without `DISHONORED_NODEPTH`.
- **Distortion setting**: `Graphics.distortion` (BaseEngine.ini `Distortion=True`, default on) in
  `dishonored_settings`; `distortion.rs tag_game_camera` leaves the pass out when it is off. The
  original has no video-menu entry for it, so there is no UI toggle. `FilteredDistortion` and
  `DropParticleDistortion` (False) are not exposed: no consumer yet.
- `scripts/render_fixes.sh` step 6: L_Boyle_Ext_P and L_Flooded_FStreets_P.

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
  `cache/oracle/shaders/<hash>/ps_*.bin`, 7289 files. The scan was done in round 2 (below): the depth reads are `SceneColorTexture` (alpha x 256), there is no separate `SceneDepthTexture`.

## Material-side findings

- **Depth-biased alpha (soft particles):** the switch is `Db_Enable_Depth_Biased_Alpha` and the scalar is `Db_Depth_Biased_Alpha_Intensity` (default 0.05); decoded and ported in round 2 (below). Before that, particles rendered as unlit `StandardMaterial` with no depth fade.
- **Water depth opacity / fake fog:** `ark_water.wgsl` already contains the scene-depth path
  under `#ifdef DEPTH_PREPASS` (thickness alpha, `F_Enable_Fake_Fog`). No camera has
  `DepthPrepass`, so the path never runs (U-764). `water.rs` also leaves
  `D_Enable_Opacity_over_Depth` undecoded (`UNDECODED_OFF`). Water belongs to area 2; the depth
  texture it needs belongs to this area.
- **Bloom parts** (`bloom.rs`) are depth-tested against the main depth buffer. This already works.

## Round 2 (2026-10-08)

### Soft sprite and mesh particles: decoded and ported

The depth-fade constant of the particle bases is **250, not Volume_Light_PMAT's 50** (the open point of
round 1: the sprite permutations do not share Volume_Light's k). Read from the compiled base passes
(`shader_maps RefShaderCache-PC-D3D-SM3.upk <out>`, then `fxc /dumpbin <out>/<n>_ps.bin`), e.g.
`DefaultAdditive_PMAT` + `Db_Enable_Depth_Biased_Alpha` + `D_Disable_Diffuse` (36033 sprite / 36034
mesh vertex factory), `DefaultTranslu_PMAT` + `Db_Enable_Depth_Biased_Alpha` (35519), the UV-distortion
additive 35911, and the same tail in 35900 / 1187 / 36531 / 34861 / 2834 / 2648 / 1048 / 35754 / 6869 /
36325 / 1087:

```
rcp  r0.x, v2.w ; mul r0.xy, r0.x, v2 ; mad r0.xy, r0, c1, c1.wzzw   (screen uv from texcoord5.xyw)
texld_pp r0, r0, s0                                                  (SceneColorTexture)
mad  r0.x, r0.w, 256, -v2.w          depth delta  = SceneColor.a x 256 - view depth (uu)
add  r0.y, 1, -c7.x                  1 - Db_Depth_Biased_Alpha_Intensity
mul  r0.z, r0.y, 250 ; mad r0.y, r0.y, 250, -0.001 ; rcp r0.z, r0.z ; cmp r0.y, r0.y, r0.z, 1000
mul_sat r0.x, r0.y, r0.x             Db = saturate(delta x k),  k = 1 / ((1 - I) x 250), 1000 when (1 - I) x 250 < 0.001
... mul_pp oC0.w, Db, opacity (translucent) / rgb x Db x opacity, w = 0 (additive)
```

So Db multiplies the particle's opacity term (the output alpha of a translucent permutation, the colour
scale of an additive one) and nothing else. The intensity default is 0.05 (fade distance 237.5 uu); an
instance may override `Db_Depth_Biased_Alpha_Intensity`. `lightCone_twoSided_PMAT` has the same term under
`Da_Enable_Depth_Biased_Alpha` / `Da_Depth_Biased_Alpha_Intensity` (default 0.5) with 500 for 250 in its
constants (shader 2726; LIKELY: the cmp / rcp structure is the same but the permutation was read by eye).
`Fire_Explosion_*` (`Enable_DepthBiasedAlpha`, constant 1/128), `Look_Up_*` (`Depth_Bias` default 0)
and the `M_EV_*` environment masters use other forms and are not ported (see the scan below).

Port: `effects.rs` `FxMaterial = ExtendedMaterial<StandardMaterial, SoftFade>` (`particle_soft.wgsl`) replaces
the particle `StandardMaterial` everywhere in `effects.rs`: one material per emitter, as before, so the sprite
meshes still batch per emitter (sprites are one dynamic mesh per emitter; mesh particles keep one entity
and one material copy per live particle, as before). `SoftFade::soft.x` is k (`soft_scale`, 0 = off: a plain
sprite, so only the materials with the switch pay the depth read). The fragment multiplies the unlit
`StandardMaterial` colour's alpha by `saturate((scene depth - pixel depth) x k)` (Bevy premultiplies by alpha
after that, so additive scales its colour and translucent its alpha, as the shader does) with the scene
depth from the depth prepass (`DEPTH_PREPASS`; without it, e.g. the capture cameras, the scene is taken as far:
Db = 1). Static level meshes of the same bases (light cones, glows, translucent props) get the same material
through `effects::PendingSoft` (`map_render::spawn_map` -> `apply_pending_soft`, one `FxMaterial` per source
material). Volume_Light_PMAT keeps its own k = 50 path (`volume_light.rs`). Cost: the depth prepass was
already on; the fade is one prepass texture load per fragment of the materials with the switch.

### SceneDepth / DestDepth scan of the compiled material shaders

The cooked exe has no editor expression classes, so the reads are found in the shaders: all 39747 shaders
of `RefShaderCache-PC-D3D-SM3.upk` were carved (`shader_maps <cache> <out>`: `<n>_ps.bin`), none of the 36742 pixel shaders names `SceneDepthTexture` or `MinZ_MaxZRatio`;
the 1745 whose constant table names `SceneColorTexture` (the D3D9 depth is the scene colour's alpha x 256)
were disassembled: 1704 sample it and use `.w` with the 256 constant (a depth read), 18 use `.w` without it,
23 read the colour only (refraction). Grouped by base material through the shader maps, the base-pass
(`TBasePassPixelShader*`) readers are below. "perms" = permutations of the base that read it / all
permutations in the cache; the port column says what draws it now. Per-map usage counts were not measured
(there is no index of material parents per map; a count needs `mic_static` over the `L_*_P` MICs).

| Base material | perms | Switch | Port |
|---|---|---|---|
| DefaultAdditive_PMAT / _2Sided / _OutBendTime | 20/62, 6/13, 2/3 | `Db_Enable_Depth_Biased_Alpha` | done: `FxMaterial` (sprites, mesh emitters, level meshes) |
| DefaultTranslu_PMAT / Translu2sided / TransluFOG | 45/83, 2/15, 4/9 | `Db_Enable_Depth_Biased_Alpha` | done |
| Glow_PMAT, LensFlare_PMAT, WindSwirls_01_PMAT, TEST_SmokeTrail_PMAT, deafult_light_surface | 9/15, 1/6, 2/5, 1/2, 1/2 | `Db_Enable_Depth_Biased_Alpha` | done where drawn as particle / level meshes (Glow's procedural mesh emitters use `GlowMaterial`: no fade, `U-305`) |
| Volume_Light_Ring_PMAT, Volume_Light_translu_PMAT | 2/5, 2/6 | `Db_Enable_Depth_Biased_Alpha` | done (same material path; not Volume_Light_PMAT's k) |
| lightCone_twoSided_PMAT | 4/6 | `Da_Enable_Depth_Biased_Alpha` | done, LIKELY (500) |
| Volume_Light_PMAT | 6/7 | (none: the Db term is always in) | done (`volume_light.rs`, k = 50) |
| river_main_PMAT, Water_translucent_PMAT, river_test_mat, Mat_River | 38/41, 9/15, 3/3, 1/2 | `D_Enable_Opacity_over_Depth` | river_main done (`ark_water.wgsl`); the others are not drawn by the port |
| water_Foam_MAT | 5/5 | `Enable_Foam` | done (`water_foam.wgsl`) |
| Water_PMAT, Water_Translu__PMAT, Water_Additive__PMAT | 4/7, 3/7, 6/13 | `Use_DepthAlpha` | not ported (no decode of the opacity-over-depth term; where they are used is unmeasured) |
| Fire_Explosion_PMAT / _02, Look_Up_Fire / Smoke / _translu* | 2/5, 2/3, ... | `Enable_DepthBiasedAlpha`, `Depth_Bias` | not ported: other forms (EFFECTS.md already approximates their colour) |
| M_EV_Lightbeam / FogSheet / FogEnvironment / FalloffSphere, M_ES_*, Mat_Depth, water_vertex | 1 - 2 each | `UseDepthBiasedAlpha` ... | not ported: environment-art masters; unmeasured whether any retail map places them |
| Distortion accumulate passes (every `bUsesDistortion` base) | 133 | `Dist_Enable_Distortion` | done (`distortion.rs`, the scene depth compare) |
| DarkVision_*, Blink_Corvo_PMAT, PPG_Outsider, Mat_Rain* / Shr_* / Camera_Splash, Mat_BlinkTargetTop*, root_Fade, test_fog_PP | 1 - 6 each | | screen / gameplay effect materials: area 3 (post) |
| generic_translucent_PMAT decals, Glass_PMAT, Oil_PMAT, Shockwave_01_PMAT, water_sewer / slide / spillway / border | 0 base-pass depth reads | | nothing to add (their base passes do not sample the scene) |
## Audit table

| Feature | Data source | Maps using it | Status |
|---|---|---|---|
| Ambient occlusion | BaseEngine.ini `AmbientOcclusion=False`; no AO effect class | none | not used by the original; nothing to do |
| HeightFog / ExponentialHeightFog | not in the exe's class list | none | not in the game |
| Fog volumes (FogVolume*DensityInfo) | not in the exe's class list; factories hidden in DefaultEngine.ini | none | not in the game |
| DisFog height fog | `DisFog` actors, `FDisFogPixelShader*` | Boyle Ext confirmed (3 layers); per-map count TODO | implemented (`ark_post`); U-321 open |
| Distortion | `Dist_Enable_Distortion` materials, global shader 237 | 12 emitters in the decoded set; per-map TODO | implemented (`distortion.rs`); no settings toggle yet |
| Depth-biased alpha (soft particles) | `Db_Enable_Depth_Biased_Alpha`, `Db_Depth_Biased_Alpha_Intensity` | TODO (count MICs with the switch on, per map) | Volume_Light beams: done (formula decoded, now fed by the prepass). Sprite/mesh particles: missing, see below |
| Water depth opacity / fake fog | `river_main_PMAT` `O_*`, `F_*`, `D_Enable_Opacity_over_Depth` | maps with river_main_PMAT water (Boyle, canals, ...); TODO count | active with the depth prepass (unverified in screenshots) |
| Other SceneDepth / DestDepth / PixelDepth terms | compiled shaders only | shader scan (round 2) | see the table above: ported where the base is drawn by the port; the rest listed with the reason |
| Bloom-part depth test | main depth buffer | all maps with bloom parts | implemented |

## Left, and why

- **Per-map counts** of the Db MICs and of the depth-reading water / fire / Look_Up bases: no index of
  material parents per map exists; run `mic_static` over the `L_*_P` MICs (or extend the oracle index)
  before deciding whether the unported bases are used by a retail map.
- **`Water_*` (`Use_DepthAlpha`), `Fire_Explosion_*`, `Look_Up_*`**: other fade forms, not decoded yet
  (`shader_maps ... Water_PMAT` then `fxc /dumpbin` on a `Use_DepthAlpha` permutation).
- **Expose `FilteredDistortion` / `DropParticleDistortion`** from BaseEngine.ini: no consumer yet.
- **DisFog open points** (U-321) belong to ark_post and are unchanged.
- **AO, HeightFog, FogVolume**: not in the game; nothing to implement.
