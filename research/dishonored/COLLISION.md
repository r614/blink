# Level collision against the original

What blocks the player's movement, the pistol shot and the vision line of sight on the two Boyle
maps (`L_Boyle_Ext_P`, `L_Boyle_Int_P`) in `Dishonored.exe`, how the port reproduces it, and how
closely the two agree when the same checks are run in both. Findings are in our own words;
addresses are virtual addresses in the image. Evidence levels: CONFIRMED (code read and measured),
LIKELY (measured, or code read with one link inferred), APPROXIMATION.

Code: `dishonored_assets::{collision_audit, level, simple_collision, prop_collision}`,
`dishonored_content::add_level_blockers`, `dishonored_world` (`CollisionWorld`, `sat`), the
`harness collision` suite (`tools/harness/src/collision.rs`), the `collision_audit` tool
(`tools/map_dump`) and `probe_launch collide` (`tools/probe_launch/src/oracle_collision.rs`).

## Method

1. **Queries.** `harness collision gen --map M` derives a deterministic batch from the map's
   original navigation mesh (Ext 77,283 checks, Int 45,475):
   - `grid`: vertical lines over a 192 uu grid of the navmesh area, from 3000 uu above the
     highest polygon to 1500 uu below the lowest;
   - `pdrop`: a player box (35 x 35 x 88 half size) dropped onto each polygon;
   - `foot` / `waist` / `head`: 600 uu horizontal lines at 20, 90 and 160 uu above each polygon
     centre (and points towards its corners on large polygons) in eight directions, with the
     player's movement flags (P); 1200 uu lines at waist height with the pistol's flags (J) and
     at head height with the vision flags (V);
   - `radial`: 400 uu player-box sweeps out of every polygon; `link`: player-box sweeps along
     every navmesh link.
2. **Oracle.** `probe_launch collide` runs the batch in the original through the engine hook's
   `LINE_CHECKS` op, which calls `UWorld::SingleLineCheck` with the given flags, extent and the
   player's pawn as source, and returns hit, time, location, normal and the hit actor and
   component. It also records each `DisDoor`'s state (`m_DoorState`, +0x311). The game is the
   GOG build at the start of the mission, the player in control at the garden start.
