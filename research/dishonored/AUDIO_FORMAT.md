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
MusicTrack 1,125, MusicSwitch 21, MusicRanSeq 149, Attenuation 951, FxShareSet (type 18) 4,
FxCustom (type 19) 628. (Earlier revisions named types 16/17; v65 numbers the effects 18/19.
The custom effects are 583 Parametric EQs, 40 Silence source plugins, 3 plugin 0x88 and 2
plugin 0x82 blocks; 596 of the 644 node effect slots point at them.)

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
u32 n game parameters {u32 id, f32 default}.

## Buses and environments (Init.bnk HIRC, U-164 resolved)

Init.bnk HIRC: 27 buses (type 8), 13 state objects, 55 effect sharesets (type 18), and the
ENVS section. All decode exactly (`audio_dump buses`, `harness audio`).

- **Bus (type 8)**: u32 parent bus (0 = master), property bundle, u8 kill newest, u8 use
  virtual, u16 max instances (playback limit), u8 max-instance override, u8 channel config
  (0x3F, 5.1), four flag bytes (the second set on a music sub-bus, the fourth on the bus above
  SFX / dialogue / ambience), u32 recovery time (ms), f32 maximum duck volume, u32 n ducks
  {u32 target bus, f32 duck volume dB, i32 fade out ms, i32 fade in ms, u8 curve}, effects
  (u8 n, [u8 bypass, {u8 index, u32 id, u8 shareset, u8 rendered}]; none in retail), one extra
  byte when the fourth flag is set, RTPCs (as node RTPCs), states (as node states).
- **Effect (types 18/19)**: u32 plugin id, u32 parameter size, parameters, u8 n media
  {u32, u32}, RTPCs. RoomVerb 0x00760003 (186 bytes: decay, HF damping, diffusion, stereo
  width, three tone filters {gain, freq, Q}, front / rear / centre / LFE levels, dry, ER and
  reverb levels, u8 ER on, u32 ER pattern, reverb delay ms, ER front/back delay ms, room size,
  density, room shape, u32 quality; the last names are LIKELY from their ranges), Matrix
  Reverb 0x00730003 (29 bytes: decay, HF ratio, u32 delay lines, dry, wet...), Delay 0x006A0003,
  Parametric EQ 0x00690003 (56 bytes: three bands {u32 type 0 LP 1 HP 2 BP 3 notch 4 low shelf
  5 high shelf 6 peak, f32 gain, f32 freq, f32 Q, u8 on}, f32 output dB, u8 LFE).
- **ENVS**: four curves {u8 enabled, u8 scaling, u16 n, points}: obstruction volume
  (0 -> -28.1 dB), obstruction LPF (0 -> 50), occlusion volume (disabled), occlusion LPF
  (0 -> 100). Obstruction / occlusion are not computed by the port.

Game parameter and state names come from the executable's `UDishonoredAudioSystem` strings
(`Global_Volume`, `Music_Volume`, `SFX_Volume`, `Voices_Volume`, `Ambient_Volume`,
`Menu_Volume`, `TimeDilation`, `Scale_RTPC`, `Pause_All`, ...) and all hash to Init ids:

| Bus | Parent | Role (from its parameters) |
|---|---|---|
| 0x26DF8722 | - | master, Volume -4 dB, `Global_Volume` (0..100 -> -96.3..0 dB) |
| 0x2714B5AF | master | music: `Music_Volume`, `TimeDilation` pitch (1 -> 0, 0.5 -> -1200, 0.125 -> -3600, <= 0.0625 -> -4800 cents), states (-9 dB, -96.3 dB) |
| 0x2AAC29B6 | master | game: children 0x28EB7999 and 0x2C0A01F0 (`SFX_Volume`, mute states) |
| 0x28EB7999 | 0x2AAC29B6 | environmental (4th flag); `TimeDilation` pitch; states |
| 0x266B2CC4 | 0x28EB7999 | SFX, `SFX_Volume`; 15 sub-buses with playback limits 1..50 |
| 0x30B38306 | 0x28EB7999 | dialogue, `Voices_Volume`; ducks 0x266B2CC4 -8 dB, 0x2714B5AF -8 dB, 0x1C78546F -7 dB (fade out 899 ms, in 1,399 ms, recovery 0) |
| 0x1C78546F | 0x28EB7999 | voices 2, `Voices_Volume`, states (+7 dB / LPF) |
| 0x0D974689 | master | `SFX_Volume` |

