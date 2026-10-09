# Movable props: pick up, carry, drop, throw

Scope: the `DishonoredMovable` physics props (bottles, glasses, pans, crates, vases, the
candlesticks) that Corvo can take in his hands, hold in front of him, drop and throw, and what
the original does with them when they strike something. Bodies (`StatePlayerCarryCorpseIdle`,
INVENTORY.md "Bodies") are a different state and are not touched here. Evidence levels:
CONFIRMED (decompiled exe / game data), LIKELY (the decompile says it, the reading of a field is a
name or a shape), OPEN (not decoded, not implemented). Addresses are in the GOG exe
(sha256 `70da9613...`). Port: `crates/dishonored_sim/src/movables.rs`.

## 1. Classes and data

| thing | what |
|---|---|
| actor | `DishonoredMovable` (`DishonoredBreakable` < `DishonoredKActor` < `KActor`), `DisDLC05_Movable`; `m_pMovableTweaks` (+0x418) `DisTweaks_Movable`, `m_pMovableComponent` (+0x400) `DisMovableComponent`, `m_Damage` (+0x404) |
| tweak | `DisTweaks_Movable` < `DisTweaks_StaticBreakable`: `m_MovableParams` {`m_WeightClass` Tiny / Small / Medium / Large (default Tiny), `m_Damage` (default 5)}, `m_bBlocksPlayer`, `m_bInteractable`, `m_bTraceForMeleeEvenWhenAttached`, `m_bIsKeyItem`, `m_bNeverBlocksPlayer`, `m_pGrabSound`; inherited `m_Health`, `m_DamageThreshold`, `m_Steps`, `m_fMinSpeedToCauseDamage` (250), `m_fMinSpeedToBeDamaged` (100), `m_fMinSpeedToBeDamagedOnDrop` (800), `m_pInteractableTweaks` (`m_InteractText` "`GBA_Use` Carry", `m_fMaxDistance` 200); `DisTweaks_KActor.m_pContactTypeOverride` |
| hands | `Twk_EmptyHands` (`DisTweaks_ItemEmpty`): `m_fInitialGrabTime` 0.066, `m_fMovableHoldDist` -100, `m_fShowMovableHeight_Tiny/Small/Medium/Large` 75 / 90 / 90 / 90, `m_bDoPhysicsWhenHoldingMovable` false (class default) |
| component | `DefaultItem.ini [DisMovableComponent]` `m_fMaxPawnSpeed_Tiny/Small/Medium/Large` 1.0 each, `m_fRadiusChannelReset` 200, `m_fCollisionSpeedToBreakWhenHeld` (unset) |
| pawn | `Attribute_ThrowStrength` 2000 (`Twk_Pawn_Corvo` attributes) |

