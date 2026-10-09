# Door keyholes, tripwires and the Interact event

Scope: the three world interactions whose Kismet events were decoded but never fired
(`KISMET.md`, "Player and world events"): `DisSeqEvent_KeyHoleUsed` (37 level instances),
`DisSeqEvent_Tripwire` (7) and `DisSeqEvent_Interact` (4). Evidence levels as elsewhere: CONFIRMED
(read from the decompiled code or game data), LIKELY, UNVERIFIED (needs a capture; never judged).
Addresses are virtual addresses in the GOG exe (sha256 `70da9613...`). Findings are in our own
words. Code: `dishonored_sim::interactables` (rules), `dishonored_assets::interactables`
(placed doors and tripwires), `dishonored_multiplayer::interact` (the mission side) and
`kismet_host::player_events::interact_events` (the events); presentation in
`dishonored_runtime::tripwires`.

## 1. Keyholes

**Entry (CONFIRMED).** Use on a focused interactable is either a tap or a hold. The use state
`DisUseState_AltInteract` (0x00AD57E0) compares the time the key has been held with its own
threshold: a hold at least that long runs the interactable's `AltInteract` (interface slot 25,
0x00A3E0B0 wrapper), a shorter press runs `Interact` (slot 24, 0x00A3DF60 wrapper). The threshold
is `DisTweaks_PlayerInput.m_fAltInteractActivateTime`, 0.3 in both shipped input tweaks. (The
door tweak has its own `m_fHolePeekingActivateTime`, also 0.3; the state's tick compares against
a literal 0.3 and no reader of the door field was found: LIKELY that the player-input value is
the one in use, identical today.)

For a `DisDoor`, slot 25 is 0x00A38580: the user must be the player pawn (the object-class byte at
pawn +0x10A is `$`; not a possessed NPC or a rat), the keyhole socket (`m_KeyHoleSocketName`, class
default `KeyHole`) must exist on the door mesh (0x00A1EF10, `GetSocketWorldLocationAndRotation`),
the pawn's rotation is set to the socket's rotation, turned 180 degrees when the socket's forward
points against the pawn's own (dot < 0), `*out = 1` (the generic wrapper then also fires
`DisSeqEvent_Interact` output 1), and `DisSeqEvent_KeyHoleUsed` output 0 fires (originator the door,
instigator the pawn, exact class). Exit is the door's interface slot 26, 0x00A2E4E0: output 1,
same originator and instigator (the level instances have one output "Out": output 1 does not
exist on them).

**The state (CONFIRMED).** `StatePlayerMasterHolePeeking` (vtable 0x010FDB08). Begin 0x00A72D50: the
door is the state's parameter; input actions are switched off (mask 0x7FFFF), then 0x508D2 on
(Use is among them: it exits; the HUD line is "Use  Exit Keyhole"); with `m_bFadeIn` the screen
fades to black over 0.2 s and `m_State` = `DPS_FADE_IN`, else 0x00A63400 runs at once. Tick
0x00A75BD0: `FADE_IN` counts up, sets a camera flag at 0.25 s, runs 0x00A63400 at 0.3 s and
becomes `IDLE`; `FADE_OUT` counts up and leaves the state (back to walking) at 0.3 s.
0x00A63400 (the peek starts): black fade 0.2 s, and the field of view becomes
`m_fKeyHoleFOV * (1 - Attribute_KeyholePeepingMagnification)` over 0.2 s (`FUN_00ACA500`).
`Dis_ExitKeyhole` (0x009EE510, controller slot 339) is the exit request. The attribute has no
base value; the "Voyeur" bone charm (a scripted charm of the DLC pack) adds 0.3, so FOV 52.5.
`DisTweaks_PlayerInput` limits the look sensitivity while peeking
(`m_fLimitedMouseSensitivity_Keyhole` 7.0, `m_fLimitedJoySensitivity_Keyhole` 0.5,
`m_bLimitControllerSensitivity_Keyhole`): the look is not locked.

**Not decoded.** The camera's position (the socket is taken), the per-tick scan of the state
(0x00AA6FE0: pawns that rendered recently within a radius, a line trace to each, a dot test: the
far side noticing the eye), the exact moment of the exit event (taken at the exit request), and
whether moving exits (the movement actions are off in the state, so it is not an exit).

## 2. Tripwires

**Class.** `DisTripwire` (instance 0x294) is a `SkeletalMeshActor` with `COLLIDE_TouchAll`,
`m_pTripwireTweaks` (`DisTweaks_Tripwire`: `m_pSkeletalMesh`, `m_pPhysicsAsset`, `m_pAnimSet`,
`m_TrippedAnimName` = `TripodTrap_Fire`) and `m_TripwireStatus` (`ETS_Armed` / `ETS_Tripped`,
saved: 0x00A19130). The mesh and physics asset are put on the component at runtime (0x00A28AC0).

**Trip (CONFIRMED, 0x00A28B40).** Only from `Armed`: status becomes `Tripped`, the tweak's tripped
animation plays, `FireGeneratedEvents(tripwire, DisSeqEvent_Tripwire, originator the tripwire,
instigator the tripper, exact 0, no output list)` (0x00A21A30: every output of every event, so
both "Tripped" and "Deactivated" of the level instances), and a telemetry record (event 0x2E,
0x00956CB0, no gameplay). No alarm and no explosion come from the wire itself.

