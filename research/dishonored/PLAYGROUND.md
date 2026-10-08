# The Playground and the layout format

The original ships no test level (the only test-like package is a HUD one, `UI_HUD_DLCTest_SF`).
The Playground is a level built for testing the player mechanics. Its layout is new; everything in
it is the original game's own data, read from the install at run time:
- static meshes and materials;
- placed actors used as archetypes (doors, a locker, a bin, a climbable chain, pickups);
- a light, the AI factions and the tweaks.

Nothing retail is committed: the layout names objects by path. The mechanics on it are the
port's normal ones, so what works there works on Boyle, and the reverse.

- Layout: `maps/playground.ron` (map name `Playground`).
- Mission (NPCs, bench, station messages, loadout): `missions/playground.ron`.
- Launch: `dishonored_runtime --playground`, or the launcher's PLAYGROUND entry (or Missions).
- Checks: `harness playground`.

## The layout format (`dishonored_ir::layout::MapLayout`)

A RON file in `maps/<name>.ron` (lower case). The content loader treats a map whose layout file
exists as a layout map; `DISHONORED_MAPS` names another directory. Coordinates are Dishonored world
space (Z up, Unreal units), angles in degrees.

| Field | Meaning |
|---|---|
| `name` | Map name (`--map`, `MissionDef.map`) |
| `packages` | Cooked packages the paths refer to. They are loaded to resolve objects; their own actors are not placed. |
| `environment` | Package whose `WorldInfo` gives the post-process settings and reflection cubemap |
| `default_material` | Material of brushes that name none |
| `sky_light` | Ambient brightness the AI light probe adds everywhere (`AiLevel::sky_light`) |
| `brushes` | Blockout geometry with an original material: `Box((x, y, z))` (`at` = centre of the bottom face), `Ramp(width, length, angle)` and `Stairs(width, steps, rise, run)` (`at` = centre of the bottom front edge, rising along the rotated +X). `yaw`, `material`, `collide` (default true), `uv_scale` (world units per texture repeat, planar mapping). |
| `meshes` | Original static meshes by object path (`Crate_01.Crate_01`): `at`, `rotation` (pitch, yaw, roll), `scale`, per-section `materials`, `collide` |
| `actors` | Original placed actors used as archetypes (`"L_Boyle_Ext_Audio:DisDoor_21"`), copied with their components, tweaks and settings: `at` (new `Location`), `yaw` (pitch and roll kept), `name` (the copy is `<map>:<name>`), `locked` and `door_state` for doors. Doors, usables, climbable chains and pickups keep their behaviour. |
| `lights` | Dynamic lights (`Point`, `Spot`, `Directional`: `at`, `direction`, `color`, `brightness`, `radius`, `falloff_exponent`, `cone`, `shadows`), or a copy of an original light actor (`from`) |
| `starts` | Player starts (`at` is the pawn's centre, `tag` for missions) |
| `points` | Named points missions anchor to (`Actor("<name>")`) |
| `water` | `DishonoredWaterVolume` boxes (`fluid_friction` 2.4 by default, `shallow`) |
| `pain` | Pain-causing `PhysicsVolume` boxes (`damage_per_sec`, `interval`, `entry`, `damage_type`) |
| `labels` | In-world text (the stations' names), drawn within `range` |
| `nav` | Generated navigation over a grid (`min`, `max`, `cell`); absent: no navigation |

### How it becomes a map

- **Rendering** (`dishonored_assets::level::layout_import::import_layout`): the level importer's
  own mesh, material, component and skeletal code, run on the listed objects instead of a level's
  actors. Brushes become world-space static meshes (one section, the material, per-polygon
  collision, planar UVs). There are no lightmaps: every surface is lit by the dynamic lights.
  Doors and usables become `SkeletalProp`s like a level's, posed by their state.
- **Simulation** (`dishonored_content::playground`):
  - collision is `CollisionWorld::from_map` over the imported geometry;
  - the copies' bodies are added as on original maps (`add_level_blockers`' rules): a usable's
    bodies at the first frame of its first stage, a door as a mover with a pose per state;
  - water and pain boxes become the original's volume types;
  - chains are moved `Climbable`s, pickups moved `PlacedPickup`s;
  - the AI's faction table comes from the original tweaks of the packages, its light probe from
    the layout's lights;
  - the navigation is generated (below).
  Derived data is cached under `DISHONORED_CACHE` keyed by a hash of the layout.
- **Navigation:** original maps carry their editor-built navmesh (`L_*_Nav`); a layout has none, so
  one is generated (`playground::generate_nav`):
  - a grid cell is walkable where a downward line finds a walkable floor (normal Z >= `WalkableFloorZ`);
  - the standing pawn's box must fit 12 uu above that floor;
  - stacked floors over a cell each get a square polygon at the floor's height;
  - neighbours link when the step between them is at most `MaxStepHeight` (44) and a waist-high line
    between their centres is clear.
  Polygons are capped at 16000 (u16 vertex indices), so the Playground generates it over rows 3 and 4 only.
  Ramps are not linked (each cell is flat).

## Stations (maps/playground.ron)

| Row | Station | What it tests | Expected (source) |
|---|---|---|---|
| start | Bench (two tables) | Use: every power at level 2, full ammunition, elixirs; or the starting loadout | `Equip` |
| start | Pickups | health and mana elixirs, bullets, food, a rune, a bone charm, coins | taken with Use |
| 1 | Step heights 30 / 44 / 50 / 56 / 60 | walking in: `stepUp` rises `MaxStepHeight` + 2.54 = 46.54 from where the box rests, 6.73 above a per-polygon floor, so it clears up to 53.3 | 50 yes, 56 no (PLAYER.md `stepUp`, COLLISION.md hover; LIKELY, U-763) |
| 1 | Slopes 40 / 45 / 50 / 55 / 60 | `WalkableFloorZ` = cos 50 = 0.643 | 40, 45 walkable; 55, 60 not (PLAYER.md) |
| 1 | Mantle ledges 60 / 120 / 200 / 260 / 300 / 330 / 360 | a standing jump at the face: the finder runs on the press and every airborne tick, edges up to 230 above the feet, and the jump lifts the feet up to 113 (106.67 + 6.73) | 330 yes, 360 no (`DisTweaks_PlayerPawn_Mantle`; LIKELY, U-763) |
| 1 | Jump gaps 100 to 500 (platforms 240 high) | airtime 0.533 s at 360 (run) / 540 (sprint) uu/s | run clears 200, sprint 300 (measured airtime) |
| 2 | Blink: floor targets at 500 / 1100 / 1600, pillars 400 / 500 / 600, a 350 wall | range 1600 (level 2) / 1100 (level 1) horizontally, 20 deg up: 1385.7 ahead and 500 up; the Blink mantle | measured (HOOK_TRIAGE.md) |
| 2 | Climbable chain (Boyle basement's `DisClimbable_1`) beside a 500 tower | grab, climb 163.8 uu/s, jump off | PLAYER.md "Climbing" |
| 2 | Pool, 600 deep, stairs out | swimming; held under (looking down, crouch held), 30 s of air, then 1 health per 61 ticks (an idle swimmer floats with its head out) | measured (U-242) |
| 2 | Fall-damage stairs, ledges 880 / 1040 / 1200 / 1360 / 1520 | `trunc(70 (v - 2250) / 650)`, death above 2900 | 0x00B4DCC0, measured (U-462) |
| 3 | Doors: unlocked wood, unlocked reinforced, locked metal; a locker; a bin | native use (below) | — |
| 3 | Lean corner | lean 15 + 5 degrees | PLAYER.md "Lean" |
| 3 | Crouch tunnel, clearance 150 | sneaking (130) fits, standing (175) does not | `m_fSneakHeight` 65 |
| 3 | Pain floor, 10 per second | entry pain, then every `PainInterval` | Engine `PhysicsVolume` |
| 3 | Shooting gallery: passive guards and boards at 10 / 20 / 40 m (500 / 1000 / 2000 uu) | pistol and crossbow hits, dispersion | PLAYER.md "Ranged" |
| 3 | Sword dummies (passive guards) | a hit does 0.2 x 70 = 14 | HOOK_TRIAGE.md |
| 4 | Stealth lane: lamps over the west half, a roof over the east half, alcoves, crates; a patrolling guard | light at the player; the generated navigation | seen value adds 0.1 x light01 (HOOK_TRIAGE.md) |

Passive NPCs (`NpcDef.passive`) are spawned without a brain: they stand and take hits, which no
original NPC does (a test-level convenience; the original has no training dummies). The
patrolling guard has the original guard faction (`NpcDef.faction`) and the original AI.

## Doors (decoded for the native use, 2026-10-07)

Missions without the original Kismet had no way to open a door; a layout map's doors are used
natively (`dishonored_multiplayer::mission::{UsableDef, door_use}`). From `DisDoor` (class record
0x012F7A20, vtable 0x010E9DB0; Ghidra `ClassDecomp2`):

- **Use** (vtable slot 242, 0x00A37C10): with a user (`+0x2C8`), a closed door (`m_DoorState`
  +0x311 = 0) asks 0x00A2D2E0 which way to open:
  - 0 (behind): usable stage 0 (`DD_Stage_OpenCCW`), state 2 (opened CCW);
  - 1: stage 1 (`OpenCW`), state 1.
  An open door closes: from state 1 with stage 2 (`CloseCCW`), from state 2 with stage 3 (`CloseCW`),
  state 0 either way. The stage is set through 0x00A4EE60 (`+0x2D4`).
- **Side** (0x00A2D2E0): `(user.Location - door.Location) . axis < 0` means behind. The axis is
  `(1, 0, 0)` taken through the inverse of the skeletal mesh's `m_OriginTransform`
  (`USkeletalMesh` +0xA0, an Arkane field after `RotOrigin`, read by 0x00A1EB30 with
  0x00402550, a 4x4 inverse) and then through the skeletal mesh component's `LocalToWorld`
  (component +0x60 rows). The port takes the component's X axis in world space, assuming an
  identity origin transform for these meshes (LIKELY, U-760).
- **Kismet open** (0x00A1A450, `DisSeqAct_Door`): direction 0 gives state 1 (stage 1), direction 1
  gives state 2 (stage 0), only from closed.
- **Can use** (0x00A4E2E0): not while deleted, not when slot 240 says no, not while disabled
  (`m_bEnabled`, bit 1 of `+0x2B0`); and while a transition runs (`+0x2C4` is `m_State`,
  `EUsableObjectState`, not the lock flag) only when the active stage is hold-to-use or the tweak
  flag `+0xF4` bit 4 (`m_bKeepFocusedInTransition`) is set. CORRECTION (world.md F-19): an earlier
  reading took `+0x2C4` for the lock and the slot-241 call for a key check; the lock flag is
  `m_bLocked`, bit 0 of `+0x2B0`, consumed by the stage selector (0x00A1A750), and CanUse is not
  the lock test. The key match against `m_MatchingKeys` is not decoded (world.md U-W4).
  The port refuses every use of a locked door and shows the tweak's `m_LockedMessage`.
- Other usables (lockers, bins) go to their next stage, wrapping (the `DisSeqAct_ActivateUsable`
  default, `kismet_host`).

Not modelled: the transition takes the stage animation's time in the original, but the port's
door collision switches pose at once, as the Kismet doors already did. Usable objects' stage
poses are not drawn (the renderer poses doors only, U-761). Door sounds are not played either.

## Loadout and the bench (`MissionDef.loadout`, `Action::Equip`)

A mission can give its players powers and items when they spawn. This is the effect of the
original Kismet handlers `OnAddPower` and `AddInventoryItem` (what the hook's `lab power` and
`lab give` call). The `Equip` action does the same from a rule; the Playground's bench is two
usable markers with `Equip` rules. A loadout gives:
- every power at its top level (`all_powers`), or listed powers with their levels;
- every ammunition type to its capacity;
- elixirs, runes and coins;
- workshop upgrades by name.
Health and mana are then filled to their maxima. `reset` first returns to the default loadout.

## Lighting

A layout has no lightmaps, so the Playground is lit by dynamic lights only:
- a copy of Boyle's moon (`L_Boyle_Ext_Light:DominantDirectionalLight_1`, shadowed);
- a grid of work lights 2000 apart, 700 up, over rows 1 to 3 (UE3 falloff exponent 2, radius
  2200, brightness 1.6), so every face of a station is lit from two sides or more; one more in the
  pool's basin and one over the far gallery targets;
- two lamps over the lit half of the stealth lane. No work light reaches the lane: the row-3
  lights end at y 5200, and the lane starts behind its wall at y 6200.

## Surfaces

- **Materials.** Every brush uses a finished Boyle material:
  - ground: street cobbles; boundary: the brick facade, with dock stone above;
  - steps: sidewalk paving; slopes and stairs: the stone ramp; mantle ledges: dock wall;
  - gap platforms: bridge stone; Blink pads: the distillery's wood floor; pillars: brick; Blink wall: stone;
  - chain tower: warehouse brick, its beam a basement wood beam; pool: basement plaster tile;
  - fall ledges: metal plate; lean corner and tunnel: plaster, the tunnel roof planks;
  - gallery boards: planks; the lane's roof: wood floor; alcoves: mossy stone.
  `harness playground` refuses blockout or dev materials.
- **Winding.** Brush triangles are wound as the cooked meshes are, for UE3's clockwise front
  faces: (b - a) x (c - a) points into the solid, and the stored normal out of it. The renderer
  keeps the indices through the coordinate change's handedness flip. The first layout importer
  wound them the other way, so every brush showed only its inside faces, lit from behind. That
  was the user's report of missing walls, and of boxes seen as a black interior grid. Collision
  ignores the winding: rays turn the normal against the ray, and box checks use separating axes.
- **Render against collision.** `playground::audit_brushes`, run by `harness playground`, checks
  each brush:
  - a colliding brush draws exactly the triangles it collides with;
  - a brush without collision is a skin at most 4 thick (the water surface, the pain floor's gravel);
  - every triangle is wound outward.
- **Boundary.** The perimeter walls rise to 3000. The highest standing point is the fall-damage
  stairs' top at 1600, and a Blink adds 500. The harness casts rays from inside at 100, 1700 and
  2900 up.
- **UV density.** Planar world-space mapping:
  - 256 uu per texture repeat by default;
  - 400 on the ground slabs;
  - 512 on the upper boundary;
  - 1024 on the water.
- **Water.** The pool shows a surface 1 below the volume's top (z -31), using the Boyle river
  material (`boyle_ext_MS.Water_boyle_ext_INST03`), with no collision. The port plays no effect
  on entering, leaving or swimming in water, on any map: `SimEvent::Swim` has no effect hookup.
  Its only water particle is the contact system's `Pfx_WaterSplash`, for projectile impacts on
  surfaces whose contact type is Water.
The AI light probe samples the same lights (`dishonored_assets::ai_level::probe_light`). The global
player-light normalization applies, because a layout has no `DishonoredMapInfo`. This keeps the
light-versus-dark comparison of the stealth lane self-consistent. The absolute light values are
not those of any original level (U-762).
