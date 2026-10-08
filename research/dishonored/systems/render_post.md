# Rendering area 3: the post-process chain

Audit and port status. Data tools used: `package_scan` (class census), `obj_dump` / `tweaks` (objects and
tweaks), `kismet_dump` (Kismet ops), `oracle index` (the decompiled engine), `shader_dump` +
`fxc /dumpbin` (the global shaders), the oracle's captured shaders and timelines
(`$DISHONORED_CACHE/oracle/shaders/70da96138fe2171c/effects`). Branch merges: `main`,
`worktree-agent-a2786b82f5f9a84b8` and `worktree-agent-a4c9db721993b0d16` were already contained.

## What the original has

`WorldInfo.m_ArkDefaultPpSettings` is the level's grade (no `PostProcessVolume`s in the retail
packages: census 0). `Startup` `AltScreen_Effects.PostProcessChain.Test_PPG` is the game chain:
`DisPostProcessManager` (`PpBridge`) switches its nodes by `eEffectPp` (Underwater, BendTime,
BendTimeOverseer, DarkVision, Drunk, Adrenaline, Blink, Possession(In/Out), Dream, Knocked, WindBlast,
ZoomLens, Blinded, Weepers, MaskOn, MusicalOverseer, UberKismet, UberUI, FakeBlink). Each effect has
`m_RequiredEffects[e]` (+0x168 + 4e) and a state `m_EffectStates[e]` (+0x1bc + e); the setters are
`FUN_00be7dc0` (require, state 0 = jump to running) and `FUN_00be7df0` (release). Callers: Bend Time
`0x009fcbc0`, Knocked `0x00aa5090` / `0x00ab2420`, Adrenaline `0x00aa65e0`, Underwater `0x00aa6750`,
Dark Vision `0x00aa68d0`, MusicalOverseer `0x00aa6970`, Possession `0x00aa6c30`, Blinded `0x00aadde0`,
Kismet `0x00b91450` / `0x00b91570`, UI `0x00c47070`.

## Audit table

