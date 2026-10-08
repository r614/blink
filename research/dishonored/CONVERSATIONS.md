# Conversations: the original dialog runtime (decompiled)

How `Dishonored.exe` runs `DisDialogTree` / `DisConversation` / `DisConv_*` graphs, read from
the Ghidra decompile (2026-10-05). Addresses are virtual addresses in the GOG
`Binaries/Win32/Dishonored.exe`. The data model is in BOYLE_MISSION.md section 4; the port is
`crates/dishonored_conversation` (runner) and `hooks.rs` (hook table, bark gating).
Decompiled output stays in the scratch folder; everything here is described in our own words.

Method: `research/ghidra/ClassDecomp.java` on every `DisConv_*` class and the runtime classes,
`DecompRange.java` over 0x00c86000-0x00cc0000 (1,594 functions), `Vtables.java` for the pawn's
`DisConvSpeakerInterface` implementation, `Consts.java` for constants. The class layouts and
defaults come from the UnrealScript decompile of `DishonoredGame.upk`.

## 1. The walker and node outputs (U-034 CONFIRMED)

- Every node class overrides vtable slot 80 (+0x140), `Evaluate(&latent, state)`. It returns the
  **index of the output link to follow**, or -1 to wait (it then sets `*latent`).
- The runtime walker is `FUN_00ca3c10(node, state, &status, ...)`. It loops: flags from slot 79
  (+0x13c), `Evaluate`, then:
  - the node's `m_Label` (when set, and the node is not a `DisConversation`) is recorded for every
    bound speaker (`FUN_00ca3a80`: per speaker memory, per conversation, a count per label);
  - -1 with `*latent` set: the node becomes the state's current node (+0x10) and the walk stops;
  - an index outside `m_OutputLinks`: the state ends with reason 0 (error);
  - a null link: the state ends with reason 1 (finished);
  - otherwise the walk continues at `m_OutputLinks[index].m_pLink` (links: array at node+0x38,
    count +0x3c, stride 0x1c, link at +0xc).
- Latent nodes are updated by slot 74 (+0x128) from the component tick `FUN_00ca66d0`, which
  follows the returned index the same way.
