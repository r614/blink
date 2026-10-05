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

Every object type is now decoded field by field and must end exactly at the end of its
body: `audio_dump hirc` (and `harness audio`) decode all 20,946 objects of the 503 banks
with 0 inexact. Layouts (`dishonored_audio::hirc`):

- **Property bundle**: u8 n, u8 ids[n], 4-byte values[n]. v65 has no bus-volume property, so
  the ids are: 0 Volume (dB), 1 LFE, 2 Pitch (cents), 3 LPF (0..100), 4 Priority (int),
  5 PriorityDistanceOffset, **6 Loop** (int; 0 = infinite, n = plays), 7 FeedbackVolume,
  8 FeedbackLPF, 9 MuteRatio, 10 Pan L/R, 11 Pan F/R (-100..100), 12 CenterPCT,
  13 DelayTime (ms, int), 14 TransitionTime (ms, int), 15 Probability (%). Measured: Loop
  appears on 967 sounds (all 0), 24 music tracks; Delay on 512 plays and 116 set-states;
  TransitionTime on 940 stops; Probability on 35 plays; Pan on 2D sounds. This resolves
  U-029's "no Loop property": the property is id 6, not 7.
- **Ranged bundle**: u8 n, u8 ids[n], {min, max}[n] (int for int properties).
- **Event (4)**: u32 n, action ids. **State (1)**: a property bundle (volume / LPF offsets
  while the state is active).
- **Action (3)**: u16 type, u32 target, property bundle, ranged bundle, then by class:
  Stop/Pause/Resume (0x01-0x03): u8 fade curve, [u8 include-pending for pause/resume],
  u32 n exceptions {u32 id, u8 is-bus}. Play (0x04): u8 fade curve, u32 bank. Mute/unmute,
  pitch, volume, LFE, LPF (0x06-0x0F): u8 fade curve, [u8 value meaning, f32 base, min, max],
  exceptions. SetState (0x12) / SetSwitch (0x19): u32 group, u32 value. Set/reset game
  parameter (0x13/0x14): u8 curve, u8 meaning, f32 base/min/max, exceptions. The low byte is
  the scope (2 target, 3 target on the posting object, 4 all, 5 all on the object, 6/7 all
  except).
- **Sound (2)**: source (u32 plugin `0x00040001` Vorbis, u32 stream type 0 embedded /
  1 streamed / 2 prefetch, u32 media id, u32 file id, [u32 offset, u32 size unless streamed],
  u8 bits), then NodeBaseParams.
- **NodeBaseParams**: u8 override FX, u8 n FX [u8 bypass, {u8 index, u32 fx id, u8 shareset,
  u8 rendered}[n]], u32 override bus, u32 **direct parent**, u8 priority override, u8
  priority distance factor, property bundle, ranged bundle, positioning (u8 override; if set
  **one** u8 3D flag; 2D: u8 panner; 3D: u32 type, u32 **attenuation**, u8 spatialized, then
  type 2 (user-defined paths): u32 path mode, u8 loop, i32 transition, u8 follow orientation,
  u32 n vertices x16 bytes, u32 n playlist x8, n x8 random ranges; type 3: u8 dynamic),
  advanced settings (9 bytes: u8 virtual queue behaviour, u8 kill newest, u8 use virtual,
  u16 max instances, u8 global limit, u8 below-threshold behaviour, u8 max-instance override,
  u8 virtual override), states (u32 n groups {u32 group, u8 sync type, u16 n {u32 state,
  u32 state object}}), RTPCs (u16 n {u32 game parameter, u32 target parameter, u32 curve id,
  u8 scaling, u16 n points {f32 x, f32 y, u32 interpolation}}). No aux sends in v65.
- **RanSeq (5)**: NodeBaseParams, u16 loop count (0 infinite), f32 transition time **(ms)**,
  f32 random min, f32 random max, u16 avoid-repeat count, u8 transition mode (0 none,
  1 crossfade amp, 2 crossfade power, 3 delay, 4 sample accurate, 5 trigger rate), u8 random
  mode (0 standard, 1 shuffle), u8 mode (0 random, 1 sequence), u8 using weight, u8 reset
  playlist at each play, u8 restart backward, u8 continuous, u8 global; children (u32 n +
  ids); playlist (u16 n {u32 id, i32 weight}). Retail: 630 random step containers with
  avoid 1; 135 continuous (crossfade, delay, trigger rate) of which 106 loop forever.
