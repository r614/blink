# Runtime instrumentation of the original game (U-011)

The probe measures the real `Dishonored.exe` (GOG build, SHA-256 `70da9613...`) while it runs,
instead of inferring behaviour from the decompiled code. It has three parts:

| Part | Where | Role |
|---|---|---|
| `probe_launch` | `tools/probe_launch` (32-bit build) | starts the game suspended, injects the DLL, drives it, runs the measurements |
| `dishonored_probe_dll` | `crates/dishonored_probe_dll` (`i686-pc-windows-msvc` cdylib) | hooks inside the game and serves the pipe |
| `dishonored_probe` | `crates/dishonored_probe` (any target, std only) | the wire protocol and a blocking `Client` the harness can use |

## Safety properties

- **Local pipe only.** The DLL serves `\\.\pipe\dishonored_probe_<pid>_<token>`. It never opens a
  network socket. The pipe has the `PIPE_REJECT_REMOTE_CLIENTS` flag and a protected DACL
  (`D:P(A;;GA;;;<current user SID>)`), so only the current user can open it. It exists only while
  that game process runs. At most 4 clients are served at once.
- **Only in a game the launcher started.** The launcher creates `Dishonored.exe` suspended. It puts
  a random 128-bit token in that process's environment (`DISHONORED_PROBE_TOKEN`). `DllMain`
  refuses to load (returns FALSE) unless the host executable is `Dishonored.exe` and the token is
  present.
