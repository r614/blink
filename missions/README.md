# Custom missions

A mission is one RON file. It's modelled on how Dishonored builds its missions (objectives
made of tasks, story flags, spawners, and Kismet "event -> condition -> action" wiring; see
`research/dishonored/MISSIONS.md`). Missions run on the authoritative simulation, so they
work solo, co-op on a dedicated server, in replays and in the harness.

## Workflow

```bash
cargo run --release -p mission -- new L_Boyle_Ext_P "My Mission" > missions/my_mission.ron
```

```bash
cargo run --release -p mission -- markers L_Boyle_Ext_P TargetPoint
```

```bash
cargo run --release -p mission -- original L_Boyle_Ext_P
```

```bash
cargo run --release -p mission -- check missions/my_mission.ron
```

```bash
cargo run --release -p dishonored_runtime -- --mission missions/my_mission.ron
```

```bash
cargo run --release -p dishonored_server -- --mission missions/my_mission.ron --port 27015
```

```bash
cargo run --release -p harness -- mission --mission missions/my_mission.ron
```

- `markers` lists the original level actors you can anchor to.
- `original` shows the original objectives and tasks of that map for inspiration.
- `check` validates every reference, including actor names against the installed level.
- `graph` prints the rule graph as Graphviz DOT.
- The dedicated server runs the mission as co-op; players join with `--connect`.
- `harness mission` plays the sample mission with a scripted agent. It's written for
  `boyle_masked_host.ron` (it looks for an `informant` marker, a `host` NPC and an `exfil`
  marker).

## File structure

```ron
MissionDef(
    title: "...", map: "L_Boyle_Ext_P", briefing: "...",
    objectives: [ ObjectiveDef(id: "main", description: "...", tasks: [ TaskDef(...), ... ]), ... ],
    flags: ["..."], flags_set: ["..."], counters: ["..."],
    markers: [ MarkerDef(id: "...", at: <Place>, radius: 250.0), ... ],
    npcs: [ NpcDef(id: "...", kind: Civilian|Guard, archetype: 0..3, at: <Place>, behavior: <Behavior>, spawn_at_start: true, name: "...", talk: "Talk", talk_conversation: "<conversation ref>", speaker: "<original spawner>"), ... ],
    player_start: "<marker>",
    crowd: CrowdDef(civilians: 18, guards: 3, center: <Place>, radius: 2500.0),
    settings: MissionSettings(respawn: false, respawn_delay: 5.0, time_limit: 900.0, high_chaos_kill_ratio: 0.2),
    rules: [ RuleDef(on: <Trigger>, when: [<Condition>...], then: [<Action>...], once: true, id: "..."), ... ],
)
```

- **Tasks:** `TaskDef(id, description, initial: Inactive|Active|Completed|Failed, hidden, optional, target: Marker("..")|Npc(".."))`. Tasks are addressed as `"objective.task"`. Activating a task reveals it. The HUD marker points at the first active, visible task that has a target (required tasks first).
- **Places:** `At(x, y, z)` in Dishonored units, `Actor("TargetPoint_5")` or `Actor("L_Boyle_Ext_Script:TargetPoint_5")` when the name exists in several sublevels, `Marker("id")`, and `Offset(<Place>, (dx, dy, dz))`.
- **Behaviour:** `Idle` (holds position), `Wander`, or `Patrol(["marker", ...])`.
- **Body archetypes:** 0 masked gentleman, 1 guard, 2 gentleman B, 3 masked lady.
- **NPC options:** `faction: "DisFaction_Defaults.Faction_Guard_Default"` gives the NPC an original faction, so its AI treats Corvo as an enemy (without one it is neutral until a rule alerts it); `passive: true` spawns it without AI (a training dummy).
- **Loadout:** `loadout: (all_powers: true, full_ammo: true, elixirs: (10, 10))` is given to players when they spawn and respawn.

### Triggers (`on:`)

| Trigger | Fires when |
|---|---|
| `Start` | the mission begins |
| `Timer(s)` | `s` seconds have passed |
| `Every(s)` | every `s` seconds |
| `Enter(m)`, `Leave(m)` | a player enters or leaves a marker's radius |
| `NpcKilled(n)` | a mission NPC dies |
| `AnyKill` | a player kills any NPC |
| `NpcAlerted(n)` | a mission NPC becomes alerted, flees or fights |
| `PlayerDetected` | a guard engages a player |
| `AlarmRaised` | the watch goes from calm to engaged |
| `PlayerKilled` | a player dies |
| `FlagSet(f)`, `FlagCleared(f)` | a story flag changes |
| `TaskState("o.t", State)` | a task reaches that state |
| `CounterAtLeast(c, n)` | a counter reaches `n` |
| `Remote(name)` | the `Fire(name)` action runs |
| `Use(m)`, `Talk(n)` | a player uses a usable marker / talks to an NPC (F) |
| `ConversationOutput(name)` | an original conversation reaches a `DisConv_KismetActivateRemoteEvent` named `name` (`"DisDialogOneShot_4/InvitationPlease_DONE"` to require the one-shot) |
| `ConversationEnded(c)` | an original conversation ends (`label`, `DisDialogOneShot_N/label` or `Package:DisDialogOneShot_N/label`) |

### Conditions (`when:`)
`Flag(f)`, `NotFlag(f)`, `Task("o.t", State)`, `CounterAtLeast(c, n)`, `CounterBelow(c, n)`,
`Detected`, `Inside(m)`, `NpcAlive(n)`, `Any([..])`, `All([..])`, `Not(..)`.

