# Navigation: the original navigation mesh

Dishonored's AI walks on a **UE3 navigation mesh** (the Pylon / `NavigationMeshBase` system of
UDK-era Unreal Engine 3), not a PathNode/ReachSpec network and not a custom Arkane format.
Each map streams one `L_*_Nav` level that holds a single `DisPylon` actor (Arkane subclass of
`Engine.Pylon`) owning two `NavigationMeshBase` objects: the walkable mesh and the obstacle
(boundary) mesh. Doors and breakable blockers are UE3 *path objects* that own dedicated
polygons and gate the edges into them.

Code: `crates/dishonored_assets/src/navigation.rs` (reader), `crates/dishonored_ir/src/nav.rs`
(IR), `crates/dishonored_world/src/nav.rs` (pathfinding). Tool: `nav_dump survey | map <map> |
comps <map> | obj <map> <out.obj>` (map_dump). All findings: GOG Definitive Edition.

## Package contents (`L_Boyle_Ext_Nav`)

| Export | Class | Notes |
|---|---|---|
| `DisPylon_5` | `DisPylon` | `m_bEnableTallBoyNavigation=true`, `ExpansionRadius=58181`, `bImportedMesh`, `bHasCrossLevelPaths`, `NavGuid`, `Location=(0,0,-101)`. Native tail: two object refs `NavMeshPtr`, `ObstacleMesh` (8 bytes, CONFIRMED by exact size). |
| `DisPylon_5.NavigationMeshBase_8708` | `NavigationMeshBase` | walkable mesh, 179 KB |
| `DisPylon_5.NavigationMeshBase_8709` | `NavigationMeshBase` | obstacle mesh, 321 KB (3612 single-edge wall polys, no edges) |
| 14 x `NavMeshBoundsVolume` | | generation bounds |
| `DishonoredMapInfo_5` + `DisPatrolManager_5`, `DisFleePointManager_5`, `DisAnimStatePool_5` | | runtime managers, no serialized data (12-byte exports) |
| `DisStaticCollisionActor_15` + `npcmovementcollision_model` | | NPC-only movement collision (`bCollideOnlyWithNPCMovement`) |

No `PathNode`, `ReachSpec`, `NavigationPoint` network exists anywhere in the Nav levels.
`DishonoredNavPoint`s live in the script levels; their `nextNavigationPoint` chain is the
engine's flat `NavigationPointList` (one linear chain over all points of a level, ending in
null), not patrol links (CONFIRMED by walking the chains in four Boyle packages).

## `NavigationMeshBase` native serialization (version 28)

After the tagged properties (only `None`), CONFIRMED by exact byte accounting on all 40
packages that contain pylons (every export ends exactly where the last edge record ends):

```text
i32  NavMeshVersionNum            = 28
i32  VersionAtGenerationTime      = 28
TArray<FMeshVertex>               FVector + TArray<u16> ContainingPolys
TArray<FEdgeStorageDatum>         u32 DataPtrOffset, u16 DataSize, FName ClassName
                                  (in-memory layout: 64 B base, 152 B path-object edges)
TArray<FNavMeshPolyBase>          TArray<u16> PolyVerts, TArray<u16> PolyEdges,
                                  FVector PolyCenter, FVector PolyNormal,
                                  FBox BoxBounds (min, max, u8 valid), f32 PolyHeight
FMatrix LocalToWorld, FMatrix WorldToLocal      (identity in every shipped mesh)
FBox    BoxBounds
edge records, in FEdgeStorageDatum order:
  FNavMeshEdgeBase / FNavMeshBasicOneWayEdge   29 B
      u16 Vert0, Vert1, Poly0, Poly1
      f32 EdgeLength, EffectiveEdgeLength
      FVector EdgeCenter
      u8  EdgeType (ENavMeshEdgeType)
  FNavMeshCrossPylonEdge                       81 B
      u16 Vert0, Vert1, Vert0', Vert1'   (the second pair repeats the vertices; meaning UNKNOWN)
      i32 -1                              (UNKNOWN)
      f32 EdgeLength, EffectiveEdgeLength; FVector EdgeCenter; u8 EdgeType
      FPolyReference x2: i32 pylon object ref, FGuid, u16 poly, u16 sub-poly (0xFFFF)
  FNavMeshPathObjectEdge                       105 B = cross-pylon record +
      FActorReference PathObject: i32 actor ref (0 = other level), FGuid
      i32 InternalPathObjectID
```

`ENavMeshEdgeType` (read from the `Pylon.ENavMeshEdgeType` enum in `Engine.upk`):
0 Normal, 1 Mantle, 2 Coverslip, 3 SwatTurn, 4 DropDown, 5 PathObject, 6 BackRefDummy,
7 Jump, 8 Removed.

Confidence: layout CONFIRMED (exact sizes everywhere, values cross-checked: vertex lists of
polys contain the edge vertices, edge `Poly0/Poly1` own the edge, path-object GUIDs match
level actors). Field names follow UE3 conventions (LIKELY); the two unknown fields of the
cross-pylon prefix are documented as such.

