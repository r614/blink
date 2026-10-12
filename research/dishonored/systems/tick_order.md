# One world tick, in the order the original runs it

Scope: the order in which one game tick of the original runs the world loop, the controllers and
their AI brains, the pawn (`ADishonoredNPCPawn`, `ADishonoredPlayerPawn`), its physics, its state
machines, its skeletal mesh (animation advance, notifies, pose, root motion) and the code that
reads the pose. It answers the caution at the end of `research/ARCHITECTURE.md` ("take the tick
order from the exe") before migration step 5 (animation state into the simulation).

Method (2026-10-11, static): the dumped decompile through `scripts/audit/show.py`, `grep_fn.py`,
`vtable.py`, `static_class.py`, and the exe's bytes (a Capstone disassembly of a few call sites
where the decompiler dropped the `this` register or a float argument, and byte searches for vtable
entries and indirect calls). Nothing was launched. GOG build, SHA-256 `70da9613...c6fb`.

Confidence: **CONFIRMED** = the decompile (or the disassembly) shows it; **LIKELY** = one link
inferred (usually a field or function name taken from UE3's layout); **SPECULATIVE** = not decoded
here. Vtable slots are given as `slot N (+0xOFF)`; field offsets are object-relative. Decompiled
code is described in words.

Two facts frame everything below:

- **The world tick has a variable length.** The time update `0x00650D20` (first call of the world
  tick) adds the frame time to `RealTimeSeconds`, multiplies it by `WorldInfo.TimeDilation`
  (`+0x384`), clamps it to [0.0005, 0.4] s, stores it (`+0x39C`) and advances `TimeSeconds`
  (`+0x38C`); a WorldInfo flag replaces it with a fixed `1 / rate` (or 1/30) step, which is what
  the oracle's fixed-step mode uses (INSTRUMENTATION.md). One world tick runs per engine frame.
  CONFIRMED.
- **Each actor gets `dt x CustomTimeDilation`.** While a dilation effect is active (the game's
  dilation manager `0x00781020` reports differing factors; Bend Time), the actor loops set each
  actor's `CustomTimeDilation` (`+0x100`) to the world or the player factor (actor flags `+0x128`
  bits 30 / 31) before ticking it; every actor is ticked with `dt x CustomTimeDilation`. Its components
  get the world `dt` and scale it by the owner's dilation themselves (the skeletal mesh does,
  `0x00754B90`). CONFIRMED.

---

## 1. The world tick and its tick groups

Call chain (CONFIRMED unless marked):

```
0x00632860 engine tick (the only caller of the world tick) [LIKELY name]
 -> 0x0065E0D0 UWorld::Tick body
    -> 0x00650D20 time update (dt, TimeSeconds; InTick World+0x134 = 1)
    -- not paused and TickType != PauseTick:
    -> 0x006552C0 TG_PreAsyncWork      (World+0x13C TickGroup = 0)
         Kismet: slot 92 (+0x170) UpdateOp(dt) on every game sequence (World+0x50 -> +0x9C list)
         0x00651A80 TickActors, global iterator (all levels' TickableActors, level+0x280)
         0x0093A6B0(dt, 5) Ark tick manager list +0x1C
         0x007214F0 skeletal barrier (finish every pose started in the group)
         0x0093A6B0(dt, 0) Ark tick manager list +0x00
    -> 0x00655430 TG_DuringAsyncWork   (TickGroup = 1)
         0x00805B70 start the rigid-body scene step (World+0x160, dt clamped to MaxPhysicsDeltaTime) [LIKELY name]
         0x00651DF0 TickActors, deferred actors of group 1
         0x00649CB0 deferred components of group 1
         0x007214F0 skeletal barrier
         0x0093A6B0(dt, 1) Ark list +0x38 and the time-sliced queue +0x8C (0x00936E70, 0.9 ms budget)
    -> 0x007DD010 (not decoded)
    -> 0x006554E0 TG_PostAsyncWork     (TickGroup = 2)
         0x007F1D60 fetch the rigid-body results (World+0x160) [LIKELY name]
         0x00652150 TickActors, deferred actors of group 2
         0x00649D60 deferred components of group 2
         0x007214F0 skeletal barrier
         0x0093A6B0(dt, 2) Ark list +0x54
    -- paused or PauseTick: 0x006511F0 instead of the three groups
    -> 0x0064B640 FTickableObject list (DAT_01448C58) ticks, then a pass over all actors' components [LIKELY]
    -> 0x0065DA60
         every PlayerController's camera: slot 233 (+0x3A4) Camera::UpdateCamera 0x005E5EB0
         TG_PostUpdateWork (TickGroup = 3): 0x006524D0 actors, 0x00649E10 components, 0x007214F0 barrier
         TG_EffectsUpdateWork (TickGroup = 4): 0x00652850 actors, 0x00649EC0 components, 0x007214F0 barrier
         game managers (0x00781020 slot +0x3C4, a global tickable +0x20), pending-kill cleanup
    -> 0x006555A0 end of tick (not decoded)
```

**The actor loop** (`0x00651A80` for group 0; `0x00651DF0`, `0x00652150`, `0x006524D0`,
`0x00652850` for the deferred groups; CONFIRMED):

1. Actors are visited in the order of the levels' `TickableActors` arrays (level `+0x280`, count
   `+0x284`; the iterator is `0x00645720`). The array is rebuilt from the level's `Actors` in
   array order (`0x00653F20`, skipping `bStatic` / `bTickIsDisabled`, `+0x120` bits 0 and 6);
   runtime-spawned actors are appended (LIKELY). There is **no sorting by class**: controllers are
   not ticked before pawns by the loop.
2. An actor whose `TickGroup` (`+0x109`, setter `0x00568410`) is later than the current group is
   appended to that group's list in the global deferred list `0x01448C70` (`+0x00` group 1,
   `+0x18` group 2, `+0x30` group 3, `+0x48` group 4) and ticked there, in deferral order.