A map's placed movables are read by `dishonored_assets::world_actors` (`MovableDef`): the tweak
(set or inherited), weight, damage, flags, speeds, contact type, prompt, the mesh and its local
box (`StaticMesh.Bounds` = the component's `GetBoundingBox`, interface slot 5, 0x00A46820).
Boyle Int places 36 (`DishonoredMovable_0 .. _115`): 19 interactable (the bottles, glasses), the
rest candlesticks, vases, a piano lid and ropes. Which ones are interactable is the tweak:
`movable_twk.bottle_01_twk`, `pub_bottle_0x_twk`, `DrinkingGlass_twk` have `m_bInteractable`.

## 2. Rules

M-1 **What can be picked up** (CONFIRMED data, LIKELY focus). Use (`m_bUseButton`, handler
0x00AD5BA0) on the focused interactable: a `DishonoredMovable` with `m_bInteractable` and not
`m_bInteractDisabled` (+0x420 bit 1), within its `m_fMaxDistance` (200). The port uses the
pickups' crosshair cone (30 degrees) and reach (`max_distance` + the pawn radius), the line check
on the crosshair channel, and takes the body or the prop nearer the crosshair when both are in
view. A negative `m_Damage` makes the prop inert (the collision handler returns, 0x00A40CF0).
Other classes (`DisWhaleOilBattery` is `PHYS_RigidBody` too) have their own interactions.
The prompt is the interactable tweak's `m_InteractText`, "`GBA_Use` Carry".

M-2 **Grab** (CONFIRMED). `StatePlayerGrabMovable` (vtable 0x011026C8, a `StatePlayerAction`,
`m_pPending_Movable` +0x70, `m_bThrowOnDrop` +0x74 bit 0) enters through 0x00A6FEF0 / 0x00A698C0
-> 0x00A69860, which (for the right hand, `pawn.+0xBD == 1`) finds the empty-hands item
(`DishonoredItemEmpty`, class 0x01471980) and calls `0x00C3D030(movable)`: if nothing is held it
stores the component in `m_pGrabbed_Movable` (+0x114), sets `m_bInitialGrab` (+0x118 bit 0), zeroes
`m_fGrabTime` (+0x11C) and calls `0x00A45FD0`, the component's pick-up: it keeps the holder in
`m_pHeldBy` (+0x80), copies its collision flags, and fires `DisSeqEvent_MovablePickedUp` through
`FireGeneratedEvents(prop, DAT_0146736C, prop, holder, 0, 0)`: the prop's events, originator the
PROP, instigator the pawn, every output. (KISMET.md of 2026-10-08 had the two the other way
round.) With the grab, the right-hand item is the empty hands, so the sword is out of use.

M-3 **Hold position** (CONFIRMED formula, LIKELY consumers). The empty hands' post-update tick
(0x00C4C510) calls 0x00C4C190 with the camera's location and rotation: with the prop's box
(`GetBoundingBox`, local) and weight class `w`

    D = box.min.x + box.max.x - m_fMovableHoldDist            (100 + 2 x box centre x)
    H = box.min.z + box.max.z + 70 - m_fShowMovableHeight[w]  (-5 for Tiny, -20 else, + 2 x centre z)
    location = eye + forward x D - up x H        rotation = the camera's rotation

(the axes are the rows of the rotation matrix, 0x00402BC0), handed to 0x00A4A2B0, which moves
the actor (through a `MoveActor` sweep when the component has no physics handle, else through the
handle). During `m_fInitialGrabTime` (0.066 s; the item's tick 0x00C4C440 adds the frame time to
`m_fGrabTime` and ends the flag at the limit, then 0x00A46330 sets the actor's collision channel
to 2) the location and rotation are the linear mix from `m_Grab_Start_Pos` / `_Rot` (+0x120 / +0x12C,
the state at the grab) by `m_fGrabTime / m_fInitialGrabTime` (0x00AC2990). Port: `movables::hold_target`
and `step_held` (the prop is kinematic; the held prop is not swept against the world: OPEN, the
`MoveActor` branch would stop it at walls).

M-4 **Carrying** (CONFIRMED for what is absent). No dedicated carry animation of the player
exists: the Startup AnimSets `Ply_Empty_*`, `Ply_Generic`, `Ply_Sword_*` hold no clip named
Movable, Grab, Throw, Hold or Pickup (only `Ply_Empty_CarryCorpse_as`, bodies). The arms stay on
the empty-hand idle and locomotion while the prop hangs in front. Ground speed:
`DisMovableComponent.m_fMaxPawnSpeed_<weight>` (retail 1.0 each; the reader was not located, the
port multiplies the ground speeds by it, so retail values change nothing). Jump height, powers:
no restriction found (OPEN). Damage taken drops the prop (0x00AB59F0 calls slot 381 with 0); so
do entering a dialog (0x00A73720), the store (0x00A75E40), a minigame (0x00A7AE90), the soiree
state (0x00A60480, 0x00A730B0), 0x00AA4D40 and the prop's own break hook (0x00A45D40, a
`DishonoredMovable` whose break step arrives); and 0x00C4C440 drops it when the actor is being
destroyed.