Units/frame: world space (LocalToWorld identity), Z-up uu. The polys sit on the floor:
median poly-centre height minus collision floor = +3.0 uu (p10 -2.4, p90 +12.1) on Boyle Ext
(`crates/dishonored_content/tests/real_nav.rs`).

## Whole-game survey (`nav_dump survey`)

40/40 pylon packages decode (38 `L_*_Nav` levels plus a few script levels with tiny pylons).
Totals: 52,882 Normal edges, 4,279 Removed, 1,761 one-way (21 Removed), 4,298 path-object,
4 cross-pylon (the two Bridge Part 1 levels, each with two pylons).

**No Mantle, DropDown, Jump, Coverslip or SwatTurn edge exists in any shipped mesh.** NPC
traversal is restricted to the walkable surface plus doors/blockers; climbing, drops and
jumps are not navmesh features in Dishonored (NPC "jump away" moves are behaviour-level).

| Map | Polys | Edges | Area (x100 uu²) | Notes |
|---|---|---|---|---|
| L_Boyle_Ext_Nav | 984 | 1345 | 10,664 | tallboy navigation enabled |
| L_Boyle_Int_Nav | 684 | 1048 | 3,598 | 132 path-object edges (doors) |
| L_Flooded_FGate_Nav | 2537 | 4355 | 10,373 | largest |
| L_Tower_Nav | 668 | 866 | 7,120 | |

## Path objects (doors and blockers)

Path-object edges reference their owner by GUID:

- `DisDoor` (in `L_Boyle_Ext_Audio`): GUID = `NavGuid`; also stores `m_DoorCwNavPolyRef` /
  `m_DoorCcwNavPolyRef` and `m_bLocked`. Doors own a strip of polys (e.g. polys 969-970 for
  `DisDoor_0`) connected to neighbours by paired path-object edges, one per direction.
- `DishonoredBreakableNavBlock` (wooden planks, `L_Boyle_Ext_Env`): GUID = `m_NavGuid`, plus
  `m_NavPolyRef` (292 entries for `_0`) and `m_bIsCurrentlyBlockingNavigation`. A blocker owns
  polys entered through path-object edges (outside -> blocker) and left through
  `FNavMeshBasicOneWayEdge`s (blocker -> outside), so an NPC caught inside can always leave.

Boyle Ext: 22 path objects (8 doors, 4 locked; 14 blockers, 7 blocking at level start).

One-way edge direction: the edge is listed in the `PolyEdges` of the poly that can use it
(the blocker poly) and leads to the other poly (CONFIRMED on all 23 Boyle one-way edges:
they all leave blocker polys 977-983).

## Connectivity (Boyle Ext)

8 islands (undirected, all edges): 554 polys around the west street/approach (centroid
-8957,-937), 353 polys in the estate courtyard and mansion surroundings (-1821,-2499), 45 polys
on the north-west rooftop area, and five small pieces. The two big islands share no edge, not
even a `Removed` one: guards of the estate and the street never path between them. 236/257
player starts and AI points are within 200 uu of the mesh; same-island pairs connect 100%
when blocking path objects are ignored and 90% with the level-start door/blocker states.

## Runtime (`dishonored_world::nav`)

- Graph: polys are nodes. Base edges link both ways, one-way edges owner -> other,
  cross-pylon/path-object edges `poly_refs[0] -> poly_refs[1]`, `Removed` edges skipped.
  Path-object edges are closed while their door is locked or their blocker intact (state
  can be changed with `NavWorld::set_blocking`); `EffectiveEdgeLength - EdgeLength` is added as
  traversal cost on path-object edges.
- Queries: spatial hash of poly bounds; `locate` picks the highest poly under a point within a
  vertical window (pawn centres sit ~90 uu above the floor); `closest_point` snaps off-mesh
  points within a radius.
- A* over polys with edge centres as entry points (UE3's navmesh search also costs moves
  between edge centres; LIKELY close, the exact UE3 heuristics and per-edge cost functions
  are not reproduced), then a funnel (string pulling) over the shared edges, with corners
  kept one pawn radius (31 uu) off the portal ends.
- `Follower`: per-agent replanning (every second or when the goal moves), waypoint
  advancing, plain serializable data (NPC brains stay deterministic and replayable).
- The collision world carries the map's navigation (`CollisionWorld::nav`, cached as
  `cache/<map>.nav.bin`); everything with a `WorldQuery` reaches it through `nav()`.

## Gaps

- Movers: `InterpActor` components collide where the Kismet has moved them (`Layered` over the static
  world); other movable actor classes (`KActor`, `DisDoor`, usables) still do not collide.
- The obstacle mesh is decoded but unused (walls come from the collision world).
- `npcmovementcollision_model` (NPC-only blockers) is not added to the NPC collision.
- `bHasCrossLevelPaths`: Ext/Int pylons are separate maps here; cross-level edges between
  streamed pylons only exist in the Bridge levels and are decoded but untested at runtime.
- UE3 edge `EffectiveEdgeLength` (the width a pawn can use) is not checked against pawn size.
- Door opening animation/time and keys are not modelled; unlocked doors are passable,
  locked ones closed.