3. Otherwise, if not yet ticked this frame (`+0x120` bit 4 against `World+0x138`), the actor's
   `Tick` (slot 103, `+0x19C`) runs with `dt x CustomTimeDilation`; if it returns false, the
   actor's components are skipped too. Then (also for an actor already ticked this frame, e.g. as
   another pawn's `Base`, which gets only its components here) `0x00647F20` ticks its components
   right away: each component (`Actor.Components`, `+0x44`)
   whose `TickGroup` (`+0x50`, setter `0x00569880`) is the current group or earlier gets
   `ConditionalTick` (`0x00569800`: if attached, slot 79 `+0x13C` `Tick(dt)`); a component with a
   later group goes to that group's component list (deferred list `+0x0C`, `+0x24`, `+0x3C`,
   `+0x54`).
4. Actors spawned during the loop (World `+0x128` list) are ticked with their components at the end
   of the same loop (`0x00650970`), or, in the DuringAsync group, sorted into the deferred lists
   (`0x00650B40`). So an actor spawned by a notify in tick N gets its first tick in tick N (LIKELY:
   that spawning appends to `+0x128` while `InTick` is set was not traced).

**Which actors are in which group.**
- `ADishonoredNPCPawn`: its `PreBeginPlay` (slot 131, `0x00B4AC80`) calls the parent's and then
  `SetTickGroup(TG_PreAsyncWork)`. CONFIRMED.
- Any actor in `PHYS_RigidBody` at `PreBeginPlay` moves itself to `TG_PostAsyncWork`
  (`AActor::PreBeginPlay` `0x006449D0`). CONFIRMED.
- The player pawn, the controllers and every component class keep the `TickGroup` of their
  defaults, which are cooked data, not code; nothing in the exe sets them (the only native
  `SetTickGroup` callers are listed in the appendix). **Open**, see section 7.

**The Ark tick manager** (`World+0x2C8`, built by `0x0093A5E0`): Arkane's C++ components register
a tick interface (`component + 0x10`) in list `+0x00` through the list's slot 0 (`0x0093D4F0`,
`0x0094AC70`, both register in list 0; CONFIRMED) and are ticked by `0x00939240` with
`dt x owner CustomTimeDilation`. List 0 runs at the end of `TG_PreAsyncWork`, **after** the
group's skeletal barrier. The `0x0094AC70` component's tick interface (vtable `0x010A5598`,
slot 0 `0x0094CAA0`) calls `0x0094BEC0`, which takes a displacement either from its own path
stepping (0.04 s sub-steps, arrival test `0x00944390`) or, off a path, from the owner mesh's
accumulated root motion (`Mesh+0x340`, horizontal part); it zeroes `Mesh+0x340` in both cases and
writes the pawn's `Velocity` (`+0x1B4`) as displacement / `dt` (not when Falling or RigidBody).
It is the module of `0x00943D00`, the NPC navmesh walk (movement.md): the NPC locomotion update
(LIKELY identification; registration, placement and the two branches are CONFIRMED).

---

## 2. The pawn's tick

### 2.1 The engine part (shared by both pawns)

```
0x006A1060 APawn::Tick (slot 103)
  -> if Base (+0x110) is set, a Pawn bit (+0x287 bit 0) is set, the Base has not ticked yet, is not
     static / tick-disabled and has the same TickGroup: Base->Tick(dt) first (Base only, not its components)
  -> 0x00654D10 AActor::Tick
       bTicked = World.Ticked; viewport-only ticks return early unless player-controlled
       optional tick-frequency throttle (WorldInfo +0x2B8 bit 10, actor +0x144..+0x158): may return
         false (no tick, no components) and accumulate dt
       RemoteRole (+0x105) == AutonomousProxy -> TickAuthoritative if its controller is a local
            player controller (or RigidBody / Interpolating), else only ProcessState and UpdateTimers
       else Role (+0x106) > SimulatedProxy  -> TickAuthoritative (slot 122, +0x1E8) 0x00654560
            Role == SimulatedProxy           -> TickSimulated (slot 123)
            Role == None and Falling/Rotating/Projectile/Interpolating -> performPhysics
       (Role / RemoteRole: LIKELY names of the two bytes; a standalone game's pawns take TickAuthoritative)
       TickSpecial (slot 124, +0x1F0) unless deleted
       a pending "set PHYS_None" (+0x128 bit 7): setPhysics (slot 134) [Dishonored addition]
0x00654560 AActor::TickAuthoritative
  -> 0x006F1950 ProcessState (latent script state code)
  -> 0x0064FD90 UpdateTimers
  -> LifeSpan countdown (destroy at <= 0.0001)
  -> performPhysics (slot 135, +0x21C) unless PHYS_None, deleted, or Role == AutonomousProxy
0x006A2230 APawn::performPhysics
  -> crouch / uncrouch requests (slots 285, 286; LIKELY names)
  -> 0x006A24A0 startNewPhysics(dt, 0) (slot 283): by Physics (+0x104)
       1 Walking slot 141 (+0x234)   2 Falling slot 140 (+0x230)   3 Swimming slot 280
       4 Flying slot 279   7 Interpolating slot 153   8 Spider slot 281   9 Ladder slot 282
       10 RigidBody slot 157   11 SoftBody slot 158   12 NavMeshWalking slot 142   14 Custom slot 143
  -> slot 276 (+0x450) post-move hook (NPC 0x00B4AEA0: mesh touch update)
  -> physicsRotation (slot 144, +0x240) when there is a Controller (+0x26C, LIKELY name) or a flag,
     and not RigidBody / Interpolating
  -> AvgPhysicsTime, PendingTouch
```

All CONFIRMED except where marked. Points that matter:

