# Differential oracle (`tools/oracle`, `crates/dishonored_oracle`)

The oracle compares the port against the original game and turns every difference into a
ranked work item with an evidence dossier, so a fix starts from evidence, not from an
investigation. It covers the whole game along two axes:

- **every map**: the 42 base persistent maps (`L_*_P` plus `DishonoredGameFull_P`), DLC maps
  with `--dlc`;
- **every system**: movement, combat, powers, stealth and detection, AI, Kismet and mission flow,
  conversations, cutscenes, inventory, upgrades and economy, chaos, save/load, UI, audio,
  physics and ragdoll, particles. Scenarios are the unit for systems, as maps are for level data.

It reports differences; it never adjusts either side to make them agree.

## Architecture

```
original (Dishonored.exe + hook, phase 2)          port (library crates, headless)
  probe_launch collide / lab / measure               dishonored_world (collision, nav)
  reflection dump, ProcessEvent traces                dishonored_sim (movement...)
             |                                                  |
             v                                                  v
  oracle cache  <exe sha256>/<subject>/<probe>-<hash>.orc    port snapshot (port/<map>.snap)
             \______________________  ______________________/
                                    \/
                     dishonored_oracle: diff, clustering, first divergence,
                     structural diff, residual classification (cause book)
                                    |
                     SCORECARD.md / scorecard.json, WORKLIST.md / worklist.json,
                     dossiers/<rank>.md          (harness_out/oracle/<command>/)
```

- `crates/dishonored_oracle` (library): no Bevy, no runtime, no game crates; only
  `ue3_package`, serde, bincode, ron, rayon and sha2. Formats, cache, scenario specs, traces,
  diff engine, structural diff, sampling, residuals, reports and the coverage scan.
- `tools/oracle` (CLI `oracle`): the headless port runner. Features: `import` (map import
  through `dishonored_content::load_match`, i.e. exactly the game's loader) and `scenarios`
  (links `dishonored_sim`). `--no-default-features` links only `dishonored_world` and keeps the
  collision fix / re-check loop under ten seconds.
- The inner loop never runs the game: original answers are read from the cache, the port's
  derived data from a snapshot written by the game's own loader.

## Commands

```text
oracle coverage [--dlc] [--no-cache] [--map M]...       static coverage of every package
oracle import collision --map M --queries Q --results R [--generator G] [--capture-seconds S]
oracle snapshot --map M...                              port snapshot via load_match (import)
oracle collision [--map M]... [--ids 1,2,3]             cached original vs port
oracle gen collision --map M --out Q [--sampling navmesh,prim] [--reach R]
oracle densify collision --map M --out Q [--per N] [--radius R]
oracle structure --map M                                port structure (vs a reflection dump)
oracle scenario [NAME...]                               scenarios on the port (+ original trace)
oracle sweep                                            everything cached -> one scorecard
oracle bench import|queries --map M                     the port's costs
oracle cache list
```

Outputs (`harness_out/oracle/...`) and caches (`$DISHONORED_CACHE/oracle`, default
`%LOCALAPPDATA%\dishonored-rs\cache\oracle`) are retail-derived: never committed.

## Formats

### Oracle cache (`cache.rs`, format version 2)

`<root>/<exe sha256, 16 hex>/<subject>/<probe>-<query hash, 16 hex>.orc`; the key is
(exe SHA-256, subject, probe, query hash). Subject = map or scenario name. File: magic
`BLKORACL`, format version (u16), probe tag (u16), key length (u32), payload length (u64),
FNV-1a 64 of key + payload, then the key and the payload (bincode, standard config). A reader
rejects another magic, version, probe, length or checksum, so a format change is a version bump
and a re-capture, never a misread. The exe hash is memoised per (path, size, mtime) in
`exe_hashes.json`. Probes: `Collision`, `Navigation`, `Light`, `Visibility`, `State`
(reflection / structure dump), `Trace`. Every payload carries a `Provenance` (capture time,
source command and files, capture seconds).

### Query sets (`query.rs`)

A `CollisionSet` holds the queries and the original's answers together (the port never
regenerates queries, so a navmesh or generator change cannot shift ids):

- `CollisionQuery`: tag `<kind>:<trace>`, start, end, extent (zero = line), trace flags, and a
  `target` (`prim:<object>` for primitive-directed queries, `near:<id>` for densification);
