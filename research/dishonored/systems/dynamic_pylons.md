# Dynamic pylons and ArkSeqAct_ChangePylonConnection

Scope: `ArkDynamicPylon`, the navmesh pylon Arkane placed on movers (the Bridge water lock, the
Bridge drawbridge), and the Kismet action `ArkSeqAct_ChangePylonConnection` that links and unlinks
it. Evidence levels: CONFIRMED (decompiled exe / game data), LIKELY (the decompile says it, the
reading of a field is a name or a shape), OPEN (not decoded, not implemented). Addresses are in
the GOG exe (sha256 `70da9613...`). Port: `crates/dishonored_assets/src/navigation.rs` (reader),
`crates/dishonored_world/src/nav.rs` (`PylonRt`, `PylonLink`, cost hook, mesh move, follower
repath), `crates/dishonored_multiplayer/src/kismet_host.rs` (`ArkSeqAct_ChangePylonConnection`,
`update_dynamic_pylons`, the AI's navigation overlay `door_nav`). Static navmesh format:
`research/dishonored/NAVIGATION.md`.

## 1. Classes and data

| thing | what |
|---|---|
| actor | `ArkDynamicPylon` < `Pylon` < `NavigationPoint` (Engine, instance 0x3bc, constructor 0x0093c680, vtable 0x010a4100) |
| fields | `m_bIsDisconnected` +0x39c bit 0, `m_PawnsMovingThruPylon` +0x3a0 (array<Pawn>), `m_PreviousPosition` +0x3ac, `m_fCeilingHeight` +0x3b8 |
| defaults | `m_fCeilingHeight` 700, `bNeedsCostCheck` true, `bStatic` false (`Pylon`: `bNeedsCostCheck` false; `DisPylon` does not set it) |
| action | `ArkSeqAct_ChangePylonConnection` < `SequenceAction` (instance 0xf8, vtable 0x010a39e8); inputs 0 "Connect", 1 "Disconnect"; output "Out"; variable "Target" (`Targets`); `bCallHandler` false |

Shipped instances (ignored test `dynamic_pylons_retail` in `crates/dishonored_assets/tests/retail.rs`,
every `*_Nav` package):

| pylon | base chain | mesh | cross edges | Kismet |
|---|---|---|---|---|
| `L_Bridge_Part1a_Nav:ArkDynamicPylon_0` at (3488.3, -181.3, 0.2) | `InterpActor_34` (hard attach, `RelativeLocation` (26.4, -881.5, 733.2)) | 4 verts, 2 polys, 3 edges | 2 in itself, 1 each in `DisPylon_10` / `DisPylon_11` | matinee "WaterLock" `Started` -> Disconnect, `Reversed` -> Connect |
| `L_Bridge_Part1b_Nav:ArkDynamicPylon_0` at (19170, 360, 1100.5) | `InterpActor_18` (soft) -> `InterpActor_2` (hard) | 63 verts, 44 polys, 129 edges | 22 in itself, 1 each in `DisPylon_5` / `DisPylon_6` | remote event `PlayDrawbridge` -> Disconnect, `Drawbridge_Lowered` -> Connect; the drawbridge matinee moves `InterpActor_2` |

The 1a pylon's `Location` is its base's placed transform applied to `RelativeLocation`
(4130.3, -786.0, -733.0) + R(yaw 40960) (26.4, -881.5, 733.2) = (3488.3, -181.3, 0.2) (CONFIRMED,
the data). Its mesh is the lock platform; before this port the reader skipped the class, so the
lock and the drawbridge were missing from the AI's navmesh and the cross-pylon edges into them
did not resolve.

### Mesh serialization (CONFIRMED data)

The meshes of an `ArkDynamicPylon` (walkable and obstacle) carry one more array than the static
`DisPylon` ones, between `WorldToLocal` and `BoxBounds`:

```text
TArray<FBorderEdgeInfo> { u16 Vert0; u16 Vert1; u16 Poly }    // 6 bytes per element
```

1a's walkable mesh has 4: (1,2,0) (2,0,0) (3,1,1) (0,3,1), the outline of its two triangles. With
the array read, every shipped dynamic mesh decodes to its last byte; no static mesh has it. The
serializer's condition was not found in the exe (OPEN: the reader keys it on the pylon class, which
matches all shipped data). The port reads and drops the array (none of the run-time paths below
read it).

`ArkDynamicPylon::PostBeginPlay` (slot 131, 0x00934b60, CONFIRMED): after `AActor::PostBeginPlay`
it overwrites `LocalToWorld` (+0x150) and `WorldToLocal` (+0x190) of both `NavMeshPtr` and
`ObstacleMesh` with the identity (0x0126b130), registers itself in a list of the world object (0x00dbf7f0 +0x298), sets `bHasCrossLevelPaths` (+0x248 bit 28), re-adds itself to the
navigation octree (slots 240 / 239), clears `m_bIsDisconnected` and sets `m_PreviousPosition` =
`Location`. So a dynamic mesh's vertices are used as stored (the reader skips the transform for
them) and it starts connected.

## 2. Rules

DP-1 **ChangePylonConnection** (CONFIRMED, `Activated` slot 93, 0x00937090). For every target
from last to first that `IsA(ArkDynamicPylon)` (class via 0x00933a70): if input 0 (Connect) has an
impulse, clear `m_bIsDisconnected` (nothing else runs); else if input 1 (Disconnect) has one and
the flag is clear, set it and call slot 245. Then `SequenceAction::Activated` (0x006eac90): the
automatic output. Connect does not move or relink anything.