- **The script `event Tick` is never raised by the native actor tick.** The only users of the
  `Tick` name (`DAT_01447FA4`) are an interaction's and the skel-control paths (`0x006A5E30`,
  `0x006B9380`, `0x007DA5D0`, `0x00723560`); `TickAuthoritative` goes straight to `ProcessState`.
  CONFIRMED.
- **`TickSpecial` is not overridden** by any Dishonored pawn (slot 124 is `APawn::TickSpecial`
  `0x00642F40` -> `0x006BCC30`, a per-material timed parameter update on the mesh). The
  ARCHITECTURE.md guess "`ADishonoredNPCPawn::Tick` / `TickSpecial`" resolves to `Tick` only.
- **`APawn::Tick` pre-ticks the pawn's `Base`, not its controller.** `+0x110` is the `Base`: the
  `RMM_Relative` root-motion path (section 5) calls `SetBase` with it and with `+0x1D8`
  (`BaseSkelComponent`) and `+0x1DC` (`BaseBoneName`). LIKELY name, CONFIRMED behaviour.
- **Physics vtable slots** (CONFIRMED from `startNewPhysics`): `physWalking` is slot 141
  (`APawn` `0x006BD150`), `physFalling` slot 140 (`0x006BF0E0`, as movement.md and U-016 say),
  `physSwimming` slot 280 (`0x006B7820`), `physLadder` slot 282 (player `0x00AA82C0`), the
  player's `PHYS_Custom` slot 143 (`0x00AA1710`).

### 2.2 `ADishonoredNPCPawn::Tick` (`0x00B6DA40`)

Order, CONFIRMED (the hierarchy is `DishonoredPawn` -> `DisPossessablePawn` -> `DishonoredNPCPawn`;
the NPC's `Tick` calls `DisPossessablePawn`'s `0x00B51DC0`, which calls `DishonoredPawn`'s
`0x00B50720`):

1. Early out: a pawn bit (`+0x964` bit 13) only stamps `bTicked` and returns true (the components
   still tick).
2. `0x00B799C0` rotation intents: the highest active `m_NPCRotationIntent` sets the final desired
   rotation and rate (U-862).
3. `0x00B64480` off-screen timer; `0x00A18640` when in `PHYS_RigidBody`; `0x00B6DFB0` (mesh flag
   from `PhysicsWeight`).
