# Door keyholes, tripwires and the Interact event

Scope: the three world interactions whose Kismet events were decoded but never fired
(`KISMET.md`, "Player and world events"): `DisSeqEvent_KeyHoleUsed` (37 level instances),
`DisSeqEvent_Tripwire` (7) and `DisSeqEvent_Interact` (4), and the projectile launchers the
tripwires' scripts fire (section 2a). Evidence levels as elsewhere: CONFIRMED
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
Disarming is the launcher's interactable (`m_bHarvested`, `m_HarvestedAmmo`, a required item),
not the wire's. The launcher is section 2a.

## 2a. Projectile launchers

Code: `dishonored_sim::launchers` (rules), `dishonored_assets::interactables::launcher` (placed
tripods and their tweaks), `dishonored_multiplayer::interact` (the mission side),
`kismet_host` (`DisSeqAct_ActivateProjectileLauncher`, `SeqAct_Toggle`). Three shipped maps have
them: the Prison Sewer (3 tripods, `WhiskeyLauncher_twk`), the Distillery (4, the same) and the
Overseers' Back (1, `BoltLauncher_twk`). Every `m_pTarget` is a `TargetPoint`; two Distillery
tripods have none.

**The action (CONFIRMED, 0x00B9B1D0).** A latent action (`DisSeqAct_ActivateProjectileLauncher <
SeqAct_Latent`, field `m_pProjInstigator`, a pawn property that no level sets) whose `Activated`
walks the "Target" link and calls `Fire` (0x00A33F70) on every `DisProjectileLauncher`, then
`SequenceAction::Activated`: it finishes in its own pass and its "Out" follows at once.

**Fire (CONFIRMED, 0x00A33F70).** Only if `m_bArmed` (bit 0 of +0x2A8). It plays the tweak's
`m_TrippedAnimName` (`TripodTrap_Fire`; `SetAnim` slot 97, `PlayAnim` slot 77, no loop), takes
the muzzle from `GetSocketWorldLocationAndRotation` (0x00732530) for `m_FiringSocketName`
(`Grenade_Socket` in both shipped tweaks; the class default is `Muzzle`), and, when the tweak has
a projectile, spawns it there with the socket's rotation (0x00C66070 -> the tweak's spawn slot,
instigator `m_pProjInstigator`). The aim is `FUN_00C3F2A0` in **mode 2**, its straight branch
(modes 0 and 1 run the ballistic solver 0x00BEC130): `target location - muzzle`, normalised
(0x004019E0); a pawn target would be the midpoint of two of its body points (0x00BC7190), any
other actor its collision component's location. Without `m_pTarget` the direction is the socket's
forward axis (`FUN_0040DA70`, rotator -> vector). The shot is **not** aimed over gravity. It then
runs `DisProjectile` slot 233 (0x00C540A0; the explosive arrow's 0x00C54E70 adds its hooks):
velocity = `m_fProjectileSpeed x direction x DisTweaks_Projectile.m_fSpeedMultiplier` (+0x98),
`m_Damage` = `m_fProjectileDamage` (int), `m_fHeadshotMultiplier` = `m_fProjectileHeadshotMult`.
The function ends by clearing `m_bArmed`: one shot per arming, with no delay.

**Flight and burst (CONFIRMED).** The projectile falls under `GetGravityZ x m_fGravityMultiplier`
(0x00C54F00 predicts it that way: 0.5 for the trap arrow, 1 for the plain arrow). A contact runs
the generic processor 0x00C4F3A0: the hit pawn takes slot 239 (0x00C4A310: `m_Damage x
m_fDamageMultiplier`, 10) through `TakeDamage`; the explosive arrow's contact handler (slot 103,
0x00C5A4C0 -> 0x00C550D0) then spawns the tweak's `m_pExplosionTweak` `DisExplosion` 10 uu back
along the impact direction, instigated by the arrow's instigator, once. `TrapExplosiveArrow_twk`
(class `DisTweaks_Arrow_Explosive`, invisible, gravity 0.5, trail `ps_TB_Trail`) uses
`TrapArrowExplosion_twk`: `m_RadialDamage` 30 on all four difficulties inside
`m_fFullDamageRadius` 300 (players 150), falling to 0 at `m_fDamageRadius` 600 (players 600),
`m_fRadialMomentum` 1500, `m_fExtraUpForce` 500, `m_AINoiseLoudness` `EAINoiseLoudness4`
(6000 uu, a Danger noise), the grenade's explosion effect and sound. The port runs it through
the grenades' `gadgets::explode_with`. The Overseers' Back launcher's `Twk_Proj_Arrows.Twk_Proj_Arrow`
is a `DisTweaks_Projectile_ThrownObject`: no explosion, the direct hit only.