- **Nothing persistent.** There is no auto-start, no service, no registry entry and no change to
  the GOG install. The launcher injects a per-session copy of the DLL from
  `%TEMP%\dishonored_probe\`, and the session file is `%TEMP%\dishonored_probe_session.txt`. The GOG
  build is DRM-free; nothing is bypassed.
- **The user's desktop is left alone.** The game window runs in the background. The DLL redirects
  only this executable's imports of `ClipCursor`, `SetCursorPos` and `SetForegroundWindow` (no-ops)
  and of `GetCursorPos` (it reports the last position the game asked for). The game therefore
  cannot confine, move or take the cursor or focus. `GetFocus`, `GetForegroundWindow` and
  `GetActiveWindow` report the game's own window (*focus faking*); without that, the engine pauses
  the world when its window is unfocused (`Engine.bPauseOnLossOfFocus`, `WorldInfo.Paused`).
  Set `DISHONORED_PROBE_FAKE_FOCUS=0` to turn focus faking off.
- **Captured data stays in scratch.** The measurement CSVs and screenshots are written where
  `--out` points. Keep that outside the repository: they are retail-derived.

## Engine facts the probe relies on

- **No ASLR.** The PE header has image base 0x00400000 and DllCharacteristics 0x8100 (NX and
  terminal-server aware, no DYNAMIC_BASE), so the Ghidra addresses are the runtime addresses.
- **`UObject::ProcessEvent`** is at 0x00470640: thiscall `(UFunction*, void* Parms, void* Result)`,
  `ret 0xC`. Its prologue `55 8B EC 6A FF` is moved into a trampoline. `AActor::ProcessEvent`
  (vtable slot 59, +0xEC; 0x00567A20) wraps it. Event wrappers call `FindFunctionChecked`
  (0x00455630) and then the virtual. The detour records the game thread and runs queued
  game-thread jobs. Console commands are `PlayerController.ConsoleCommand` (parms: FString command,
  bool, FString return at +0x10) called through the virtual.
- **`GObjObjects` (0x01423630) and `GNames` (0x01435674)** are found at runtime by scanning .data
  (0x01269000..0x014843BC). A TArray qualifies as the object array when object *i*'s
  `ObjectInternalInteger` (+0x4) is *i*. It qualifies as the name array when entry 0 reads `None`
  and entry 1 reads `ByteProperty` (characters at +0x10). UObject: Outer +0x24, Name +0x28,
  Class +0x30.
- **The engine object** is `Transient.DishonoredEngine_0` (a `GameEngine` subclass; `GEngine` is
  the global at 0x0144721C). `GamePlayers` is at +0x480, `GameViewport` at +0x48C, the local
  player's `Actor` (the PlayerController) at +0x3C, `Controller.Pawn` at +0x248, and
  `PlayerController.PlayerInput` at +0x464. Actor fields: Location +0xC4, Rotation +0xD0,
  Physics +0x104, WorldInfo +0x160, Velocity +0x1B4. Pawn fields: Health +0x344,
  HealthMax +0x348. `WorldInfo.TimeSeconds` is at +0x38C.
- **Rendering is threaded.** `Present` runs on the render thread, not the game thread. There is no
  script `PlayerTick` in this build (Dishonored made it native), so the per-tick sample is taken
  in the D3D9 `Present` hook, once per rendered frame, and timed with `WorldInfo.TimeSeconds`.
- **D3D9.** A throwaway `Direct3DCreate9` gives d3d9's shared `IDirect3D9` vtable. Its
  `CreateDevice` (slot 16) is patched, and the device the game creates gets these slots patched:
  Present (17), DrawPrimitive (81), DrawIndexedPrimitive (82), DrawPrimitiveUP (83) and
  DrawIndexedPrimitiveUP (84) (UE3's full-screen passes: post, fog, the ArkPp LUT), CreatePixelShader (106),
  SetPixelShader (107) and SetPixelShaderConstantF (109).
  - Pixel shaders are identified by an FNV-1a 64 hash of their bytecode and their CTAB names.
  - The constant registers are mirrored, so a draw with a chosen shader yields its live
    constants.
  - The back buffer is captured with GetRenderTargetData into a system-memory surface.
- **Keyboard.**
  - The game reads the keyboard from window messages that it defers. DirectInput is used for the
    mouse only: 0x009CA1C0 creates one device.
  - The probe skips the messages and calls the entry the message processor calls (as in
    `UpdateModifierState`, 0x009C7D70): `FViewportClient::InputKey(FViewport*, ControllerId,
    FName Key, EInputEvent, Amount, bGamepad)`. That is vtable +0x18 of `GameViewport + 0x38`,
    with the viewport at `GameViewport + 0x40`, called on the game thread.
  - Keys are engine names (`SpaceBar`, `W`, `LeftShift`, `C`, `Enter`, ...).
  - **`LocalPlayer.ControllerId` (+0x5C) stays -1** until the start screen's "press any key". With
    -1, the viewport client routes no key to the player. `measure boot` writes 0 there.
- **Loading movies.** A level's loading movie (Bink) waits for a key that the engine reads only
  from a focused window. The launcher passes `-nomovie`. `-nosound` crashes the game (a fatal
  error at 0x009B18A0), so the game plays sound.
- **Moving the pawn.** `Actor.SetLocation` returns false for the player pawn, and
  `Actor.SetPhysics(PHYS_Falling)` is ignored (the player overrides `setPhysics`, slot 134).
  - Drops are made by writing `Location.Z` and stepping forward for one frame, so `physWalking`
    finds no floor.
  - Fixed viewpoints write `Location` and set `Physics` to `PHYS_None` (0).

## Running it

All commands run from the worktree root. Only the DLL and the launcher need the 32-bit target
(`rustup target add i686-pc-windows-msvc`).

```text
CARGO_BUILD_JOBS=3 cargo build -p dishonored_probe_dll -p probe_launch --target i686-pc-windows-msvc
P=target/i686-pc-windows-msvc/debug/probe_launch.exe
$P launch                      # game windowed 1280x720, -nostartupmovies -nomovie; prints the pipe
$P measure boot                # front end -> `open L_Boyle_Ext_P` -> ControllerId 0 -> in control (~4 min)
$P status                      # hooks, globals, local PC / pawn, frame counters, map
$P measure jump 4 --out DIR    # standing jumps (+ parabola fit)
$P measure fall 800 900 1000 --out DIR
$P measure run 0.4 0.6 at=-3400,-3150,-190 --yaw 270 --out DIR   # run, sprint, slide on a flat runway
$P measure sdf                 # live constants of every drawn distance-field-shadow light shader
$P measure views --out DIR     # the `harness fidelity` viewpoints, HUD hidden -> DIR/<name>.bmp
$P measure consts <ctab filter> <draws> [name...]   # distinct live values of pixel shader constants (arrays in full)
$P measure psconst X [draws]   # live PS constants of a shader: X = a shader_maps blob (.bin),
                               # 0xHASH, or a CTAB-name filter (every shader declaring it)