4. `0x00B51DC0` -> `0x00B50720` `DishonoredPawn::Tick`:
   1. `0x006A1060` `APawn::Tick` (2.1): base, script state, timers, **physics** (NPC `physWalking`
      `0x00B4E9F0`: the navmesh walk `0x00943D00` while the locomotion component has a path, else
      `0x006BD150`; NPC `physFalling` `0x00B4E9A0`), then `physicsRotation` (NPC slot 144
      `0x00B6E970`, which copies step 2's result into `DesiredRotation` / `RotationRate`),
      `TickSpecial`.
   2. Per-tick virtuals, in this order: slot 452 (`+0x710`, NPC: none); slot 358 if flagged;
      `0x00B4DFD0` (health regeneration through slot 359 `0x00B57FD0`); slot 361 if flagged (timed
      attribute modifiers `0x00C86530`); slot 369 (NPC `0x00B80070` -> `0x00B51670`: the inventory
      tick `0x00C19A20`, which drops items and eases item aim `0x00C0B990`); slot 401 if flagged;
      slot 395 if flagged (NPC `0x00B83380`, rotation intents again).
   3. `DisPossessablePawn`: when `+0x890` is set, an update that copies the cylinder size and
      calls an object at `+0x88C` (LIKELY the possession proxy).
5. **The NPC state machine**: `0x00A6DE60` on the machine at pawn `+0xCEC` (pending transition
   first, then the current state's slot 77 `+0x134` `Tick(dt)`).
6. `0x00B7C4A0`, the tether tracker `0x00B68AD0`, timers (`+0x950` -> slot 82 `0x00B77B40`), the
   queued entries of `+0xD1C` (`0x00B6CC10`) and `+0xD28` (to the mesh, slot 106), `0x00B7CD10`
   (navmesh teleport flag), `0x00B6E640`.

Then the actor loop ticks the NPC's components (section 3).

### 2.3 `ADishonoredPlayerPawn::Tick` (`0x00ABB1E0`)

1. `0x00B50720` `DishonoredPawn::Tick`:
   1. `APawn::Tick` (2.1); the player's `performPhysics` is `0x00AA1670`: `APawn::performPhysics`,
      then `Velocity += ` a pending impulse (`+0x924`, cleared). So an impulse added during tick N
      moves the pawn from tick N+1.
   2. Slot 452 = `0x00AB0850`: `0x00ADBD00` (mantle-allowed flag, movement.md), the first-person
      view placement (`0x00AABA60`), **then the player's three state machines** (slot 509
      `0x00AA1CA0`: `0x00A6DE60` on pawn `+0xA2C`, `+0xA30`, `+0xA34`), then a pending state
      request (`+0x8A0` bit 9).
   3. The other per-tick virtuals as for the NPC (regeneration, modifiers, the inventory tick).
2. Player-only per-tick checks (crouch `0x00AB0960`, drowning, ...).

So the player's state machines run **after** the player's physics in the same tick (CONFIRMED).
That is the structural reason for movement.md's jump finding: `StatePlayerMasterJump` sets
`Velocity.Z` after that tick's physics, and the first displacement comes a tick later.

### 2.4 Controllers and the AI brain

```
0x00B633E0 DishonoredNPCController::Tick (slot 103)
  -> if the pawn (+0x248) is valid:
       pawn missing or pawn +0x508 bit 10 set (slot 294 0x00B4E800): 0x00B26720 on the brain (+0x380), 0x00936310 (+0x384)
       else: 0x00B24A10 brain tick (DishonoredAIBrain at controller +0x380)
  -> 0x00642F60 AController::Tick: bTicked; TickAuthoritative (ProcessState, timers); slot 261 if a pawn
0x00B24A10 DishonoredAIBrain tick (when active, +0x3C bit 6)
  -> timer +0x8C (reset to 1 s by 0x00B23A90)
  -> 0x00B24340 brain state change notification
  -> 0x00B0B510 count down the brain's timed entries (+0x164)
  -> 0x00B20320: 0x00B17100 stimulus queue drain and arbitration (systems/ai.md), then 0x00B28D80 on every brain process (+0x150)
  -> 0x00B17880 (0x00B00F70, 0x00B0F400, 0x00B174E0, 0x00CBA9D0(dt))
  -> 0x00B17100 arbitration again
  -> 0x00AEAAA0 on the current behaviour (+0x14C): its update (slot 96), its sub-processes (0x00B2AD80),
     its sub-state machine (0x00B2A1E0 -> 0x00A6DE60)
  -> 0x00B0C2E0 steering: sum of the steering contributions (+0xC8), normalised above 1.01, added to the
     locomotion's desired move (0x0093DFE0, +0x140)
```

CONFIRMED (the brain object is `+0x380`, from the disassembly of `0x00B633E0`; the current
behaviour `+0x14C`, from `0x00B24A10`'s disassembly).

The player side: `DishonoredPlayerController::Tick` `0x00AB6E80` -> `APlayerController::Tick`
`0x00655040`: view target, the `Interactions` (`+0x48C`, slot 74 `Tick(dt)`), then
`0x00647DD0`: the `PlayerInput` (slot 87 `+0x15C`) and **`eventPlayerMove(dt)`** (the `PlayerMove`
name at `DAT_01447CB4`; movement.md's `PlayerMove_Walking` `0x005D3A00` sets the pawn's
`Acceleration`), then `ProcessState`, timers, `TickSpecial`. CONFIRMED.

**Controller vs pawn.** Controllers are ordinary actors in the same loop. Nothing in the engine
ticks a controller before its pawn (`APawn::Tick` pre-ticks only the `Base`), so within a group the
order is the `TickableActors` order, i.e. spawn order. NPC controllers and the player's are spawned
and possessed by UnrealScript (`Possess` and `SpawnDefaultController` have no native caller in the
exe), so the order cannot be read here. UE3's stock scripts spawn an AI controller after its pawn
and the player controller (at login) before its pawn; if Dishonored keeps that, the NPC's brain runs
after the NPC pawn (its decisions reach the pawn's physics and state machine the next tick) and the
player's input reaches the player pawn's physics the same tick (movement.md assumes the latter).
SPECULATIVE until the spawn order or the controllers' `TickGroup` is read from data or a capture.

---

## 3. The skeletal mesh component: animation advance, pose, `SpaceBases`

```
0x00647F20 TickActorComponents (or a deferred-components pass) -> 0x00569800 ConditionalTick
 -> 0x007567B0 USkeletalMeshComponent::Tick (slot 79) [all Dishonored skeletal components inherit it]
     -> 0x00754B90 tick body (dt x owner CustomTimeDilation)
          bRecentlyRendered (+0x308 bit 1) = LastRenderTime (+0x1C0) > TimeSeconds - 1
          if no "no skeleton update" flag (+0x2C0, or the parent anim component's), attached, Animations (+0x1E0):
            0x007124C0 TickAnimNodes, only if bRecentlyRendered or +0x2F8 bit 4
               TickTag (+0x2E0) += 1
               AnimTree: slot 120 (+0x1E0) 0x0059C0D0 (dt, weight 1)
                 for every node of AnimTickArray (+0x1A8) in order, with its weight from the parents:
                   weight > 1e-5: slot 94 if newly relevant, NodeTotalWeight (+0x70), slot 81 when
                                  the init tag changed, TickTag, TickAnim (slot 85, +0x154)
                   weight 0 and was relevant: slot 95
                   (slots 94 / 95 / 81: UE3's OnBecomeRelevant / OnCeaseRelevant / InitAnim, LIKELY)
                   e.g. ArkAnimNodeLocomotion::TickAnim 0x0091B280 (ANIMATION.md 3),
                        AnimNodeSequence::TickAnim 0x0059D6A0, AnimNodeBlendBase 0x00592FA0,
                        AnimNodeSlot 0x005B7BB0, DisAnimNodeSlot 0x00A9A900
                 then a second node list (+0x1B4): TickAnim for each node whose TickTag is stale
                 then 0x0059B3A0: the sync groups (AnimGroups +0xE0) advance their sequences
            0x00717FC0 TickSkelControls
          if no "no skeleton update" flag and (bRecentlyRendered or +0x2F8 bit 2) and not
          (+0x2B8 and +0x2BC), or a physics query (slot 170) asks for it; and no ParentAnimComponent:
            0x00754160 UpdateSkelPose: starts the pose evaluation (+0x410 bits 4 and 3 set)
     -> append the component to the pending-pose list (0x014496B4)
0x007214F0 skeletal barrier (end of every tick group), until the list is empty:
   for each pending component whose pose job is done (0x00955D00): remove it, and if it is attached
   and its owner is alive, slot 176 (+0x2C0) FinishUpdate(dt)
   (a sweep that finds no finished job sleeps 0.1 ms and retries)
0x00756CA0 USkeletalMeshComponent FinishUpdate (player and item components: 0x00B62270, which calls it first)
  -> 0x007471D0 if a pose was started:
       wait for the job (0x00955D50), copy its bone atoms into LocalAtoms (+0x214) for the required
       bones (+0x23C), read the root bone's atom as RootMotionDelta (job +0x80 rotation, +0x90
       translation, +0xA0 bHasRootMotion = atom w > 0) and copy it to the component (+0x420..+0x43C)
       0x00746280 root motion (section 5)
       0x00725EA0 FaceFX (facefx.rs), slot 178
       0x00731270 compose: SpaceBases (+0x208) from LocalAtoms, with the skel controls  [only caller]
       physics bodies from the new pose (0x007B78F0) when the mesh has kinematic bodies
  -> transform / bounds update (0x00569790 or slot 80)
  -> if PhysicsWeight (+0x1F8) == 0 and the owner is a Pawn: 0x006BCF70 double-buffered copy of
     some bones' SpaceBases into the pawn (+0x47C / +0x488, flip +0x494) [LIKELY: ragdoll velocities]
```

All CONFIRMED except the names in brackets. Consequences:

- **Animation advances after the pawn's physics, in the same tick.** The component ticks after the
  owner's whole `Tick` (physics, state machines, inventory) in the actor loop, or later if its
  `TickGroup` is later (section 7). It advances by the same tick's `dt`.
- **The pose is evaluated in the same tick, but only published at the group's barrier.**
  `UpdateSkelPose` starts the evaluation (an Edge job, `0x00956C60`, when the mesh has Edge data,
  `SkeletalMesh+0xE4 > 0` and `+0x2F8` bit 19 clear; otherwise the tree's `GetBoneAtoms`, slot 90
  `+0x168`, synchronously). `SpaceBases` are written only by `0x00731270` from `0x007471D0`, i.e.
  at the barrier ending the mesh's tick group (or when something forces a synchronous update,
  `0x00754EF0`, which also waits for a pending job).
- **So a gameplay read of the pose in tick N sees:** tick N-1's pose if it runs before the barrier
  of the mesh's group (the owner's own `Tick`, its state machines and item contexts ticked from
  it, any actor of the same group, including actors later in the list); tick N's pose after it
  (later groups, the Ark list 0 locomotion, the camera update, `TG_PostUpdateWork` components).
- **Not every tick evaluates a pose.** Without `+0x2F8` bit 4 (`bTickAnimNodesWhenNotRendered`,
  LIKELY name) a mesh not rendered for 1 s does not advance at all (no time, no notifies); without
  bit 2 (`bUpdateSkelWhenNotRendered`, LIKELY) it keeps its old pose. A mesh with an update-rate
  divisor (`+0x238`) re-evaluates only every N-th frame, but never skips while root motion is in
  use (it skips only when `RootMotionMode` and the previous mode are `RMM_Ignore` and the rotation
  mode is 0). A ragdoll at full physics weight stops evaluating after 5 frames (`+0x2F4`). A forced
  ref pose (`+0x2B8`, LIKELY `bForceRefpose`) skips the update from its second frame. CONFIRMED
  mechanics; which NPC meshes set these flags is data.

---

## 4. Notifies

```
0x0059D6A0 AnimNodeSequence::TickAnim
  not in a sync group: PreviousTime = CurrentTime; if playing:
    0x00598440 AdvanceBy(MoveDelta = SkelComp.GlobalAnimRateScale x AnimSeq.RateScale x Rate x dt, dt,
                         bFireNotifies = component +0x2F8 bit 19 clear)
  in a sync group: register with the group (master / notify master by weight); advanced later by
    0x0059B3A0, where only the group's notify master fires notifies
0x00598440 AdvanceBy
  -> if not bNoNotifies (+0xD8 bit 10) and bFireNotifies, and (synced and relevant, or
     NodeTotalWeight >= NotifyWeightThreshold +0xEC): IssueNotifies(MoveDelta) (slot 121, +0x1E4)
  -> then CurrentTime (+0xDC) += MoveDelta; loop or stop at the end (end callbacks slots 122 / 123)
slot 121 on DishonoredAnimNodeSeq (and its Attack / Mantle / Player / Versus subclasses): 0x00A9BD80
  -> 0x00A9BC90 the active duration notifies (+0x114, {notify, remaining, duration}): remaining -= dt,
     notify slot 77 (+0x134) Tick(seq, elapsed before, elapsed after, duration); at remaining <= 0
     slot 76 (+0x130) End(seq) and removal
  -> 0x00599A50 UAnimNodeSequence::IssueNotifies: every notify whose Time is in [CurrentTime,
     CurrentTime + MoveDelta) (wrapping when looping): slot 118 (+0x1D8) of the node
       DishonoredAnimNodeSeq 0x00A8F340: the base handler 0x0058C610 (the notify's Notify, slot 73
       +0x124), then for a notify with a duration: Tick(seq, 0, elapsed, duration) and either End
       at once (window inside the step) or entry into the active list
```

CONFIRMED. Notify handlers by class (from their vtables):

| Notify | Handlers | Called from |
|---|---|---|
| `DishonoredNotify_AttackZone` | start: slot 73 `0x00A8E5A0`; every step of the window: slot 77 `0x00A8E760` (the "tick handler" of tuning.rs); end: slot 76 `0x00A8E710` | start and first step `0x00A8F340`; later steps and end `0x00A9BC90` |
| `DishonoredNotify_Ragdoll` | slot 73 `0x00A8CFE0` | the crossing (`0x00A8F340` -> `0x0058C610`) |
| `DisNotify_DropItem` | slot 73 `0x00A8D400` | the crossing |
| `DishonoredNotify_SeverLimb` | slot 73 `0x00A966D0` | the crossing |

Consequences:

- **Notifies fire during the animation advance of the tick whose step crosses them, before that
  tick's pose is evaluated**, and before `CurrentTime` is updated (a handler reading the node's
  time sees the start of the step). The window is **half-open, `[t0, t1)`**: a notify exactly at
  the step's start fires (a clip started at 0 fires its time-0 notifies on its first advance); one
  exactly at `t1` fires on the next step. CONFIRMED (`0x00599A50`: first candidate is the nearest
  notify with `Time - CurrentTime >= 0`, it fires while `MoveDelta - (Time - CurrentTime) > 0`).
