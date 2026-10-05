# Audio: Wwise packages, soundbanks and media

Applies to the GOG Definitive Edition install described in `GAME_FILES.md`
(`Dishonored.exe` SHA-256 `70da9613…6bc6fb`). Everything below was measured on the
retail files with `tools/audio_dump` (`survey`, `list`, `event`, `emitters`,
`decodeall`) and the research bins `exescan` and `ak_survey`. Nothing decoded is
written outside `harness_out/`.

## Inventory

| Item | Count / value |
|---|---|
| `.pck` file packages (base + DLC05/06/07) | 503 (311 in `CookedPCConsole`) |
| Soundbanks | 503, one per package, all **bank version 65** |
| Banks with an STID section | 489 (each names only itself) |
| Embedded media (DIDX/DATA) | 3,265 files, 89 min of audio (one, in Bank_Power_Player, is only the prefetched head of a streamed file) |
| Streamed media (package stream table) | 1,908 files, 1,463 MB, ~16.5 h |
| `Init.bnk` | v65, sections BKHD, STMG, HIRC, ENVS (no media) |
| Other | `MISSIONAUDIOCACHINGLIST.AKMAP`, `WwiseDefaultBank_WwiseGlobal.pck` |

Codecs: **every** media file is Wwise Vorbis (WAVE format tag `0xFFFF`): 2,710 mono,
2,463 stereo, almost all 44.1 kHz. No PCM, IMA ADPCM or XMA. The `fmt ` chunk is
always 0x42 bytes with the `vorb` block embedded (no separate `vorb` chunk); 125
files add `cue `/`LIST`/`smpl` chunks (markers/loop points, unused here).

`decodeall --streams`: **5,173 / 5,173 files decode** (0 failures, 0 silent or
non-finite results), 1,083.7 minutes of PCM.

## AKPK file package (`.pck`)

Little-endian.

```
"AKPK"  u32 header_size
u32 version (1)
u32 language_map_size  u32 soundbank_lut_size  u32 stream_lut_size  u32 externals_lut_size
language map:   u32 count, {u32 string_offset, u32 language_id}[count], UTF-16 names ("sfx")
soundbank LUT:  u32 count, {u32 id, u32 block_size, u32 size, u32 start_block, u32 language}[count]
stream LUT:     same layout (20-byte entries; offset = start_block * block_size)
externals LUT:  u32 count (always 0)
```

Four table sizes are present (header size 0x5C for one bank + one stream); the
reader also accepts the three-table layout and 24-byte (64-bit offset) entries.

## SoundBank sections (v65)

`BKHD` (u32 version=65, u32 bank id, ...), `DIDX` ({u32 media id, u32 offset into
DATA, u32 size} x n), `DATA`, `HIRC` (u32 count, then {u8 type, u32 size, u32 id,
body}), `STID` (u32 type, u32 count, {u32 bank id, u8 len, chars}).

HIRC type histogram over all banks: State 1,627, Sound 5,508, Action 4,805,
Event 2,497, RanSeq 944, Switch 28, ActorMixer 706, Layer 353, MusicSegment 1,600,
MusicTrack 1,125, MusicSwitch 21, MusicRanSeq 149, Attenuation 951, FxShareSet 4,
FxCustom 628.

Layouts used by `dishonored_audio::hirc` (decoded as far as playback needs):

- **Event (4)**: u32 n, u32 action ids[n].
- **Action (3)**: u16 type, u32 target, property bundle (u8 n, u8 ids[n], 4-byte
  values[n]), ranged bundle (u8 n, u8 ids[n], {f32 min, f32 max}[n]), then
  type-specific data. Play (`0x04xx`): u8 fade curve, u32 bank id. SetState
  (`0x12xx`) / SetSwitch (`0x19xx`): u32 group, u32 value. There is no `isBus`
  byte in v65.
