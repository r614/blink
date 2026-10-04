# Unknowns Register

Priority: **P0** project blocker · **P1** major mechanic blocker · **P2** fidelity issue · **P3** polish.
Status: OPEN / ACTIVE / RESOLVED. Resolved items stay listed with their resolution.

| ID | P | Status | Unknown | Next experiment |
|---|---|---|---|---|
| U-001 | P0 | RESOLVED | Can cooked packages be read at all (compression, versions)? | Resolved: v801/30, LZO chunks; 470/470 open. |
| U-002 | P0 | RESOLVED | Export table layout for this licensee | Resolved: 68+4N bytes, flags high dword first; strict end check passes everywhere. |
| U-003 | P0 | RESOLVED | StaticMesh serialization | Resolved: generic UE3 v801 + legacy kDOP; 405/405 decode on Boyle Ext. |
| U-004 | P0 | RESOLVED | Does imported geometry render recognizably with correct transforms? | Resolved visually 2026-10-04 (LOG: First render). A numeric transform check against the original is still owed under U-011. |
| U-005 | P2 | OPEN | 137 unresolved cross-package resource imports on Boyle Ext | Index `DishonoredGame.upk` and startup packages; log which paths miss. |
| U-006 | P2 | OPEN | World offset between `L_Boyle_Ext_P` and `L_Boyle_Int_P` | Compare door and transition actor positions in both maps. |
| U-007 | P1 | OPEN | Is any walkable geometry BSP (`Model`/`BrushComponent`) rather than static meshes? | Count `Model` nodes and surfaces in Boyle packages; render BSP if it exists. |
| U-008 | P1 | OPEN | Animation: Sony Edge compression (`ACF_EdgeAnim`) has no public decoder | Plan: (1) gameplay never depends on curves; timing comes from AnimSequence length/rate + AnimNotify data, which already decode as properties. (2) Before Phase II S0, port the Edge decompressor out of `Dishonored.exe` (the PC build decodes at runtime, so the routine exists in the binary). (3) Stopgap if needed: capture bone poses from the running game for a few locomotion clips. |
| U-009 | P1 | OPEN | Player movement constants (walk/run/crouch speeds, accel, gravity, jump Z, air control) | Read `DishonoredPlayerPawn` defaults from `DishonoredGame.upk` and the INIs, then measure in game. |
| U-010 | P1 | OPEN | Blink targeting algorithm (ray/sweep shape, ledge snap, clearance) | Find the Blink power class in `DishonoredGame.upk` (script bytecode + defaults); check `DefaultPower.ini`. |
| U-011 | P0 | OPEN | Runtime instrumentation of the original game (position/velocity/state capture) | Locate the player pawn in memory on hash `70da9613…`. Options: GNames/GObjects scan, or a debugger on `Dishonored.exe`. |
| U-012 | P1 | OPEN | Semantics of `DisPrimTraceMask` (crosshair/projectile/melee/vision trace channels) | Map which primitives disable which channel; check against Blink/projectile behaviour. |
| U-013 | P2 | OPEN | Lightmap/shadowmap usage (`LightMapTexture2D`, `ShadowMap2D`) | Decode lightmap coordinates (UV1) and textures for recognizable lighting. |
| U-014 | P2 | OPEN | Remaining property-parse failures in script packages (~0.09%) | Re-sweep after the template-name fix; the class-specific prefixes are listed in `PACKAGE_FORMAT.md`. |
| U-015 | P1 | OPEN | Collision source for gameplay: kDOP triangles vs `RB_BodySetup` convex hulls vs simplified collision meshes | Compare against PhysX/NxCharacter use in the exe; test whether hidden `Collision_model*` actors are player blockers. |