- **The melee hit reads the notify state after the pose.** The attack zone's tick handler opens
  the versus and damage zones from inside the advance (tuning.rs, melee.md); the sweep is the
  melee item context's slot 84 (`+0x150`) `0x00C13640`, which takes no `dt` and is reached by:
  - for the player: `0x0065DA60` camera update -> `Camera::UpdateCamera` `0x005E5EB0` ->
    `DishonoredPlayerCamera` slot 239 `0x00AD80A0` -> `0x00ABA860` -> pawn `0x00B49F50` ->
    inventory `0x00C17C90` -> `0x00C16E30` -> `0x00C061F0` (each running context of each item)
    -> slot 84. CONFIRMED. This is after `TG_PostAsyncWork`, so the sweep uses tick N's notify
    state and (if the arms mesh ticks in an earlier group) tick N's pose.
  - for other owners: the item's `DishonoredInventoryItemPostUpdateTickComponent` (`Tick` slot 79
    `0x00C0D030` -> `0x00C061F0` -> slot 84; skipped when a byte at `+0x10A` of the actor it
    resolves is `0x24`, presumably the player, who has the camera path). Its `TickGroup` is data;
    the class name says `TG_PostUpdateWork` (LIKELY). Same conclusion.

---

## 5. Root motion

- **Extraction (CONFIRMED):** the root bone's atom of the evaluated pose is the root motion of
  the tick: the Edge job's output is read in `0x007471D0` (root bone index from the mesh,
  `SkeletalMesh+0xE0 -> +0x10`); the synchronous path gets it from the tree's `GetBoneAtoms`
  (slot 90, out parameters `RootMotionDelta` and `bHasRootMotion` at job `+0x80` / `+0xA0`). Both
  are handed to `0x00746280` at the barrier, i.e. in the same tick as the advance, after the
  pawn's physics. Where `AnimNodeSequence.RootBoneOption` / `RootRotationOption` are applied
  inside the evaluation was not traced (open).