- `CollisionAnswer`: hit, time, location, normal, actor and component (interned names);
- the world `state` and its `state_settings` (actor -> state), e.g. the door states read when
  the checks ran; a non-initial state is folded into the query hash, so each state of a sweep
  is its own cache entry;
- the `profiles` (`TraceProfile`: name, tag, flags, extent, call-site address, source) the
  queries were expanded over.

The text exchange with `probe_launch collide` is unchanged (`id sx sy sz ex ey ez hx hy hz
flags [target]`); `oracle import` turns a capture into a cache entry.

### Scenario spec (`scenario.rs`, RON, `fixtures/oracle/scenarios/<system>/<name>.ron`)

`name`, `system`, `map`, `description`; `setup` (player position / yaw / floor snap / sword /
health / mana, items, equips, powers, NPC placements by spawner or tweak with an AI state, story
flags, seed, Kismet on/off, keep-alive); `tick_hz` (fixed step) and `ticks`; `input` (per tick:
`Move`, `Look`, `Press(button)` with a hold, `Console`); `checkpoints` (metrics `At`, `Max`,
`Min`, `Rise`, `RiseFrom`, `TimeToReach` with a value, tolerance and the reference's source);
`record` (`Pawn`, `Vitals`, `Camera`, `Npc`, `Kismet`, `ProcessEvent` filters, `SimEvents`);
per-channel `tolerances` (longest prefix). A map sweep is a scenario with `sweep`: probe,
generator, `structure` (run the structural diff first), `sampling` (`Navmesh`,
`PrimitiveDirected(reach)`, `Densify(rounds, per, radius)`), `profiles` (`Enumerated` from the
exe's trace call sites, or `Listed`), `states` (named settings, or `each_state_of: "DisDoor"`).
The spec's hash (canonical RON) is the cache key of its traces.

### Traces (`trace.rs`)

Channels (name, unit), frames (tick, values; NaN = not sampled) and ordered events (tick, kind,
object, name, detail). The same shape for both sides.

### Structure dumps (`structure.rs`)

Objects keyed by the stable path both sides share (`<package>:<actor>[.<component>]`), each with
a class and named fields (`Bool`, `Int`, `Float`, `Floats`, `Str`, `Ref`). The port names its
fields after the original's properties, so no mapping table is needed; a field the port does
not model is reported absent.

### Work items, dossiers and the cause book (`report.rs`, `residual.rs`)

A `WorkItem` has a stable id `<probe>:<subject>:<key>`, title, probe, system, count, impact
(count x probe weight), maps, evidence lines (tables of example queries, counterparts,
offsets), repro commands, code pointers and a `cause`. `fixtures/oracle/causes.ron` holds
rules matching item ids (`*` wildcards) to a cause kind (`PortBug`, `PortMissing`,
`KnownDifference`, `OracleLimitation`) with its evidence. Every scorecard starts with the
residual table per probe.

## Definition of done

Coverage-complete equivalence first, sampling second:

1. **Structural diff** of the system's live objects, field by field (primary proof).
2. **Primitive-directed sampling**: every collision primitive (kDOP triangle group, convex
   element, volume) hit from five directions around its normal, not only spatial grids.
3. **Extent and channel enumeration**: every trace profile the exe uses, from its trace call
   sites (Ghidra index), not hand-picked.
4. **State sweeps**: everything Kismet or gameplay can toggle is re-queried in each state.
5. **Adaptive densification** around every disagreement.
6. **Classified residuals**: done means every remaining difference is classified by cause in
   the cause book; agreement need not read 100 %.

Collision is the first system with this treatment; the same pattern (structure dump ->
directed queries over every primitive and profile -> state sweeps -> densification -> cause
book) carries to navigation, light level, vision and the behaviour systems.

Phase 1 status: the formats express all six; implemented without the hook are the structural
diff engine (port side dumped, original side pending), primitive-directed generation (aimed at
the port's collision sources until the original's primitives come from the dump), profile
expansion (the four hook-measured profiles until the enumeration exists), state settings in
sets and keys, densification (`oracle densify` writes the next capture's queries), and the
cause book with the residual summary.

## The probes

| # | Probe | Kind | Phase 1 |
|---|---|---|---|
| 1 | Coverage scan | static, packages vs port source | done (below) |
| 2 | Live state dump | hook, reflection walk of GObjects / UProperty | format + diff engine; dump in phase 2 |
| 3 | Query oracles | hook: collision, navigation, light, visibility | collision: cached captures vs headless port |
| 4 | Behaviour traces | hook, fixed-step scenario in both, first divergence | spec + trace + diff; port runner for movement; original traces in phase 2 |
| 5 | Ghidra index | native table, vtables, cross-references per exe hash | phase 3 |

### Coverage scan

Each package is scanned once (in parallel, cached per file size and mtime): objects per class,
placed actors (`Actor` subclasses in `PersistentLevel`), set properties per class (tagged
properties = values authored or overridden), enum values (material blend modes and lighting
models), class definitions with supers (the hierarchy), and every `UFunction` (native flag,
native index, bytecode size; the layout check must end exactly at the export's end, 0 failures
over the install). A map is its persistent package plus its `WorldInfo.StreamingLevels` (the
port's `map_packages` rule).

What the port handles is read mechanically from its source: every string literal of
`crates/*/src` (minus `ue3_package`, the probe crates, this crate and test code), with the call
it sits in and its enclosing function or `const` item:

- a class is **direct** when named, **inherited** when an ancestor is an `is_a(..)` argument,
  **pattern** when only an `ends_with` / `starts_with` / `contains` on a class name matches;
  string tests on other values (animation names, paths) do not count;
- Kismet ops count only in the interpreter (`dishonored_kismet/src/runtime.rs`) and the Kismet
  host (`dishonored_multiplayer/src/kismet_host*`); sequences are run structurally
  (`load::kind_of` -> `Kind::Sequence`);
- names found only in the port's no-op lists (`PRESENTATION`, `NOT_MODELLED`) or in functions
  that only describe ops (`deactivation`, `renders_moved`) are **acknowledged**, not handled;
- a property is read when its name is a literal or a dot-separated segment of one; editor and
  cook metadata (Kismet layout, editor colours, lighting GUIDs, Lightmass settings, dialog
  editor bookkeeping) is left out, listed in `EDITOR_ONLY_PROPS`.

`harness_out/oracle/coverage/classes.tsv` lists every class with its handling and the file:line
that decided it. Static coverage says what the port looks at, not that it behaves like the
original; the query and behaviour oracles measure that.

**Results (2026-10-07, main at be0a3da plus this branch; 470 packages, 0 parse failures).**
Overall = mean of the placed-actor, Kismet-op and property-setting rates.

| map | placed actors | Kismet ops | props read | overall |
|---|---|---|---|---|
| DishonoredGameFull_P | 3/3 | 1099/1194 (92.0 %) | 84.5 % | 92.2 % |
| L_ArtDealer_P | 430/472 (91.1 %) | 403/460 (87.6 %) | 67.7 % | 82.1 % |
| L_Boyle_Ext_P | 1080/1218 (88.7 %) | 1599/1702 (93.9 %) | 67.3 % | 83.3 % |
| L_Boyle_Int_P | 1425/1623 (87.8 %) | 2207/2420 (91.2 %) | 68.9 % | 82.6 % |
| L_Bridge_Part1a_P | 840/928 (90.5 %) | 782/851 (91.9 %) | 62.5 % | 81.6 % |
| L_Bridge_Part1b_P | 1758/1972 (89.1 %) | 1088/1226 (88.7 %) | 61.6 % | 79.8 % |
| L_Bridge_Part1c_P | 577/647 (89.2 %) | 292/313 (93.3 %) | 63.4 % | 81.9 % |
| L_Bridge_Part2_P | 1497/1704 (87.9 %) | 1481/1735 (85.4 %) | 61.3 % | 78.2 % |
| L_Brothel_P | 1658/1849 (89.7 %) | 2114/2402 (88.0 %) | 63.1 % | 80.3 % |
| L_Distillery2_P | 847/922 (91.9 %) | 763/872 (87.5 %) | 62.2 % | 80.5 % |
| L_Distillery_P | 878/991 (88.6 %) | 562/630 (89.2 %) | 62.2 % | 80.0 % |
| L_Flooded_FAssassins_P | 990/1101 (89.9 %) | 1392/1590 (87.5 %) | 59.0 % | 78.8 % |
| L_Flooded_FGate_P | 1399/1736 (80.6 %) | 1980/2258 (87.7 %) | 67.8 % | 78.7 % |
| L_Flooded_FIntro_P | 378/480 (78.8 %) | 450/570 (78.9 %) | 57.1 % | 71.6 % |
| L_Flooded_FRefinery_P | 803/976 (82.3 %) | 665/825 (80.6 %) | 59.5 % | 74.1 % |
| L_Flooded_FStreets_P | 1197/1347 (88.9 %) | 753/914 (82.4 %) | 61.3 % | 77.5 % |
| L_Galvani1_P | 874/935 (93.5 %) | 547/617 (88.7 %) | 65.2 % | 82.4 % |
| L_Galvani2_P | 899/967 (93.0 %) | 626/712 (87.9 %) | 66.8 % | 82.5 % |
| L_Isl_HighChaos_P | 2080/2286 (91.0 %) | 2349/2651 (88.6 %) | 62.6 % | 80.7 % |
| L_Isl_LowChaos_P | 1912/2131 (89.7 %) | 1382/1580 (87.5 %) | 60.6 % | 79.3 % |
| L_LightH_HighChaos_P | 628/647 (97.1 %) | 901/982 (91.8 %) | 65.9 % | 84.9 % |
| L_LightH_LowChaos_P | 589/610 (96.6 %) | 667/742 (89.9 %) | 64.6 % | 83.7 % |
| L_Out_HighChaos_EmilyAlive_P | 130/154 (84.4 %) | 76/82 (92.7 %) | 56.0 % | 77.7 % |
| L_Out_HighChaos_emilyDead_P | 127/154 (82.5 %) | 68/74 (91.9 %) | 56.1 % | 76.8 % |
| L_Out_P | 306/347 (88.2 %) | 80/86 (93.0 %) | 54.6 % | 78.6 % |
| L_OutsiderDream_P | 269/288 (93.4 %) | 364/517 (70.4 %) | 56.8 % | 73.5 % |
| L_Ovrsr_Back_P | 1327/1486 (89.3 %) | 1055/1208 (87.3 %) | 61.0 % | 79.2 % |
| L_Ovrsr_Kennel_P | 390/418 (93.3 %) | 513/552 (92.9 %) | 68.7 % | 85.0 % |
| L_Ovrsr_P | 1691/1924 (87.9 %) | 2797/3043 (91.9 %) | 63.8 % | 81.2 % |
| L_Prison_P | 1447/1544 (93.7 %) | 1680/1889 (88.9 %) | 62.1 % | 81.6 % |
| L_PrsnSewer_P | 983/1096 (89.7 %) | 924/1041 (88.8 %) | 60.4 % | 79.6 % |
| L_Pub_Assault_P | 1385/1501 (92.3 %) | 1963/2283 (86.0 %) | 64.2 % | 80.8 % |
| L_Pub_Day_P | 1502/1647 (91.2 %) | 2133/2434 (87.6 %) | 64.1 % | 81.0 % |
| L_Pub_Dusk_P | 1490/1617 (92.1 %) | 2084/2380 (87.6 %) | 63.7 % | 81.1 % |
| L_Pub_Morning_P | 1230/1342 (91.7 %) | 1535/1769 (86.8 %) | 60.7 % | 79.7 % |
| L_Pub_Night_P | 1021/1081 (94.4 %) | 1092/1254 (87.1 %) | 57.9 % | 79.8 % |
| L_Streets1_P | 1354/1592 (85.1 %) | 1929/2158 (89.4 %) | 62.0 % | 78.8 % |
| L_Streets2_P | 1470/1686 (87.2 %) | 1620/1858 (87.2 %) | 63.6 % | 79.3 % |
| L_Streetsewer_P | 1174/1253 (93.7 %) | 1256/1404 (89.5 %) | 56.3 % | 79.8 % |
| L_Tower_P | 524/548 (95.6 %) | 1566/1710 (91.6 %) | 67.6 % | 84.9 % |
| L_TowerRtrn_Int_P | 1415/1746 (81.0 %) | 1858/2086 (89.1 %) | 59.4 % | 76.5 % |
| L_TowerRtrn_Yard_P | 1398/1715 (81.5 %) | 1674/1804 (92.8 %) | 63.0 % | 79.1 % |
| all 470 packages | 42141/47354 (89.0 %) | 49131/55542 (88.5 %) | 62.2 % | 79.9 % |

By distinct class over all packages: 60 of 120 placed-actor classes and 125 of 266 Kismet op
classes are handled. Largest unhandled placed actors: `DisNPCDistractor` (1364),
`DisSplineLoftActor` (664), `PathTargetPoint` (349), `DisStaticMeshActor` (271), `DisFish`
(216), `SplineLoftActor` (170), `DishonoredNote` (163), `DisWhaleOilBattery` (146). Largest
unimplemented Kismet ops: `DisSeqEvent_Distracted` (1350), `DisSeqAct_Lock` (319),
`DisSeqEvent_AttentionIncreasedTo` (284), `DisSeqEvent_BreakableBroken` (233),
`DisSeqAct_RefillWhaleOilBattery` (224), `SeqAct_SetPhysics` (218). The scan writes 1400-odd
coverage work items with per-map counts and example objects.

**How much gameplay is UnrealScript.** Over every class of the install: 4772 functions, 2572
native and 2200 script, the script ones totalling 169 KiB of bytecode (the classes a map uses
and their supers: about 800 native, 300 script, 25 KiB). Most per-tick behaviour is native
(the player's `PlayerTick` is native in this build); the script is mostly event glue, so the
phase 3 decompiler is for the event handlers and the Ghidra index for the bulk.

### Collision oracle (probe 3, first query oracle)

`harness collision` (collision branch) and `oracle collision` share the query generator and
the per-query coordinate hash; the oracle reads the captured sets from its cache instead of
fixtures, runs them in parallel against the port snapshot, and clusters disagreements by the
original's hit component (or the port's source when the original hit nothing). Offsets that are
constant for a whole (line/box, port hit kind, trace) key are one rule-level cluster
(`systematic`), not hundreds of per-object items.

Main (be0a3da), captures of 2026-10-07 (exe `70da9613...`): Boyle Ext 44328 / 77283 agree
(57.36 %), Boyle Int 21221 / 44798 (47.37 %, 677 pawn hits excluded). The top items: every
vision line hits 4.00 uu earlier in the original (6237 + 5534 queries; classified
`PortMissing`: the non-accurate line back-off is on the collision branch, not main), then
per-component clusters such as port shadow-caster boxes (`L_Boyle_Int_box_shadow`) blocking
where the original does not.

After the collision audit (COLLISION.md) `port::collision_answers` and `harness collision` both
answer through `dishonored_world::engine_check` (the line back-off per hit kind, movers and
doors posed in the capture's recorded states; snapshot version 2). With the audit's port and the
exterior's new capture: Boyle Ext 76784 / 76826 (99.95 %), Boyle Int 44748 / 44798 (99.89 %);
`harness collision` on its stored fixtures (fractions rounded to 1e-4) gives 99.94 % and 99.90 %.
`harness collision` still keeps its own comparison and cause classification; moving them onto
`dishonored_oracle::diff` is open.

## Speed

Measured 2026-10-07 on the 16-thread dev machine, debug builds (workspace crates at
opt-level 1, dependencies at 3), sccache, `CARGO_BUILD_JOBS=3`.

**Today's costs (before the oracle):**

| Operation | Measured |
|---|---|
| Map import, `map_dump L_Boyle_Ext_P --no-textures` (geometry) | 5.1 s |
| Map import with textures (`map_dump`) | 9.2 s |
| `load_match` cold (fresh derived-data cache), Boyle Ext / Int | 9.0 s / 7.0 s |
| `load_match` warm (cached collision, nav, NPCs...) | 0.20 s / 0.17 s |
| Port WorldQuery throughput, 77283 Boyle Ext queries (61575 lines, 15708 pawn boxes) | 1 thread 72 k/s; 16 threads 358 k/s (0.22 s) |
| Same, 45475 Boyle Int queries | 1 thread 58 k/s; 16 threads 263 k/s |
| `package_scan` of all 470 packages (single thread, decompress + tables) | 26.9 s (25.2 s with properties) |
| Original side, `probe_launch collide` (4096 checks per pipe request) | 77283 checks in about 11.3 s, 45475 in about 6.9 s (about 6.8 k/s with name lookups; from the capture files' timestamps) |
| Original side, boot to an in-control level | about 4 min (INSTRUMENTATION.md) |

**Oracle vs targets:**

| Operation | Target | Measured |
|---|---|---|
| Re-check one map after a fix (edit in `dishonored_world`, `cargo build -p oracle --no-default-features`, `oracle collision --map L_Boyle_Ext_P`) | < 10 s | 9.4 s (build 8.2 s: world 3.7 s + oracle; run 1.3 s incl. 1473 dossiers) |
| Same with the default features (rebuilds `dishonored_sim`, about 16 s more) | | 25-32 s: use `--no-default-features` for collision fixes |
| Coverage scan of all 470 packages | < 30 s | cold 4.3 s scan, 5.0 s total; warm 0.07 s scan, 0.6 s total |
| First oracle capture per map | about 30 s | 7-11 s per 45-77 k queries once in a level; the map switch is phase 2 (one session, console `open`) |
| Warm re-sweep of every cached map and scenario | 1-2 min | 2.1 s (coverage 0.45 s, collision of both Boyle maps 1.5 s, scenarios 0.18 s) |
| Import a capture into the cache | | 0.3-0.6 s (22 MB text -> 6.7 MB binary) |

Scenario runs on the port take 10-170 ms (movement, 90-165 ticks, first run includes the
snapshot load). The re-check target needs the narrow build; an importer fix also needs
`oracle snapshot --map M` (the warm `load_match`, 0.2 s, plus the content caches the importer
change invalidated: up to about 9 s cold).

## Phase plan

**Phase 1 (this work, no hook).** Crates, cache and spec formats, the headless port runner
(collision queries, movement scenarios), the diff engine, the scorecard / work-list / dossier
writer, the coverage scan of every package with per-map numbers, the cause book, the import of
the collision audit's captures, the measured costs.

**Phase 2 (hook side).**
- One persistent hooked session that switches maps with the console `open` and waits for the
  level, instead of one boot (about 4 min) per map; fixed time step from `lab setup`.
- Native bulk calls on the game thread (`LINE_CHECKS` already batches 4096 per request; extend to
  navigation, light, LOS) and shared memory for bulk results instead of the pipe.
- The binary reflection dump of every live object (GObjects walk, UProperty chains, field values,
  component trace flags, transforms, shapes) into `StructureDump`s under `Probe::State`, keyed
  by the same paths as the port; `oracle structure` then diffs field by field.
- ProcessEvent / Kismet / state traces per tick of a scenario, written as `Trace` under
  `Probe::Trace` with the scenario's spec hash; `oracle scenario` reports the first divergence.
- Captures driven by the scenario specs: setup through `lab give/power/equip`, input through
  `InputKey`, state sweeps by writing the toggled actors' states (doors: `m_DoorState` +0x311).
- Primitive-directed queries aimed at the original's primitives from the dump (each kDOP group,
  convex element and volume), densification rounds captured automatically.

**Phase 3 (static index).**
- Ghidra index per exe hash: native function table names, per-class vtables, cross-references,
  and the trace call sites with their flags and extents -> `TraceProfile`s (`Profiles::Enumerated`).
- An UnrealScript bytecode decompiler (UELib's token tables, EXISTING_WORK.md) for the 2200
  script functions (169 KiB), mostly event glue; the scan's `FunctionDef`s already locate each
  function's bytecode.
- Code pointers in dossiers from the index (which native implements the diverging behaviour).

**Phase 4 (scenario library per system).** Scenarios for every system listed above, starting
from the measured baselines of INSTRUMENTATION.md and HOOK_TRIAGE.md (movement, sword, detection,
weapon spread, powers, grenade fuse...), each with checkpoints and recorded traces, and runner
support per system in the port (NPC setup, powers, items, Kismet).

## First port findings from the scenario runner

- `movement/run_sprint`: run 360 and sprint 540 uu/s plateaus match the original.
- `movement/jump_standing`: apex 106.667 uu matches; the vertical speed sampled after the
  launch tick is 750 uu/s against the original's 800 on its first airborne frame. Unclassified
  until the two sampling points are aligned (the original samples per rendered frame, the port
  after the tick's gravity step).