$P kill                        # end the session's game (by pid)
```

Helpers:

- `console "<cmd>"`, `key <Name> [down|up|tap]`, `find <Name|Outer.Name> [Class]` and `name <addr>`.
- `peek <pc|pawn|engine|input|addr> [+off|*]... <len>` reads memory (`*` dereferences).
- `write <addr> <hex>` writes memory.
- `call <pc|pawn|addr> <Class.Func> [f:|i:|x:]...` runs ProcessEvent with raw parameters.
- `shaders [filter]` lists pixel shaders and `capture <out.bmp>` saves the back buffer.
- `measure trace N` prints N per-frame samples as CSV.

The harness can use `dishonored_probe::Client::connect(pipe)` directly. The pipe name is the
second line of the session file.

The original runs at 20-35 fps in a background window, and its physics are frame-rate dependent.
The measurements therefore use each sample's `TimeSeconds`, and the jump analysis fits a parabola.

## First measurements (2026-10-06, Boyle exterior, Corvo unarmed: sword sheathed)

| Quantity | Original (measured) | Port (`dishonored_sim`) | Verdict |
|---|---|---|---|
| Jump launch speed | 800.0 uu/s (`Velocity.Z` on the first airborne frame) | `JumpZ` 800 | same |
| Jump apex | 106.45-106.67 uu over 4 jumps; parabola fit 106.66-106.67 | 106.7 (`JumpZ^2 / (4 x 1500)`) | **confirms the halved apex** (refutes 213) |
| Effective gravity in flight | 2999.5-3000.7 uu/s^2 (fit) | 2 x 1500 (`physFalling` keeps `2 x avg - v0`) | same |
| Jump airtime (back to take-off height) | 0.533 s (from the fit: `2 x 800 / 3000`); raw frame-to-frame 0.59-0.77 s at 20-35 fps | 0.533 s | same |
| Fall damage onset | 800 uu drop: no damage; 868 uu: 1 point | damage above 2250 uu/s at impact, a drop of about 844 uu | consistent |
| Fall damage size | 900 uu drop: 3; 1000: 19; 1100: 32; 1200: 51; 1300: 58 of 70 | `70 x (v - 2250) / 650` with `v ~ sqrt(4 g h)`: 7, 21, 34, 46, 58 | consistent; frame-dependent impact speed scatters a few points |
| Run speed | 400.0 uu/s, 90 % reached in 0.16-0.18 s | 400 x sheathed factor 1.0 (360 with the sword out) | same |
| Sneak speed | 275.0 uu/s | 275 | same |
| Sprint speed | 600.0 uu/s, +90 % of the gain in 0.15 s | 600 | same |
| Slide | starts at 600, eases to 275 in 1.10 s; 434.7 uu/s at 0.555 s; 483 uu covered until it reaches 275 | `v0 + (target - v0)(1 - cos(pi t / 1.1)) / 2`: 434.9 at 0.555 s, 481 uu | same |
| Slide physics | `PHYS_Custom`; the pawn drops 61 uu at slide start (sneak cylinder) | sneaking during the slide | same |
| Directional dominant light, distance-field shadow | `DistanceFieldParameters` = (-0.366667, 3.75, 2.0) | (-0.375, 4, 2) | **differs**: scale 3.75 (U-401, U-460) |
| Same, light colour x brightness | (0.0947, 0.1614, 0.25), falloff exponent 0 | | for calibration |
| Spot lights with distance-field shadows | (-0.34, 3.125, 2.0): bluish lights (1.08, 3.23, 5.0) and (3.97, 5.86, 7.5); (-0.495, 100, 2.0): warm lights (2.0, 1.21, 0.68), (2.5, 2.13, 1.16), (5.0, 3.33, 1.38) | directional only | U-461 |
| Base pass overrides in the light pass | `DiffuseOverrideParameter` = `SpecularOverrideParameter` = (0, 0, 0, 1) | (0, 0, 0, 1) assumed (U-320) | same (light pass only) |

In every set, bias = -0.5 + 0.5 / scale, so the shader form `saturate((d + x) y)^z` is a ramp of
width 1 / y centred on the 0.5 iso-line. The port's formula is right, but its scale of 4 must be
3.75 for the moon on Boyle Ext.

## Reference captures (graphics calibration)

`measure views --out DIR` places the camera on each `harness fidelity` viewpoint:

- The pawn is frozen at the right spot. Its camera, read back from `PlayerCamera.CameraCache.POV`
  (Camera +0x32C), lands within 0.5 uu (`spawn1` within 5 uu, from the idle eye bob).
- The capture uses the field of view of 75 degrees.
- `ShowHUD` hides the HUD.

Converted to PNG with the viewpoint's name, the captures are the `--reference DIR` input of
`harness fidelity` (SSIM per viewpoint). The 2026-10-06 set is in the session scratch
(`U_hook/views`). Recapture it with the command above; never commit it.

## Native instruments (protocol 2, 2026-10-06)

| Op | Use |
|---|---|
| `ALLOC` | zeroed scratch memory in the game (FString data, fake parameters) |
| `NATIVE_CALL` | call a native function on the game thread (thiscall / stdcall / cdecl; `esp` restored from `ebp`; optional x87 result) |
| `HOOK_SET` / `HOOK_FETCH` | entry logger on a native function: the first whole instructions (5-16 bytes, no relative branches: check the bytes with `read` first) move into a stub that records the world time, `ecx`, the return address, N stack arguments and chosen memory reads (`a<i>:off:len`, `c:off:len`, `m:addr:len`, with an optional pointer hop `a<i>*pre:off:len`) |
| `EVENT_TRACE` / `EVENT_FETCH` | ProcessEvent calls of chosen UFunctions with the first parameter bytes |
| `WATCH_ABS` | absolute memory ranges recorded with every sample (after the pawn ranges) |

`FIND` also returns class default objects when the name starts with `Default__`.

### Setting up a scenario (`probe_launch lab ...`)

- `lab setup [HZ]`: tutorials off (`DisGlobalUIManager` +0x304 bits 3-4; a tutorial window pauses
  the world), the player kept alive (`DishonoredPawn.m_MinimumScriptedHealth` +0x594 = 1; damage
  is still logged), pistol, sword, Blink 2, Windblast 2, and the fixed time step (default 60).
- `lab give <Tweak>` / `lab power <Name> <level>`: the Kismet handlers `OnAddInventoryItem` /
  `OnAddPower`, called with the action's class default object (`m_pItemToAdd` +0xF8,
  `m_PowerName` +0xF8 FString in scratch memory, `m_PowerLevel` +0x104). Power levels are
  1-based (`m_CurrentLevel` 1 = `m_Levels[0]`).
- `lab equip <ItemClass> <1|2>`: `Dis_EquipItemByType` (1 right hand, 2 left hand).
- `lab fixedstep <HZ|0>`: `GUseFixedTimeStep` (0x0141B27C) and `GFixedDeltaTime` (0x012696B0,
  double) in the time update 0x009DDDF0. Every tick then advances the world by exactly 1/HZ, so
  timings are exact to a tick (real time runs slower). `-benchmark` sets the same flag but
  also drops the ini files and hangs at the front end.
- `lab npcs [radius]`, `lab hook ...`, `lab hookfetch`, `lab ncall`, `lab trace`, `lab tracefetch`.
- `measure boot` reopens the level when it is already loaded (after the player died).

Dead ends: the `DishonoredCheatManager` natives are stubs in retail (their vtable entries are
the empty 0x00459D10 / 0x005CB0C0), even after creating the manager (`CheatClass` +0x460,
`AddCheats`). In PowerShell an empty `""` argument is dropped: run `find "" Class` from bash.

### Oracle measurements (`probe_launch measure ...`)

| Command | What it reads |
|---|---|
| `gun [--shots N \| --rapid S] [--weapon crossbow]` | offset of every shot (0x00AA2B10: offset in 720-line pixels, aim point, `m_fDispersion` +0x12C), the traced pistol shot (0x00BE1560 called from 0x00C2A3E0) against `CameraCache.POV`, the reload timeline (0x00C2AA80, 0x00C23620, 0x00C28C50, 0x00C236A0) |
| `blink [at=X,Y,Z] [--yaw] [--pitch] [--mana]` | target (`m_TargetPoint` +0x60), the flight, mana (+0xA60) and its regeneration |
| `mana [--from M]` | regeneration from a written value |
| `windblast npc=ADDR [--dist D]` | the damage path 0x00B5CCD0 (damage, hit location, momentum, damage type) and the blast origin / direction (+0xA4 / +0xB0) |
| `sword npc=ADDR [--swings N] [--defend S] [--block]` | presses vs every damage-path call and health |
| `detect npc=ADDR [--dists ...] [--stances stand,crouch] [--move strafe\|sprint]` | the NPC's `PlayerAttnInfo` (`DisAttentionInfo_Complex`: accumulator +0x8C, level +0xAC, flags +0x510, LOS norm and vision debug values +0x514..+0x528) and the player's `DishonoredVisibilityComponent` (+0x68..+0x78) |
| `vis at=X,Y,Z ...` | the visibility component's light and speed values at points |
| `lean [at=X,Y,Z] [--yaw] [--hold N]` | Q then E held: `CameraCache.POV` location and rotation per frame (side / down / roll) |
| `bendtime [--frames N]` | casts Bend Time: `WorldInfo.TimeDilation` (+0x384), `BendTimeSeconds` (+0x398), the clocks, the pawn's `CustomTimeDilation`, the component's flags / start |
| `grenade [--pitch] [--hold]` | throws a grenade and samples the new `DisProjectile_Grenade` (the component tick 0x00C5A2A0 hooked with `c:0x64:4 c:0xB0:8` gives the fuse exactly) |

Results and their comparison with the port: `research/dishonored/HOOK_TRIAGE.md`.

### Draw attribution (protocol 3)

| Op / command | Use |
|---|---|
| `DRAW_CENSUS` / `census [frames]` | pixel shaders drawn over the next frames, with draw and primitive counts and CTAB names |
| `DRAW_SKIP` / `skip [0xHASH \| CTAB filter]...` | drop every draw made with those shaders (rendering only; no argument restores them) |
| `fnv <file>...` | the probe's FNV-1a 64 hash of shader blobs (e.g. `shader_maps` output), to name a live shader's material |

A capture per drawn shader with that shader skipped, compared over a screen region, tells which
draws make a feature (the `moon` glow: the sky dome, U-580). Status also reports `ps_sets`,
`ps_sets_known` and `draws`: in one session no `SetPixelShader` call matched a created shader
(every draw counted under hash 0, `measure consts` sampled nothing); a relaunch fixed it, so
check `ps_sets_known` before relying on shader filters.

## Release-time regression baseline

Run in a fresh session: `launch`, `measure boot`, `lab setup 60`, then the four scenarios. The
expected values were measured on 2026-10-06 (fixed 1/60 s step); a port change that touches
these mechanics should reproduce them (HOOK_TRIAGE.md has the port's side).

| Scenario | Command | Expected (original) |
|---|---|---|
| Movement | `measure run 0.4 0.6 at=-3400,-3150,-190 --yaw 270`, `measure jump 2` | run plateau 360, sprint 540 uu/s with the sword drawn (400 / 600 sheathed); jump launch 800, apex 106.67 uu |
| Sword fight | `lab equip DisWepMelee 1`, `lab npcs` (the doorman `Pwn_EliteGuard_MTall_2_Doorman` near (-1611, -4599, -682)), `measure sword npc=ADDR --swings 1` | one hit of 10 on 50 (`DisDamageType_FastHit_Right`), 0.30-0.35 s after the press |
| Stealth detection | `measure detect npc=<party guard Pwn_EliteGuard_MTall_1_Party near (-3010, -2195)> --dists 200,600,800,1300 --stances stand` | LOS norm = d / 3500 (inner cone); value per second = `DistanceTuning(1 - norm) + 0.1 x (raw - 0.2) / 0.3`: e.g. 600 uu: norm 0.164, 0.703 /s; 1300 uu: 0.358, 0.226 /s |
| Weapon spread | `lab equip DishonoredWepPistol 2`, `measure gun --shots 60` | r / d mean 0.5 +- 0.04, variance 0.083 +- 0.015, KS below the 5 % critical value; trace minus port angle below 0.1 deg; reload start / timed stage / load 0.283 / 0.716 / 1.483 s after the shot |

Always end with `probe_launch kill`.

## Not done yet

- **Per-tick samples.** Samples are per rendered frame (from `Present`); with the fixed time step
  each frame is one tick of known length, which covers most needs.
- **No mouse look.** Turning is done by writing `Controller.Rotation`.
- **Hooks are entry-only.** Return values and out parameters need a later function's entry
  (e.g. the pistol's trace instead of the direction written by 0x00AA2B10).