M-5 **Drop** (CONFIRMED). Use while holding: 0x00AD5BA0 asks the pawn (slot 382, `IsHolding`
0x00B519F0 -> `m_pGrabbed_Movable != 0`) and calls slot 381 (`SetThrowOnDrop`, 0x00AAD900) with 0:
it stores the flag in the grab state and requests the transition out (0x00A62750 -> slot 92);
the state's exit (0x00A77540 -> 0x00A698E0) calls the item's release `0x00C45340(throw =
m_bThrowOnDrop)`:
 1. a box trace from the pawn (the player's view offset 0x00AABA60) to the prop's box centre
    with the box's half extents (0x00BE1560, mask 0x20DF); on a hit the prop is moved to the hit
    location (0x00A4A2B0): geometry between the hands and the prop never keeps it;
 2. the component release 0x00A46380: physics back to `PHYS_RigidBody` (10), `m_pHeldBy_Old` =
    the holder (collision with him is off until the prop is `m_fRadiusChannelReset` away,
    0x00A465E0), and `DisSeqEvent_MovableDropped` (`DAT_01467368`, originator the prop,
    instigator the pawn, every output) for a drop and a throw alike;
 3. the prop's `Instigator` (+0x15C) becomes the pawn (0x00A4ADA0), and the "player deserves
    blame" flag (`DishonoredKActor` +0x354 bit 0, and +0x2B4 bit 0 of the instigator's record) is
    set from whether the instigator is the local player.
A drop gives the prop no velocity.

M-6 **Throw** (CONFIRMED rule, LIKELY units). Primary (button mask 4) with the empty hands
equipped and a prop held: 0x00AB4240 -> slot 381 with 1. The release adds the throw: the pawn's
aim direction (slot 250) as a unit vector times `Attribute_ThrowStrength` (`FUN_00C86A50(0x0145E1B8)`,
2000) is handed to the prop's movable interface (slot 11, via 0x00A46740, which first stores the
prop's location in `m_ThrowFromLocation` +0x74). The port launches it with that velocity (no
weight factor: dishonored-bevy's `[1.0, 0.85, 0.6, 0.4]` is a guess); whether the interface
applies it as a velocity or an impulse divided by the body's mass is OPEN (the mass is in the
PhysX body, not in a tweak found).

M-7 **Hits** (CONFIRMED structure, LIKELY amounts): the breakable's rigid-body collision
0x00A40CF0 (`DishonoredBreakable` slot 164; `DishonoredMovable` overrides slot 238 / 239 =
`GetDamage` / `SetDamage`, 0x00A428C0 / 0x00A428D0, on `m_Damage`):
 - `m_Damage` < 0: nothing. Contacts within 1/6 s of the last one are ignored (0x00A38720 / the
   constant 0.16666667).
 - threshold `T`: if `m_fMinSpeedToBeDamaged <= m_fMinSpeedToCauseDamage` (or the latter is 0),
   `m_fMinSpeedToBeDamaged`, or `m_fMinSpeedToBeDamagedOnDrop` while `m_bDropped` (+0x384
   bit 1); else `m_fMinSpeedToCauseDamage`. An impact at or below `T` does nothing. A glass
   bottle (1000 / 250): `T` 250; a candlestick (100 / 250): 100, 800 after a drop.
 - above `T` and above `m_fMinSpeedToCauseDamage`: the struck pawn takes `m_Damage` (the call
   through slot 82 / +0x148 with `GetDamage()`), unless the pawn is the instigator/holder; a
   height test on the pawn (object Z against the pawn's feet + the mesh offset + 40) decides
   whether it counts (OPEN: not implemented); `m_bDropped` ends.
 - above `T` and above `m_fMinSpeedToBeDamaged`, with break steps left: the prop takes
   `m_Damage` itself (`SetDamage(m_Damage - dealt)`, then `TakeDamage`): a bottle (health 5,
   `m_Damage` 5) breaks at the first such hit and its `m_Damage` is 0 afterwards. With
   `m_Damage` 0 and a non-pawn contact the amount comes from a speed and mass formula
   (0x00BF0EC0, OPEN, not implemented).
 - `m_bDropped`'s setter was not found; by the tweaks (bottles 1000 / 4000 on drop, thrown at
   2000) it belongs to a drop and not to a throw (the port sets it on a drop only).