3. **Comparison.** `harness collision check --results R` answers every query in the port (the
   static world plus the movers and doors posed in the recorded states) and counts a query as
   agreeing when both miss, or both hit within 2 uu (lines) or 4 uu (boxes) along the check.
   Checks whose original hit is a living pawn are left out (the port's world has no pawns).
   Disagreements are grouped by what each side hit, and classified (below).
4. **Replay without the game.** `--store` writes `fixtures/collision/<map>.oracle`: the query
   id, a hash of its coordinates, the hit flag and the hit fraction, and the door states; no
   names, no retail data. Plain `harness collision` regenerates the queries, checks the hashes
   and compares the port with the stored answers; the agreement floor is 99.93 % (Ext) and
   99.89 % (Int).

## Trace flags

Measured with the hook on the calls the game makes (CONFIRMED):

| Check | Flags | Notes |
|---|---|---|
| Player movement (`UWorld::MoveActor` of the pawn) | 0x000020DF | `TRACE_Blocking`; the trace type comes from the source (the player's pawn: `Move_Player`) |
| Pistol shot (0x00C2A3E0 -> 0x00BE1560) | 0x020A28DF | `Gameplay_Projectile`, `TRACE_ComplexCollision`, `TRACE_Blocking` |
| Crosshair | 0x01022097 | `Gameplay_Crosshair`, complex collision |
| Vision line of sight | 0x08023086 | `Gameplay_VisionLOS`, `TRACE_Movers` and level geometry, complex collision, no `TRACE_Blocking`, no `TRACE_Others` |

Dishonored replaces UE3's `BlockZeroExtent` / `BlockNonZeroExtent` with
`UPrimitiveComponent::m_CollisionTraceTypes` (`DisPrimTraceMask`, +0x140, seven bits: move
non-pawn, move NPC, move player, crosshair, projectile, melee, vision; set by
`SetTraceBlocking`, 0x0052A230). The bit a check tests is chosen at 0x00529990 (the gameplay
flags first, else 0x10000000 NPC movement, 0x20000000 player movement, else by the source
actor). A component is checked when it collides (0x00529E90), its bit is set and its owner's
`ShouldTrace` (vtable +0x1C4) accepts the flags. Zero-extent and extent checks test the same bit.

## Engine rules

- **Static meshes** (`UStaticMeshComponent::LineCheck`, 0x007717D0): the simplified collision
  (`RB_BodySetup.AggGeom`) when the mesh sets `UseSimpleLineCollision` (lines) or
  `UseSimpleBoxCollision` (boxes), the component has an owner and the check does not ask for
  complex collision (0x20100); else the kDOP triangles. A mesh that wants its simplified
  collision but has none is not hit. Flags are resolved through the archetype chain (cooked
  objects store deltas).
- **Back-off.** A hit not asked as `TRACE_Accurate` is moved back along the check by
  `clamp(0.1, 0.1 / L, 4 / L)` of its length L on kDOP triangles (4 uu on long checks) and by
  1 uu on simplified collision; boxes report the separating-axis entry backed off the same way.
- **Extent checks** (`dishonored_world::sat`): an axis-aligned box swept against a kDOP triangle
  (0x005DEB70) or a convex element (0x007AF030) by separating axes (0x005DEA10 / 0x005DE780). A
  box that starts overlapping the shape is stopped where it starts when it is no deeper than it
  has left to travel, or when it does not leave through the face it is leaving by; otherwise the
  shape is ignored. A line that starts inside a convex element hits it at time 0.
- **`ShouldTrace`.** World geometry (`bWorldGeometry`) answers `TRACE_LevelGeometry` without
  needing `BlockActors`. Other actors answer `TRACE_Others` with `TRACE_Blocking` only when the
  source is blocked by them (`IsBlockedBy`: the actor's `bBlockActors` and the component's
  `BlockActors`). `AKActor::ShouldTrace` (0x00782D60) answers `TRACE_Movers`: without
  `TRACE_Blocking` (the vision line) whatever the blocking flags, with it only when blocking.
  `DisPickup_Base` (0x00A1D820) and `DisAbstractItemPickup` (0x00C452F0) add their own
  exclusions (held items, pawn sources) and then defer to it. `DisSkeletalBreakable`
  (0x00A28580; usable objects) answers the vision line as level geometry when its tweak says
  `m_bShouldTraceAsWorldGeometry`; `DisDoor` (0x00A2C760) always; both not when transparent for
  vision (the actor's `m_bTransparentForVisionChecks` if it sets
  `m_bOverrideDefaultVisionTransparency`, else the tweak's). `DisWallOfLight` (0x00A21B30)
  answers the projectile and melee checks while powered.
- **Physics asset collision.** A skeletal component collides through its physics asset: one
  `RB_BodySetup` per bone with box, convex, sphere and capsule elements placed by their `TM`
  relative to the bone, posed by the component's current animation.
- **Walkable slope and step height** (read live from the player's pawn, CONFIRMED):
  `Pawn.MaxStepHeight` (+0x250) 44, `MaxJumpHeight` (+0x254) 122, `WalkableFloorZ` (+0x258)
  0.6428 (cos 50 deg, `DisTweaks_Pawn.m_fMaxWalkableFloorInDegrees`). The port reads the same
  values (`movement.max_step_height` 44, `walkable_floor_z` 0.64278764).

## Movement and the back-off

The pawn's movement consumes the same backed-off times (CONFIRMED, engine hook, 2026-10-07, Boyle
exterior, fixed 1/60 s step). physWalking's floor check (0x006BD150) is a `MultiLineCheck`
(0x0064A860) with flags 0x20DF and the pawn's extent, `MaxStepHeight + 2.54` = 46.54 uu down; it
keeps the nearest hit and computes the floor distance as `Time x 46.54`, which it holds between
2.413 and 3.048 (moving to 2.7305). That time is backed off by the hit kind, so the real gap
under the box is larger. Measured by reading the pawn's `Location` and running the floor check
under it with and without `TRACE_Accurate` (0x80000):

| Spot | Floor | Location.Z | Box check, not accurate | Box check, accurate (real gap) |
|---|---|---|---|---|
| Runway (-3400, -3160), standing | kDOP (`ext_boyle_ground_garden_01`), z -310.0 | -215.770 | 2.730 | 6.730 |
| Same, crouched (`CollisionHeight` 65) | same | -238.270 | 2.730 | 6.730 |
| After a Blink along the runway | same, z -310.28 | -216.047 | 2.731 | 6.731 |
| After a mantle onto a 119 uu ledge (-2370, -3500) | simplified collision (`Env_Blockout.collisions.Collision_model`), z 10.0 | 101.231 | 2.731 | 3.731 |

So a walking pawn rests 2.7305 + 4 = 6.73 uu above per-polygon floors and 2.7305 + 1 = 3.73 uu
above simplified collision. The port's movement sweeps use the engine time
(`CollisionWorld::sweep_box` applies `hit_kind::engine_time`) and give the same numbers at both
spots (`harness collision probe`: 0.05866 and 0.05868, as the original). Horizontal moves of a
tick (a few uu) are backed off by a tenth of their length (the `0.1` clamp for checks under
40 uu), so a pawn pressing into a wall closes in on it geometrically: the pawn that walked into
the ledge above stopped 0.04 uu from its face.

`dishonored_sim`'s mechanics tests use a per-polygon fixture floor, so their standing height is
`STAND_Z + FLOOR_DIST + 4` (the tests assumed `+ FLOOR_DIST` before the audit; the hook
measurement settled it).

## Sources

Active colliding components per class (`collision_audit L_Boyle_Ext_P L_Boyle_Int_P`, effective
flags). "Agrees" means the class showed no unexplained disagreement in the last oracle round.

| Class (component) | Ext | Int | Original | Port | Status |
|---|---|---|---|---|---|
| `StaticMeshCollectionActor`, `StaticMeshActor` (static mesh) | 2317 | 2164 | World geometry; per-component trace types; simplified or kDOP collision per mesh flags | Same; static triangles and convex elements | Agrees |
| `DisStaticMeshActor` | 0 | 6 | World geometry (the Wall of Light generator) | Static | Agrees |
| `BlockingVolume` (brush) | 8 | 7 | Movement types only: stops the player's movement lines and boxes, not the pistol or the vision line (measured on `BlockingVolume_5`) | Brush hulls on the movement channels | Agrees (U-640 CONFIRMED) |
| `DisStaticCollisionActor` | 1 | 1 | NPC movement only (`npcmovementcollision_model`): measured, stops the NPC movement line and box, not the player, the pistol or the vision line | NPC channel | Agrees (U-643 CONFIRMED) |
| `InterpActor` | 5 | 6 | Movers: block where the matinee has them | Movers at their start position | Agrees at the start state |
| `DisDoor` (skeletal) | 8 | 18 | Physics asset posed by the door state; seen by vision unless transparent (the sewer grate doors) | Movers posed per Kismet state (closed: reference pose, open: last frame of the open stage); vision per tweak | Agrees |
| `DishonoredUsableObject` and subclasses (skeletal) | 31 | 50 | Physics asset posed by the anim tree: at the start the first frame of the first stage's transition animation (bins' lids closed; the reference pose has them open) | Same pose (`usable_start_bodies`) | Agrees (Int toilet and globe: unknown, U-651) |
| `DishonoredUsableObject` and subclasses (static part) | 22 | 33 | Blocking; vision per tweak (lockers, bins: seen) | Static, vision per tweak | Agrees |
| `DishonoredMovable`, `DishonoredBreakableNavBlock`, `KActor`, `DisSpeaker_PA` | 47 | 154 | Simulated physics props (`PHYS_RigidBody`) fail `IsBlockedBy`: only the vision line and the crosshair meet them; a prop whose tweak sets `m_bFixed` (`PHYS_None`, Boyle's piano) blocks everything | Static where placed: vision and crosshair only, or every channel when fixed | Agrees at the start state (U-650) |
| `DisWhaleOilBattery` | 9 | 7 | As physics props; a battery in a receptacle (`DisWhaleOilReceptacle.m_pBattery`) is held in place and blocks | Same; the held battery at its placed transform | Agrees but for its socket offset (dynamic, U-650) |
| Pickups (`DisAbstractItemPickup*`, `DisStatPickup`, `DisKey_Base`, elixirs, charms) | 72 | 133 | Not blocking (`bBlockActors` false); `AKActor::ShouldTrace` lets the vision line and the crosshair meet them (paintings, hemlock, urn) | Static where placed, vision and crosshair only | Agrees (picked up later: U-650) |
| `DisWallOfLight` (static mesh) | 0 | 1 | Projectile and melee checks while it collides (powered, time running) | Not static: the mission layers it in on the projectile and melee channels while it collides (`dishonored_sim::walls::layer`) | Agrees in game; the static audit no longer sees it |
| `DisClimbable` (skeletal) | 1 | 1 | Physics bodies that swing; off while climbed | Not modelled | U-654 |
| `DisFish`, rats (`DisGameCrowdAgentSkeletalRat`), `DisDetectionEye` | 19+ | 1+ | Moving or non-blocking actors | Not part of the static world | Dynamic |
| Volumes and triggers (`TriggerVolume`, `PhysicsVolume`, audio, stealth, tether, hideout, water volumes, `DisTrigger`, `Trigger`, `Trigger_LOS`) | 138 | 127 | Collide (touch) but do not block | Not collision | Agrees |
| `DisWhaleOilReceptacle` | 0 | 1 | Not blocking, not a KActor | Not collision | Agrees |

## Agreement

Before: the first rounds (`h_ext2` with the first exterior oracle run, `h_int1` with the first
interior run); the player's movement checks then left pawns out of the flags. After: the stored
oracle results replayed by `harness collision` (exterior: a new oracle run with the final
flags; interior: the third run).

| Map | Kind | Trace | Queries (after) | Before | After |
|---|---|---|---|---|---|
| Ext | grid | P | 4614 | 99.15 % | 99.50 % |
| Ext | pdrop | P | 1864 | 95.76 % | 99.95 % |
| Ext | foot | P | 11297 | 97.12 % | 99.95 % |
| Ext | waist | P | 11285 | 97.45 % | 100.00 % |
| Ext | waist | J | 11392 | 98.41 % | 99.99 % |
| Ext | head | P | 11283 | 97.42 % | 100.00 % |
| Ext | head | V | 11392 | 97.36 % | 99.96 % |
| Ext | link | P | 2418 | 86.26 % | 99.92 % |
| Ext | radial | P | 11281 | 86.05 % | 99.90 % |
| Ext | all | | 76826 | 95.55 % (73845 / 77283) | 99.94 % (76778) |
| Int | grid | P | 1097 | 99.00 % | 99.00 % |
| Int | pdrop | P | 1046 | 99.14 % | 99.90 % |
| Int | foot | P | 6779 | 98.82 % | 99.99 % |
| Int | waist | P | 6774 | 98.83 % | 99.88 % |
| Int | waist | J | 6920 | 97.53 % | 99.94 % |
| Int | head | P | 6776 | 99.47 % | 99.94 % |
| Int | head | V | 6920 | 97.21 % | 99.90 % |
| Int | link | P | 1758 | 98.57 % | 99.83 % |
| Int | radial | P | 6728 | 98.08 % | 99.91 % |
| Int | all | | 44798 | 98.37 % (44732 / 45475) | 99.90 % (44753) |

Where both hit, the original's hit minus the port's along the check is within +-0.04 uu
(10th to 90th percentile) for lines on kDOP and simplified collision, and -3.95 uu for boxes
on brushes and physics bodies (their back-off is not measured, U-653).

### Fixes in this round

- Usable objects rest on the first frame of their first stage (Boyle's bins: the lid closed;
  Ext head-height P from 99.39 % to 100 %, V from 98.90 % to 99.70 %).
- Usable objects' static parts answer the vision line per their tweak (lockers, bins): Ext V
  99.70 % to 99.96 %, Int V 99.36 % to 99.45 %.
- Pickups answer the vision line and the crosshair (`AKActor::ShouldTrace`); a battery held by a
  receptacle blocks: Int V 99.45 % to 99.57 %, Int waist J 99.90 % to 99.94 %.
- Doors transparent for vision by their tweak (the interior's sewer grate door, `DisDoor_0`) let
  the vision line through: Int V 99.57 % to 99.90 %.
- Simulated physics props answer the vision line and the crosshair only; the Wall of Light
  stops projectile and melee checks (the interrupted session's work, tested here: Int foot P
  99.34 % to 99.99 %, radial 98.50 % to 99.90 %).
- Port bug: a box sweep tested convex elements only through the triangle tree's leaves, so a
  box deep inside a large element (a blockout collision box) missed it when no leaf of its faces
  overlapped the sweep; elements are now found by their own bounds.

## Remaining disagreements

Each disagreement is classified by `harness collision` (written to `<map>_causes.txt` with the
query ids; counts below from `check` against the oracle runs, where the original's hit actor is
known):

| Cause | Ext | Int | Meaning |
|---|---|---|---|
| Triangle edge or vertex (floating point) | 14 | 6 | Moving the sample by 0.1 uu makes the port agree |
| Coincident surfaces, different back-off | 21 | 13 | Both hit within 4.5 uu: two surfaces in the same place (a kDOP floor over a blockout box, a roof over a ceiling) with different back-offs; which component the original keeps is a tie (U-656) |
| Sample inside geometry | 0 | 1 | The check starts inside a shape |
| Dynamic state | 0 | 16 | The Int battery at its receptacle's socket (10), a rigid key pickup at rest (2), doors 2 and 15 (4, within 3 uu) |
| Unknown | 13 | 9 | U-651 |

An experiment that chose between coincident surfaces by their backed-off times (the minimum of
the times each component reports) fixed 18 exterior grid drops and broke 14 interior ones the
other way, so the tie rule is not that simple; it was not kept.

## Open items

- U-651: the unknown disagreements (ids there).
- U-650: physics props, pickups, breakables and usables collide where the map places them; the
  original moves, breaks and removes them at run time.
- U-652: sphere and capsule elements as polyhedra. U-653: the back-off on brushes and physics
  bodies. U-654: the climbable chains. U-655: the usable start pose rule is measured, not read in
  code. U-656: the tie between coincident surfaces.
- For the oracle tool, later: samples aimed at every collision primitive, every query extent and
  channel the executable uses, a structural diff of the live collision components, and state
  sweeps of Kismet-toggled collision.
