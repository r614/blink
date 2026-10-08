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
  (links the game's match: `dishonored_content`, `dishonored_multiplayer`, `dishonored_sim`,
  `dishonored_ai`, `dishonored_audio`, plus the presentation handlers cut from the runtime). `--no-default-features` links only `dishonored_world` and keeps the
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
oracle scenario [NAME...] [--system S]... [--all] [--jobs N] [--json]
                                                        scenarios on the port in parallel (+ original trace)
oracle sweep                                            everything cached -> one scorecard
oracle bench import|queries --map M                     the port's costs
oracle cache list
oracle gen collision --map M --sampling prim --profiles enumerated --out Q   every exe trace profile
oracle index build [--project DIR]... [--tables DIR] [--no-decomp]   static index (phase 3)
oracle index fn|class|uscript|xref|search|natives|traces|field|vcall|stats|raw ...  (`oracle index help`)
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
| 2 | Live state dump | hook, reflection walk of GObjects / UProperty | format + diff engine; phase 2: reflection and filtered dumps (`session dump`), full dump deferred |
| 3 | Query oracles | hook: collision, navigation, light, visibility | collision: cached captures vs headless port |
| 4 | Behaviour traces | hook, fixed-step scenario in both, first divergence | spec + trace + diff; full-system port runner and a 46-scenario library (phase 4, below); phase 2: original traces (`probe_launch scenario`) |
| 5 | Ghidra index | native table, vtables, cross-references per exe hash | phase 3 |
| 4 | Behaviour traces | hook, fixed-step scenario in both, first divergence | spec + trace + diff; port runner for movement; original traces in phase 2 |
| 5 | Static index | native table, vtables, cross-references, trace sites, decompiled C and UnrealScript per exe hash | done (phase 3, below) |

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

### Static index (probe 5, phase 3)

`oracle index build` builds one index per exe hash under `$DISHONORED_CACHE/oracle/index/<exe
sha256, 16 hex>/` (retail-derived: never committed); `oracle index ...` queries it
(`crates/dishonored_oracle/src/index`, Ghidra script `tools/ghidra_index/scripts/BlinkIndex.java`).

**Build.** Two halves, joined by name:
1. *Exe* (`BlinkIndex.java`, one `analyzeHeadless -readOnly` pass over an analysed project; the
   project comes from `--project DIR` (repeatable: the first unlocked copy is used) or
   `BLINK_GHIDRA_PROJECTS`, `;`-separated; a fresh one is `analyzeHeadless <dir> dishonored -import
   Dishonored.exe`, 860 s of auto-analysis). It writes plain tables (`tables/*.tsv`):
   - the `FNativeFunctionLookup` tables (`{"UClassexecFoo", &UClass::execFoo}` arrays): 335
     tables, 2560 exec functions, each labelled;
   - class records: the UTF-16 class name each native class registers (the record points one
     character past the `A` / `U` prefix), its InternalConstructor (the record pointer that stores a
     vtable) and every vtable the constructor stores (the largest is the primary; the others are
     interface vtables), slots up to the next code-referenced address;
   - every function, every call instruction (direct; virtual: `CALL [reg+off]` or `CALL reg` after
     `MOV reg, [reg+off]`, recorded as the slot byte offset; imports), every reference from code
     to ASCII or UTF-16 text;
   - trace call sites: the callers of `UWorld::SingleLineCheck` (0x0064E7A0) and
     `UWorld::MultiLineCheck` (0x0064A860), given their real signatures, then each call's
     `TraceFlags` resolved through the decompiler's p-code (constants, copies, casts, OR / AND,
     phis, and up to two levels of callers when the flags are a parameter) and its `Extent`
     resolved to a constant vector (built on the stack or a global) or marked dynamic;
   - Ghidra's decompiled C of every function (`ParallelDecompiler`, 60 s per function) into
     `decomp.c` with an offset table.
2. *Script* (`index::script`, `index::uscript`): every class of the 137 script packages (from the
   coverage scan's class definitions), with property layouts (the `UStruct::Link` rules of
   `prop_layout`), enums, constants, structs, defaults, and every function and state decompiled
   (below). Natives join the exe by exec name (`UActorexecTrace` = `Actor.Trace`).

The research docs are read too: every 8-digit address they cite is kept with its line (`fn` shows
them as research notes), and a backticked `Class::Function` right before an address names it.

**UnrealScript decompiler** (`index::uscript`). UE3 v801 tokens (UELib's table after the
`RangeConst` removal): 0x00-0x5A expressions, 0x60-0x6F extended natives, 0x70+ natives by index,
resolved through every package's `FunctionDef`s (operators print as operators with their
precedence, `super(Class).F()` from final calls to non-final functions, enum names for `switch`
cases). Object references are 4 bytes serialized and 8 in memory, and jump offsets are in memory
bytes: a function counts as decompiled only when the parse ends exactly at `ScriptStorageSize`
and its in-memory position at `ScriptBytecodeSize`. Control flow is rebuilt from the jumps (if /
else, else-if chains, while, for with its step when a `continue` jumps to it, do-until,
foreach with break / continue, switch with break); anything else stays a labelled `goto`, so
nothing is ever dropped.

**Results (2026-10-08, GOG exe `70da9613...`).**