**Environments**: `DishonoredAudioVolume.m_Environment` names (Boyle Ext: `PC_streets_01`,
`PC_Streets_02`, `Room_Small_Wood` (Matrix Reverb), `sewer`, `Hall_Stone_Wood`,
`Plate_Small`, `Verb_Ext_Flooded`) hash to the Init sharesets. Wwise v65 predates aux sends:
the game sets per-object environment values (the pre-2012 `SetGameObjectEnvironmentsValues`);
voices on the environmental bus send to them. The port gives every game object the
environment of the smallest audio volume around it at control value 1 (LIKELY) and sends the
voices routed through 0x28EB7999 (scaled by the attenuation's wet curve when it has one); the
reverb returns are mixed at the output. The RoomVerb's own dry level (0 dB on most presets) is
ignored for these sends (LIKELY: environmental FX are wet-only paths).

**Audio volumes** (`DishonoredAudioVolume`: `m_pSoundEvent`, `m_SoundParameters`,
`m_Environment`, `InOutVolume.m_VolumeKind`): Boyle Ext has 45 (`audio_dump volumes`), all but
three with `Set_State_Amb_LVL_Boyle_Ext_Streets` / `_Int_Streets` / `_Sewers` (ambience state
group 0xC9B2E8ED). The port posts the event (and sets the RTPCs) when the listener enters a
volume (smallest containing one).

**Playback (`engine`)**: each voice's output bus is the first node up its chain that sets one;
per block the bus volume = Volume property + state offsets + RTPC curves (global game
parameters) + ducking, pitch likewise, summed up the bus chain into the voice gain and pitch.
Bus playback limits kill the oldest (or refuse the newest) voice. Ducking follows the rules
above (linear dB ramps; the curve byte is not applied). Parametric EQ custom effects are run
per voice (effects of the nearest node that overrides its parent's, else the top node). The
reverb is a feedback delay network with the shareset's parameters (APPROXIMATION of the
proprietary RoomVerb algorithm); EQ is RBJ biquads.

The options sliders still scale the port's category gains; how the menu maps its sliders onto
the `*_Volume` parameters is not traced (U-300).

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
  interior; Brothel's street events exterior). No Boyle Ext event sets it: the 45 audio volumes
  post only ambience states (group 0xC9B2E8ED), and the music switch maps state none to no
  child, so the original relies on a value set earlier (or natively; not found). The runtime sets
  exterior on Boyle Ext only while the group is unset, so returning from the interior keeps the
  interior value as Wwise would (U-161). Boyle rules: immediate, source fade 8 s with 8 s offset,
  destination fade 4 s.

APPROXIMATIONS: the LPF value -> cutoff mapping (one-pole, 20 kHz at 0 to 20 Hz at 100;
U-163: no 101-entry cutoff or coefficient table exists in the image, `audio_dump floattables`;
the runtime likely computes it), linear resampling, the reverb and EQ algorithms (parameters
are the retail ones), duck curves, obstruction / occlusion, stingers.

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
- Writer of the intensity and the rats flag (U-160): the manager's tick `FUN_00c5b610` stores
  `FUN_00c5b4d0(tweaks)` into +0x44 and its second result into bit 0 of +0x48. `FUN_00c5b4d0`
  sums `m_fCombatIntensityContribution[type]` over the types `FUN_00c5b190` collects and flags
  any type 1 (`DCMIOT_Rat`). `FUN_00c5b190`: the NPCs from a native query on the player pawn
  (vtable +0x700, arguments 2, 4; their `DisTweaks_NPCPawn.m_CombatIntensityType`, +0x2D2) and
  the crowd rats whose target (+0x440, +0x20) is the player (rat tweak +0x13C). The port lists
  the NPCs in combat with the player (the query filter is not decoded). The host replicates per-NPC hostility,
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
- Lookup (U-162 resolved): `FUN_00be61a0` takes the exact (striker, struck) entry
  (`FUN_00be4a40`), else `FUN_00be4b10`: over striker depth i and struck depth j (class
  `+0x44` parent links, stopping at the base contact class), the entry with the smallest i + j,
  ties to the smaller striker depth. Unless the table is flattened (+0x38 bit 1), each
  sub-intersection slot that is empty in that entry is looked up again the same way among
  entries that have it; each found slot is triggered (vtable +0x124).
- Sound slot (`DisContactSub_SoundCue` trigger 0x00BC6290): no event -> nothing; with
  `m_bScaleWithSpeed`, no sound at or below `m_fMinSpeed`, else volume = speed / `m_fMaxSpeed`
  clamped to [`m_fMinVolumeScale`, 1]. 0x00BA95C0 posts it on a temporary object at the
  contact (skipped beyond the event's maximum attenuation distance) after setting a game
  parameter `Scale_RTPC` (global id 0x014671E0, initialised by `FUN_00ba3120`) to that volume.

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

`crates/dishonored_runtime/src/audio.rs`: a worker thread loads the player/combat banks, the
map's ambience, music and emitter banks (and the banks of Kismet-posted events), the contact
system, the AnimSet sound notifies and the map's music tweaks, and decodes the short one-shots;
the `dishonored_audio::engine` renders everything (one endless bevy_audio source, 48 kHz).
Pawns, placed emitters and impacts are Wwise game objects at their positions; the listener is
the game camera. Sounds come from: AnimNotify_AkEvent / footfall notifies reported by the
animation code (`AnimSounds::advance`), footfall contacts (stride fallback for pawns without
notifies), simulation events (`dishonored_audio::cues`), Kismet (`MissionView::sounds`, and
`MissionView::ambient` starting, stopping or toggling placed ambient sound actors), and the
music manager on `MatchView::ai`. Level travel loads the next map's banks; states persist.
`harness audio` checks decode sanity per codec class, every mapped cue, the exact HIRC decode,
container loops and crossfades, avoid-repeat, music transitions, the music manager on the
Boyle tweaks, footstep surfaces from the collision import and the notify sounds.

Dialog VO: the per-voice `Bk_<GUID>.<LANG>` AKPK files (BOYLE_MISSION.md section 4) load
through `SoundLibrary::load_bank` (a `Bk_` name falls back to `<name>.INT`), and
`dishonored_audio::vo::VoResolver` maps a line's `Play_<hex>` event to its clip; clip lengths
come from the WEM header alone (`media_length`: `vorb` sample count / rate) so the server can
time lines without decoding. `crates/dishonored_runtime/src/dialogue_vo.rs` decodes and plays
them (3D on the speaker, Wwise attenuation curve of the sound).