DP-2 **Abort the pawns moving through** (CONFIRMED slot 245, 0x00934c40). For each pawn of
`m_PawnsMovingThruPylon`, its AI component container (pawn +0x3e4, 0x00943110 -> 0x00942fd0 type
0x65) gets flag 0x40 at +0x104 (0x0093e620): its navigation process repaths. Pawns are added and
removed by slots 243 / 244 (0x00937030 / 0x00937060, through 0x0093e970). OPEN: where slots 243 /
244 are called from (which pawns count as "moving through"). Port: a `Follower` records the
dynamic pylons whose polys its corridor crosses with their abort counts and repaths the next tick
when one changes (LIKELY equivalent: a pawn whose path crosses the pylon).

DP-3 **The pylon moves with its base** (CONFIRMED; the slot name LIKELY). In game, `Pylon` slot 204
(0x00660240, LIKELY `UpdateComponentsInternal`) calls slot 249 whenever the pylon's
`CylinderComponent` needs an update (it moved; in the editor always). `ArkDynamicPylon` slot 249
(0x009345b0): first `Pylon`'s (octree remove / add); then, with `GIsGame` (0x0141b21c):
- if not disconnected: set `m_bIsDisconnected` and call slot 245 (DP-2). A moving pylon is
  always disconnected; only Kismet Connect reconnects it.
- delta = `Location - m_PreviousPosition`; if |delta| >= 50 on any axis: add delta to every
  vertex of `NavMeshPtr` (+0x314) and `ObstacleMesh` (+0x318), recompute each poly's bounds and
  centre from its vertices (slot 247 for the normal), refresh the edges and the meshes' octrees
  (0x00685c50, 0x00685b70, 0x0066ac40, 0x0066a910, 0x0067b6f0), then `m_PreviousPosition` =
  `Location`. Translation only: the mesh is never rotated, even when the base turns (the
  drawbridge pitches; its mesh is carried along the arc in 50 uu steps, unrotated).

The pylon's `Location` follows its `Base` chain by the engine's attachment (hard: base transform
x `RelativeLocation`; soft: moved by the base's motion); both are a rigid carry of the placed
location. Port (`KismetRun::update_dynamic_pylons`): the pylon is at
`D x placed`, D = the Kismet's current x placed transform of the nearest base in its chain that
the Kismet has moved (`mover_delta`); when that position differs from the last one by more than
0.05 uu (`PYLON_MOVE_EPSILON`, a float round-trip guard of the port) slot 249 runs
(`PylonLink::moved`).

DP-4 **Cost of a disconnected pylon** (CONFIRMED). Path search costs each edge through
0x00672ae0 (in-mesh edges: the owning mesh's pylon) or 0x0067ab00 (cross-pylon edges: the pylon
of poly 0, else of poly 1, the first with `bNeedsCostCheck`; 0x0067ace0 sends a path-object edge
whose actor is known to that actor's `CostFor` instead). Only then is the pylon's `CostFor`
(slot 251) called. `ArkDynamicPylon::CostFor` (0x00933340): if `m_bIsDisconnected` and the edge
is cross-pylon (edge +0x2d set: the byte 0x0067a230 also uses to resolve the poly through its
`PolyReference`), the edge cost becomes 10,000,000 and it returns true. In-mesh edges of the
pylon are untouched (a disconnected pylon stays walkable inside), and the cost is not a wall: with
no other way the route still crosses (port: `NavWorld::pylon_cost`, used by `corridor`).

DP-5 **Ceiling** (CONFIRMED slot 246, 0x009333f0): the pylon's ceiling point is the given point
raised by `m_fCeilingHeight`, or by the pawn's +0x4f0 value when that is <= 0. Not used by the
port's path queries (OPEN for the port: the poly heights it bounds come from the cooked mesh).

DP-6 **Save** (CONFIRMED slots 69 / 70, 0x00933370 / 0x009333b0): `m_bIsDisconnected` is saved
and restored (one byte). Port: `HostState::pylons` (the connection, location, mesh position and
abort count per pylon) is part of the saved Kismet host state.

## 3. Port status

| piece | status |
|---|---|
| reader: `ArkDynamicPylon` meshes (border edges, identity transform), base chain, flags | done (`read_package_pylons`, `NavPylon::dynamic`) |
| cross-pylon edges into dynamic pylons resolve | done (the pylon is now in `NavData`) |
| DP-1 Connect / Disconnect | done (`ArkSeqAct_ChangePylonConnection` in `kismet_host.rs`) |
| DP-2 repath of pawns crossing the pylon | done by corridor (`Follower::pylons`); the original's registration point is OPEN |
| DP-3 follow the base, disconnect on move, 50 uu translation steps | done (`update_dynamic_pylons`, `PylonLink::moved`, `NavWorld::translate_pylon`) |
| DP-4 10,000,000 cross-edge cost | done (`NavWorld::pylon_cost`) |
| DP-6 save | done (`HostState::pylons`, serde default) |
| nav cache | `cache/<map>.nav2.bin` (pylon table added; old `.nav.bin` files are ignored) |
| base chains of movers | OPEN outside this system: the port's mover collision (`place_movers`) moves only actors the Kismet moves itself; `InterpActor_18` (part of the drawbridge, hard-attached to `InterpActor_2`) is not carried. The dynamic pylon is (it walks its own chain). |

Tests: `dishonored_world` `nav::tests::{disconnected_dynamic_pylon_costs_its_cross_edges,
disconnected_edge_is_a_cost_not_a_wall, moving_dynamic_pylon_disconnects_then_moves_its_mesh_in_50_uu_steps,
follower_repaths_when_a_crossed_pylon_aborts}`; `dishonored_multiplayer`
`kismet_host::tests::change_pylon_connection_and_a_moving_base_drive_the_ai_navigation`; retail dump
`dishonored_assets` `dynamic_pylons_retail` (ignored).