| | |
|---|---|
| Script | 137 packages, 3004 classes, 4772 functions: 2200 script (169 KiB) + 2572 native, 13 state code blocks; **0 decompile failures**, 0 definition failures; 2 functions keep a `goto` (else-if inside a loop). Decompile time 0.3-0.5 s |
| Spot checks | `DisDoor.m_DoorState` at +0x311 and `DisDoor` vtable +0x1C4 = 0x00A2C760 (`ShouldTrace`), both as COLLISION.md measured; `APawn::physWalking` (0x006BD150) is `Pawn` vtable slot 141 and its `MultiLineCheck` passes 0x20DF with the pawn's extent (COLLISION.md); `DishonoredNPCPawn.BreakConstraint`, `Actor.DisplayDebug`, `HUD.DisplayLocalMessages` read the same as UE Explorer's output (`uc_all`), with operators, loops and `continue` restored |
| Exe | 73960 functions, 258262 calls (28909 virtual), 49922 string references; 2560 of 2572 natives with an address (the 12 left are interface stubs: `OnlineSubsystemPC`, `UIDataStoreSubscriber`...); 2760 class vtables |
| Traces | 219 call sites, 72 distinct constant flag values (23 sites also take run-time flags); 29 profiles with a constant extent, 42 with the run-time-extent sites taken at the pawn's extent. The hook-measured movement (0x20DF), vision (0x08023086, through a parameter) and pistol (0x020A28DF) flags are among them; the crosshair's 0x01022097 is not (0x01002097 is: its complex-collision bit is added at run time) |
| Decompiled C | 73682 functions, 1 failure, 61.1 MB |
| Build | `oracle index build` end to end 493 s: Ghidra export 474 s (natives 3 s, classes 6 s, functions 38 s, traces 44 s, decompiler 385 s on the 16-thread machine; 681 s under load), the script 6 s, assembly 7 s. `--no-decomp` (the previous C is kept) about 100 s; from existing tables (`--tables`) 6-9 s |
| Size | `index.bin` 7.2 MB (functions, natives, vtables, traces, decompiled-C offsets, research notes), `xref.bin` 10.9 MB (calls, string references), `script.bin` 5.0 MB, `decomp.c` 61.1 MB, `tables/` 20 MB |
| Query latency (warm, whole process) | 90-110 ms for every command (`fn` with its C, `class`, `uscript`, `xref`, `search`, `natives`, `traces`, `field`); the index loads in 25-30 ms (own little-endian codec; serde/bincode took 350-600 ms) |

**Wiring.** `Profiles::Enumerated` resolves to the index's profiles
(`dishonored_oracle::index::enumerated_profiles`); `oracle gen collision --sampling prim
--profiles enumerated` uses them (Boyle Ext: 1.05 M primitive-directed queries over 42 profiles,
against 50 k over the 4 measured ones). Every report's work items get code pointers into the
original when the index exists (`index::annotate`, from `write_report`): coverage items the
class's constructor, vtable, natives and script functions, property items the field's offset;
collision items the trace entry points.

**Limits.** `GNatives` is filled at run time in this build (the exec pointer arrays in `.data`
are not indexed by native number), so native numbers map to addresses through the lookup tables'
names; a hook dump of `GNatives` would confirm the numbering (U-830). Run-time extents (mostly the
tracing actor's cylinder) are not resolved statically (U-831). The slot offset of a virtual call
does not say which class's vtable it goes through (`xref` lists the calls through that offset in
any class). Ghidra's names are its own (`FUN_...`) except the exec natives, the research docs'
names and a few hook-confirmed entry points.

**For agents (usage).** Run everything from the repository root with `DISHONORED_CACHE` set (the
dev-env scripts set it); `oracle index help` lists the commands, each prints text or, with
`--json`, JSON. To find what implements a behaviour in the original: `oracle index search
<regex>` (classes, script functions, exec natives, Ghidra and research names, strings, script
bodies), then `oracle index fn <address|exec name|Class.Function|Class::vslotN>` for the native
(callers, callees, strings, trace sites, research notes and the decompiled C; `--no-c` to skip
the C), `oracle index class <Class>` for its layout with offsets, vtable overrides (`[ShouldTrace]`
style names where known), natives and decompiled script (`--brief` for just fields and function
names), `oracle index uscript <Class>[.<Function>]` for the script alone, `oracle index field
<Class> 0x<offset>` to name a `*(this + 0x311)` seen in the C, `oracle index vcall <Class>
<slot|0xoffset>` for a `(**(code **)(*this + 0x1c4))` call, `oracle index xref <address|text>`
for who calls or references something, and `oracle index traces` for every trace call site with
its flags and extent. Quote addresses as 8 hex digits. The output is retail-derived: describe
findings in your own words in research docs and never paste decompiled code into the repository.

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

## Original side: the hooked session (phase 2)

Status 2026-10-08: the persistent session, per-tick scenario traces into the cache, the by-name
primitives and collision through the session work on L_Boyle_Ext_P and L_Boyle_Int_P. The full
reflection dump of every live object and snapshot / restore are deferred (a filtered dump of the
objects a request names exists: `session dump`).

### How it works

- **Tick boundary.** The DLL patches the engine loop's time update (0x009DDDF0, called once per
  iteration by `FEngineLoop::Tick` 0x009E03B0, before the engine tick; prologue
  `55 8B EC 83 EC 20`). Its callback runs on the game thread between two world ticks: the world
  tick counter (`game_tick`), queued game-thread jobs, the old per-frame recorder, the spawn
  watch, tick scripts and **lockstep** parking (the game thread waits at the boundary while the
  client reads, writes and calls; `step N` releases exactly N ticks). With the fixed step from
  `lab setup` / `session open` (`GUseFixedTimeStep` 0x0141B27C, `GFixedDeltaTime` 0x012696B0)
  every released tick advances the world by 1 / Hz however long it was parked. A parked game
  whose client stays silent for 5 minutes is released.
- **Sampling point (traces).** Every channel of every frame is read at the boundary: the state
  at the end of world tick k (input processed by the PlayerController, pawn physics done, Kismet
  and timers run), before tick k + 1 starts. Frame 0 is the state right after the setup. The
  input of tick k (key events through `FViewportClient::InputKey`, view writes, console
  commands) is applied at boundary k and acts during the world tick that produces frame k + 1.
  This is the port runner's convention (`trace.push(t + 1)` after `step_world` with the input of
  t). Each frame also records `world.time` (`WorldInfo.TimeSeconds`) and `world.dt` (its
  difference), so frame-timing differences are visible as such.
- **Tick scripts.** A scenario's whole input and its read list are uploaded once
  (`SCRIPT_START`); the DLL applies and samples at each boundary and parks at the end. No pipe
  round trip per tick: 70-110 ticks / s on Boyle (faster than real time at 60 Hz).
- **Reflection.** `FIELDS` walks a struct's `Children` chain (UField.Next +0x38, UStruct
  Super / Children / PropertiesSize +0x44 / +0x48 / +0x4C, UProperty ArrayDim / ElementSize /
  Flags / Offset +0x3C / +0x40 / +0x44 / +0x5C, the subclass pointer +0x6C; UFunction flags
  +0x80, iNative +0x84, NumParms / ParmsSize / ReturnValueOffset +0x91 / +0x92 / +0x94; UEnum
  names +0x3C). The client decodes typed values as JSON.