### Actions (`then:`)

| Action | Effect |
|---|---|
| `SetTask("o.t", State)`, `ShowTask`, `HideTask`, `ShowObjective`, `HideObjective` | objectives and tasks |
| `SetFlag`, `ClearFlag`, `Add(c, n)`, `Set(c, n)` | flags and counters |
| `Message(text)` | HUD message |
| `Notify(title, text)` | target-notification paper |
| `Sound(wwise_event)` | posts an original Wwise event |
| `Spawn(n)`, `Despawn(n)`, `MoveNpc(n, m)`, `RunNpc(n, m)`, `Patrol(n, [m..])`, `Alert(n)`, `Calm(n)` | NPC control |
| `After(s, [..])`, `Fire(name)`, `EnableRule(id)`, `DisableRule(id)` | flow |
| `Usable(m, prompt)`, `NotUsable(m)` | make a marker usable with F |
| `Conversation(ref, [npc..])` | start an original conversation (`DisSeqAct_DialogInputs`); see below |
| `StopConversation(ref)` | stop matching conversations (`""` = all); their `Interrupted` output runs |
| `Equip((all_powers: true, full_ammo: true, elixirs: (10, 10)))` | give every player powers and items (`reset`, `powers: [("Blink", 1)]`, `runes`, `coins`, `upgrades`) |
| `Complete(text)`, `Fail(text)` | end the mission |

### Original conversations

Dishonored's own dialog graphs (`DisDialogOneShot` -> `DisDialogTree` -> `DisConversation`,
see `research/dishonored/BOYLE_MISSION.md` section 4) play with their subtitles, player
choices and VO, all read from your install when the mission loads (nothing is copied into
the mission file). `mission check` validates every reference.

- **References.** `"L_Boyle_Int_Script:DisDialogOneShot_9/Hits_On_Player"` names a
  conversation by its `m_Label` (or `DisConversation_N`) inside a one-shot; the part after
  `/` can also be one of the one-shot's Kismet hook names (what `DisSeqAct_DialogInputs`
  fires, e.g. `".../Come In"`) or a hook type (`".../DDH_PLAYER_USED"`). A bare one-shot
  (`"L_BoyleStreet_Ext_Script:DisDialogOneShot_4"`) enters its use hook, like pressing F on the
  NPC in the original. A bare label works when it is unique among the loaded packages.
  `conv list <package>` prints the one-shots, hooks, labels and outputs of a package.
- **Speakers.** A one-shot's speakers are `m_MeSpeaker` then `m_OtherSpeakers`, each attached to
  an original spawner. Mission NPCs speak for them when `NpcDef.speaker` (or an `at:
  Actor(..)` place) names that spawner, or when listed in `Conversation(ref, [npc, ...])` in
  speaker order. Lines of unbound speakers play 2D.
- **Talking.** `NpcDef.talk_conversation: "<ref>"` plays the reference when a player talks to
  the NPC (it implies a "Talk" prompt); an NPC already in a conversation ignores it.
- **Approach.** An NPC standing in for a one-shot's me-speaker also gets the one-shot's
  `DDH_PLAYER_APPROACHES` hook: coming within its close/medium/far distances enters the hook at
  that output (Lord Brisby's quest offer starts this way).
- **Story flags.** `DisConv_CheckStoryFlag`/`DisConv_SetStoryFlag` read and write the mission's
  flags by their original `SF_*` flag name (e.g. `GaveInvitationtoGuard`), so `FlagSet(..)` and
  `Flag(..)` see what conversations do and vice versa. Declare the ones your rules use.
- **Choices.** A `DisConv_PlayerChoice` waits for the conversation's initiator to press 1-4
  (the `CHOICE_n` input, shared with the travel-door confirmations); the chooser stands still
  meanwhile.
- **Timing.** A line lasts as long as its original VO clip (read from the `Bk_<GUID>.INT` voice
  bank) plus its end delay; the few lines whose VO is missing from the install are timed from
  their text. Cutscene links (`DisConv_Soiree`) are recorded, not played.
- **Dialog packages.** The packages named in references are loaded, plus the mission maps'
  packages for unqualified references and anything listed in `dialog_packages: [..]`.

`Enter`/`Leave` are edge-triggered, like the original's touch events: a player already inside
a zone when a rule becomes relevant does not re-enter it. Pair such rules with a
`TaskState(..)` or `FlagSet(..)` trigger and an `Inside(marker)` condition (see the sample's
exit rules).

Rules fire once by default (`once: false` to repeat). They cascade within a simulation step,
the way Kismet chains within a frame: an action that sets a flag or task immediately triggers
rules listening for it.

### End of mission
The mission ends on `Complete` / `Fail`, when the time limit runs out, or when every player is
dead and `respawn` is off. The end screen shows the outcome and Dishonored-style statistics:
hostiles and civilians killed, times detected, alarms, time taken, chaos (Low/High by kill
ratio, an approximation), and the Ghost / Clean Hands feats.

## Sample
`boyle_masked_host.ron`: three masked ladies, one of them the host. An informant reveals
which, with optional "harm no other guest" and "remain undetected" goals, reinforcements on
alarm, and extraction.

`boyle_conversations.ron`: original conversations at the street gate. The doorman runs his
original invitation check (InvitationPlease, then Come In or CantLetYouIn), and Lydia Boyle
(player choices), Miss White (her use hook) and Lord Brisby (approach hook, then use hook)
speak their own dialog trees. `harness conversation` plays it end to end.