**What trips it (CONFIRMED).** Touch (0x00A30C00): any live actor overlapping the wire's physics
bodies unless it is a `DisGameCrowdAgentSkeletalRat` or a `DisRatSwarm` (0x00C70090, 0x00A1A8C0;
the class names read from the exe's class descriptors at 0x01378B00 / 0x01379398). Two more
tests ignore a `DisTrigger` / `Trigger_LOS` and apply only in the DLC game infos. Damage
(0x00A28BB0, from `TakeDamage_Native`): trips it with the damage causer as instigator unless the
damage type's `m_bDontTriggerTripWire` is set; in the shipped data only `DisDamageType_WindBlast`
carries it. Jumping or Blinking over it does not touch it.

**The action.** The level's Kismet decides: `Tripwire` -> `DisSeqAct_ActivateProjectileLauncher`
(0x00B9B1D0: each "Target" that is a `DisProjectileLauncher` fires, 0x00A33F70), `SeqAct_Gate`s.
The launcher (`DisProjectileLauncher` < `DisSkeletalBreakable`, `m_bArmed`) plays its tripped
animation, spawns the tweak's projectile (`m_pProjectileTweak`, `m_fProjectileSpeed`,
`m_fProjectileDamage`) at its `m_FiringSocketName` ("Muzzle") toward `m_pTarget` and disarms.
Disarming is the launcher's interactable (`m_bHarvested`, `m_HarvestedAmmo`, a required item),
not the wire's. Launchers are not ported: see F-6.

## 3. Interact

**Rule (CONFIRMED).** The Use handler 0x00AD5BA0 calls the focused interactable's `Interact`
through 0x00A3DF60 (output 0) and the alternate through 0x00A3E0B0 (output 1) when the slot
returned true: `FireGeneratedEvents(user, DisSeqEvent_Interact, user, interactable, exact 1,
[output])`. Pickups use 0x00A3E200 (slot 27), which fires no event. The event goes to the events
registered with the *user pawn*: `SequenceEvent::RegisterEvent` (0x006E50B0) adds an event to its
`Originator`'s list only. `m_pAssociatedTweak` is not a filter: the event's activation (slot 102,
0x00B8E830) stores the interactable's tweak there for the event's variable output.

**The four Prison instances** (HUD-Notes-Tutorials, Objectives) have no `Originator`: registered
with nobody, they cannot fire in the original either. UNVERIFIED until the capture
(`kismet/interact_prison`); the port fires the event for events whose Originator is the user pawn
(none in the shipped data).

## 4. Findings

| ID | Finding | Status |
|---|---|---|
| F-1 | Keyhole entry: hold Use `m_fAltInteractActivateTime` (0.3) at a shut keyhole door -> peek; `KeyHoleUsed` output 0 | ported: `interactables::step_peeks`, `kismet_host::interact_events` |
| F-2 | Peek view: socket as the eye, view turned toward the pawn's own direction, FOV `m_fKeyHoleFOV * (1 - magnification)`, black fade 0.2 / 0.3 s | ported (`PeekView`); camera origin = socket is LIKELY |
| F-3 | Exit: Use leaves (fade-out 0.3 s), output 1 at the request; a dead pawn or an opened door ends it | ported; exit timing and the door-opened end are LIKELY |
| F-4 | Tripwire trips once on touch (not by rats) or damage (not Windblast); `Tripwire` fires every output | ported: `interactables::step_tripwires` |
| F-5 | Interact output 0 on the use of a Kismet usable, output 1 on a door's hold; events registered with the user pawn | ported; the Prison events are dead data (UNVERIFIED) |
| F-6 | Projectile launchers (fire on `ActivateProjectileLauncher`, disarm, harvest) | left open: needs a tweak-driven projectile with its own explosion; `DisSeqAct_ActivateProjectileLauncher` is not run by the host |
| F-7 | Camera scan of the peek (0x00AA6FE0), the exit event's exact moment, the peek's look sensitivity limit in the input path | left open (not decoded / presentation input) |
| F-8 | Projectile contact with a wire uses a 1 uu radius; the shipped projectile collision extents were not read | Unmeasured |

## 5. Against dishonored-bevy

Their `keyhole.rs` / `traps.rs` were read; the flow (hold Use at a shut door, view through the
socket, Use leaves; a wire drawn between its ends) is the same and the data extraction idea
(socket through the mesh bind pose) was reused with credit (NOTICE). Where the decompile
disagrees, the decompile wins: hold time 0.3 (`m_fAltInteractActivateTime`), not 0.4; the look is
free (limited sensitivity), not a +-0.45 / 0.3 rad cone; the eye is the socket, not 0.07 m past
the door; the view faces the way the pawn already looks (dot of the socket forward with the pawn's
view), not "away from Corvo's side"; a wire trips on the physics asset's bodies for any live
non-rat actor, a projectile or damage (not Windblast), not on Corvo's capsule alone against the
pulley bones; the wire event fires every output. Their launcher code (`launch`, `aim`,
`fly_darts`, disarm) is not taken: it needs the projectile tweaks the sim does not model (F-6).

## 6. Scenarios and captures

`kismet/keyhole_boyle_int`, `kismet/tripwire_prsnsewer`, `kismet/interact_prison` (queued in
`fixtures/oracle/capture_queue.txt`). Static checks: the peek starts once, `KeyHoleUsed` fires,
the FOV is 75, the peek ends; the wire trips once and its events fire. Unmeasured: the peek's
start time, the black fade, the trip time, and that the Prison's Interact events never fire.
