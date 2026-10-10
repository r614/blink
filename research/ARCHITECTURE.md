# Architecture: subsystems, events and the data that composes them

The product is a hackable Dishonored core: every subsystem (animation, stealth, combat, sound,
NPCs, effects...) reproduced 1:1 from the original's data and code, and composable into missions
and game modes, with multiplayer on top. This note fixes the shape the port is moving to and why.
It is the contract new code is written against; `research/LOG.md` records how far the migration
has got.

## Why the shape changes (2026-10-10)

The port was first built as a deterministic, Bevy-free gameplay simulation (`dishonored_sim`,
fixed 60 Hz, shared by the server, client prediction, replays and the harness) with animation,
particles and sound as "presentation only" in the Bevy runtime. That split suits an arena mode;
it fights the original, where animation *is* the gameplay state: `State*Master*` states and
`DisItemContext_*` request animation chains, notifies (`AttackZone`, `Ragdoll`, `SeverLimb`,
`DropItem`, `FireProjectile`...) fire gameplay from the clip's playhead, and root motion moves
the pawn. Under the old split every animation-driven mechanic was written two or three times
(timing as sim constants, pose as a presentation picker that re-infers the clip from sim state,
notifies read again presentation-side), with two clocks (the sim's `DT` and the render delta) to
drift between and a lossy channel from the AI (`npc_move: u8`). That is the structural cause of
"sound / blood / particles out of sync" and "NPC movement not synced with its animation", and
why the oracle, which compares end states and scalar channels, could not see it (U-865).

The one subsystem transcribed as *engine* code from the exe on day one, the Edge animation
decoder, has not churned since. Subsystems re-expressed feature by feature churn every day.

## The three rules

1. **One tick-stamped `Event` stream is the only coupling between subsystems.** Inputs, AI
   intents, animation requests, notifies, root motion, hits, deaths, spawns, effect and sound
   requests, Kismet events: all `(tick, Event)`. No subsystem reaches into another's state.
2. **A subsystem is one function over its own state slice and its own IR:**
   `step(state, assets, world, inbox, DT) -> outbox`. Deterministic, Bevy-free, serializable
   state. The simulation is an ordered list of these at fixed `DT`, in the original's tick order
   (controllers, pawn physics, components: animation and root-motion extraction, notifies; to be
   confirmed from `ADishonoredNPCPawn::Tick` / `TickSpecial`). Presentation subsystems (particle
   simulation, audio, skinning) have the same shape and step in the same `DT` quanta; their state
   is simply not replicated. The renderer only reads state. The oracle is a subsystem whose step
   compares the inbox and state against a capture.
3. **The composition language is the original's data, not Rust.** Tweaks, AnimSets and
   AnimTrees, ParticleSystems, Kismet, spawners and routes, `MissionDef`. A subsystem is an
   interpreter of a cooked format. Rust states and behaviours interpret tweaks; they are never
   "features". A custom NPC is a tweak, a custom finisher is an AnimSet with notifies, a custom
   mode is a rule subsystem plus a `MissionDef`.

## The layers

| Layer | Subsystems | Composed by |
|---|---|---|
| Engine (generic, 1:1 with UE3 / Arkane native code) | packages and IR, collision and movement, animation (AnimTree evaluator, AnimSets, notify dispatch), effects (Cascade), audio (Wwise semantics), Kismet and Matinee, navigation, conversation and FaceFX, ragdoll | cooked assets |
| Gameplay (Arkane's layer) | pawn states and item contexts, AI brain and behaviours, powers, inventory, attention and stealth, damage | tweaks |
| Mode | mission rules, free for all, Contracts, spectate | `MissionDef`, mode config |
| Client | skinning, effect rendering, audio playback, HUD, menus | settings |

"Pawn states and item contexts" is the subsystem that does not exist yet as one thing: today it
is timers in the sim modules and pickers in the runtime.

## What moves where

- **Animation state into the sim.** `PlayerState` gains an animation state (tree instance,
  chain / state, playheads, blend weights, locomotion modifier and phase; tens of bytes, replicated
  like the rest). Notifies fire at tick boundaries from the sim's playhead and replace the copied
  constants (`SwingTiming::PLAYER`, `SWING_TAIL`, the `gameplay_config.json` notify times). Root
  motion feeds movement for every clip that has it, generalizing `sync_motion`. The sim evaluates
  only what gameplay reads (root motion, notifies, playheads, weights); the renderer evaluates the
  full pose from the same state with the same code.
- **The AI requests chains and locomotion transitions** through the pawn states, the way
  `ArkComponentLocomotion` and the item contexts do; `npc_move: u8` retires.
- **`dishonored_anim`**: Edge sampling, a generic evaluator over the cooked AnimTree graph (the
  node classes as an open set; the locomotion node is already decoded), slot channels, notify
  dispatch as "everything in `(t0, t1]`" through a class -> handler table, root-motion extraction.
- **`dishonored_fx`**: the Cascade emitter simulation (`EmitterSim`), with the emitter's modules
  as an open list with per-class spawn / update instead of a flat struct of options, and the
  instance lifecycle (activate, deactivate, completion, auto-destroy, instance parameters) as
  generic rules. Not replicated: clients spawn the same instance from the same tick-stamped
  notify. Deterministic for the oracle and replays.
- **The runtime becomes a skinner and renderer.** No pickers, no inference of clips from sim
  state.

## The measuring instrument

Animation and effects become first-class oracle categories: per tick, node weights, sequence
times and bone angles (the probe records `SpaceBases` and every AnimTree node), notify-fire tick
and effect activate / finish tick, diffed continuously with a residual in ticks, degrees and
counts (`oracle anim compare`). Jank is a continuity property; end-state comparison does not see
it.

## Migration order

1. Measure first: `oracle anim info` / `anim compare`, then `effects/` the same way.
2. `dishonored_fx` extracted from `present_rules.rs`; then its modules as an open list.
3. The cooked AnimTree graph decoded into an open node IR; then the generic evaluator, verified
   against the current layered output and the captures on idle, walk and one attack.
4. Tick-stamped animation events and fixed-`DT` presentation clocks (removes the drift before the
   larger move).
5. Animation state into the sim; notifies replace the copied constants; root motion generalized.
6. Pawn states and item contexts as one subsystem; the AI requests through them.
7. The runtime's pickers deleted as the generic path covers each case.

Two cautions: take the tick order from the exe, not from design; and do not build a framework
first. The three rules are the contract; a scheduler crate is not.
