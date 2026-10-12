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
   (decoded in [`research/dishonored/systems/tick_order.md`](dishonored/systems/tick_order.md):
   Kismet first; per pawn its physics, then its state machines, then its animation advance with
   the notifies fired inside it; the pose and the `RMM_Translate` root motion published at the
   end of the pawn group; then the NPC locomotion step, whose velocity the next tick integrates.
   Controllers and their brains are separate actors in spawn order, not first). Presentation
   subsystems (particle simulation, audio, skinning) have the same shape and step in the same
   `DT` quanta; their state is simply not replicated. The renderer only reads state. The oracle
   is a subsystem whose step compares the inbox and state against a capture.
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

- **Animation state into the sim.** Each pawn gains an animation state beside its `PlayerState`
  (`WorldState::anim`: the layers' sequence ids into the config's table, playheads, the flags
  their notifies raised; an NPC's tree state with the install), replicated in the snapshot's
  extras. Notifies fire during the advance from the sim's playhead and replace the copied
  constants (`SwingTiming::PLAYER`, the assassination and cast times; the runtime's `SWING_TAIL`
  goes with step 7). Root motion feeds movement for every clip that has it, generalizing
  `sync_motion`. The sim evaluates only what gameplay reads (root motion, notifies, playheads,
  weights); the renderer evaluates the full pose from the same state with the same code.
- **The AI requests chains and locomotion transitions** through the pawn states, the way
  `ArkComponentLocomotion` and the item contexts do; `npc_move: u8` retires.
- **`dishonored_anim`**: Edge sampling, a generic evaluator over the cooked AnimTree graph (the
  node classes as an open set; the locomotion node is already decoded), slot channels, notify
  dispatch as "everything in `[t0, t1)`" (the original's window, `tick_order.md` section 4)
  through a class -> handler table, root-motion extraction.
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

**Animation done 2026-10-11** (LOG.md, ORACLE.md "Animation residuals"): every scenario that
records `Bones` / `AnimState` is compared with its capture on every run. The runner samples the
simulation's own NPC tree state per tick in the capture's numbering (node weights, sequences,
times, children weights) and poses it for the bones the capture refreshes; the scorecard carries
an `anim` column (body mean / max at lag 0, the best lag) and an "Animation residuals" table
(per bone group, what plays on `FullBodySlot` on each side); specs check `AnimBoneMean` /
`AnimBoneMax`, `AnimSeq`, `AnimSeqTime` and `AnimLag` (`dishonored_oracle::anim`). Effects stay
as events (`Effect` / `EffectEnd` ticks); the player's tree (U-972) is not sampled.

## Migration order

1. Measure first: `oracle anim info` / `anim compare`, then `effects/` the same way. Animation in
   the scenario runner's checks and scorecard: done 2026-10-11 ("The measuring instrument").
2. `dishonored_fx` extracted from `present_rules.rs`; then its modules as an open list.
3. The cooked AnimTree graph decoded into an open node IR; then the generic evaluator, verified
   against the current layered output and the captures on idle, walk and one attack.
4. Tick-stamped animation events and fixed-`DT` presentation clocks (removes the drift before the
   larger move).
5. Animation state into the sim; notifies replace the copied constants; root motion generalized.
   **Done 2026-10-11** (LOG.md): `dishonored_sim::anim`. Moved: the sequence table
   (`GameplayConfig::sequences`: every notify of the player's, the crossbow's and the elite
   guard's sets, extracted with the config so the CI and the tests run without the install);
   per-pawn layers (`WorldState::anim`, in the snapshot's extras) advanced by `DT x` the time
   scale in the decoded order, every crossed notify a `SimEvent::AnimNotify`; the swing's phases
   from its `AttackZone` / `AttackInterruptable` (the player's and the elite guard's chains), the
   paired moves' ends and root motion, the casts, the grenade, the crossbow mesh's shot, the
   Spring Razor, the throw's ragdoll, the finishers' bend windows from the playhead; the
   hand-copied notify times of `tuning.rs` and friends read from the table; with the install the
   host's `AnimSource` gives each NPC an `AnimTreeInstance` state (`NPC_Human_at` /
   `NPC_Dead_at`, the `TreeState` split from a shared `TreeShape`). Stays: the choke, carry and
   reload stages count time against the table's exits (their sequences are not played, or play
   for their notifies only); the Windblast warmup and the mantle keep their decoded config values;
   the NPC tree's root motion is published, not applied (U-977, applied by step 6); one NPC profile (the doorman's)
   for every NPC and timed swings for the special classes (U-978); the runtime's pickers still
   draw (step 7).
6. Pawn states and item contexts as one subsystem; the AI requests through them.
   **Done 2026-10-11** (LOG.md): `dishonored_sim::pawn_states` and `dishonored_sim::npc_loco`.
   Moved: the brain's requests (`pawn_states::Request`: the item context with its start and
   variation, the GenericAction chain and serial, the locomotion config, stance, speed and path),
   handed to the step in `WorldState::requests` (`Crowd::requests`); the NPC master state machine
   (`Master`: Walk, ActionMelee, ActionParry, PlayAnim, Thrown, Dead_Limp; `WorldState::pawn_states`)
   with the cooked transition logic (`NpcFsm`, 201 rows) and the next-tick application; the anim
   player's full-body chains (the group pick avoiding the last, the step's rate roll and blend-in,
   the next step, the stop at the group's blend-out) ending the states that play them; the context
   gate (slot 95); the elite guard's attacks as ActionMelee of the context's chain (no travel),
   the side and back steps and the behaviours' GenericActions as PlayAnim, the parried attacker's
   `Parry` lose step, deaths through Dead; the chains' root motion in the decoded modes (U-977:
   `RMM_Translate` at the barrier, `RMM_Velocity` / `RMM_Accel` as the next tick's velocity); the
   locomotion component's start, stop and turn sequences from the path, root-motion driven while
   fully in (U-280), fed to the simulation's tree. `PlayerInput::npc_speed` retired (the speed is
   the locomotion's request); `npc_move` is no longer sent (the step publishes the running context
   into `last_input` for the runtime's picker); `MatchView::npc_actions` comes from the pawn states.
   Stays: the special classes' contexts start timed swings with the travel stand-in (U-978); the
   strong hit reaction, stun, choke, assassination, aim, minigame and surprise are not states yet
   (their timers stay in the sim modules; decoded, LOG); `Call` rows of the transition logic allow
   (the states' `CanTransitionTo` not decoded, LIKELY); the component's undecoded parts (U-981) and
   its LOD flag (U-980); the runtime still draws its own picks and its speed-threshold start / stop
   (step 7).
7. The runtime's pickers deleted as the generic path covers each case. **Done 2026-10-11**
   (LOG.md): the runtime draws `WorldState::anim` (`SessionView::anim`) and plays
   `SimEvent::AnimNotify`. Third person: every NPC is posed from its simulation `TreeState` with
   `dishonored_anim` (`npc_anim::pose_tree`: the tree's shape per part of the pawn's model, the
   pawn tweak's sets as the evaluator's clips over the `AnimBank`), the AI's look-at aims and the
   matinee's `MatineeStackNode` channels set on a copy (`TreeState::present`), FaceFX on the
   output; a pawn without a tree (a human player's body) stands in `NPC_Human_at` as cooked.
   First person: the rig poses the simulation's full-body / upper-body / left-arm layers (and the
   crossbow mesh's) through the decoded filters, each layer weighted from its own playhead and
   the requesting state's blend time (`SeqPlay::blend`); with nothing on the full body the rig
   stands in `Sword_Ready_Idle`'s first frame. Notifies: `anim_notify` resolves each event through
   the sequence table (sounds, footfalls, the local lens effects; particles, drops, severs and the
   ragdoll hand-over on the drawn mesh); trails read the simulation's open `DisNotify_Trails`
   windows; the NPC feet are the tree's `m_bFootOnGround` (`TreeState::feet_on_ground`). Deleted:
   `npc_anim::animate` and its chains, `present_rules::collect_hits` / `HitKind`, the runtime's
   notify tables (`SetData::notifies`, `crossing`, the rig's particle / lens / trail tables), the
   viewmodel's pickers (`full_body`, `upper_body`, `left_arm`, the transitions, `SWING_TAIL`) and
   every animation clock (the `Local<u64>` tick accumulators, `AnimInput::dt`; the ragdoll steps
   by the simulation's ticks). What the simulation does not request yet is not drawn (LOG.md:
   the player's navigation and most of its states, the NPCs' hit reactions, blocks, stuns,
   ambient idles, blinks, slaves and get-ups); waits on step 6.

Two cautions: take the tick order from the exe, not from design (done for pawns, controllers and
animation in [`tick_order.md`](dishonored/systems/tick_order.md); its open items are the components' tick groups
and the controllers' spawn order); and do not build a framework first. The three rules are the
contract; a scheduler crate is not.