- A second walker, `FUN_00c9f3e0`, only scans (used for tests such as "does this lead to a
  line"): random branches are not evaluated there, the first linked output is taken.

### Conditions (all return 0/1 unless noted)

| Node | Evaluate | Returns |
|---|---|---|
| `CheckStoryFlag` | 0x00ca8ba0 | 1 when the flag is set (flag set at +0x88, GUID at +0x8c) |
| `SeenDialogLabel` | 0x00ca7570 | 1 when speaker `m_iSpeaker` remembers `m_LabelToCheck` in `m_pConversation` (+0x90); 0 when `m_pConversation` is not set or the speaker is not bound |
| `HadConversation` | 0x00ca7650 | 1 when speaker `m_iSpeaker` took part in `m_pConversation` |
| `ConversationFired` | 0x00ca8eb0 | 1 when the global memory has `m_pConversation` |
| `IsSpeakerAvailable` | 0x00ca76f0 | 1 when the speaker slot is bound and its "available" bit is set |
| `CheckSpeakerSuspicionLevel` | 0x00cab4b0 | the speaker pawn's suspicion level (an AI value), 0 without pawn |
| `IsPossessed` | 0x00cab940 | 0 not possessed, 1 possessed, 2 possessed and the possession is of a given class (three outputs) |
| `SpeakerCombatStatus` | 0x00caba70 | 0 when the pawn is dead/incapacitated or the AI state is 5, 1 for state 3, 2 for state 4, 3 otherwise (four outputs) |
| `HasInventoryItem` | 0x00cab690 | 1 when the speaker is the player and has the item (object or class) |
| `HasInventoryItemEquipped` | 0x00cab730 | 1 when the player has the class equipped |
| `HasAbstractItem` | 0x00cab870 | 1 when the player's count of the abstract item >= `m_Quantity` |

So conditions are "output 0 = false, output 1 = true", as the data suggested. The Kismet
`DisSeqCond_CheckStoryFlag` has the opposite order (output 0 = set); the two systems differ.

### Group branches (U-038)

`SpeakerInStoryGroup` 0x00caddc0, `SpeakerHasTweaks` 0x00cadba0, `FactionBranch` 0x00cad560,
`SpawnerBranch` 0x00cad9d0, `SpeakerSupportsDialogTree` 0x00c9c430: the speaker pawn is tested
against each `m_Branches` entry in order; the **first match's index** is returned, else
`count` (the extra "Everyone Else" output). Story group and tweaks entries carry an exact-match
flag (exact: the pawn's own object; otherwise it may derive from it). The spawner branch matches
when the pawn is in the spawner's spawned list. No pawn: "Everyone Else".

### Branches

- `RandomBranch` 0x00cad760: `m_Branches` are percentage weights (class default 50/50). Roll
  `rand() * 100/32767` (constant 0x011a0bd8); take the first output whose cumulative weight
  exceeds the roll, else the last. With `m_bTryKeepUnique` (default true): if the pick equals the
  stored last pick and there are two or more outputs, take the other output whose weight is
  closest to the picked one's (first on ties). The stored last pick lives in
  `DisConvGlobalMan` keyed by tree, conversation and node index, and **starts at 0**, so the
  first evaluation never takes output 0 when another exists.
- `SequentialBranch` 0x00ca98a0: counter c (state per conversation instance, or global with
  `m_bApplyGlobally`, bit 2 at +0x94); next = c + 1; if next > count: next = count, or 1 with
  `m_bLooping` (bit 1); returns next - 1.
- `RandomSequentialBranch` 0x00cb6fa0 (extends the sequential branch, so it has `m_bLooping`;
  `m_bApplyGlobally` default true): a shuffled index list (built once: random sort keys, sorted)
  walked by the same counter; past the end it returns `count` ("Depleted", label from slot 87),
  or restarts with looping. The list is reshuffled only when a pending counter equal to the
  count commits (slot 75) and looping is set; inside a running conversation the counter is
  written at once and the old order repeats. "Apply globally" only counts in trees no one-shot
  owns (`FUN_00c8c820`).
- `TimeLimit` 0x00ca1da0: last pass time (0 = never; the code treats a stored time under 0.0001
  as never). If `now - last <= m_fTimeLimitInSeconds` it returns 1, else it records `now` and
  returns 0. Global only in trees no one-shot owns. Time is the world time.

### Pending state (U-035 detail)

Branch writes (random last pick, sequential and random-sequential counters, time limits) check
whether the state already runs a conversation (`FUN_00c8b2b0`: state+0xc). If not (walking from a
hook through tree-level nodes), the new value goes to a "pending" field and the node is queued;
`FUN_00c97100` commits the queue (slot 75 of each node) when a conversation actually starts.
A hook walk that starts nothing leaves the committed state unchanged.

## 2. Conversations: start, priority, concurrency, end (U-035 CONFIRMED)

- `DisConv_ConversationRef` 0x00ca4250: the referenced conversation is output 0 (hidden link).
  Returns 1 ("couldn't start") when `m_bFireConvOnlyOnce` (bit 0x800 at conversation+0xbc) and the
  global memory has it, when `m_bNotWhenPlayerIsInCombat` (0x1000) and the player is in combat,
  when it has no Start link, or when the start fails; returns 0 after a successful start (the walk
  then enters the conversation node, whose Start link follows). It also works inside a running
  conversation: the state switches conversations.
- Start `FUN_00ca3700(conversation data, state)`:
  1. `FUN_00c96410`: with `m_bPlayerMustBeNear` (0x400) the player must be strictly within
     `m_fPlayerNearRadius` (conversation+0xc4) of one of the speaker positions.
  2. `FUN_00c9e2b0` binds every speaker slot (`FUN_00c9c6c0`): ME = the owner, INITIATOR = the
     initiator, PLAYER = the player, LINKED_NODE = resolved through the `DisConv_Speaker` node.
     Binding asks the speaker `Available(priority, flags)`; a slot that fails makes the start
     fail unless its speaker node is `m_bOptional` (bit 8 at +0x94; the other bits are
     `m_bNotWhileInCombat` 1, `m_bNotWhileSuspicious` 2, `m_bNotWhileFearful` 4).
  3. The pawn's `Available` (`FUN_00c9d1f0`, via the `DisConversationComponent`): returns 5
     (`DDA_IN_NON_INTERRUPTABLE_CONVO`) when the speaker's current state came from a hook with
     flag 0x4; else 0 (available) when the new priority is **strictly higher** than the current
     one, or when the request is not a fresh start (flag 4 is only passed when the state has no
     conversation yet); else 4 (`DDA_IN_HIGHER_PRIORITY_CONVO`). `FUN_00c9aec0` then makes any
     result but 5 available when the requesting hook has flag 0x2 ("interrupts").
  4. Each bound speaker is engaged (`FUN_00ca59f0`): if it was in another state, that one ends
     with reason 6 (new conversation), or 3 (death) for the dying / stealth-kill hooks 57-59.
     The engage call carries `m_bIsBlocking` (NPCs; bit 0) or `m_bIsBlocking_Player` (player;
     bit 1), plus `m_bStopBlockingForAINotice`, `m_bCanStartDuringEnemyAwareness`,
     `m_bAllowPlayerToCarryBody` for the AI. Each speaker's memory counts the conversation;
     the global memory too (used by fire-once and `ConversationFired`).
  5. Pending branch state commits.
- Priority of a state: the conversation's `m_Priority` (enum: NOT_IN_A_CONV 0, LOW 1, MEDIUM 2,
  HIGH 3 (default), NOT_INTERRUPTABLE 4); before a conversation starts, the hook node's
  `m_Priority` (default LOW).
- Each `DisConversationComponent` (one per NPC) holds two state slots and alternates between
  them, so a speaker is in at most one conversation; a new one that passes the availability
  rules takes the speaker and ends the old one.
- End `FUN_00ca4410(state, reason)` (`DisConvoEndReasonEnum`: error 0, finished 1, behaviour
  change 2, death 3, death witness 4, player walked away 5, new conversation 6, ...):
  - reason != finished: the latent node is aborted (slot 76), speakers are released with the
    reason (3 becomes 4 for the other speakers), then the conversation's **output 1
    ("Interrupted") is walked with the normal walker** while the state is still bound: instant
    nodes run and the first line starts playing, then the state is cleared, so nothing waits for
    that line;
  - finished: speakers released, no Interrupted branch.
- Player walked away `FUN_00ca5610`, every tick before the node update: only for conversations
  without `m_bIsBlocking_Player`, once per conversation (`m_bDidPlayerWalkAwayCheck`); when the
  "player near" test above fails, output 2 ("Player Walked Away") is walked if linked, else the
  conversation ends with reason 5 (which runs Interrupted).

## 3. Lines, choices, soirees

- `DisConv_Blurb` Evaluate 0x00c9fb90 -> `FUN_00c9deb0`: no bound speaker for `m_iSpeaker`:
  returns 0 at once (skips the line). Else asks the speaker for the Wwise event of
  `m_iBlurbGUID` (speaker slot +0x44) and plays it (+0x3c `FUN_00c9fbf0`); waits.
- Update `FUN_00c9e0f0`: stage 0 waits for the speaker to be ready (+0x54); stage 1 waits until
  the speaker reports no voice playing (`FUN_00c9a160`: no Wwise playing ID on its components),
  then counts `m_fBlurbEndDelay` down and returns 0.
- No VO event: nothing is posted and no subtitle is created (`FUN_00c9fbf0` adds a subtitle only
  for a playing ID with non-empty text), so the line ends on the next tick plus its end delay.
  There is no reading-time fallback (U-036 CONFIRMED).
- `DisConv_PlayerChoice` Evaluate 0x00c9c570: no static choice -> output 0. Otherwise opens the
  player's choice UI and waits; if the UI cannot open, output 0. Update 0x00c9c610: no timeout;
  input counts once the choice UI has faded in (a value > 0.9999). The chosen index i < static
  count -> output i; optional choices (`m_Choices_Optional`) are shown after the static ones when
  their story flag equals `!m_bStoryFlagMustBeFalse`, and choosing the j-th optional entry fires
  output `static count + j` (`FUN_00c8b810`).
- `DisConv_Soiree` Evaluate 0x00ca7a60: returns 0 at once in a conversation without
  `m_bIsBlocking`. With `m_UsePreviewBlurbsOnly` it plays the chain of preview blurbs linked from
  output 1 (each node of it updated in turn), then returns 0. Otherwise it finds the matinee by
  GUID (`FUN_00cb5de0`), plays it and waits until it finishes, then returns 0; not found -> 0.
  So output 0 always continues after the soiree; output 1 is the preview chain (U-037).
- Text (U-040 CONFIRMED): the blurb text getter `FUN_00c92ce0` returns `m_Text` when the
  language is INT, else the single `m_PerLanguageData` entry the language's cooked packages
  carry (anything else: "<Lang not found!>"). The GOG install is cooked for INT only.

## 4. Hooks and barks (U-038)

- Firing a hook on an NPC (`FUN_00ca5670`, component): blocked while the owner is dead except
  for hooks 57-59; tries the one-shot trees bound to the NPC, the additional generic trees,
  then its generic tree (and each tree's `m_pFallbackTree` chain, `FUN_00ca5150`). The hook
  node's slot 86 picks the output; the walk runs as above. On success the NPC's stored time for
  that hook becomes `now + rand * (max - min)` and the global last-fire time of the hook is
  stored (`DisConvGlobalMan` +0x558). **Correction (systems/conversation.md R1):** the request
  gate (`FUN_00ca6bd0`) lets the hook fire again only when `now - stored > min`, so the
  effective refire delay is `min + rand * (max - min)`; the minimum is added by the gate. The
  request queue, weights and caps are decoded in systems/conversation.md R3.
- The per-hook table (126 rows of 0x50 bytes at `DAT_01471d0c`, built by `FUN_00c8d820`):
  class, fire group, refire min/max/global, bust time, flags. Most AI hooks: refire 1 s / 1 s,
  flags 0x80; use/approach/Kismet/audio-log/heart hooks: 0 s; "can't reach" and "go home" 8 s;
  combat cries 0x88 (battle sound); dying / immolated 0x8e; ouch 0x8a; stealth kills 0x86;
  search 0xa0. The values are in `crates/dishonored_conversation/src/hooks.rs`.
- Flags: 0x2 interrupts, 0x4 not interruptible, 0x8 battle sound, 0x20 search sound, 0x1000
  notice sound (the blurb start emits an AI noise with the tweak loudness, `FUN_00c9bb50`),
  0x40 deprecated, 0x2000 radial (bypasses attention), 0x4000 player used (the NPC turns to the
  player for the 1 s bust time).
- `DisTweaks_ConvSettings` class defaults (the `Twk_ConvSettings` object is not shipped):
  overrides for search 8-12 s, fleeing 2-5 s, combat threat 5-13 s, patrol misc 5-12 s and
  20-60 s; fire groups with max playing per group / initiator and minimum time between firings
  (e.g. Casual 3/1, 3 s; Agitated 1/1, 2.5 s; Combat 5/2; GameplayImportant unlimited, ignores
  the global cap); `m_MaxDialogHooksAtOnce` 5; `m_fMaxDialogDistance` 5000.
- `DisConv_Hook_Distance` (approach hooks) slot 86 `FUN_00cb4ff0`: over the speaker positions,
  the nearest distance d to the initiator; only when d <= far (and, for the "aware" variant, the
  NPC's attention target is the initiator): close output if linked, d <= close and the last
  distance > close; else medium likewise; else far when far < last; the last distance becomes d.
  Edges are inclusive; moving beyond far leaves the last distance unchanged.

## 5. Still open

- The queue in `DisConvGlobalMan` (pending/firing hook groups, loud/quiet split by
  `m_fDialogDistanceConsideredLoud`, `m_fHookWeightRandRange`) is decoded in
  systems/conversation.md R3 (2026-10-08); the port gates by group caps and spacing only.
- The AI side of availability (`DDA_REJECTED_BY_AI_BEHAVIOR`, the `m_bNotWhile*` speaker flags)
  and the AI state numbers behind `SpeakerCombatStatus` and the suspicion level.
- Matinee playback for soirees (the port plays the preview chain as a stand-in).
