# Target Map: Lady Boyle's Last Party (`L_Boyle_Ext_P` + `L_Boyle_Int_P`)

## Why this map

Mission 6, the Boyle estate masquerade, is the strongest available match for the
Contracts mode and is also a reasonable content-pipeline target:

- **Built-in social stealth.** The original mission is a masked party where the
  player has to work out which of three identically dressed women is the target.
  Many civilian guests in masks gives exactly the visual ambiguity Phase II needs.
- **Topology.** Streets and canal edges outside the estate, a walled garden and
  courtyard, the mansion's ground-floor halls, upper floors and balconies, and
  rooftops all around. Every layer from the ideal topology is present.
- **Loops.** The garden, the street perimeter and the mansion floors form several
  route loops, with no one-way progression once you are inside the estate.
- **Guards and civilians** exist in the original data, both inside and outside.
- **Size.** The exterior set is ~36 MB over 7 packages (Env 18.9 MB); the interior
  is ~44 MB.

Secondary candidate: the Hound Pits pub (`L_Pub_Day_P`, 27 MB). It is small and
enclosed, which makes it a good movement and Blink fixture bed if Boyle proves too
complex for controlled experiments.

## Required packages

Exterior (first vertical slice):

| Package | Size | Role |
|---|---|---|
| `L_Boyle_Ext_P` | 7.4 MB | persistent level: WorldInfo, PlayerStarts, streaming list |
| `L_Boyle_Ext_Env` | 18.9 MB | art geometry: StaticMeshCollectionActors, meshes, textures |
| `L_Boyle_Ext_Light` | 0.4 MB | lights (dominant directional/spot/point) |
| `L_Boyle_Ext_Nav` | 0.4 MB | navigation |
| `L_Boyle_Ext_Script` | 7.9 MB | Kismet, NPC spawners, movers |
| `L_Boyle_Ext_Fx`, `_Audio` | | effects, Wwise emitters |

Interior (next): `L_Boyle_Int_P`, `_Env`, `_Basement`, `_Light`, `_Nav`, `_Script`,
`_Stealth`, `_Fx`, `_Audio`.

Also related: `L_BoyleStreet_Ext_Script`, and `Boyle_AbstractInv_MS_SF` (UI/inventory).

## Known dependencies

- Texture mips stream from `Textures.tfc` (and probably `Lighting.tfc` for lightmaps).
- 137 resource references are imports that do not resolve to any Boyle package. Their
  source is unknown; candidates are `DishonoredGame.upk` and the startup packages
  (UNKNOWNS U-005).
- Light maps and shadow maps (`LightMapTexture2D`, `ShadowMap2D`) are not used yet.

## First import results (2026-10-04)

`map_dump L_Boyle_Ext_P` across the 7 exterior packages:

- 3,573 collection components, 5 StaticMeshActors, 3,401 mesh instances
- 405 unique static meshes, 0 decode failures; 122,821 render triangles;
  119,789 kDOP collision triangles
- 372 materials, 471 textures (2 V8U8 normal maps now mapped to RG8 snorm)
- 5 PlayerStarts, e.g. `(-2663, -1879, 98)`, yaw 180°
- Import time ~1.9 s (release)

## Anticipated blockers

- Materials are UE3 graphs. The renderer only picks a diffuse texture by heuristic,
  so some surfaces will be wrong (decals, blended terrain, emissive windows).
- No lightmaps yet, so lighting will look flat compared to the original.
- Interior and exterior are separate persistent maps in the original, joined by a
  load transition. Contracts will need both stitched into one world. The world
  offsets have to be checked (U-006).
- Some geometry may live in `BrushComponent`/`Model` (BSP) rather than static meshes
  (U-007).