- **Sound (2)**: source = u32 plugin (`0x00040001` Vorbis), u32 stream type
  (0 embedded, 1 streamed, 2 prefetch), u32 media id, u32 file id (bank id when
  embedded, = media id when streamed), [u32 offset, u32 size unless streamed],
  u8 source bits; then NodeBaseParams.
- **NodeBaseParams**: u8 override FX, u8 n FX, [u8 bypass, {u8 index, u32 fx id,
  u8, u8}[n]], u32 override bus, u32 **direct parent id**, u8 priority override,
  u8 priority distance factor, property bundle (u8 n, ids, f32/u32 values; id 0 =
  Volume dB, 2 = Pitch cents), ranged bundle (u8 n, ids, min/max), positioning:
  u8 override, if set: u8 3D flag, if 3D: u32 type, u32 **attenuation id**,
  u8 spatialized, ... (aux, advanced settings, states and RTPCs follow; not decoded).
- **Containers (RanSeq 5, Switch 6, ActorMixer 7, Layer 9)**: NodeBaseParams first;
  the children are exactly the objects whose direct parent is the container, and the
  children list (`u32 n` + ids) is then located in the body by content. RanSeq: the
  playlist (u16 n, {u32 id, i32 weight}) follows the children. Switch: u8 group
  type, u32 group id, u32 default switch, u8 continuous validation precede the
  children; switch groups ({u32 switch id, u32 n, u32 node ids[n]}) follow.
- **Music switch (12)**: children list found by scanning for the longest list of
  known ids; the (state/switch id, child id) pairs end the object. The count before
  the pairs does not match the number of pairs (Bank_Music_Boyle: 10 declared,
  12 zero bytes, 9 pairs), so they are read backwards from the end.
- **Music track (11)**: u32 n sources, then sources as for Sound.
- **Attenuation (14)**: u8 cone enabled (+16 bytes), i8 curve index[4] (dry, wet,
  LPF, spread), u8 n curves, {u8 scaling, u16 n points, {f32 distance, f32 value,
  u32 interpolation}[n]}. Distances are game units (e.g. footsteps 3,000, sword
  whoosh 2,000, Boyle party music 9,000).

Not decoded: RTPCs, state-driven property offsets, randomizer ranges (pitch/volume
spread is skipped), loop counts (no `Loop` property appears on the looping ambience
sounds in v65; see U-029), container play modes (sequence vs random), transition
rules, music cue timing, buses/effects.

## WEM / Wwise Vorbis

`fmt ` (0x42 bytes): WAVEFORMATEX with tag 0xFFFF, cbSize 0x30, then at +0x18 the
0x2A-byte `vorb` block:

| vorb offset | field |
|---|---|
| 0x00 | sample count (frames) |
| 0x04 | "mod signal"; anything other than 0x4A/0x4B/0x69/0x70 means modified packets |
| 0x10 | setup packet offset (relative to `data`) |
| 0x14 | first audio packet offset |
| 0x24 | uid |
| 0x28 / 0x29 | log2 blocksize 0 / 1 (8 / 11) |

Packets in `data` have a 2-byte size prefix (no granule). The setup packet is the
standard Vorbis setup with codebooks replaced by 10-bit indices into an external
codebook library and the fixed-value fields (floor type 1, residue type as 2 bits,
mapping type 0, window/transform types) stripped; audio packets drop the packet-type
bit and the window flags. `dishonored_audio::vorbis` rebuilds the identification,
comment and setup headers and the audio packets (the layout documented by the ww2ogg
project) and decodes them with `lewton` directly (no Ogg framing needed). Every
setup header seen uses external codebooks (e.g. 44 codebooks in a 228-byte setup).

### Codebook library: found inside `Dishonored.exe`

The external codebook library is part of the Wwise runtime, which is statically
linked into the game executable. In the PE image (base 0x400000), `.data` file offset
**0xFAEE78** holds a table of 599 ascending pointers (VA 0x011D89F8..0x011EA32F) into
`.rdata`; 598 are packed codebooks (each pointer delta equals the codebook's packed
size, validated by expanding all of them) and the last marks the end. The runtime
locates the table by content (a run of ≥200 pointers whose targets expand to exactly
their stride), so no offset is hard-coded and no third-party codebook file is needed.