- **`0x00746280`** (field names LIKELY, UE3 layout; mode numbers CONFIRMED):
  1. A pawn with a flag (`+0x284` bit 25, LIKELY `bRootMotionFromInterpCurve`; the native
     `SetRootMotionInterpCurrentTime` exists) and this mesh: the pawn's root-motion curve
     (`+0x454..+0x478`, advanced by its rate x `dt`) replaces the delta.
  2. The pending mode (`+0x386`) becomes `RootMotionMode` (`+0x384`), except that a switch to
     mode 2 (`RMM_Ignore`) while the pose has root motion is delayed by one tick (`+0x388`).
  3. No root motion or mode 2: the accumulated delta (`+0x340`) and the velocity (`+0x350`) are
     zeroed. Otherwise the delta is rotated into world space by the mesh's `LocalToWorld`, scaled
     by `+0x368..+0x370` (`RootMotionAccelScale`), **added** to `+0x340` and divided by `dt` into
     `+0x350`.
  4. Rotation (`+0x38C` == 1, `RMRM_RotateActor`): a world-space quaternion delta (`+0x330`).
  5. Mode 0 (`RMM_Translate`) with the previous mode (`+0x385`) also 0: **the owner is moved by
     the delta, with the new rotation, right now** (`0x0064CCD0`, LIKELY `UWorld::MoveActor`), the
     accumulated delta is zeroed, and a pawn owner gets slot 232 with `(Rotation, 0, 0, -1, 1)`
     (LIKELY `SetDesiredRotation`) so that its `physicsRotation` keeps the turn; the NPC pawn's
     slot 232 (`0x00B6E960`) returns 1 and does nothing. A switch from 0 to 2 applies the rotation
     once with a zero move.
  6. Mode 4 (`RMM_Relative`): the same move made relative to the owner's `Base` (`SetBase(None)`,
     move, `SetBase(old base, ...)`).
  7. The other modes (1, 3) leave the delta in `+0x340` for the movement code.
- **When it moves the pawn:**

| Mode | Applied by | Tick |
|---|---|---|
| 0 `RMM_Translate` (+ `RMRM_RotateActor`) | `0x00746280` at the barrier, `MoveActor` | same tick N, after the pawn's physics and state machines |
| 4 `RMM_Relative` | same, around a `SetBase` | same tick N |
| 1 / 3 | `physFalling` `0x006BF0E0` reads and zeroes `Mesh+0x340` (mode 3 as `Velocity = delta / dt`) | next tick N+1 (physics runs before the components) |
| NPC locomotion, off a path | Ark list 0 `0x0094BEC0`: `Velocity` = accumulated root motion / `dt`, `Mesh+0x340` zeroed; NPC `physWalking` integrates it | velocity set at the end of tick N's PreAsync group (after the barrier), position moves in tick N+1 |
| NPC locomotion, on a path | same function: `Velocity` from the path step; the mesh's root motion is discarded, except while the component's start / stop / turn sequence is fully in (states 4, 8, 13, 14: the off-path branch's displacement; `dishonored_sim::npc_loco`) | as above |

  The rows are CONFIRMED as code; the mode names are the cooked enum's (`ERootMotionMode`: 0
  `RMM_Translate`, 1 `RMM_Velocity`, 2 `RMM_Ignore`, 3 `RMM_Accel`, 4 `RMM_Relative`, 5 `RMM_MAX`: there
  is no `RMM_Interpolate` and no mode 5; 2026-10-11, U-977). In the pawn physics only `physFalling`
  reads the mesh's mode (1 and 3); `physWalking` and `CalcVelocity` do not, so while walking a
  pawn's root motion moves it only through mode 0 (same tick) or, for NPCs, through the Ark
  locomotion's velocity (next tick). The modes an NPC's chains set (`m_RootMotionMode` /
  `m_RootRotationMode` through pawn slot 469 `0x00B6E680`; restored to (1, `RMRM_RotateActor`) by slot
  470 `0x00B71280`) and the NPC's spawn modes (`Default__DishonoredNPCPawn.AnimMesh`: `RMM_Velocity`,
  `RMRM_Ignore`) are applied by `dishonored_sim::pawn_states`; the cooked
  `Engine.Default__SkeletalMeshComponent` ticks in `TG_PreAsyncWork` (section 7).