M-8 **Noise** (CONFIRMED table, LIKELY gates). A contact is raised by the physics body with the
prop's contact type: `m_pContactTypeOverride` (bottles and glasses `DisContactType_Env_Glass`,
pans `Env_Metal`, vases `Env_Stone`, the piano `Env_Wood`; the candlesticks `Env_Metal`), against
`Environment`. The contact system's `DisContactSub_AINoise` (0x00BD6C20): Env_Glass 500 uu
Anomaly, Env_Metal 2000 uu Anomaly, Environment 800 uu Anomaly (stealth.md section 5.2; the
glass pair scales with `Attribute_BreakingGlassSoundAttenuation` of the struck object, not
modelled). Gates: `DishonoredKActor.m_ContactPhysicsInfo` class defaults `m_fMinImpactSpeed` 50 and
`m_fRefireDelay` 0.3 (the reader of that struct was not located; LIKELY). The break step has its
own noise (`m_AINoiseLoudness`, world_actors). AI reaction: a heard Anomaly noise sends the
listener to investigate the spot (stealth.md 5.3): a thrown bottle is a distraction because of
its noise, not a dedicated mechanism; bodies are found by sight (`CorpseFound`).

M-9 **Physics** (bounded model, physics.md section 7). A released prop falls with
`DefaultGravityZ` (-1500) and is swept as a box (its mesh box) against the world on the
non-pawn movement channel; a contact removes the speed along the normal, a floor-like normal
ends the flight (no bounce, no friction model: OPEN, `PhysicalMaterial` restitution and friction
are PhysX data). Pawns it overlaps (alive, not carried, not unconscious, not the one who just
released it) are hit as in M-7.

## 3. Kismet

`DisSeqEvent_MovablePickedUp` (127 level instances) fires at the grab, `DisSeqEvent_MovableDropped`
(1) at the release; both are fed by `kismet_host/player_events.rs` from `SimEvent::MovablePickedUp`
/ `MovableDropped` (the prop's Kismet actor is found by package and name). The break is the
actor's `DisSeqEvent_BreakableBroken` (world_actors): the sim emits `SimEvent::MovableImpact` with
the breakable index, `WorldActorsState::step` applies `m_Damage` to it, and `WorldEvent::Broken`
output 0 (the last step) removes the prop (`movables::on_broken`).

## 4. Status

| id | rule | status |
|---|---|---|
| M-1 | pick-up conditions, reach, prompt | ported (cone and reach shared with pickups: LIKELY) |
| M-2 | grab, `MovablePickedUp` | ported |
| M-3 | hold position, initial grab slide | ported; the swept `MoveActor` is OPEN |
| M-4 | carrying restrictions | ported: sword out, drop on damage / death / knock-out / choke / possession; no animation exists; speed factor retail 1.0 |
| M-5 | drop, `MovableDropped`, release trace | ported |
| M-6 | throw impulse | ported as a velocity (units LIKELY) |
| M-7 | hit thresholds, damage to pawns and to the prop | ported; the pawn height test and the `m_Damage` 0 formula OPEN |
| M-8 | contact noise | ported (gates LIKELY) |
| M-9 | flight and rest | ported as the bounded model; bounce, spin and friction OPEN |
| - | `DisSeqEvent_Corpse` (bodies picked up / dropped) | not fed (not this task) |
| - | NPC `DisItemContext_NPCThrowObject` (NPCs throwing objects) | out of scope |
| - | DLC `DisDLC07PullTargetInterface` (`m_bPullDisabled`) | out of scope |

Open: the 8 `MovablePickedUp` events of Boyle Int sit in `candle_flames_01_prefab_Seq` (picking
up a candlestick puts its flame out), but the candlesticks' tweak (`candle_stick_01_twk`,
`LargeMoveableBase`) sets no `m_bInteractable`: either the flag is read from somewhere else for
them (the prefab instances, an archetype of the actor) or they are not grabbed by Use; the port
follows the tweak, so the candlesticks stay put and those 8 events never fire.

Scenarios: `fixtures/oracle/scenarios/movable/` (`pickup_bottle`, `drop_bottle`, `throw_bottle`;
Boyle Int bottle `DishonoredMovable_29` at (-1067, -1987, 102); the hold place, the throw and the
break are Unmeasured), queued in `capture_queue.txt`. Port run (2026-10-08): the pick-up holds the
bottle at (-1067, -1930.29, 114.34) for the eye at (-1067, -1850, 186.83) and pitch -20; level, a
throw leaves at 2000 uu/s and breaks it on the far wall (impact speed 2031, `BreakableBroken`
fired, `Env_Glass` noise 500 uu at the wall). Tests: `crates/dishonored_sim/tests/movables.rs`,
`crates/dishonored_assets/tests/movables_retail.rs`.
