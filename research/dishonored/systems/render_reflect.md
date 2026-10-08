# Rendering area 2: reflections and water

Status: planar reflections implemented (`crates/dishonored_runtime/src/reflect.rs`); not yet
seen on screen (the coordinator's screenshot pass is the first run). Left items are listed at the
end with the reason.

Branch state: `main`, `worktree-agent-a4c9db721993b0d16` and the first audit pass
(`worktree-agent-ac1dbd3043e441369`) are merged in.

## Sources

- `package_scan <CookedPCConsole>` gives the class histogram over every package;
  `package_scan <dir> --class C` lists the exports of class C.
- `obj_dump <package.upk> SceneCaptureReflect` prints the actors' and components' properties;
  `obj_dump <package.upk> TextureRenderTarget2D` the render targets.
- `find_export <CookedPCConsole> <path> --suffix` finds which packages carry a render target.
- `oracle index class SceneCaptureReflectComponent` (and `SceneCaptureComponent`,
  `TextureRenderTarget2D`) for the class defaults; `oracle index fn 006e8200` for the plane.

## What the retail packages use

| Feature | Data source | Maps using it | Status |
|---|---|---|---|
| `SceneCaptureReflectActor` + `SceneCaptureReflectComponent` | 37 placed actors (+ the Engine default) in 30 packages | MainMenu_Env, Boyle_Ext_Env, Bridge_Part1a/1b/1c/2_Env, Brothel_Env, Distillery(2)_Block, Flooded_FRefinery_Intro/Script, Flooded_FStreets_Script (2), Isl_Geom_Master/Slave, LightH_Geom_Master/Slave, LightH_HighChaos_Geom, Ovrsr_Back_Env, Prison_Env, PrsnSewer_Water (4), Pub_{Assault,Day,Dusk,Morning,Night}_Env, Streets1/2_Custom, Streetsewer_Block (4), TowerRtrn_Yard_Env, Tower_Fx | Implemented (`reflect.rs`). Actors with `bEnabled` false (PrsnSewer 4 of 4, Streetsewer 3 of 4, FStreets 1 of 2) wait for Kismet's `SeqAct_Toggle`: carried to the renderer (round 2, U-962). |
| Mirror plane | Actor X axis through its location (`SceneCaptureReflectComponent` slot 88, 0x006E8200: `d = normal . Location`). Every placed rotation has pitch 16384, so the plane is horizontal | all | Implemented (`import_reflect_capture`) |
| `ReflectionChannels` | Capture: BSP / Static / Dynamic / Skeletal / Particles / Decals cleared, `Group_1` set (all actors). Primitive: only `Group_1` set, on a subset (Boyle Ext: 51 components: `Skies.SkyDome_Multi_UV`, canal ground, `sewer.*`, stairs, walls, handrails, `WaterLock_01`) | all | Implemented: a capture's channel set is a render layer; flagged instances (`MeshInstance::reflect_channels`) are drawn in layer 0 and theirs. Assumption: a primitive's struct default is all false (the instances that carry the property only list `Group_1`). |
| `SceneCaptureComponent` defaults | `bEnabled` true, `bEnablePostProcess` false, `bEnableFog` false, `ClearColor` black, `ViewMode` `SceneCapView_LitNoShadows`, `FrameRate` 1000 (reflect class), `bSkipUpdateIfTextureUsersOccluded` true, `MaxUpdateDist` 0 | all | Implemented: black clear, no shadows (no light reaches the capture camera: lightmaps and ambient only), `FrameRate` throttle, skip while no water that samples the target is visible, distance limit, `bEnabled`. |
| `bEnableFog` true | Boyle_Ext_Env, Isl_Geom_Master/Slave | Boyle Ext, Isl | Implemented (round 2, LIKELY, U-960): the capture camera has its own `ArkPost` pass with the level's DisFog layers (`reflect::spawn_captures`). |
| `bEnablePostProcess` true | Isl_Geom_Master/Slave | Isl | Implemented (round 2, LIKELY, U-960): the level's ArkPp colour pipeline on the capture, no bloom parts. The Isl captures set no `PostProcess` chain and not `bUseMainScenePostProcessSettings` (class default false), so what the original's capture path applies is not decoded. |
| `FrameRate` | 100 (Boyle Ext, Streets1/2), 120 (Pub x5, Hub target), default 1000 elsewhere | all | Implemented (a capture per 1/FrameRate seconds; at these rates every frame). |
| Render targets (`TextureRenderTarget2D`) | 69 exports: `boyle_ext_MS.RealTime_refl`, `Hub_MS.water_RTR`, `Bridge_MS.Bridge_reflection`, `Flooded_MS.flooded_reflection`, `StreetsShared_MS.streets_reflection`, `Sewer_MS.RealTime_refl`, `StreetSewer_MS.RealTime_refl`, `tower_01_MS.water_RTR`, `Brothel_MS.water_RTR`, `Prison_MS.water_prison_RTR`, `Overseer_MS.overseer_back_reflection`, `Distillery_MS.RTCM_step1`. All: `m_ResolutionType` `TRT_HALFSIZE` (SizeX / SizeY 1), format `PF_A8R8G8B8`, clamp, default `ClearColor` green | all | Implemented: half the back buffer in each direction (resized with it), `Rgba8UnormSrgb`. |
| Water samples the target | `C_Realtime_reflection_Texture` of `river_main_PMAT` instances (`C_Enable_Realtime_Reflection`, shader 11099) and `water_Foam_MAT` | Boyle Ext, Pub, Isl, others | Implemented in `ark_water.wgsl` (screen UV + `W1_Distorsion1_intensity1` x n.xy) and `water_foam.wgsl` (screen UV + `W_Distorsion_intensity` n + `Wavy_Cube` wavy). `--no-reflections` restores the cube fallback. |
| Water permutations without realtime reflection (`river_main_PMAT` under_01 etc.) | `C_Cubemap_texture` | many | Already decoded (cube term, 10608). |
| `SceneCapture2DActor` (`L_TowerRtrn_Int_Script`, `saili_FT_LensTV.screen_TextureRender`) | 1 actor (+ the Engine default); target 256 x 256 fixed; `FrameRate` 10, `NearPlane` 1, `FarPlane` 1100, `FieldOfView` 80 (class default), at (4961.6, 6646.2, 9931.3) yaw 32960 | TowerRtrn Int (the Yard roof package carries the target and its material but no actor) | Implemented (round 2): a fixed-view capture (`ReflectCapture::scene2d`) drawing the whole scene (layer 0), and the TV material `saili_FT_LensTV.screen_MAT` rebuilt from its compiled base pass (`tv_screen.rs`). |
| `generic_TRTC` (`TextureRenderTargetCube`, 128, `PF_A8R8G8B8`) | referenced by materials in ~90 packages as the default `C_Cubemap_texture`. No `SceneCaptureCubeMapActor` exists outside Engine defaults and the executable and scripts never name it, so nothing draws into it | nearly every map | Left (U-961): what a never-drawn D3D9 render target cube returns is not in the data; the level reflection cube stands in as before. |
| `SceneCapturePortalComponent`, `SceneCaptureCubeMapComponent` | Engine defaults only | none | Not used |
| Static cubemaps (`TextureCube` 420, `MaterialExpressionTextureSampleParameterCube` 247) | material cube parameters | every map | Water, spillway and decals sample them (earlier work); generic materials' cube terms not audited here. |
| Water volumes (`DishonoredWaterVolume` 157, `DisWaterVolumeManager`, `DisWaterSource`) | level actors | many | Gameplay side; underwater rendering not touched. |
| River `river_main_PMAT`, island sea `water_Foam_MAT`, spillway sheets | `water.rs`, `water_foam.rs`, `spillway.rs` | Boyle, canals, Isl, PrsnSewer, Pub | Decoded earlier; the first two now take the planar target. |

## How it is built

- `dishonored_ir`: `ReflectCapture`, `MeshInstance::reflect_channels`,
  `MaterialParams::render_targets` (texture parameters bound to a `TextureRenderTarget2D`).
- `dishonored_assets::level`: `import_reflect_capture`, `channel_mask`; the material chain records
  render-target parameters.
- `reflect.rs`: one `Camera3d` per actor, rendering to an `Image`, inactive until a frame it is
  due. Per frame (PostUpdate, after transform propagation): per render target the actor whose plane
  is nearest below the viewer draws; the camera is the viewer's position, forward and up reflected
  about the plane; the projection's `near_clip_plane` is the plane (oblique near plane); the target
  is resized to the back buffer. GPU work: the flagged geometry at half resolution. No per-entity
  CPU work.
- `map_render::spawn_map`: flagged instances get `RenderLayers` (the main camera sees layer 0);
  merged batches are keyed by their layers; water entities carry `ReflectUser`.
- The reflected view is the true mirror flipped in x (reflecting a basis flips handedness), so the
  water shaders sample the target at `1 - u`.

## Round 2 (2026-10-08)

- **Capture fog and post (U-960, LIKELY).** `bEnableFog` (Boyle_Ext_Env, Isl_Geom_Master / Slave) and
  `bEnablePostProcess` (Isl) give the capture camera an `ArkPostSource` (`ark_post.rs`, `capture`: no bloom
  parts) with the level's DisFog layers and `m_ArkDefaultPpSettings`; the pass runs on the capture's own
  colour and depth, so the fog is computed from the mirrored camera (its position below the plane, the
  mirrored rays) exactly as the pass does for the game view. The system queries that drive the game
  camera's `ArkPostSource` (Dark Vision's grade, Blink / Bend Time) skip capture cameras. Not decoded: the
  original's capture path for DisFog / ArkPp (the Isl components set neither a `PostProcess` chain nor
  `bUseMainScenePostProcessSettings`), so whether it applies the main settings is an assumption.
