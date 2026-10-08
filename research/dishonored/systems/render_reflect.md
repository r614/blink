# Rendering area 2: reflections and water (audit, in progress)

Status: audit only. Nothing implemented yet. The task moved to another agent after this first
pass.

Branch state: `main` and `worktree-agent-a4c9db721993b0d16` are merged in. The only conflicts
were in the append-only research/LOG.md and research/UNKNOWNS.md; both sides were kept.

## Sources

- `package_scan <CookedPCConsole>` gives the class histogram over every package.
- `package_scan <dir> --class C` lists the exports of class C in each package.
- The counts below include each class's `Default__` object in Engine.u.

## What the retail packages use

| Feature | Data source | Packages / maps using it | Blink status |
|---|---|---|---|
| Planar reflection capture (`SceneCaptureReflectActor` + `SceneCaptureReflectComponent`) | Actors in level packages (38 actors) | MainMenu_Env, Boyle_Ext_Env, Bridge_Part1a/1b/1c/2_Env, Brothel_Env, Distillery(2)_Block, Flooded_FRefinery_Intro/Script, Flooded_FStreets_Script (x2), Isl_Geom_Master/Slave, LightH_Geom_Master/Slave, LightH_HighChaos_Geom, Ovrsr_Back_Env, Prison_Env, PrsnSewer_Water (x4), Pub_{Assault,Day,Dusk,Morning,Night}_Env, Streets1/2_Custom, Streetsewer_Block (x4), TowerRtrn_Yard_Env, Tower_Fx | Missing. The materials fall back to the level cube (U-764). |
| Planar reflection targets (`TextureRenderTarget2D`) | `*_MS.RealTime_refl`, `*_reflection`, `water_RTR`, `water_prison_RTR`, `RTT_waterflooded01`, `RTCM_step1` | 69 exports, mostly the same packages as above, plus Boyle_Int_Basement, Flooded_* Env/FX, Isl/LightH LowChaos, Pub_*_Light, Streets1_Water, Tower_Env/Script, Ovrsr_Interior | Missing |
| Scene capture 2D (`SceneCapture2DActor`) and `saili_FT_LensTV.screen_TextureRender` | L_TowerRtrn_Int_Script; the target is also in TowerRtrn_Yard_Roof_Script | 1 actor (Sokolov's lens / TV screen) | Missing |
| Dynamic cube target `Materials_ref.CubeMap.generic_TRTC` (`TextureRenderTargetCube`) | Referenced by materials in about 90 level packages. No `SceneCaptureCubeMapActor` exists outside Engine defaults, so nothing ever renders into it. | Nearly every map | Not checked yet: what does the target hold when nothing draws it (clear colour)? |
| Static cubemaps (`TextureCube`, 420) and `MaterialExpressionTextureSampleParameterCube` (247) | Material cube parameters (`C_Cubemap_texture`, etc.) | Every map | Partly done. Water and spillway sample material or level cubes. The generic materials' cube terms have not been audited. |
| Portal capture (`SceneCapturePortalComponent` / `PortalTeleporter`) | Engine defaults only | None | Not used |
| Cube capture (`SceneCaptureCubeMapComponent`) | Engine defaults only | None | Not used |
| Water volumes (`DishonoredWaterVolume` 157, `DishonoredWaterVolumeInfo` 55, `DisWaterVolumeManager` 5, `DisWaterSource` 124) | Level actors | Many maps | Gameplay side. Underwater rendering has not been checked. |
| River water `river_main_PMAT` | `water.rs`, `ark_water.wgsl` | Boyle, canals and other maps (count not taken) | Decoded. The realtime-reflection term uses the cube instead (U-764). |
| Island sea `water_Foam_MAT` | `water_foam.rs` | L_Isl_* | Decoded. The planar reflection is replaced by the cube. |
| Spillway sheets `water_spillway_translu_PMAT` | `spillway.rs` | PrsnSewer, Pub, Out | Decoded. They use a material cube. |

## Next steps (plan)

1. Dump the properties of every `SceneCaptureReflectActor` / `SceneCaptureReflectComponent`.
   Use `obj_dump <pkg> SceneCaptureReflect`. Fields to read:
   - TextureTarget
   - Location / Rotation (which give the mirror plane)
   - FrameRate
   - MaxUpdateDist / MaxStreamingUpdateDist
   - bSkipUpdateIfOwnerOccluded
   - ViewMode
   - ClearColor
   - Show flags
   - Each TextureRenderTarget2D's SizeX / SizeY / Format
2. Find the engine rules in the decompiled engine (oracle index):
   - FSceneCaptureProbeReflect: the mirror plane is taken from the actor's transform, the view
     is reflected about it and an oblique clip plane is applied.
   - The update frequency and distance culling.
   Cite the function addresses.
3. Implement in Blink: one Bevy `Camera` per active reflect actor, rendering to an `Image`
   target with the dumped size. Mirror the main camera about the plane each frame (GPU work
   only), with an oblique near plane via a custom projection. Throttle by FrameRate and
   MaxUpdateDist, and only for actors whose target a visible material uses. Bind that image as
   `C_Realtime_reflection_Texture` in ark_water (screen UV + intensity x n.xy, shader 11099),
   water_foam and the other users. This replaces the U-764 cube fallback.
4. `generic_TRTC`: work out what the original samples when nothing captures it. Match that.
5. SceneCapture2DActor (TowerRtrn lens TV): a fixed camera drawing into
   `screen_TextureRender`.
6. Audit the reflection-vector / cube terms of the generic materials (generic_PMAT,
   generic_translucent_PMAT, etc.). Count the users of each base with the material-chain
   tools.
7. Settings: check whether the original's video options have a reflection toggle
   (DishonoredSystemSettings / `bAllowDynamicShadows`-style flags such as
   `SceneCaptureStreamingMultiplier`). Mirror it in `dishonored_settings`.
8. Add Boyle_Ext, Pub_Day, Streets1, PrsnSewer, Isl_LowChaos and TowerRtrn_Int to
   scripts/render_fixes.sh.