---

## 6. The pawn state machines and animation requests

- **Where they tick (CONFIRMED):** `DishonoredNativeStateMachine` tick `0x00A6DE60` (a pending
  transition is resolved first, then the current state's slot 77 `+0x134` `Tick(dt)`, e.g.
  `StatePlayerMasterChoke` `0x00A71100`, `StateNPCMasterThrown` `0x00A98DD0`,
  `StateNPCMasterDead_Limp` `0x00A9CFC0`; the action states share `0x00A60FA0`, which pushes the
  item context's rotation intents through `0x00B71570` / `0x00B71600`). It is called from:
  - the NPC's `Tick` (2.2 step 5), on pawn `+0xCEC`, after the NPC's physics;
  - the player's slot 509 (2.3), on pawn `+0xA2C`, `+0xA30`, `+0xA34`, after the player's physics;
  - an AI sub-state machine inside the brain's behaviour update (`0x00B2A1E0`), i.e. inside the
    controller's tick.
- **When an animation requested by a state in tick N plays:** the state machine ticks inside the
  pawn's `Tick`, and the pawn's skeletal component ticks after that `Tick`, so a slot request made
  there is advanced in tick N's `TickAnimNodes` (the new sequence's first step is `[start,
  start + rate x dt)`, its blend weight starts moving) and is part of tick N's pose at the barrier;
  the pawn's own code sees it in `SpaceBases` from tick N+1. The ordering is CONFIRMED; that the
  slot request itself (`DisAnimNodeSlot`, the chain player) takes effect on the next `TickAnim`
  without a deferral of its own was not traced (LIKELY).
- **Requests from the AI** (behaviours, the ambient idles `0x00B39620`, brain processes) are made
  in the controller's tick. They reach the pawn's animation in the same tick if the controller
  ticks before the pawn's skeletal component, else in tick N+1 (section 2.4, SPECULATIVE order).
- **Rotation:** the item contexts' intents pushed in tick N are read by `0x00B799C0` at the start of
  tick N+1's NPC `Tick` and applied by that tick's `physicsRotation`. CONFIRMED.

---

## What runs when in one tick

For an NPC pawn and the player pawn in `TG_PreAsyncWork`, their meshes assumed in the same group
(section 7).