- **Calls.** Functions without a native index go through `ProcessEvent`. `ProcessEvent` returns
  early for functions with a native index (operators, `SetLocation`, `VSize`...), so those run
  their exec thunk (`UFunction` +0x9C) directly with an `FFrame` like the one `ProcessEvent`
  builds (vtable 0x00FB944C, Node +0x10, Object +0x14, Code = the function's own parameter
  script of `EX_NativeParm` tokens, Locals = our parameter block +0x1C).

### Commands (32-bit `probe_launch`, JSON out; ORACLE agents use these)

```text
P=target-agents/i686-pc-windows-msvc/release/probe_launch.exe
$P session start --map L_Boyle_Ext_P [--hz 60]   launch if needed, front end, map, setup -> JSON timings
$P session <op> '<json>' | key=value...          one request -> {"ok":..,"result":..,"ms":..}
$P session repl                                  JSON lines {"op": ...} on stdin -> JSON lines out
$P session stop                                  end the game
$P scenario <NAME | file.ron>... [--no-reload] [--partial] [--channel N=OBJ:PATH] [--json OUT]
                                                 per-tick trace -> oracle cache (Probe::Trace, spec hash)
$P collide <queries> <out>                       collision checks (unchanged format)
```

Ops (arguments as JSON keys):

| op | arguments | result |
|---|---|---|
| `status` | | map, pawn, pc, tick, lockstep, parked, fixed step |
| `open` / `reload` | `map` (default: the current one), `hz` 60, `keep_alive` true | load / control seconds; the map is fresh (the restore primitive: reload, then re-apply the setup) |
| `setup` | `hz`, `keep_alive` | tutorials off, keep alive, fixed step, no pause on a real focus loss (`Engine.bPauseOnLossOfFocus`; an open pause menu is closed) |
| `give` / `power` / `equip` | `item` / `name`, `level` / `class`, `hand` | as `lab give / power / equip` |
| `find` | `name` (`Outer.Name`, `Base_N`), `class`, `limit` | addresses and full names |
| `read` | `object`, `path` (or none: every field, `filter`) | typed value(s) |
| `write` | `object`, `path`, `value` | the value read back |
| `dump` | `class` and / or `name`, `filter` (field-name substring), `limit` | objects keyed `<package>:<path>` with their fields |
| `props` | `class` or `object`, `filter`, `functions` | property names, types, offsets (and functions) |
| `call` | `object`, `function` (`Func` or `Class.Func`), `args` (object by name or list) | `return` and out parameters |
| `signature` | `object` or `class`, `function` | parameters with optional / out / return flags |
| `spawn` | `class`, `location`, `rotation` (degrees), `spawner` (pc), `package` (to load the class) | the new actor |
| `step` | `n`, `free` | runs n ticks; the world then stays parked (lockstep) until `resume` |
| `resume` | | lockstep off |
| `key` | `key` (engine name), `event` down / up / tap, `ticks` | handled |
| `console` | `command` | the console's answer |
| `place` | `location`, `yaw`, `pitch`, `snap` | where the pawn was put |
| `collide` | `queries` [[sx,sy,sz, ex,ey,ez, hx,hy,hz, flags]...], `source` | hits with actor / component |
| `events` | `functions` (substrings), `classes` (is-a), `max`, or `fetch: true` | traced ProcessEvent calls with ticks |
| `spawns` | `max`, or `fetch: true` | actors that appeared, with ticks |
| `select_power` | `component` (`DisActivePowerComponent_DarkVision`, `DishonoredActivePowerComponent_Possess`...) | the left hand on that power (after `power`) |
| `run` | `ticks`, `channels` {name: "object:path"}, `keys` [[tick, key, down]], `console` [[tick, cmd]], `events`, `spawns`, `damage` (pawn damage path), `every` | per-tick frames (end-of-tick state) inside the game, events / spawns / damage with ticks |
| `attention` | `npc` | the NPC's `PlayerAttnInfo` (`DisAttentionInfo_Complex`) |
| `layout` | `map` (or `file`), `offset` [0, 0, 20000], `reload`, `host` | builds a layout map in the host level (below) |
| `shaders` | `since` (id) | the distinct pixel and vertex shaders the game created (D3D9 `CreatePixelShader` / `CreateVertexShader`), bytecode written to `$DISHONORED_CACHE/oracle/shaders/<exe16>/{ps,vs}_<fnv64>.bin`; id, kind, creation tick, CTAB parameter -> register layout |
| `census` | `frames` | pixel shaders drawn per frame (draws, primitives, CTAB) while the world runs |
| `psconst` | `hash`, `draws` | live constants of the next draws with that shader, by CTAB parameter name |

Object specs: `pawn`, `pc`, `camera`, `worldinfo`, `input`, `engine`, `0x<addr>`,
`Default__<Class>`, or an object name (`DisDoor_21`, `PersistentLevel.DisDoor_21`). Paths:
`Location.Z`, `Controller.PlayerCamera.CameraCache.POV`, `StreamingLevels[0].PackageName`.
Values: numbers, enum names (`"PHYS_Walking"`), bools, strings, objects as
`"<Class> <Path>@0x<addr>"` (any of these, an address or an object spec is accepted when
writing), structs as objects or lists (`[x, y, z]`).

