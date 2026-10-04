# Dishonored Package Format (UE3 v801 / licensee 30)

Applies to: GOG Definitive Edition, `Dishonored.exe` SHA-256
`70da96138fe2171cca18888a0adbb731447cd15758e55535672625f09f6bc6fb`.
Implementation: `crates/ue3_package`, `crates/dishonored_assets`.

Confidence key: **CONFIRMED** = implemented and validated against the full install;
**STRONG** = validated on the target map; **LIKELY** = matches reference code, not yet
exercised.

## Validation status

| Check | Result |
|---|---|
| Open + decompress + names/imports/exports, all 470 base-game `.upk` | 470/470 (CONFIRMED) |
| Export table ends exactly at `DependsOffset` (strict check in reader) | every package (CONFIRMED) |
| UObject header + tagged properties, all non-script exports in the base game | 1,260,662 ok / 1,079 failed before the template-name fix (0.09%), see below |
| Same, all 17 `L_Boyle*` packages | 74,408 / 0 failures (CONFIRMED) |
| `UStaticMesh` LOD0 decode, Boyle Ext | 405 / 0 failures (STRONG) |
| `UTexture2D` decode incl. TFC streaming, Boyle Ext | 471 ok (STRONG) |

## File summary (`FPackageFileSummary`) — CONFIRMED

All little-endian.

```
u32  Tag = 0x9E2A83C1
u16  FileVersion = 801
u16  LicenseeVersion = 30
i32  HeaderSize
FStr FolderName                ("None")
u32  PackageFlags              (0x228A0009 on level packages)
i32  NameCount,  NameOffset
i32  ExportCount, ExportOffset
i32  ImportCount, ImportOffset
i32  DependsOffset
i32  ImportExportGuidsOffset, ImportGuidsCount, ExportGuidsCount
i32  ThumbnailTableOffset
Guid PackageGuid
TArray<{i32 ExportCount, i32 NameCount, i32 NetObjectCount}> Generations
i32  EngineVersion = 9098
i32  CookerVersion = 133
u32  CompressionFlags = 2 (LZO)
TArray<{u32 UncompressedOffset, u32 UncompressedSize, u32 CompressedOffset, u32 CompressedSize}> CompressedChunks
u32  PackageSource
TArray<FString> AdditionalPackagesToCook
```

`FString`: `i32 len`; `len > 0` ANSI including NUL, `len < 0` UTF-16 of `-len` units.

## Compression — CONFIRMED

The summary is stored raw. Everything from the first chunk's `UncompressedOffset` on
is stored as chunks. All table offsets and `SerialOffset`s address the
**uncompressed** image. `ue3_package` rebuilds the full image in memory:
header bytes `[0, first.UncompressedOffset)` copied raw, then each chunk inflated
at its `UncompressedOffset`.

Chunk framing (`FCompressedChunkHeader`):
```
u32 Tag = 0x9E2A83C1
u32 BlockSize (0x20000)
u32 TotalCompressed, u32 TotalUncompressed
{u32 Compressed, u32 Uncompressed} x ceil(TotalUncompressed / BlockSize)
block payloads (LZO1X), back to back
```
Decoded with the pure-Rust `lzokay-native` crate (MIT).

The same framing is used for LZO-compressed bulk data (flag `0x10`), e.g. texture
mips in `.tfc` files.

## Name table — CONFIRMED
`FString Name; u64 Flags`. `FName` in streams = `i32 Index, i32 Number`;
`Number > 0` renders as `Name_{Number-1}`.

## Import table — CONFIRMED (28 bytes)
`FName ClassPackage, FName ClassName, i32 Outer, FName ObjectName`.

## Export table — CONFIRMED (68 + 4·N bytes)
```
i32  Class, Super, Outer
FName ObjectName
i32  Archetype
u32  ObjectFlagsHigh, u32 ObjectFlagsLow      <- high dword stored FIRST
i32  SerialSize, SerialOffset
u32  ExportFlags
TArray<i32> GenerationNetObjectCount
Guid PackageGuid
u32  PackageFlags
```
Object references: `0` null, `>0` export `i-1`, `<0` import `-i-1`.

Relevant flags (64-bit, after recombining):
`RF_HasStack = 0x0000000002000000`, `RF_ClassDefaultObject = 0x0000020000000000`,
`RF_ArchetypeObject = 0x0000040000000000`, load flags `0x70000`, `RF_Public = 0x4`.

Evidence for the dword order: placed actors with script state (e.g. Boyle Ext
`StaticMeshActor_1737`) only parse when `0x02000000` is read from the *second* dword;
class default objects carry `0x200` in the *first* dword.

## Object serialization (export payload) — CONFIRMED for level content