Packed codebook format: u4 dimensions, u14 entries, u1 ordered, then either
(ordered) u5 initial length + run lengths, or u3 codeword-length bits + u1 sparse +
per-entry (present bit) lengths; u1 lookup type (0, or 1 with u32 min, u32 max, u4
value bits, u1 sequence flag and quantvals values). Expansion writes the standard
`BCV` codebook header and widths.

## Names

Event ids are **FNV-1 32-bit over the lowercased name** (offset basis 2166136261,
prime 16777619, multiply then xor). Verified: all 1,553 distinct `AkEvent` export
names in the 470 base-game packages hash to an event id present in some bank
(`ak_survey`). `STID` only names banks, so sound/container names are not available;
events are found through the game packages:

- `AkEvent` exports carry `RequiredBank` -> `AkBank` (name = bank package).
- `AkAmbientSound`/`AkAmbientSoundMovable` actors carry `PlayEvent` and `Location`
  (L_Boyle_Ext: 89 emitters, all resolving; L_Boyle_Int: 42).
- Player/combat events are exported (with their `RequiredBank`) by `Startup` and
  `DishonoredGame` (the packages holding the player AnimSets, weapon and power
  archetypes; which object posts each one is not traced yet): e.g. `Snd_Weapon_Sword_Whooshe_ak`,
  `Snd_Imp_Sword_vs_Sword_ak`, `Snd_Weapon_Gun_Corvo_Fire`, `Snd_Crossbow_P_Fire`,
  `Snd_Power_P_Blink_Start`/`_Stop`, `Snd_Pwr_P_WindBlast_lvl_1`, `Darkvision_Start`/`_Stop`,
  `Snd_P_Mantle_{Low,Medium,High}_cue_ak`, `Snd_VO_Bark_Player_Pain_*`, `Snd_VO_Bark_P_Death_cue_ak`.
- Footsteps are separate events per surface and gait: `FS_P_<Surface>_{Sn,R,Sp}`
  (sneak/run/sprint), `FS_P_<Surface>_Fall_{Small,High}`, `FS_Guard_<Surface>_{R,Sp}`;
  surfaces Carpet, Dirt, Flesh, Foliage, Garbage, Glass, Grass, Gravel, Metal, Mud,
  Rooftile, Stone, Stone_Wet, Water, Wood.
- Impacts: `Imp_{Bullet,Arrow,Stone,Sword}_on_<Surface|Body|Head>`.

Boyle: `Play_Amb_LVL_Boyle_Estate` is a switch container on state group 0xC9B2E8ED;
the level selects the exterior with `Set_State_Amb_LVL_Boyle_Ext_Streets` (state
0x7E247930) -> 78 s stereo street bed. `Play_Music_Boyle` is a music switch on state
group 0xE268D01C (`Music_State_Boyle_Explor` = 0xCC5F53AA, also Suspens, Fight_1..4,
Escape_*). The party music (`Snd_Music_Player_Boyle`, `_02`, attenuation 9,000 uu) is
placed in `L_Boyle_Int_Script` at (-226, -2049, 469) and (2792, 3397, 665), i.e. in
the interior map, which does not share a known offset with the exterior (U-006).

## Runtime use

`crates/dishonored_runtime/src/audio.rs`: banks load and every postable sound is
decoded on a worker thread at startup; `SimEvent`s map to the events above
(`dishonored_audio::cues`); local player 2D, others 3D (bevy_audio spatial panning,
listener = game camera) with the Wwise dry-volume attenuation curve per sound;
footsteps generated from movement with the surface guessed from the colliding mesh
name; ambience = level bed + score + placed emitters. `harness audio` verifies decode
sanity per codec class and that every mapped cue resolves to decodable sound.
