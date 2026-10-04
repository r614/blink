# Sony Edge animation clips in Dishonored (`ACF_EdgeAnim`)

Every `AnimSequence` in the install (113,232) uses Edge compression on PC. There is no public
decoder: UModel never supported it, and the only known tool ships in a leaked Sony SDK. That
SDK was **not** used. This format was reverse engineered from the retail PC binary and
validated against the data.

Applies to: `Dishonored.exe` SHA-256 `70da96138fe2171cca18888a0adbb731447cd15758e55535672625f09f6bc6fb`.
Implementation: `crates/dishonored_assets/src/edge_anim.rs`. Tools: `exe_probe` (disassembly),
`edge_probe` (structure/validation), `anim_sheet` (stick-figure renders), `edge_survey`.

## Code map (VA in the hash above)

| Address | Role | Evidence |
|---|---|---|
| `0x009DBC10` | blend-tree evaluator (SPU job emulation, stages by jump table) | only reference to tag `0x45413035` (`cmp dword [eax], 'EA05'`) at `0x009DBE99` |
| `0x009DADA0` | frame-set decompressor (dispatch per channel type) | called from the clip stage; reads header `+0x16..+0x26`, `+0x40..+0x50` |
| `0x009CD3A0` | constant-channel decoder (generic bit-packing) | fully reproduced |
| `0x009CE380` | animated-channel decoder (generic) | args established; semantics matched empirically (below) |
| `0x009D4D90` / `0x009D5390` | fixed-format rotation path (flag bit 0 clear) | not yet needed |
| `0x009D6CA0` / `0x009D6D80` | fixed-format vec3 path (flag bit 1/2 clear) | not yet needed |
| `0x009DB640` | pose blend (joint stride 0x30 = rot/trans/scale) | call sites at `0x009DC936..` |
| `0x00EC3D14` | memcpy (DMA emulation) | |

## Container

`AnimSequence` native data after tagged properties: `i32 RawAnimationData count (0)`, then
`TArray<u8> CompressedByteStream`, which holds the Edge blob. Header integers are
little-endian (converted at cook). **Bit streams remain big-endian, MSB-first**: the decoder
loads 16 bytes at `bitpos >> 3` and byte-swaps each dword (`psrlw/psllw 8` then `pslld/psrld 16`).

## Header (all offsets from blob start)

| Off | Type | Field | Confidence |
|---|---|---|---|
| 0x00 | u32 | tag `0x45413035` | CONFIRMED |
| 0x04 | f32 | duration (s) | CONFIRMED (matches SequenceLength) |
| 0x08 | f32 | sample frequency (Hz) | CONFIRMED (evaluator multiplies time by it) |
| 0x0C | u16 | header size = first frame set offset | CONFIRMED |
| 0x0E | u16 | num joints (= AnimSet track count) | CONFIRMED |
| 0x10 | u16 | num frames | CONFIRMED |
| 0x12 | u16 | num frame sets | CONFIRMED |
| 0x14 | u16 | unknown (0x2000) | UNKNOWN |
| 0x16..0x24 | 8 × u16 | const R, const T, const S, const user, anim R, anim T, anim S, anim user channel counts | CONFIRMED |
| 0x26 | u16 | flags: bit0/1/2 = R/T/S use generic bit-packing specs | CONFIRMED (branches at `0x009DB0B3`, `0x009DB1AF`, `0x009DB2A5`) |
| 0x38 | u32 rel | frame-set DMA table (8 B/entry: u16 ?, u16 size, u32 offset) | CONFIRMED |
| 0x3C | u32 rel | frame-set info (4 B/entry: u16 start frame, u16 ?) | CONFIRMED |
| 0x40..0x4C | u32 rel | const R / T / S / user data | CONFIRMED |
| 0x50 | u32 rel | packing specs | CONFIRMED |
| 0x54, 0x58 | u32 | unknown offsets | UNKNOWN |
| 0x60 | u16[] | joint tables: const R (padded to 8), then const T, const S, const user, anim R, anim T, anim S, anim user (each padded to 4). Padding entries = `num_joints` (scratch slot) | CONFIRMED |

"rel" = offset relative to the field's own position (0 = absent).

## Packing spec (u32, one per channel group)

Per component X, Y, Z: `sign (1 bit) | exponent bits (4) | mantissa bits (5)` at bit positions
X: 31 / 27–30 / 22–26, Y: 21 / 17–20 / 12–16, Z: 11 / 7–10 / 2–6; bits 0–1 = rebuilt quaternion slot.
Spec order: `[const R, anim R × n]`, then `[const T, anim T × n]`, then S, for each generic type.

Value decode per component (from `0x009CE0E2..0x009CE225`):
- exponent bits = 0: two's-complement integer of `1 + mantissa` bits, divided by `2^mantissa − 1`.
- otherwise: IEEE single built as `sign<<31 | (e + 128 − 2^(eb−1))<<23 | m<<(23 − mb)`.

Quaternions (smallest-three): missing component `sqrt(clamp(1 − x² − y² − z², 0, 1))` placed at
slot `0..3`. The decoded components fill the remaining slots in order. Memory order x, y, z, w.

## Frame sets

Each set is a DMA unit covering frames `[start, next_start]`. The final set holds only the
clip's last frame. Layout:

```
u16 sizes[8]: s0..s3 = initial-frame bytes for R, T, S, user; s4..s7 = key bytes for R, T, S, user
initial frame R | T | S | user          values of every animated channel, packed back to back
key mask        (Σ anim channels) × (F − 1) bits, channel-major, MSB-first; F−1 = inner frames
keys R | T | S(aligned 4) | user        values at keyed inner frames, channel-major
(next set's header + initial frames)    the closing keyframe of this set
```

Evidence: initial-frame sizes equal the summed spec bit widths exactly (115/115, 188/188,
199/199 bytes). Key-section sizes equal `Σ popcount(mask_c) × bits_c` **exactly for every set
tested**. The end of the keys lands exactly on the next set's offset (7840/7840, 9104/9104,
3504/3504). Channel-major order gives smooth trajectories (mean 1.3–7° per key) while
frame-major gives noise (49–91°).

## Sampling

Frames without a stored key are linearly interpolated between the surrounding keys
(nlerp with hemisphere fix for rotations). The binary's 8-float table at `0x010C1A00` is
probably a polynomial slerp approximation (LIKELY; `nlerp` is used meanwhile).

## Validation

`anim_sheet` renders `Empty_Idle`, `Empty_WalkN` and `Empty_RunN` on the elite-guard
skeleton: walk shows alternating legs with counter-swinging arms, and run shows knee drive and
arm pumping. Joint `j` maps to `AnimSet.TrackBoneNames[j]`, then to the mesh bone by name.

## Open items

- Fixed-format paths (flag bits clear): not implemented. Coverage from `edge_survey` is in LOG.md.
- Header `+0x14`, `+0x54`, `+0x58` and DMA/info second u16 fields are unknown.
- Root translation / root motion handling (currently rotation-only retargeting).
- Exact interpolation (slerp polynomial) vs nlerp: small fidelity difference.