**Interact (CONFIRMED, interface slot 24, 0x00A3B6E0).** Slot 9 of the launcher's interface
(0x00633610) is "no alternate action", so the Use state (0x00AD5BA0 -> 0x00A3DF60) runs `Interact`
at the press. (1) An armed launcher is disarmed (0x00A34280: `m_bArmed` cleared, the tweak's
`m_DeactivatedAnimName` `TripodTrap_Defuse` plays). (2) If not `m_bHarvested` and the tweak
needs no item (`m_pItemRequiredToHarvestAmmo`) or the player holds it (0x00C0B890), each of the
12 `m_HarvestedAmmo` counts goes into the inventory (`AddAmmo` 0x00C052E0: what fits under the
capacity); the whiskey tripod gives 1 incendiary bolt (type 4), the bolt launcher 1 bolt (type 2);
any amount added counts as a harvest (the projectile tweak's pickup sound, the HUD ammo notice
0x00B94FB0). (3) Otherwise the projectile tweak's cannot-pickup sound plays and, when the launcher
was not armed, its interactable's `m_CannotUseMessage`. (4) `m_bHarvested` is then **stored from
what step 2 did on every path** (`bit1 = local_10`), so a press that took nothing clears it: the
second press after a harvest takes nothing, the third takes again (UNVERIFIED, decoded; needs
the capture). (5) It answers "used" when it was armed or harvested now; the wrapper fires
`DisSeqEvent_Interact` output 0 (instigator the launcher) and shows the interactable's
`m_UseMessage` ("Launcher disarmed"). The prompt is the interactable tweak's `m_InteractText`
("`GBA_Use` Disarm", title `m_Name` "Projectile Launcher") within `m_fMaxDistance` 200.
`AltInteract` (slot 25, 0x00A33E30) only fills the user's interaction-slot table and answers
"used". `SeqAct_Toggle` on a launcher (`OnToggle_Native` 0x009ED800 -> slot 238, 0x00A21650):
Turn On arms, Turn Off disarms, otherwise it flips.

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
| F-6 | Projectile launchers (fire on `ActivateProjectileLauncher`, disarm, harvest) | ported: `launchers::{fire, step_shots, interact, toggle, focused}`, `MissionMatch::{fire_launcher, interact_launcher, step_launchers}`, the host's `DisSeqAct_ActivateProjectileLauncher` and `SeqAct_Toggle`; section 2a |
| F-9 | The harvested flag is stored from the press on every path (second press takes nothing, third takes again) | ported as decoded; UNVERIFIED (scenario `kismet/launcher_disarm_prsnsewer`, Unmeasured) |
| F-10 | Launcher muzzle pose, the tripped / defused animations, the sounds and HUD ammo notice, the `ps_TB_Trail` trail, `AltInteract`, the focus probe's gating, the no-socket muzzle, `DisSkeletalBreakable` damage | left open (the muzzle is the idle bind pose, LIKELY; the state is replicated for a future animation, no skeletal mesh is driven) |
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
`fly_darts`, `disarm_focus`) was read: the shape (a sequence fires the tripod; the shot sweeps
through characters and then the world; the player disarms it for ammunition) is kept, the rules
are the decoded ones. Where the decompile disagrees, the decompile wins: no ballistic `aim` arc
(mode 2 of 0x00C3F2A0 is a straight line; the arrow falls under its own gravity), no fire
delay (the shot leaves in the activation's pass, not at `def.fire_at`), the speed is
`m_fProjectileSpeed x m_fSpeedMultiplier`, the target is `m_pTarget` (without one the socket's
forward axis, not "at Corvo"), the burst is the tweak's explosion (30 / 300 / 600, players
150 / 600) after a 10-point direct hit, the prompt reach is 200 uu on the tripod's collision
bounds (not a 0.8 view cone), and disarming is one press that also gives the ammunition.

## 6. Scenarios and captures

`kismet/keyhole_boyle_int`, `kismet/tripwire_prsnsewer`, `kismet/interact_prison`,
`kismet/launcher_prsnsewer`, `kismet/launcher_disarm_prsnsewer` (queued in
`fixtures/oracle/capture_queue.txt`). Launcher checks: Static (action runs once, one shot per
`Fire`, speed 2000, `m_Damage` 10, one burst, the 6000 uu noise, launcher counts, the disarm and
the bolt given); Unmeasured (the one-tick host latency, the flight time, the burst damage 30 at
the pawn, the second and third press). Static checks: the peek starts once, `KeyHoleUsed` fires,
the FOV is 75, the peek ends; the wire trips once and its events fire. Unmeasured: the peek's
start time, the black fade, the trip time, and that the Prison's Interact events never fire.