| Feature | Data source | Maps | Status |
|---|---|---|---|
| Level ArkPp grade (LUT), far DOF, grain, DisFog, bloom parts, distortion | WorldInfo, DisFog actors, particle materials | all | done before this branch (`ark_post.rs`, `bloom.rs`, `distortion.rs`) |
| Bend Time vector field + blur, Blink lens, Dark Vision grade / eyelid / souls | captured shaders, `Twk_*` | all | done before this branch (`power_pp.rs`) |
| **Kismet screen grades** (`DisSeqAct_UberPostProcess`, 15 ops) | the op's `m_Parameters` (`ArkUberPpParameters` + override flags), weight, fade times; `Default__DisSeqAct_UberPostProcess` for omitted fields; state machine `FUN_00bef9b0` | Tower (4: Accusation / Attract / Stun / Empress), Prison (1), PrsnSewer (1), Pub_FromFlooded (1), Pub_FromTwrReturn (1), Flooded_FAssassins (3, Daud) | **implemented** (`kismet_host/post.rs`, `MissionView.post_fx`, ArkPp override layer); protocol 19 |
| **Menu background blur** (`Epp_UberUI`, `UI_Blur` tweak) | `AltScreen_Effects.PostProcessChain.UI_Blur` (`DisTweaks_PostProcess`, the only instance): far blur 0.9 at focus 0, post-desaturation 0.5, fade 0.2 s; UI manager `0x00c47070` | pause menu (and any movie that asks) | **implemented** for the pause menu (`power_pp.rs`); the other menus' request flag (+0x17c bit 0 of each movie) is not read (LIKELY the pause menu sets it) |
| **Possession screen field** | captured shader `0x9312eee076ac74a4` (disassembled, translated), R / G timeline `possession_post_timeline.json` | all (cast) | **implemented** (`possession_field` in `ark_post.wgsl`, `power_pp::PossessPp`); the previous-field input is taken as black |
| **Arm glow** (`Use_Power`) | `PlayerArms_Tatoo_inst` / `Shr_DefaultPlayer`: shader 37254 disassembled; `arm_glow_timeline.json` | all (cast) | **implemented** (`viewmodel::arm_glow_materials`: baked HDR emissive texture, `Use_Power` scales the emissive colour); `Tex#4` taken as `M_Mask` (LIKELY) |
| **Light shafts** (`bRenderLightShafts`) | `LightComponent` properties; `GetPosition` `0x004e0440`; `0x00837480` and `0x00820300` / `0x00821930`; global shaders 158 / 157 / 155 | Pub_Day / Dusk / Morning / Night / Assault, LightH (High / Low chaos), TowerRtrn_Yard, Boyle_Int, OutsiderDream | **implemented** (see below); the setting `PSI_GraphicsPC_LightShaftEnable` toggles it |
| `DisSeqAct_PostProcess` (Epp_Knocked, Epp_Weepers, Epp_MaskOn) | op `m_Effect`; `FUN_00b91450` | Prison (2 Knocked), Tower (2 Knocked, 1 Weepers), Pub_FromTwrReturn (2 MaskOn), Pub_FromPrison (1 MaskOn) | state recorded (`HostState.post_effects`); **blocked**: the KO / Weepers / MaskOn screens are chain materials with no shader map and no capture |
| Adrenaline, Plague, ZoomLens (spyglass), Underwater vectors, KO frame blend, Blinded noise, Outsider (`PPG_Outsider`), BendTime under water, FakeBlink | `PPG_*` / `TEST_PPG_*` materials (21 `ArkPpNodeMaterial`) | gameplay | **blocked**: no shader maps; need oracle shader captures (or an expression-graph compiler) |
| `DisBlindedPpController` (noise seed 1253, speed 10), `DisOpacityParameterPpController` (`vOpacity`, fade 0.2 s) | `Test_PPG` controllers | gameplay | drive blocked materials |
| Kuwahara (`ArkPpNodeKuwa`), `ArkPpNodeAA` | chain | KO chain only | not reachable without the KO materials |
| Motion blur / radial blur actors, `SeqAct_SetMotionBlurParams` / `SetDOFParams` / `ColorScaleVolume` | class census: 1 each (class definitions only) | none | nothing to port |
| Damage / low health | no post effect found: the chain has no health node | n/a | the original draws it in the HUD, not here |
| Video options | `PSI_GraphicsPC_LightShaftEnable` (wired), gamma (wired earlier); the original has no bloom / DOF / motion blur option | | done |

## Kismet screen grades (`DisSeqAct_UberPostProcess`)

`FUN_00b91570` (the action's Activated): Start (input 0) copies `m_Parameters` (0x60 bytes) to the
manager's Kismet slot (`m_KismetPPParams` +0x224, weight +0x220, fade-in +0x21c, fade-out +0x218,
`FUN_00be7e90`) and requires effect 0x12; Stop (input 1) releases it. The slot's per-tick state machine
is `FUN_00bef9b0` (stopped / warming / running / cooling, with resume-from-level when Start or Stop
arrives mid-fade); `kismet_host/post.rs::KismetPost::step` is that function. The override fields are
`m_b*` flagged groups and fields (defaults from `Default__DisSeqAct_UberPostProcess`: DOF, colour
balance tones / opacity / pre-desaturation and exposure / gamma on; post-desaturation, grain,
brightness, contrast off). The client blends the overridden ArkPp settings in as a layer
(`ArkPostSource.kismet_post`) the way Dark Vision's grade is, now also including the DOF terms.
The slot ticks at the player's dilated dt (LIKELY: the controller's tick, `0x00aae45f`).
`m_bAlwaysOutOfBendTime` (Flooded_FAssassins) only affects the Kismet interpreter's dt for the op itself.

## Possession