Trace channels per `Record`: `Pawn` (`pawn.x/y/z/vx/vy/vz/speed2d/crouched/physics`;
crouched is Dishonored's `m_bSneakModeToggled`, `bIsCrouched` stays false), `Vitals`
(`pawn.health`, `pawn.mana` = `m_Mana`), `Camera` (`camera.x/y/z/pitch/yaw/roll/fov` from
`CameraCache.POV`), `Npc(name)` (`npc.<name>.x/y/z/health/physics`), `Kismet` (events of kind
`Kismet`: ProcessEvent on `SequenceOp` objects: `Activated` / `Deactivated`...),
`ProcessEvent(filters)` (kind `ProcessEvent`; `Class:<Name>` traces every call on that class),
`SimEvents` (kind `Spawn`: actors that appeared). `first_divergence` compares the event kinds
both sides record and lists the others as one-sided. Buttons: Jump SpaceBar, Crouch C, Sprint
LeftShift, Primary / Secondary mouse buttons, Block LeftControl, Lean Q / E, Use F (DefaultInput
`m_PCBindings`); Walk and Reload have no key in the original; moves need -127 / 0 / 127.

Setup gaps (an error, or a warning recorded in the provenance with `--partial`): story flags
(the pawn's `m_StoryFlagInstances` are readable, not written yet), NPCs from tweaks and AI
states (an existing pawn or a spawner's pawn can be moved), `kismet: false` (the original always
runs its Kismet: a warning), `seed`.

### Measured (2026-10-08, 16-thread machine with other agents compiling, CPU at 100 %)

| Operation | Measured |
|---|---|
| Boot to in control (launch, front end, `open`, briefing) | 27-50 s (front end 19-38 s; 159 s once under full load); was about 4 min with the frame-count waits |
| Map switch (`session open`) | Boyle Int -> Ext 10.7 s, Ext -> Int 4.4-10.7 s (load 0.8-2.5 s, the rest is the briefing and the in-control check) |
| Scenario trace (tick script, Pawn + world clock) | 71-113 ticks / s (90 ticks in 0.8-1.3 s, 165 in 1.5 s); every channel kind (Pawn, Vitals, Camera, Kismet, ProcessEvent, Spawn) 240 ticks in 3.2 s (75 / s) |
| Trace per tick with a client round trip (lockstep `step 1` loop) | 6-9 ticks / s (why the tick scripts exist) |
| `step n` free run | 26-38 ticks / s in the level, 72 / s at the front end |
| By-name read / write / call | 10-70 ms per CLI call (process start + reflection lookups); `repl` avoids the start |
| Filtered dump | 18 doors with every field 225 ms (209 KB JSON); 33 NPCs' health fields 58 ms |
| Collision through the session | 20000 checks 0.1 s in the game (200 k/s), 4.5 s with the hit names |

### Capture requests answered (2026-10-08, Boyle Ext, fixed 60 Hz, end-of-tick sampling)

Measured with `session run` (tick scripts) on the running original; ticks are frames after the
press boundary (press at k: the input acts in the tick producing frame k + 1). The Playground
scenarios' own traces need the Playground in the original (next deliverable); these values are
what their checkpoints ask for, measured on Boyle placements.

| Request (scenario) | Original | Notes |
|---|---|---|
| Dark Vision L2 cost / duration (`powers/dark_vision`) | **20** mana; **30.0 s** (`m_fTimeRemaining` set to 30 the tick after the press, counting from 2 ticks later); fade in 0.033 s, fade out 0.2 s | a second press cancels it without cost; mana regenerates the 20 from 3.0 s on (1 point / 7 ticks) while it is active |
| Bend Time L1 cost (`powers/bend_time_l1`) | **60** mana, taken 9 ticks after the press (cast animation); `TimeDilation` 1 -> 0.85 in 9 ticks (-1/60 per tick) | |
| Possession L2 cost (`powers/possession_guard`) | **60** mana, 42 ticks (0.70 s) after the press (held 30 ticks, guard 300 uu ahead); the player travels into the guard in 4 ticks; the guard is hidden 44 ticks later | L1 cannot take a human |
| Devouring Swarm cost / rats (`powers/devouring_swarm_l1`) | **40** mana (L1 and L2); a `DisRatSwarm` actor 39 ticks after the cost, its `DisRatSwarmSpawner` releases **30 rats (L1), 60 (L2)** over 16 ticks, individual `DisGameCrowdAgentSkeletalRat` crowd agents (pooled: the spawn watch saw 27 new ones, the rest reused); `m_fDuration` 30 s, then the swarm flees / ends and the rats despawn over about 10 s | the port's single-group model has no per-rat agents |
| Pistol on a guard (`combat/pistol_hit`) | **45** `DishonoredDamageType_Bullet` (300 uu, body) 3 ticks after the press; plus **15** `DishonoredDamageType_BulletBlast` to pawns near the impact | |
| Crossbow bolt on a guard (`combat/crossbow_hit`) | **40** `DisDamageType_Arrow`; the `DisProjectile_Arrow` spawns 3 ticks after the press, hits 2 ticks later at 300 uu | |
| Grenade (`combat/grenade_fuse`) | armed when cooked in the hand (10 ticks after the press), **3.0 s** fuse from arming; thrown on release (18 ticks), launch (798.8, 2369.0, -7.7) = 2500 uu/s horizontal at -5 deg pitch, gravity 1500 uu/s^2 (-25 uu/s per tick), first displacement one tick after the spawn; impact damage 6 (`DishonoredDamageType_Projectile`); explosion (`DisExplosion`) at fuse 0: **50** to the guard at 50 hp (killed), 11 to a 10 hp NPC, **33** to the player about 1050 uu away (`DisDamageType_Grenade`) | per-tick trace in the session scratch |
| Spring razor (`combat/spring_razor`) | placed (`DisGadget_SpringRazorPlaced`) 13 ticks after the press at a guard's feet; triggers 53 ticks later: the hook sees **5** `DisDamageType_SpringRazor` per pawn in range, but the guard's health goes 50 -> 0 (the razor kills: the amount is changed after the damage-path entry) | |
| Windblast L2 knock-back (`powers/windblast_l2_hit`) | cost 50 the tick after the press; damage 19 ticks after: **26** (`DisDamageType_WindBlast`, momentum (756, 2243, 1307)) at 182.8 uu; the guard becomes `PHYS_RigidBody`, peak horizontal speed **1005 uu/s** 2 ticks later, then 100 `DisDamageType_Impact` (killed) 5 ticks later | the spec's 2373 is the momentum, not the ragdoll's speed |
| NPC slash (`combat/guard_attack`) | **15** `DisDamageType_NPCSlash` (an alerted guard on an NPC) | a sword hit on an unaware guard is 99999 (a one-hit kill) |
| Hearing (`ai/investigate_noise`) | a pistol shot fired up 800 uu behind the party guard: its `PlayerAttnInfo` jumps to 1.0 / level 3 in the shot's tick, level 4 two ticks later, the guard starts moving **3 ticks (0.05 s)** after the shot | |
| Patrol speed (`ai/patrol`) | **116.3 uu/s** (three walking Boyle guards, steady) | |
| Detection at the 600 placement (`stealth/detect_600`) | LOS norm **0.353 = 600 / 1700** (outer cone) at first, rate 0.339 / s; level 1 at frame 45; the head turns and the norm becomes 0.169 (= 591 / 3500, inner cone), rate 0.79 / s; level 4 at frame 111 | U-864: the 2026-10-06 0.164 was taken with the head already turned |
| Detection at the 1300 placement (`stealth/detect_1300`) | the snapped placement is on the lower floor (z -217.8): no LOS for 30 ticks, then norm 0.78-0.88 at rate 0.1 / s, level 1 at frame 169, then 0.373 | the spec's placement is not the 2026-10-06 one |
| Lean (`movement/lean`) | Q held: the camera moves **88.9** uu sideways and **16.0** down, roll **-10** deg; 78.5 uu at 0.2 s | HOOK_TRIAGE's 119.7 / 21.4 was measured from another pose |
| Fall 1200 (`movement/fall_1200`) | landing speed **2675** uu/s, damage **45** (`DmgType_Fell`), gravity 3000 uu/s^2 | |
| Sneak speed, sword drawn (`movement/crouch_tunnel`) | **247.5** uu/s (`m_bSneakModeToggled`, CollisionHeight 65) | = the port |
| Blink L2 effects (`powers/blink_l2_level`) | the camera lens effect `Blink2.Blink_PlayerWind_01` lives 59 ticks (0.98 s); `Powers.Ps_Tattoo_Glow_02` (the hand mark) ends 23 and 29 ticks after the cast; no arrival system is recycled through the emitter pool | effect starts are not visible (spawned by script calls, which do not pass through ProcessEvent): lifetimes of pooled effects are known only from their ends (U-805) |
| Sounds (several) | not captured (U-803) | |
### Power screen effects: shaders and parameters (2026-10-08)

The cooked PPG_* post materials (Bend Time vectors, Blink, Possession, Dark Vision) and
`BlinkTarget_PMAT` have no shader map on disk; the running game creates their shaders, and the
DLL keeps the bytecode of every shader it creates (`SHADER_DUMP`, protocol op 42). Shaders are
created as materials are first used during the load (7283 distinct, 6365 PS + 918 VS, 12.7 MB on
Boyle Ext); casting a power created no new ones, so the effect shaders come from the store and
are identified by drawing.

`tools/probe_launch/scripts/effect.ps1 -Label L -Power P -Level N -Component C` (session ops `census`,
`select_power`, `run`, `shaders`, `psconst`) writes
`$DISHONORED_CACHE/oracle/shaders/<exe16>/effects/<label>.json`: the census before the cast and
while active, the shaders drawn only while active (hash, CTAB layout; bytecode in the store as
`ps_<hash>.bin`), up to 12 constant sets per such shader, and `DisPostProcessManager`
(`L_Boyle_Ext_P...DishonoredGameInfo.pPpManager`) parameters per tick (`m_BendTimeIntensity`,
`m_BlinkParams`, `m_PossessionParams`, `m_PossessionStateTime`) plus `TimeDilation`. Captured:
`bendtime_l1` (the ArkPp vector-field blur `gSrcTexture / gVectorFieldScales / gVectorFieldTexture`,
two passes 0xbd9a42b61794aabb and 0x6095a5daa9109b1d, the vector-field material pass
0xbbe665d07e36986b; `m_BendTimeIntensity` 0 -> 0.63 over 15 ticks), `blink_l2` (14 shaders while
aiming, the same vector-field blur and SceneColor-sampling material passes), `darkvision_l2`
(27, mostly the lit scene variants Dark Vision switches to), `possession_l2` (17 during the
transfer). The "drawn only while active" sets include scene shaders whose draws changed with the
view; decode with `fxc /dumpbin` against the CTAB names. Never committed.
### The capture queue (`probe_launch queue run`)

`probe_launch queue run [fixtures/oracle/capture_queue.txt] [--out harness_out/oracle/capture_queue_results.json] [--skip-layouts] [--only PREFIX]`
runs the queue in the running session, in file order: a scenario name is captured like
`probe_launch scenario NAME` (fresh map, strict setup: a setup the original cannot reproduce
fails with the reason instead of being forced), a `session <op> key=value...` line runs that op.
The results file maps every line to ok / failed / skipped with the reason or the answer; the
traces go to the oracle cache. Run it from the repository root.

First full run (2026-10-08, main edf53a7's queue of 101 lines: 68 scenarios, 33 session lines):
98 ok, 3 failed in 924 s (one session; Playground specs built as layouts). The failures are
setup gaps: `SleepDart` and `StickyGrenade` ammo (their inventory tweaks are not loaded in
Boyle; mapping them needs their tweak names and a `DynamicLoadObject`, about an hour) and a
mid-run `Hand` switch (scripted hand switches: the power wheel / item select path, about half a
day). `oracle scenario` then reports a first divergence for all 68; most are at tick 0, the
settled height: the original hovers 3.73 uu above a floor (cube or Boyle runway), the port 6.73,
which masks later channels until that is resolved or tolerated.

Third full run (2026-10-08, main d42497c's queue of 160 lines, this branch's build in
`target-agents-hook`): 160 ok, 0 failed, 1098 s.

### Layout maps in the original (the Playground)

A scenario whose map has a layout file (`maps/<map>.ron`, the Playground) is captured by
building the layout in the host level `L_Boyle_Ext_P` at +20000 uu z (`session layout
map=Playground`; 161 boxes, 9 meshes, 13 actors, 2 volumes in 2.5 s): every box, stair step and
ramp is an `InterpActor` with the level blockout cube `Env_Blockout.staticmeshes.Cube_100`
(100 uu, simple box collision) scaled and rotated to it (a `DynamicSMActor_Spawnable` turns
`PHYS_RigidBody` and falls once a pawn steps off it); original meshes the same way; doors,
usables, pickups and the chain copied with `Spawn(ActorTemplate = the placed original)` (the
chain from Boyle's basement is replaced by the exterior's `DisClimbable_3`); the pool and the
pain floor reuse a host `DishonoredWaterVolume` / `PhysicsVolume` moved and scaled. The trace's
positions are converted back (`pawn.*`, `camera.*`, `npc.*` minus the offset). Engine checks hit
the cubes exactly (line hits 1 uu above a top face, pawn boxes at top + 72 + 1). Not built: the
lights (the stealth lane's light levels differ), labels and the nav grid; PhysX bodies
(ragdolls, physics props) fall through the cubes (U-806). Measured on the original Playground:
`movement/step_50` climbed (z 91.2 -> 141.2), `step_56` not, the 45 degree slope walked at
400 uu/s to its 400 top, the 55 degree slope not.

### NPCs in original traces

- Placement by position (`npcs: [(position: ..)]`): a live NPC of the host level of the kind
  (`*Guard*` tweaks for guards, the others for civilians; not Tallboys, the boatman, Shaw) is
  teleported there; `Passive` unpossesses it (no controller, no AI: it takes damage and falls
  like a dummy); `Idle` keeps its own AI; `Alerted` is approximated by filling its player attention
  (1.0, a warning); `faction` writes `DishonoredNPCPawn.m_pCurFactionTweak` (a tweak by name,
  `None` = null). Pawn tweaks, routes and other states are setup gaps (U-802). An NPC by
  `spawner` is the spawner's live pawn (a pointer in its fields, its decoded arrays, or a pawn
  pointing back at it).
- `settle`: the placed player is set falling (`Physics` written; `SetPhysics` is ignored for
  the player) so it lands and hovers before tick 0 like the port's settled pawn. On the
  Playground's cubes the original hovers 3.73 uu above the floor (centre floor + 91.23), the
  port 6.73 (94.23): reported as the first divergence of the Playground movement specs.
- Brain channels per `Npc(id)` (through `Pawn.Controller.m_pAIBrain`, `DishonoredAIBrain`):
  `npc.<id>.suspicion` (`m_SuspicionLevel`), `.top_attn_level`, `.top_enemy_attn_level`,
  `.player_attn.level` / `.value` (`m_pAttentionProcess.m_pPlayerAttentionInfo`, accumulator
  +0x8C, level +0xAC), `.attn_count` and `.attn<0..3>.level` / `.value`
  (`m_AttentionInfos`), and `NpcBehavior` events naming the class of `m_pCurrentBehavior`
  (+0x14C, detail `current`) and of each occupied slot of `m_ActiveBehaviorStack` (+0x100, 19
  slots, detail `slot<k>`) whenever they change.
### Sounds, effects and AI noises in traces (U-803, U-805)

Three native entry hooks turn the paths that bypass ProcessEvent into trace events (capture:
`Sounds` records `Sound` and `Noise` events, `Effects` records `Effect`, any `Npc(..)` record adds
`Noise`; `session run` takes `"sounds"`, `"effects"`, `"noises": true` and answers
`hook_events`):

| Kind | Native | Event |
|---|---|---|
| `Sound` | 0x009B2BE0 (audio device post, thiscall `(UAkEvent*, AActor* source, ...)`; `Actor.PostAkEvent`'s exec 0x005C1050 -> 0x006CD730 -> it; native callers directly) | object = source actor, name = `Package.AkEvent` |
| `Effect` | `UParticleSystemComponent::ActivateSystem` 0x008B1F80 (pooled effects too: U-805) | object = the component's Owner (+0x48), name = Template (+0x1C4), detail `comp=0x..` |
| `Noise` | AI noise dispatch 0x00C5A760 (thiscall on the noise manager, `(event*)`) | object = source (+0x24), name = `EAINoiseLoudness` (+0x20), detail `context=` (`EAINoiseContext`, +0x21), location (+0x14), +0x38 |

Measured (Boyle Ext, a pistol shot then a sprint): `Snd_Weapon_Gun_Corvo_Prefire` 1 tick after
the press, `..._Fire` 8 ticks after, rats' screams, `Snd_P_Sprint_Breath`, music state posts
(source None); effects `Ps_PlayerPistol_StdMuzzle_01` and the pooled `Ps_Pistol_StdTracer_01` in
the shot's tick, the impact 3 ticks later; noises: the shot `EAINoiseLoudness4` Danger at the
player, the bullet's impact `EAINoiseLoudness3` Danger, sprint steps `EAINoiseLoudness12m`
(context shown as Anomaly: the context byte is read at +0x21 and not yet cross-checked against
the decompiled dispatch) every 16 ticks, a `6m` Footfall at the stop. Footstep sounds were not
seen in this path (the footstep AkEvents may be posted by another native: open).

### Setup additions (2026-10-08, second round)

- Ammo: `Bullet` / `BulletExplosive` / `Bolt` / `SleepDart` / `IncendiaryBolt` / `SpringRazor` /
  `Grenade` / `StickyGrenade` with `:<n>` write `DishonoredInventory.m_AmmoInfo[eDisAmmoType]`
  (after giving the weapon); a weapon's ammo selection (`left_hand` or a mid-run `Hand`) writes
  `m_CurAmmoType` (+0x118) and `m_iCurAmmoType` (+0x120; the loaded round is kept, a warning).
- `HealthElixir:<n>` / `ManaElixir:<n>`: `m_ElixirCounts[0 / 1]`; `Drink(..)` presses R / T.
- `Upgrade:<Name>`: `DishonoredPlayerPawn.OnGiveUpgrade` with `DisSeqAct_GiveUpgrade`'s
  default pointing at `Twk_Upgrades.Twk_Upgrade_<Name>`.
- Mid-run `Hand(<power>)`: a write of `DisItemPowers.m_pCurrentActivePower` at that tick.
- NPC `tweak`: the host level's live NPC with that tweak (e.g. the Boyle doorman).
- Airborne starts (no floor within half height + 10 below the start, no `settle`): the pawn
  starts `PHYS_Falling` with zero velocity at frame 0, as the port does (the fall specs before
  this fix hovered at their start height: re-captured).
- `session` key=value: a token without `=` continues the previous value
  (`console command=adddarkness 11`). The darkness / debug console commands are not in retail
  ("Command not recognized" in Launch.log): the darkness sequence of the queue needs direct writes
  of the darkness manager instead.
- Shader timelines: `tools/probe_launch/scripts/shader_timeline.ps1` samples one pixel shader's
  constants per frame across a power (through one `session repl`) with the power's state:
  `effects/darkvision_eyelid_timeline.json` (0x3c1c89ab574a7626: `UniformPixelVector_1` the
  eyelid colour (0.1275, 0.0892, 0.0582, 1), `UniformPixelVector_2` the alpha, 0 -> 1 within the
  first 0.35 s, 1 for the 30 s, then down through the 0.2 s fade-out) and
  `effects/blink_lens_timeline.json` (0x0b36c71ce1512549: `UniformPixelVector_1.x` rises to 0.091
  while aiming and settles at 0.07; during the travel x and y peak at (1.47, 1.88) / (0.61,
  2.43), then decay to 0 over about 1 s).
### Usage for fuzzing agents

Start (or reuse) the session with `probe_launch session start --map <M>` (a running session is
reused; `session status` tells the map). Before each experiment call `session reload` (a fresh
map: the restore primitive) and `session setup`, then put the world in the state you need with
`place`, `give` / `power` / `equip`, `write` (any property by path) and `spawn`. Freeze the world
with `step n=0`, then alternate `call` / `write` / `key` and `step n=K` and observe with `read`,
`dump class=... filter=...`, `events` and `spawns`; `resume` when done. Look up names first with
`props class=X filter=Y functions=true` and `signature`. For a whole scripted run, write a
scenario RON and run `probe_launch scenario file.ron --json out.json`: the trace lands in the
oracle cache under the spec hash and `oracle scenario NAME` reports the first divergence against
the port. In PowerShell pass arguments as `key=value` (it strips quotes inside JSON); in bash
JSON works. One game at a time: only the hook agent launches it; other agents use the running
session through these commands.

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

**Phase 3 (static index): done 2026-10-08** ("Static index" above).
- Ghidra index per exe hash: native function table names, per-class vtables, cross-references,
  and the trace call sites with their flags and extents -> `TraceProfile`s (`Profiles::Enumerated`).
  Done, plus string references and the decompiled C of every function.
- An UnrealScript bytecode decompiler (UELib's token tables, EXISTING_WORK.md) for the 2200
  script functions (169 KiB), mostly event glue; the scan's `FunctionDef`s already locate each
  function's bytecode. Done: 0 failures over the install.
- Code pointers in dossiers from the index (which native implements the diverging behaviour).
  Done for coverage and collision items. Open: the `GNatives` numbering from a hook dump (U-830),
  run-time trace extents (U-831), and per-scenario pointers once phase 4's scenarios name their
  classes.

**Phase 4 (scenario library per system).** Scenarios for every system listed above, starting
from the measured baselines of INSTRUMENTATION.md and HOOK_TRIAGE.md (movement, sword, detection,
weapon spread, powers, grenade fuse...), each with checkpoints and recorded traces, and runner
support per system in the port (NPC setup, powers, items, Kismet).

### Phase 4 status (2026-10-08): the scenario library and the full port runner

**Runner** (`tools/oracle/src/runner.rs`). Every scenario is played through the game's own
match: the spec becomes a `MissionDef` (NPCs placed by position or original spawner with an AI
state `Idle` / `Patrol` / `Wander` / `Alerted` / `Passive` and a faction, the loadout from
`items` / `powers` through `apply_loadout`, story flags, the populated level's original
spawners, the original Kismet started by its remote event, seeds), built by
`dishonored_content::mission_setup` on `MapContents::load` (= `load_match`), then
`GameSetup::build`, `Game::add_player` and `Game::step` at 60 Hz, as the runtime's offline
session does. Input per tick: move, look, buttons with a hold, the left-hand selection
(`Hand`), elixirs (`Drink`) and console hooks (`teleport`, `health`, `mana`, `power`, `give`,
`alert`, `calm`, `saveload` = the game's `save::encode` / `save::restore` mid-run,
`cansave`; anything else is logged as unsupported). `setup.settle` runs unrecorded ticks
first. Setup notes (what the port could not set as specified, e.g. pawn tweaks of placed NPCs,
`keep_alive`) ride with each result.

**Channels** (always recorded): pawn (position, velocity, mode, crouch, under water, air),
vitals, camera (eye, view, lean angle and roll), left hand (item, phase, ammo, dispersion, Blink
target), powers (time scale, world dilation, Dark Vision, possession stage), inventory, world
counters (projectiles, windblasts, traps, swarms and their rats, time bends), the first
projectile, live effects / particles, and per named NPC position, yaw, health, condition, AI
state, stance, attention on the player (value, level, LOS norm, sighted), suspicion and time
scale. Enumerated channels carry a fixed legend in their unit (`enum:Walking,Falling,...`,
declaration order). Events: `Sim` (every SimEvent with `key=value` details, ids as names),
`Kismet` (activations), `Projectile` / `ProjectileEnd` (player shots carry their offset from
the view in 720-line pixels, `off_px`, the dispersion, `disp`, and `rd` = off / disp),
`Console`, and the presentation events below.

**Presentation** (`tools/oracle/src/present.rs`, U-865). The presentation rules live in a
Bevy-free module, `crates/dishonored_runtime/src/present_rules.rs` (which simulation events
ask for which effect or beam, `FxEventState::requests`; which sounds are posted where,
`SoundEventState::posts`; the Blink indicator and attached effects; the emitter step and the
one-shot despawn rule; `collect_hits`). The game's `effects.rs` / `audio.rs` / `characters.rs`
call it, and the oracle compiles the same file through `#[path]`, so both run one copy of the
rules (no source extraction, nothing to drift). The gameplay ParticleSystems (`GAMEPLAY_PARTICLES`) are decoded headless
through the level importer and cached; the contact system is loaded for impact and blood paths.Events: `Effect` (system, instance, position, emitters, untextured emitters, emitters without a
lifetime), `EffectEnd`, `EffectMissing` (the runtime asks for a system it did not decode),
`Sound` (the AkEvent posted; `(no cue)` for an unmapped one), `Anim` (hit reactions).

**Spec additions** (`scenario.rs`): metrics `Final`, `Delta`, `DropFrom`, `Diff`, `TimeBelow`,
`TimeAt`, `Gap(a, b)`, `Abs`, and event metrics over patterns `Kind:Name@object[key=value]`
(`*` wildcards): `Count`, `FirstAt`, `Between`, `EventValue`, `EventSum`, `EventMean`,
`EventVar`, `Lifetime` (effect spawn to end), `Unended`; comparisons `Eq` / `AtLeast` /
`AtMost`; reference status `Measured` (hook), `Static` (tweaks or decompiled code),
`Unverified` (measured but not known to be sampled like the port: reported, not judged, listed
as a capture request) and `Unmeasured` (the port's value is reported as a capture request);
`capture` (how to take the original's side). `spec_hash` now covers only the run-defining
fields (map, setup, step, ticks, input, record, sweep), so correcting a reference keeps the
captured traces.

**Library** (`fixtures/oracle/scenarios/<system>/`, 46 scenarios): movement 14 (jump launch /
apex / airtime, run and sprint, steps 50 / 56, slopes 45 / 55, mantles 330 / 360, falls 800 /
1200, drowning, lean, crouch tunnel), combat 9 (sword timing and damage on a guard, guard
attack, block, pistol hit / dispersion settle / 40-shot spread, crossbow, grenade fuse, Spring
Razor), powers 10 (Blink level 1 / 2 / aimed up / onto the floor, Windblast level 1 / 2 on a
guard, Dark Vision, Possession, Bend Time, Devouring Swarm), stealth 4 (detection at 600 /
1300 on Boyle, lit / dark lane), AI 3 (patrol, hearing, a placed pawn's rotation), inventory /
economy 3, save / load 2, Kismet 1 (Boyle's original Kismet from `ChangeLvl_fromHub_toBoyle`).
Positions and baselines come from INSTRUMENTATION.md / HOOK_TRIAGE.md (Boyle Ext runway, the
doorman, the party guard `DishonoredSpawner_46`) and the Playground stations; static references
from the tweaks (`Twk_Blink.m_pGroundPS`, `Twk_WindBlast.m_EffectTemplate`,
`Twk_Pawn_DefaultNPC.m_VisionConePatrolTemplate`, `Twk_Powers_Possess.m_pHandCastParticle`,
`Twk_DevouringSwarm.m_pSpawnFX`, the grenade's `m_pExplosionEffect`, ...) and decoded rules
(AI.md attention increases, COLLISION.md step / slope limits).

**Results (2026-10-08, worktree branch, release build, 16 threads):** 46 scenarios in 6-10 s
(the Boyle Kismet one 5-10 s alone; the rest 0.01-1.8 s each; maps and effects load once):
28 pass, 13 fail, 5 unmeasured; checks 84 pass, 24 fail, 36 unmeasured, 10 unverified (46
capture requests). Matching the hook's measurements: Blink range (1096.7 / 1597.7), aimed-up
target (1385.8 / +500.0), travel 0.267 s, cost 20 and refund; Windblast 26 damage, cost 50; Bend
Time 0.15 / 0.85 / 12.0 s; grenade fuse 3.0 s; pistol shot to load 1.483 s, dispersion 70 / s,
r / d mean 0.518 and variance 0.087 over 40 settled shots; sword press to damage 0.300 s;
run 360 / sprint 540, apex, airtime 0.533; drowning 30 s of air then 61 ticks a point; lean
20 degrees held, roll 10; step, slope and mantle limits. Divergences: Blink arrival effect
never ends and is always the fall one (U-860); muzzle flashes never end (U-861); a grenade
throw shows the pistol muzzle (it is reported as `Fired`); sword hits draw no blood (the contact
system's `Pfx_BloodMotion` is not in `GAMEPLAY_PARTICLES`); no effect at all for Dark Vision,
Possession, Devouring Swarm, the grenade explosion and trail, Spring Razor, the pistol tracer
and the crossbow muzzle and trail; crossbow shot to load 1.583 s (1.555, U-501); hearing a
shot raises attention to the maximum (U-862); passive NPCs turn to yaw 0 (U-863); lean onset
10.7 degrees at 0.2 s (14.2); sword damage 14 for 10 (U-113, classified KnownDifference).

**Rule fixes (2026-10-08, after the effects merge):** 46 scenarios: 37 pass, 4 fail, 5
unmeasured; checks 111 pass, 7 fail, 36 unmeasured, 9 unverified (before: 35 pass / 6 fail).
Fixed in the port: the jump launches after the press tick's walk (hook trace: vz 800 then 750);
the lean camera is the decoded spring point (head displacement per tick matches); reload and
fire stages count ticks like the original (f32 countdown, anim time against AnimStateExit,
the shot a tick after the fire action starts); the sword can be sheathed (400 / 600);
the runner snaps with the engine's line check and keeps a placed NPC's yaw. Still failing:
`ai/investigate_noise` (the guard reaches Combat: instant NPC turning and the port-only
`criminal()` rule, U-862), `combat/crossbow_reload` (fire action two ticks long, U-868),
`combat/sword_hit_dummy` damage (U-113, KnownDifference), `movement/lean` angle onset / limit /
soften (lean height smoothing, U-867).
**Usage for agents.** `oracle scenario --system powers` (or `--all`, or names, `combat/*`),
then read `harness_out/oracle/scenario/results.json`: `totals`, `by_system` (pass, fail,
unmeasured, error), and per scenario `status`, `first_divergence` (`against`: `original` when
a captured trace exists, else the first failing checkpoint, with its tick), `checks` (metric,
port `actual`, `expected`, `cmp`, `status`, `reference`, `source`), `notes` and `trace_file`
(the full trace JSON: `channels`, `frames`, `events`). `capture_requests.json` lists every
unmeasured / unverified check with the port's value and the `capture` recipe for the hook.
Build with `cargo build --release -p oracle` into the shared target and copy the binary away
(other worktrees build `oracle` into the same directory).

## First port findings from the scenario runner

- `movement/run_sprint`: run 360 and sprint 540 uu/s plateaus match the original.
- `movement/jump_standing`: apex 106.667 uu matches; the vertical speed sampled after the
  launch tick is 750 uu/s against the original's 800 on its first airborne frame. Unclassified
  until the two sampling points are aligned (the original samples per rendered frame, the port
  after the tick's gravity step).
- Same scenario with the phase 2 trace (both sides sampled at the end of the tick, 60 Hz,
  2026-10-08). The original, Jump pressed at boundary 30: tick 30 z -217.567 vz 0 Walking;
  tick 31 z -217.567 **vz 800 Falling** (the jump sets the velocity after that tick's physics: no
  displacement yet); tick 32 z -204.650 vz 750; 33 -192.567 / 700; 34 -181.317 / 650;
  35 -170.900 / 600; rise to the apex 106.667. The port launches and integrates in the same tick
  (750 at its tick 31), so it is one tick early, not differently sampled (U-763 (3)). The first
  divergence reported is earlier, at tick 0: the snapped start height (original -217.567 = floor +
  `CollisionHeight` 87.5 + 1; port -221.567), and the original then settles 2.6 uu higher.
- `movement/run_sprint` original: 400 / 600 uu/s plateaus (the spec does not draw the sword;
  the port's pawn always has it drawn: 360 / 540).