- **Switch (6)**: NodeBaseParams, u32 group type (0 switch, 1 state), u32 group, u32 default
  switch, u8 continuous validation, children, u32 n groups {u32 switch, u32 n, ids}, u32 n
  params {u32 node, u8 first only, u8 continue playback, u32 on-switch mode, i32 fade out ms,
  i32 fade in ms}.
- **ActorMixer (7)**: NodeBaseParams, children. **Layer (9)**: NodeBaseParams, children,
  u32 n layers {u32 id, RTPCs, u32 game parameter, u32 n associations {u32 child, u32 n
  points, points}}.
- **Music node** (segment, switch, playlist): NodeBaseParams, children, f64 grid period ms,
  f64 grid offset ms, f32 tempo, u8 beats per bar, u8 beat value, u8 meter override, u32 n
  stingers {u32 trigger, u32 segment, u32 sync play at, i32 don't-repeat ms, u32 look-ahead}.
- **MusicSegment (10)**: music node, f64 duration ms, u32 n markers {u32 id, f64 position ms,
  u32 name length + bytes}. Markers are the entry cue (first) and exit cue (last).
- **MusicTrack (11)**: u32 n sources, sources; u32 n clips {u32 sub-track, u32 source,
  f64 play-at, f64 begin trim, f64 end trim, f64 source duration} (ms); u32 n sub-tracks
  (only when clips exist); u32 n clip automations {u32 clip, u32 type, u32 n, points};
  NodeBaseParams; u32 track type (random/sequence sub-tracks); i32 look-ahead ms.
- **Transition rules** (music switch and playlist): u32 n {u32 source id, u32 destination id
  (one each in v65, -1 = any), source fade {i32 time, u32 curve, i32 offset}, u32 sync type
  (0 immediate, 1 grid, 2 bar, 3 beat, 4 next cue, 5 custom cue, 6 entry, 7 exit cue),
  u32 cue filter, u8 play post-exit, destination fade, u32 cue filter, u32 jump-to, u16 entry
  type, u8 play pre-entry, u8 match source cue name, u8 transition-segment flag, then the
  transition object **always present** {u32 segment, fade in, fade out, u8 pre-entry, u8
  post-exit}}.
- **MusicSwitch (12)**: music node, rules, u32 group type, u32 group, u32 default, u8
  continue playback, u32 n {u32 state, u32 child}. (The "10 declared, 9 pairs" puzzle was the
  last rule's transition object.)
- **MusicRanSeq (13)**: music node, rules, u32 n items, then the playlist tree depth first
  {u32 segment, u32 item id, u32 n children, u32 type (0 sequence continuous, 1 sequence
  step, 2 random continuous, 3 random step; -1 for leaves), i16 loop (0 infinite), u32 weight,
  u16 avoid repeat, u8 using weight, u8 shuffle}.
- **Attenuation (14)**: u8 cone [4 f32], i8 curve index[4] (dry volume, wet, LPF, spread),
  u8 n curves {u8 scaling (2 = dB), u16 n, points}, RTPCs. Footsteps: dry 0 dB to -96.3 dB at
  2,000 uu, LPF 0 -> 52, spread 100 % -> 37 %.

`Init.bnk` STMG (decoded exactly, `dishonored_audio::init`): f32 volume threshold, u16 max
voices, u32 n state groups {u32 id, u32 default transition ms, u32 n {from, to, ms}}, u32 n
switch groups {u32 id, u32 game parameter, u32 n {f32 value, u32 switch, u32 interpolation}},
u32 n game parameters {u32 id, f32 default}. Bus objects (HIRC type 8) are not decoded
(bus volumes and ducking are not applied, U-164).

## Playback semantics (`dishonored_audio::engine`, `music`)

A software engine plays the hierarchy with the semantics above, deterministic given its
inputs (fixed-seed random numbers), at 48 kHz stereo; the runtime feeds its output to
bevy_audio as one endless source.

- Events run their actions: play (delay, fade-in, probability), stop/pause/resume (target or
  all, optionally only on the posting object, fade-out), states, switches (per object), game
  parameters, volume / pitch / LPF / mute modifiers.
- Random/sequence containers: step mode picks one child per play (sequence index, standard
  random with weights and avoid-repeat, shuffle), memory per object or global. Continuous
  mode plays the playlist with the loop count; the next item starts at the end (none /
  sample accurate), after the delay, crossfaded over the transition time (amp or power
  curve), or at the trigger rate. Item ends are timed from the media header.
- Sounds loop with Loop (and the `smpl` loop points of the WEM when present).
- Switch containers on states or per-object switches; continuous validation follows changes
  with the per-child fade times. Blend containers apply their RTPC crossfade curves.
- Volume = sum of the parent chain's Volume + rolled randomizer ranges + active state offsets
  + RTPC curves + modifiers; pitch and LPF likewise. 3D: dry volume, LPF and spread from the
  attenuation curves at the listener distance, equal-power panning from the listener's right
  vector, spread blends toward centre. 2D: Pan L/R. Instance limits (max instances, kill
  newest / oldest, global or per object).
- Voices out of range become virtual: time advances without decoding, the media is released
  after 5 s and the voice resumes at the elapsed position.
- Interactive music: switch containers follow their state group to a playlist or segment
  (nested switches too); playlists iterate their tree (loop counts, weights, avoid-repeat,
  shuffle); segments are scheduled entry-cue to exit-cue on the sample clock with pre-entry
  and post-exit overlap; clips honour play-at and trims; random/sequence sub-tracks pick one
  sub-track per play. A state change picks the most specific transition rule, synchronises
  (immediate, grid, bar, beat, cue, exit), fades the source (time, curve, offset) and the
  destination, and plays the transition segment when the rule has one. Boyle:
  `Play_Music_Boyle` -> switch on 0xE268D01C (Explor / Suspens / Fight_1..4 / Fight_Rat /
  Escape_*) -> nested switch on the interior/exterior group 0x0956DE29 (0x21720269 interior,
  0x33692463 exterior), set by the ambience room events (`Set_State_Amb_LVL_Boyle_Int_*`
  interior; Brothel's street events exterior). No Boyle event sets exterior: the runtime
  sets it for Boyle Ext (U-161). Boyle rules: immediate, source fade 8 s with 8 s offset,
  destination fade 4 s.

APPROXIMATIONS: the LPF value -> cutoff mapping (one-pole, 20 kHz at 0 to 20 Hz at 100;
U-163), linear resampling, buses (U-164), FX (628 custom FX: reverb etc. not applied),
stingers.

## Music manager (`DisGlobalMusicManager`, decompiled)

Native state machine (`DisMusicState_{Exploration, Suspense, RatAttack, Combat, Chase}`,
class records 0x01377668 / 0x013779d8 / 0x01377820 / 0x013774b0 / 0x013772f8) driven by the
game info tick; each state posts its `DisTweaks_MusicEvents` event (map tweak, e.g.
`Twk_Boyle_MusicEvents`: suspense 800 uu, chase 15 s, chase ranges 2,000 / 4,500 uu,
intensity thresholds 0.1 / 2 / 5 / 100, contributions guard 1, elite 2, tallboy 100).

- Selection `FUN_00c52a40`: intensity (`m_fCombatIntensity`, manager +0x44) > 0 -> Combat;
  else `m_bAttackedByRats` (+0x48 bit 0) -> RatAttack; else no living hostile NPC within
  `m_fEnemyRangeForSuspense` of the player (`FUN_00c483c0`: pawn list, alive flag
  `+0x508 bit 10`, relationship == 2) -> Exploration; else from Combat or RatAttack ->
  Chase with `m_fChaseTime`, otherwise Suspense.
- Ticks (vtable 0x124..0x134 -> `FUN_00c52bf0/c00/c20`): Exploration, Suspense and RatAttack
  re-select every tick; Combat re-selects and stores the intensity; Chase re-selects only when
  its timer ran out or combat / rats resume.
- Combat entry sets level 4, then `FUN_00c486d0` picks level 0..3 from thresholds
  `m_fCombatIntensity[1..3]` and posts `m_pStartCombat[level]` on change. Chase entry sets
  level 2, then each tick counts the timer down and `FUN_00c48620` posts
  `m_pStartChase[0]` (hostile within 2,000) or `[1]` (within 4,500), or ends the chase.
- Exploration / Suspense / RatAttack entries post `m_pStartExploration` (tweak +0xcc) /
  `m_pStartSuspense` (+0xd0) / `m_pStartRatAttack` (+0xd4) through `FUN_006cd730`.
- The writer of the intensity was not found (U-160): the port sums the tweak contributions
  of the NPCs in combat with the player (guards 1). The host replicates per-NPC hostility,
  combat and suspicion near the humans in `MatchView::ai`; the client runs the manager.

## Contact system: footsteps, landings, impacts

`DishonoredGame.Phy_ContactSystem.ContactSystem.Dis_ContactSystem` holds 646 (striker,
struck) intersections with sub-intersections (sound cue with speed volume scaling, particle,
AI noise, rumble, camera effect, decal, shake). Strikers: `DisContactType_Footfall_*`
(`_Run/_Sneak/_Sprint` and their `_Player` variants), `JumpLand(_Big/_Stealth)`, weapons,
projectiles (Bullet, Arrow...); struck: the surface's `DisContactType_Env_*` from the
material's `PhysMaterial` -> `PhysicalMaterialProperty` (`DisContactPhysicalMaterial.
m_pContactType`, default `DisContactType_Environment`). Example: Footfall_Run_Player x
Env_Wood -> `FS_P_Wood_R`; Footfall_Run x Env_Gravel -> `FS_Guard_Gravel_R`.

- `DishonoredNotify_Footfall` (`FUN_00a8d5f0` -> `FUN_00b66ed0`): one contact per foot at most
  every 0.25 s; `FUN_00b66a80` traces from the foot bone (or the pawn) down 1.5 x the cylinder
  height and triggers the contact (`FUN_00be61a0`) with the pawn's footfall type and the hit's
  contact type.
- The collision import carries a contact type per triangle (kDOP `MaterialIndex` -> section
  material or instance override -> physical material): `CollisionWorld::tri_surface`
  (cache `{map}.collision2.bin`). Boyle Ext: 678,066 of 751,965 triangles have one (stone
  274k, metal 240k, wood 116k, carpet 34k, dirt 14k...).
- Lookup walks striker then struck class lineages (most derived first) and resolves each
  sub-intersection kind separately (LIKELY, U-162).

## AnimNotify sounds

`AnimNotify_AkEvent` (`AkEvent`, `bFollowActor`, `BoneName`, `VolumeMultiplier`,
`PitchMultiplier`; Notify `FUN_00716bd0`) posts the event on the mesh owner at the bone, after
setting the RTPCs "<event>v" / "<event>p" to the multipliers. The Startup AnimSets hold 391 of
them in 171 clips (sword whooshes at e.g. 0.156 s of `Sword_Ready_AttackRight_A_Small`,
equip, reload, mantle), and 22 footfall notifies. The animation code reports each clip window
it advances (`audio::AnimSounds::advance`); notifies inside the window post at their time
(this resolves U-031: sword whooshes come from the attack clips, not from the sim event).

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

Dialog VO: the per-voice `Bk_<GUID>.<LANG>` AKPK files (BOYLE_MISSION.md section 4) load
through `SoundLibrary::load_bank` (a `Bk_` name falls back to `<name>.INT`), and
`dishonored_audio::vo::VoResolver` maps a line's `Play_<hex>` event to its clip; clip lengths
come from the WEM header alone (`media_length`: `vorb` sample count / rate) so the server can
time lines without decoding. `crates/dishonored_runtime/src/dialogue_vo.rs` decodes and plays
them (3D on the speaker, Wwise attenuation curve of the sound).