| # | Phase | What runs | Functions | Confidence |
|---|---|---|---|---|
| 0 | Time | `dt` = frame time x `TimeDilation`, clamped [0.0005, 0.4]; `TimeSeconds += dt` | `0x00650D20` | CONFIRMED |
| 1 | Kismet | every game sequence's `UpdateOp(dt)`: events raised by actors in tick N run here in tick N+1 | `0x006552C0` slot 92 | CONFIRMED |
| 2 | PreAsync actor loop | actors in `TickableActors` order; later-group actors deferred | `0x00651A80` | CONFIRMED |
| 2a | player controller (if before its pawn) | interactions, `PlayerInput`, `PlayerMove` (pawn `Acceleration`), script state | `0x00655040`, `0x00647DD0` | CONFIRMED (position: SPECULATIVE) |
| 2b | NPC pawn `Tick` | rotation intents; base first; script state; timers; physics (navmesh walk / `physWalking` / `physFalling`) with the `Velocity` set by tick N-1's locomotion; `physicsRotation`; per-tick virtuals (regen, inventory); **state machine**; tether, timers, queues | `0x00B6DA40` | CONFIRMED |
| 2b' | player pawn `Tick` | base; script state; timers; physics; pending impulse into `Velocity`; mantle flag; **three state machines**; inventory; player checks | `0x00ABB1E0` | CONFIRMED |
| 2c | that pawn's components | skeletal: **time advance and notifies** (attack zone opens, drop, sever, ragdoll), skel controls, pose evaluation started | `0x007567B0` | CONFIRMED |
| 2d | NPC controller (wherever it is in the list) | brain: timers, stimulus arbitration, processes, behaviour update (sub-state machines), steering into the locomotion | `0x00B633E0`, `0x00B24A10` | CONFIRMED (position: SPECULATIVE) |
| 2e | actors spawned during the loop | their first tick, with components | `0x00650970` | LIKELY |
| 3 | Ark list 5 | | `0x0093A6B0(5)` | CONFIRMED |
| 4 | Pose barrier | jobs finished: `LocalAtoms`, **root motion** (`RMM_Translate`: `MoveActor` now, with `RMRM_RotateActor`; other modes accumulated), FaceFX, **`SpaceBases`** | `0x007214F0`, `0x00756CA0`, `0x007471D0`, `0x00746280`, `0x00731270` | CONFIRMED |
| 5 | Ark list 0 | NPC locomotion: `Velocity` from the path step or, off a path, from this tick's accumulated root motion (used by tick N+1's physics); the mesh's accumulation zeroed | `0x00939240` -> `0x0094CAA0` -> `0x0094BEC0` | CONFIRMED (identity LIKELY) |
| 6 | DuringAsync | rigid-body step started; group-1 actors and deferred components; barrier; Ark list 1 | `0x00655430` | CONFIRMED |
| 7 | PostAsync | rigid-body results; group-2 actors (ragdolls, `PHYS_RigidBody` actors) and components; barrier; Ark list 2 | `0x006554E0` | CONFIRMED |
| 8 | Tickables | `FTickableObject` list; component updates | `0x0064B640` | LIKELY |
| 9 | Cameras | `UpdateCamera`; for the player: view target, then **the player's item contexts** (melee sweep with tick N's notify state and pose) | `0x005E5EB0`, `0x00AD80A0`, `0x00C061F0` | CONFIRMED |
| 10 | PostUpdateWork | group-3 actors and components (NPC item contexts through the item's post-update component, the mantle edge finder); barrier | `0x006524D0`, `0x00C0D030` | CONFIRMED (group of the component: LIKELY) |
| 11 | EffectsUpdateWork | group-4 actors and components; barrier | `0x00652850` | CONFIRMED |
| 12 | End | managers, pending kills | `0x0065DA60` tail, `0x006555A0` | CONFIRMED / not decoded |

---

## Consequences for the port

What the simulation's step order must be to match (no implementation here):

1. **Kismet first.** Run `UpdateOp` before the actors: an event raised by gameplay in step N is
   executed in step N+1 (this settles kismet.md K-1's UNVERIFIED in the direction it feared: the
   port, which ticked Kismet after `MissionState::step`, ran those events one tick early; done
   2026-10-11: `kismet_host::update` opens `MissionMatch::step`, `kismet_host::raise` feeds the
   step's events at its end).
2. **Per pawn: physics, then states, then animation.** For each pawn: (NPC) apply last tick's
   rotation intents; integrate physics with the velocity / acceleration as they stand (the NPC's
   velocity comes from last tick's locomotion step); rotate; run the pawn's state machines (player:
   three, NPC: one) and the inventory; **then** advance that pawn's animation by the same `DT`,
   firing notifies in `[t0, t1)` during the advance (handlers see `t0` and the previous pose);
   then evaluate the pose. A state's request made in the step plays in the same step's advance.
3. **Publish the pose and root motion at the end of the pawn group.** After all pawns of the group
   have advanced: compose `SpaceBases`; apply `RMM_Translate` root motion (and `RMRM_RotateActor`)
   to the pawn's location immediately (same step, after its physics; so the step's end position
   includes this step's root motion), accumulate the other modes for the next step's physics.
   Anything that reads bones inside the pawn step must read the previous step's pose.
4. **Then the NPC locomotion step** (on a path: the path step; off a path: this step's root motion
   turned into a velocity), whose velocity the next step's physics integrates.
5. **Controllers and brains are not "first".** Their place is the spawn order (open). Until it is
   known, the reproducible choice that matches UE3's stock spawning is: player controller before
   the player pawn (input moves the pawn the same step), NPC brain after the NPC pawn (AI decisions
   act from the next step). This replaces ARCHITECTURE.md's "controllers, pawn physics,
   components".
6. **Melee and other bone readers after the pose.** The melee sweep runs after the PostAsync
   group with the step's notify state and pose: in the port, after step 3.
7. **Variable `dt` vs fixed `DT`.** The original's tick length is the frame time; the port's fixed
   `DT` corresponds to the oracle's fixed-step mode only. Per-actor dilation (Bend Time) scales the
   pawn's `dt` and, through the owner, its animation's.
8. Off-screen NPCs may not animate (section 3, data-dependent): notifies and root motion of an NPC
   not rendered for a second do not happen unless its mesh asks to tick when not rendered.

---

## 7. Open

- **Default `TickGroup` of the skeletal mesh components (NPC and player), the controllers and the
  player pawn.** Data, not code. If the NPC mesh ticks in `TG_PreAsyncWork` (its pawn's group),
  the order above holds; if in `TG_DuringAsyncWork` (the stock UE3 `ActorComponent` default as
  far as remembered, SPECULATIVE for this build), every
  PreAsync actor (all pawns, all brains) and the Ark list 0 locomotion run before any animation,
  and the locomotion consumes the previous tick's root motion. Two hints favour PreAsync (LIKELY,
  not proof): Arkane put the locomotion list after the PreAsync barrier, and the kinematic-bone
  update that the pose finish calls (`0x007B78F0` -> `0x007B6F60`) warns and does nothing while
  `World.TickGroup` is `TG_DuringAsyncWork`. Read
  `TickGroup` (`+0x50`) of the cooked `Default__SkeletalMeshComponent`, of the NPC and player
  archetypes' `Mesh`, and of `DishonoredInventoryItemPostUpdateTickComponent`; read the actors'
  `TickGroup` (`+0x109`) of the controllers and the player pawn. Partly read (2026-10-11, the
  cooked packages): `Engine.Default__SkeletalMeshComponent` has `TickGroup` `TG_PreAsyncWork`,
  `bTickAnimNodesWhenNotRendered` and `bUpdateSkelWhenNotRendered` true, and
  `Default__DishonoredNPCPawn.AnimMesh` overrides no `TickGroup`: the NPC mesh ticks in its pawn's
  group, the order above. The player archetype's mesh, the controllers and the player pawn are
  still to read.
- **Spawn order of controllers vs pawns** (UnrealScript): decides whether the brain and the player
  input act in the same tick. A capture recording the `TickableActors` order (or the order of
  `Tick` calls) settles it.
- `RootBoneOption` / `RootRotationOption` handling in the evaluation (and whether the locomotion
  node's specific-anim child extracts root motion); the exact class of the two Ark components
  (`0x0093D4F0`, `0x0094AC70`). There is no mode 5 (section 5).
- `0x007DD010` (between DuringAsync and PostAsync) and `0x006555A0` (end of tick) were not decoded.
- The tick-frequency throttle (WorldInfo `+0x2B8` bit 10) and which actors carry a frequency.

## Appendix: native `SetTickGroup` callers

Actor (`0x00568410`): `AActor::PreBeginPlay` `0x006449D0` (rigid body -> 2), NPC `PreBeginPlay`
`0x00B4AC80` (-> 0), `0x00C71B70` / `0x00C81260` (physics-state switches of one class: take
another actor's group / 0; class not identified), `0x00C7AF60`. Component (`0x00569880`): light environments `0x004DB9C0` / `0x005041A0` (-> 3 on a
flag), `AnimNotify_Trails` `0x00751AE0` (-> 3), emitters `0x0089D3F0` / `0x0089D420` /
`0x008BE4D0`. None for skeletal meshes, pawns' meshes or controllers.