Shader `0x9312eee076ac74a4` (`PPG_PossessionVectors`): a swirl field, `UniformPixelVector_1.xy` = the
manager's possession R, G, `UniformPixelScalars_0.x` = world time, `Texture2D_0` = the material's
first expression texture. The disassembly is translated line by line (`possession_field`), field
format = Bend Time's (rg = blur vector, ba = second pass offset), through the existing two-pass blur.
Drive (timeline `possession_post_timeline.json`): from the Active stage R rises 0.18 / s to 0.2 and G
0.54 / s to 0.6 (held), when possession ends they rise 0.667 / s to 1.2 and 0.833 / s to 1.5.
(The brief said 0.66 s; the timeline's slopes are per 60 Hz frame: 0.2 takes 1.1 s.)

## Arm glow

`Shr_DefaultPlayer` base pass (shader 37254, all lighting policies carry the same term): the glow is
`Use_Power x G(uv)` with `G = (2 - 0.2) P + 0.2 SP.z |TG|^2`,
`P = (1 - Tex#4.y)(1 - D.r)^40 x (10, 5, 1)` where `1 - D.r >= 1e-6`; the instance's `F_Firy_Amount`
0.996 zeroes the firy terms. G is baked once into an HDR emissive texture. `Use_Power` is 1 from
the tick after a cast, held 1.0 s (Blink, Dark Vision), 1.7 s (Possession), 1.08 s (Bend Time), then
falls linearly over 1 s of world time (`power_pp::ArmGlow`).

## Light shafts

Engine: `0x00837480` (per light: downsample, radial blur, apply); a light takes part when
`bRenderLightShafts` and `BloomScale > 0.001 or OcclusionMaskDarkness < 1`, it is in front of the
camera, and (for point / spot lights) within 5 radii. The retail shaft lights are all
`DirectionalLightComponent`s (most in a `StaticLightCollectionActor`, brightness 0, no lighting
channels: they exist for the shaft); the position is `GetPosition` (`0x004e0440`): `-direction x
TraceDistance` (100000). The component transforms of a collection actor come from its native data
(the last `count x 64` bytes). Shaders (global cache 158 / 157 / 155, disassembly translated in
`shaft_down` / `shaft_blur` / `shaft_apply`):

1. Downsample (158): 4 taps at 2 scene texels (scene alpha = depth / 256: here the depth buffer):
   alpha = max(mean saturate(depth / OcclusionDepthRange), border term); rgb = bloom of pixels
   beyond half the range above `BloomThreshold`, x `BloomScale`, x 0.25, confined to 0.2 (aspect
   space) around the light.
2. Radial blur (157): 32 iterations x 2 taps towards the light, weights `a` (`4a^2` below 0.25),
   step `min(|d|, sqrt(|d| / 2))`, / 64.
3. Apply (155): scene x factor (`OcclusionMaskDarkness` where occluded, `1.5 - OMD / 2` where open,
   1 far from the light) + tinted bloom x saturate(`BloomScreenBlendThreshold` 2^(-3 luma)) x 4.

The shaft targets are the scene / `DAT_0144e320` with `DAT_0144e324/328` as their size: the same
globals the bloom's separable blur reads (`FUN_0085c3d0`), the filter buffers
(`FilterDownsampleFactor`), 4 (LIKELY: the port's decoded bloom filter buffers are quarter size;
the writer of the globals goes through a struct pointer and was not found). Open: the targets'
sampler filter (bilinear clamp assumed; `DAT_0144e358`) and the single shaft light (the original
loops over all lights; the maps have one).

## Blocked (needs data not readable here)

* Shader captures for Adrenaline, Plague, ZoomLens, KO, Blinded, Outsider, Underwater vectors,
  MaskOn / Weepers and Possession's In / Out materials.
* The per-movie blur request flag of the UI manager beyond the pause menu.

## What to look at in screenshots

* `scripts/render_post.sh` maps: the Pub's sun shafts (bloom around the window light, occluded
  geometry darkened, open sky brightened), LightH, TowerRtrn_Yard, OutsiderDream.
* Pause menu: world blurred (0.9 far blur) and desaturated 0.5, fade 0.2 s.
* Possession cast: a swirl blur after ~1.5 s, stronger when it ends; arms' tattoos glow after a cast.
* Tower banquet (Kismet): the Accusation / Attract / Stun / Empress grades.
