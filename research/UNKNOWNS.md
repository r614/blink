# Unknowns Register

Priority: **P0** project blocker · **P1** major mechanic blocker · **P2** fidelity issue · **P3** polish.
Status: OPEN / ACTIVE / RESOLVED. Resolved items stay listed with their resolution.

| ID | P | Status | Unknown | Next experiment |
|---|---|---|---|---|
| U-001 | P0 | RESOLVED | Can cooked packages be read at all (compression, versions)? | Resolved: v801/30, LZO chunks; 470/470 open. |
| U-002 | P0 | RESOLVED | Export table layout for this licensee | Resolved: 68+4N bytes, flags high dword first; strict end check passes everywhere. |
| U-003 | P0 | RESOLVED | StaticMesh serialization | Resolved: generic UE3 v801 + legacy kDOP; 405/405 decode on Boyle Ext. |
| U-004 | P0 | RESOLVED | Does imported geometry render recognizably with correct transforms? | Resolved visually 2026-10-04 (LOG: First render). A numeric transform check against the original is still owed under U-011. |
| U-005 | P2 | RESOLVED | 137 unresolved cross-package resource imports on Boyle Ext | Resolved 2026-10-04: `DishonoredGame.upk` + `Startup.upk` indexed, MIC chains walked across packages; 5 refs (2 paths) remain (LOG: Cross-package materials). |
| U-006 | P2 | OPEN | World offset between `L_Boyle_Ext_P` and `L_Boyle_Int_P` | Compare door and transition actor positions in both maps. |
| U-007 | P1 | OPEN | Is any walkable geometry BSP (`Model`/`BrushComponent`) rather than static meshes? | Count `Model` nodes and surfaces in Boyle packages; render BSP if it exists. |
| U-008 | P1 | RESOLVED (generic path) | Animation: Sony Edge compression (`ACF_EdgeAnim`) has no public decoder | Plan: (1) gameplay never depends on curves; timing comes from AnimSequence length/rate + AnimNotify data, which already decode as properties. (2) Before Phase II S0, port the Edge decompressor out of `Dishonored.exe` (the PC build decodes at runtime, so the routine exists in the binary). (3) Stopgap if needed: capture bone poses from the running game for a few locomotion clips. |
| U-007 | P1 | RESOLVED | Is any walkable geometry BSP (`Model`/`BrushComponent`) rather than static meshes? | Resolved 2026-10-04 for Boyle Ext: level Models are empty (156-192 B); BrushComponents are volumes only (LOG: Map coverage). |
| U-008 | P1 | OPEN | Animation: Sony Edge compression (`ACF_EdgeAnim`) has no public decoder | Plan: (1) gameplay never depends on curves; timing comes from AnimSequence length/rate + AnimNotify data, which already decode as properties. (2) Before Phase II S0, port the Edge decompressor out of `Dishonored.exe` (the PC build decodes at runtime, so the routine exists in the binary). (3) Stopgap if needed: capture bone poses from the running game for a few locomotion clips. |
| U-009 | P1 | OPEN | Player movement constants (walk/run/crouch speeds, accel, gravity, jump Z, air control) | Read `DishonoredPlayerPawn` defaults from `DishonoredGame.upk` and the INIs, then measure in game. |
| U-010 | P1 | OPEN | Blink targeting algorithm (ray/sweep shape, ledge snap, clearance) | Find the Blink power class in `DishonoredGame.upk` (script bytecode + defaults); check `DefaultPower.ini`. |
| U-011 | P0 | OPEN | Runtime instrumentation of the original game (position/velocity/state capture) | Locate the player pawn in memory on hash `70da9613…`. Options: GNames/GObjects scan, or a debugger on `Dishonored.exe`. |
| U-012 | P1 | OPEN | Semantics of `DisPrimTraceMask` (crosshair/projectile/melee/vision trace channels) | Map which primitives disable which channel; check against Blink/projectile behaviour. |
| U-013 | P2 | RESOLVED | Lightmap/shadowmap usage (`LightMapTexture2D`, `ShadowMap2D`) | Resolved 2026-10-04: two-texture directional lightmaps decoded and baked (LOG: Baked lighting). ShadowMap2D textures documented but not used yet. |
| U-014 | P2 | OPEN | Remaining property-parse failures in script packages (~0.09%) | Re-sweep after the template-name fix; the class-specific prefixes are listed in `PACKAGE_FORMAT.md`. |
| U-015 | P1 | OPEN | Collision source for gameplay: kDOP triangles vs `RB_BodySetup` convex hulls vs simplified collision meshes | Compare against PhysX/NxCharacter use in the exe; test whether hidden `Collision_model*` actors are player blockers. |
| U-016 | P2 | OPEN | Pawn collision is a capsule vs UE3's flat-bottomed cylinder | Walkability uses contact face normals and step-ups check the support point height (cylinder emulation). Measure ledge-edge behaviour in the original. |
| U-017 | P2 | OPEN | Crouch height: `m_fCrouchHeight` (33) vs `m_fSneakHeight` (65) | Using 65 as the sneak half-height. Verify with original-game capture. |
| U-018 | P1 | OPEN | Blink travel speed/duration (SPECULATIVE 9000 uu/s) and exact target query shape | Capture Blink traces from the original (U-011); disassemble the Blink power class. |
| U-019 | P2 | OPEN | Mantle timings (animation-driven) and probe pattern | Now decodable via Edge: read mantle anim lengths and notifies. |
| U-020 | P0 (Phase II) | OPEN | Core product test: do humans hesitate between NPCs and players? | Needs human playtests. Automated proxies: compare player vs NPC motion statistics. |
| U-021 | P2 | PARTLY RESOLVED | Sword timings, parry window, reload/equip times | Swing, parry, assassination, equip and reload now come from AnimNotify data (LOG 2026-10-04). Stagger and blocked recoil are still SPECULATIVE; verify the windows in the original game (U-011). |
| U-022 | P2 | OPEN | Edge fixed-format channel path (410 clips with flags=2) | Port 0x009D4D90/0x009D5390. |
| U-023 | P2 | OPEN | Edge root motion / translation tracks (rotation-only retargeting today) | Compare against original playback. |
| U-024 | P2 | OPEN | Edge clips with fewer joints than their AnimSet tracks (e.g. Powers_Sprint 29/30, Empty_AimAdd* 67/69): which tracks are omitted | Compare per-clip joint tables against the set; check the blend-tree evaluator for a per-sequence track map. |
| U-025 | P2 | LIKELY | First-person weapon socket convention (weapon mesh -> handAttachment_*_jnt) | Sword: Rz(180)*inverse(RotOrigin) (blade leaves the thumb side and points forward-up in Sword_Ready_Idle); guns: inverse(RotOrigin). Chosen visually; confirm against original captures (U-011). |
| U-026 | P2 | OPEN | Exact reconstruction of Dishonored's two-coefficient directional lightmap (`NormalizedAverageColor` × `DirectionalMaxComponent`) and the tangent-space basis it uses | Disassemble the lightmap pixel shader from the shader cache (`RefShaderCache`/`*ShaderCache*.upk`) or compare flat-wall brightness against original-game captures (U-011). Current renderer uses the flat-normal average. |
| U-027 | P2 | OPEN | MIC static switch values (`StaticParameters` in the static permutation resource) | Decode the MIC native data after properties; replaces the "explicit textures only" heuristic for S_/E_/O_ slots and gives the true channel masks. |
| U-028 | P2 | OPEN | DisFog semantics (LUT `m_RawLut`/`FogLUT`, `NoFogPlane`, custom transition height, interaction of overlapping layers) and `ArkPpConfig` struct defaults/meaning of `m_Exposure` | Decode the `ArkPpConfig`/`DisFogComponent` script structs in `Engine.upk`; compare against original captures. |
