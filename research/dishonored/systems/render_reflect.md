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
| `SceneCaptureReflectActor` + `SceneCaptureReflectComponent` | 37 placed actors (+ the Engine default) in 30 packages | MainMenu_Env, Boyle_Ext_Env, Bridge_Part1a/1b/1c/2_Env, Brothel_Env, Distillery(2)_Block, Flooded_FRefinery_Intro/Script, Flooded_FStreets_Script (2), Isl_Geom_Master/Slave, LightH_Geom_Master/Slave, LightH_HighChaos_Geom, Ovrsr_Back_Env, Prison_Env, PrsnSewer_Water (4), Pub_{Assault,Day,Dusk,Morning,Night}_Env, Streets1/2_Custom, Streetsewer_Block (4), TowerRtrn_Yard_Env, Tower_Fx | Implemented (`reflect.rs`). Actors with `bEnabled` false (PrsnSewer 4 of 4, Streetsewer 3 of 4, FStreets 1 of 2) wait for Kismet: left (U-962). |
| Mirror plane | Actor X axis through its location (`SceneCaptureReflectComponent` slot 88, 0x006E8200: `d = normal . Location`). Every placed rotation has pitch 16384, so the plane is horizontal | all | Implemented (`import_reflect_capture`) |
| `ReflectionChannels` | Capture: BSP / Static / Dynamic / Skeletal / Particles / Decals cleared, `Group_1` set (all actors). Primitive: only `Group_1` set, on a subset (Boyle Ext: 51 components: `Skies.SkyDome_Multi_UV`, canal ground, `sewer.*`, stairs, walls, handrails, `WaterLock_01`) | all | Implemented: a capture's channel set is a render layer; flagged instances (`MeshInstance::reflect_channels`) are drawn in layer 0 and theirs. Assumption: a primitive's struct default is all false (the instances that carry the property only list `Group_1`). |
| `SceneCaptureComponent` defaults | `bEnabled` true, `bEnablePostProcess` false, `bEnableFog` false, `ClearColor` black, `ViewMode` `SceneCapView_LitNoShadows`, `FrameRate` 1000 (reflect class), `bSkipUpdateIfTextureUsersOccluded` true, `MaxUpdateDist` 0 | all | Implemented: black clear, no shadows (no light reaches the capture camera: lightmaps and ambient only), `FrameRate` throttle, skip while no water that samples the target is visible, distance limit, `bEnabled`. |
| `bEnableFog` true | Boyle_Ext_Env, Isl_Geom_Master/Slave | Boyle Ext, Isl | Left (U-960): the DisFog layers are applied by the main view's `ArkPost` pass, not by a material; the capture's fog is missing. |
| `bEnablePostProcess` true | Isl_Geom_Master/Slave | Isl | Left (U-960), as above. |
| `FrameRate` | 100 (Boyle Ext, Streets1/2), 120 (Pub x5, Hub target), default 1000 elsewhere | all | Implemented (a capture per 1/FrameRate seconds; at these rates every frame). |
| Render targets (`TextureRenderTarget2D`) | 69 exports: `boyle_ext_MS.RealTime_refl`, `Hub_MS.water_RTR`, `Bridge_MS.Bridge_reflection`, `Flooded_MS.flooded_reflection`, `StreetsShared_MS.streets_reflection`, `Sewer_MS.RealTime_refl`, `StreetSewer_MS.RealTime_refl`, `tower_01_MS.water_RTR`, `Brothel_MS.water_RTR`, `Prison_MS.water_prison_RTR`, `Overseer_MS.overseer_back_reflection`, `Distillery_MS.RTCM_step1`. All: `m_ResolutionType` `TRT_HALFSIZE` (SizeX / SizeY 1), format `PF_A8R8G8B8`, clamp, default `ClearColor` green | all | Implemented: half the back buffer in each direction (resized with it), `Rgba8UnormSrgb`. |
| Water samples the target | `C_Realtime_reflection_Texture` of `river_main_PMAT` instances (`C_Enable_Realtime_Reflection`, shader 11099) and `water_Foam_MAT` | Boyle Ext, Pub, Isl, others | Implemented in `ark_water.wgsl` (screen UV + `W1_Distorsion1_intensity1` x n.xy) and `water_foam.wgsl` (screen UV + `W_Distorsion_intensity` n + `Wavy_Cube` wavy). `--no-reflections` restores the cube fallback. |
| Water permutations without realtime reflection (`river_main_PMAT` under_01 etc.) | `C_Cubemap_texture` | many | Already decoded (cube term, 10608). |
| `SceneCapture2DActor` (`L_TowerRtrn_Int_Script`, `saili_FT_LensTV.screen_TextureRender`) | 1 actor | TowerRtrn Int | Left: needs the TV material's use of the target and a fixed camera; one actor, no water. |
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

## Left (and why)

1. (U-962) Kismet's `SetEnabled` / `SeqAct_Toggle` on capture actors (the PrsnSewer, Streetsewer and
   FStreets actors start disabled): the Kismet runtime is simulation-side and has no render hook.
2. (U-960) Capture fog and post-process (`bEnableFog`, `bEnablePostProcess`; Boyle Ext, Isl).
3. (U-961) `generic_TRTC`: not derivable from the data.
4. `SceneCapture2DActor` (Tower Return lens TV).
5. Underwater views (the camera below the plane draws no reflection).
6. The original's video options have no reflection switch, so there is no `dishonored_settings`
   entry; `--no-reflections` is the debug flag.