```
[Dominant*LightComponent only]  TArray<u16> DominantLightShadowMap
[RF_HasStack]  i32 Node, i32 StateNode, u64 ProbeMask, u16 LatentAction, [i32 Offset if Node != 0]
[Component classes]  i32 TemplateOwnerClass, [FName TemplateName if owned by a CDO/Class]
i32 NetIndex
tagged properties ... "None"
class-specific native data
```

- The state frame has **no StateStack array** at this version (22 bytes when `Node != 0`).
- "Component classes" = class name ends in `Component`, plus `Distribution*`
  (UDistributionFloat/Vector derive from UComponent). A level prefab component
  flagged `RF_ArchetypeObject` does **not** have a TemplateName.

### Property tag — CONFIRMED
```
FName Name ("None" terminates)
FName Type
i32   Size
i32   ArrayIndex
StructProperty: FName StructName
BoolProperty:   u8 Value  (Size = 0)
ByteProperty:   FName EnumName; payload is FName when Size==8, u8 when Size==1
payload[Size]
```
Native (immutable) structs serialized raw: `Vector`(12) `Rotator`(12, i32 P/Y/R, 65536 = 360°)
`Color`(4, BGRA) `LinearColor`(16) `Vector2D`(8) `Guid`(16). Other structs are nested
tagged properties. Arrays: `i32 Count` + elements; element type comes from class
metadata, so callers decode arrays (`array_objects`, `array_structs`).

Dishonored-specific struct observed on every StaticMeshComponent:
`m_CollisionTraceTypes: DisPrimTraceMask { m_bTraceForGameplay_Crosshair, _Projectile,
_Melee, _VisionLOS }` — per-primitive gameplay trace channels. Directly relevant to
Blink targeting, projectile and melee traces, and AI vision (see UNKNOWNS U-012).

## `UStaticMesh` native data — STRONG

```
FBoxSphereBounds { Vector Origin, Vector BoxExtent, f32 SphereRadius }
i32 BodySetup
BulkArray<kDOPNode, 32 B>   { Vector Min, Vector Max, i32 bIsLeaf, i16 a, i16 b }   <- legacy layout (Dishonored quirk)
BulkArray<kDOPTriangle, 8 B> { u16 v0, v1, v2, u16 MaterialIndex }
i32 InternalVersion (18)
TArray<LOD> (count, then LOD0..):
  FByteBulkData RawTriangles   (flags,count,size,offset + inline payload)
  TArray<Section> {
    i32 Material, u32 EnableCollision, u32 OldEnableCollision, u32 bEnableShadowCasting,
    u32 FirstIndex, u32 NumTriangles, u32 MinVertexIndex, u32 MaxVertexIndex, i32 MaterialIndex,
    TArray<{i32 BaseIndex, i32 NumPrimitives}> Fragments,
    u8 LoadPS3Data (0 on PC)
  }
  Position stream: u32 Stride, u32 NumVerts, BulkArray<Vector>
  Tangent/UV stream: u32 NumTexCoords, u32 ItemSize, u32 NumVerts, u32 bFullPrecisionUVs,
                     BulkArray<{FPackedNormal TangentX, FPackedNormal TangentZ, UV[NumTexCoords] (half2|float2)}>
  Color stream: u32 Stride, u32 NumVerts, [BulkArray<FColor> if NumVerts > 0]
  u32 NumVertices
  BulkArray<u16> Indices
  BulkArray<u16> WireframeIndices
  ... (remaining LOD/mesh fields not needed yet)
```
`BulkArray` = `i32 ElementSize, i32 Count, payload`. `FPackedNormal` byte `b` → `b/127.5 - 1`.
The kDOP triangle list is the mesh's per-poly collision and indexes the LOD0 position
stream; it is the first collision source for M5.

## `UTexture2D` native data — STRONG

```
FByteBulkData SourceArt
TArray<{FByteBulkData Data, i32 SizeX, i32 SizeY}> Mips
Guid TextureFileCacheGuid
(optional further mip arrays — ignored)
```
`FByteBulkData` = `u32 Flags, i32 ElementCount, i32 SizeOnDisk, i32 OffsetInFile` then
the inline payload unless flags contain `0x01` (separate file), `0x20` (unused) or `0x40`
(elsewhere in this package). `0x10` = LZO chunk framing.

Streamed mips: flag `0x01`, file = `<TextureFileCacheName>.tfc` in `CookedPCConsole`,
read `SizeOnDisk` bytes at `OffsetInFile`, then LZO-decode.

Formats seen in Boyle Ext: `PF_DXT1` (sRGB and linear), `PF_DXT5`, `PF_V8U8` (normal maps).
`SRGB=false` property marks linear textures (normal maps).

## Level content

- Placed actors: outer = `TheWorld.PersistentLevel`. Exports under other outers
  (e.g. `Env_Blockout.collisions.*`) are archetypes/prefab templates, not placed.