- **Kismet toggles (U-962).** `SceneCaptureActor.OnToggle` (`oracle index uscript SceneCaptureActor`):
  `SeqAct_Toggle` input 0 sets `SceneCapture.bEnabled` true, 1 false, 2 inverts it. The Kismet host
  (`kismet_host.rs toggle_actors`) records per `SceneCapture*` actor the last explicit On / Off and the
  parity of the Toggles since, `view()` copies them to `MissionView::captures` ("package:actor", the same
  key as `ambient`; protocol 23), and `reflect::update_cameras` evaluates `toggled_enabled(level bEnabled,
  explicit, flipped)` into each camera's `enabled` every frame. A non-mission session (no Kismet) keeps the
  level's value.
- **`SceneCapture2DActor` (the lens TV).** The actor's `Location` / `Rotation` give the fixed view (X
  forward, Z up), `FieldOfView` is horizontal (UE3), the target is a fixed 256 x 256 one (no
  `m_ResolutionType` is set, so the enum's first value `TRT_FIXEDSIZE`; `SizeX` / `SizeY` 256). It captures at `FrameRate` 10 while enabled
  and its users (`bSkipUpdateIfTextureUsersOccluded` default false) are not required to be visible. The
  material `saili_FT_LensTV.screen_MAT` (`TV_Screen_MAT` is a separate material in the shader cache, not examined)
  is an unlit emissive whose graph is stripped; its compiled base pass (shader 8137 and the other
  lightmap policies, all the same emissive) is decoded in `tv_screen.rs`: three taps of the target at
  noise-driven channel offsets, scan lines, vignette and a wobbling flicker, x 10. Capture lighting
  difference: `SceneCapView_LitNoShadows` (no shadows) is not reproduced for the 2D capture, which sees the
  layer-0 lights with Bevy's shadows.

## Left (and why)

1. (U-961) `generic_TRTC`: not derivable from the data.
2. Underwater views (the camera below the plane draws no reflection).
3. The original's video options have no reflection switch, so there is no `dishonored_settings`
   entry; `--no-reflections` is the debug flag (it also leaves the TV capture out).
4. Kismet toggles have only been exercised through the code path (no run): a mission run past the event
   that turns a PrsnSewer / Streetsewer capture on is the check.