- Most static geometry: `StaticMeshCollectionActor`, property `StaticMeshComponents`
  (array of component refs) followed natively by one `FMatrix` (16 × f32, row-vector,
  translation in row 3) per entry = the component's `CachedParentToWorld`. The component's
  own `Translation/Rotation/Scale/Scale3D` apply on top (`world = ParentToWorld * local`).
- `StaticMeshActor`: `Location`, `Rotation`, `DrawScale`, `DrawScale3D` →
  `T * R * S`; component local transform composed on top.
- `PlayerStart` actors give spawn candidates.
- Rotator → matrix uses UE3 `FRotationMatrix` (see `dishonored_assets::util`).

## Precomputed lighting on static mesh components — STRONG

Implementation: `dishonored_assets::lightmap`. Validated on all 3,401 Boyle Ext static mesh
components (3,387 with a texture lightmap, 14 without, 0 vertex lightmaps, 0 parse
failures; the strict check that the stream ends exactly at the export end holds for all).

`StaticMeshComponent` native data (after `None`), identical for components of
`StaticMeshActor` and `StaticMeshCollectionActor`:
```
TArray<FStaticMeshComponentLODInfo> LODData:
  TArray<UShadowMap2D*> ShadowMaps
  TArray<UShadowMap1D*> ShadowVertexBuffers
  FLightMapRef:
    u32 LightMapType          0 = none, 1 = FLightMap1D (vertex), 2 = FLightMap2D
    TArray<FGuid> LightGuids  (lights baked into this lightmap)
    FLightMap2D (type 2):
      3 x { UTexture2D* Texture, FVector ScaleVector }     <- 3 floats, not FVector4
      FVector2D CoordinateScale
      FVector2D CoordinateBias
  u8 bLoadVertexColorData     (0 on Boyle Ext; non-zero is followed by FColorVertexBuffer:
                               u32 Stride, u32 NumVerts, BulkArray<FColor>)
```
Lightmap UV = `UV[LightMapCoordinateIndex] * CoordinateScale + CoordinateBias`.
Example (`StaticMeshActor_1737.StaticMeshComponent0`): textures
`NormalizedAverageColor0_139` scale (1,1,1), `DirectionalMaxComponent0_139` scale
(8.32, 5.27, 6.98), null scale (3.49, 4.92, 5.78); UV scale (1/32, 1/4), bias (0.78125, 0.25).

### Dishonored's directional lightmap encoding — LIKELY
This is **not** UE3's classic 3-coefficient (+ simple) directional encoding. Every texture
lightmap has exactly two non-null textures, always created in pairs with the same suffix:

| Slot | Texture name | Scale | Meaning (inferred) |
|---|---|---|---|
| 0 | `NormalizedAverageColor0_N` | always (1,1,1) | chroma: average incident colour normalized to its max channel |
| 1 | `DirectionalMaxComponent0_N` | HDR, per lightmap (≈1–10) | intensity along each of 3 tangent-space basis directions in R, G, B |
| 2 | null | non-zero | the UE3 "simple lightmap" coefficient, stripped by the cooker on PC |

Both are `LightMapTexture2D`, `PF_DXT1`, sRGB (class default), streamed from `Lighting.tfc`,
packed as atlases (several components per texture, disjoint rects; scale vectors are
per component, not per texture). `ULightMapTexture2D` native = `UTexture2D` native + one
extra zero dword + `u32 LightmapFlags` (1 = `LMF_Streamed`).

Renderer reconstruction (APPROXIMATION, see LOG and UNKNOWNS U-016):
`irradiance = srgb(avg) * Scale0 * mean_i(srgb(dirmax)_i * Scale1_i)` — the exact value for
a flat normal if the original weights the three basis intensities by `saturate(N·B_i)²`
normalized to sum 1. Per-pixel normal-map interaction is dropped. Each component's rect
(padded by 1 texel) is baked with its own scales into an `RGB9E5` atlas that keeps the
original layout, so Bevy's `Lightmap { uv_rect }` uses the UE3 bias/scale directly.

### Shadow maps
`ShadowMap2D` (one per light per component, 888 in Boyle Ext Env): properties
`Texture` (→ `ShadowMapTexture2D`, `PF_G8`, `SRGB=false`, `Lighting.tfc`),
`CoordinateScale`, `CoordinateBias`, `LightGuid`, `bIsShadowFactorTexture` (false = signed
distance field shadows). They hold precomputed shadowing for lights that are not baked into
the lightmap (dominant/toggleable). Not used by the renderer yet; the dominant light uses
Bevy's real-time shadow maps instead.

## Remaining parse gaps

- Script packages: component templates under class defaults and a handful of exotic
  component classes (scene captures, SpeedTree, WorldRain) — see the full-install sweep.
- `USkeletalMesh`: Dishonored adds `FUserBounds` and `TArray<u8> EdgeSkeleton` (from
  UEViewer); not implemented yet.
- `AnimSequence`: Edge animation compression, no public decoder (UNKNOWNS U-008).
