# Contracts — social-stealth multiplayer (Phase II)

Code: `crates/dishonored_multiplayer/src/contracts.rs` (rules), `crates/dishonored_ai`
(crowd and guards), `crates/dishonored_runtime/src/hud.rs` (player information).
Tests: `crates/dishonored_multiplayer/tests/contracts.rs`; harness: `harness contracts`.

Faithful Dishonored mechanics are untouched (`dishonored_sim`). Everything below is
multiplayer design layered on top. All values live in `ContractsRules` / `ScoreValues`.

## Loop

```
spawn among civilians -> receive contract -> infer direction/distance -> observe the crowd
-> identify the target by appearance and behaviour -> stalk -> approach -> assassinate
-> world reacts -> escape -> next contract
```
At the same time each player is someone's target: blend, misdirect, evade, survive.

## Contracts graph

- Active players form a **single directed cycle** (`A→B→C→…→A`), shuffled
  deterministically on every rebuild. There are no reciprocal pairs with 3+ players; with
  exactly 2 players the pair is reciprocal by necessity.
- On death: the victim leaves the cycle, and its pursuer inherits its target (the pursuer
  gets a fresh reacquire period). On respawn, or when someone joins or leaves, the cycle is
  rebuilt.
- `validate_cycle` asserts the invariants. The harness runs it on every tick of a 10-minute,
  8-player match with the crowd.

## Information model (never exact coordinates)

`TargetInfo` (sent privately to its owner inside the snapshot) contains:
- `appearance`: the target body's archetype ("masked gentleman, dark coat, metal mask").
- `bearing_octant`: the world-space octant (8 directions). The HUD shows it relative to view.
- `proximity`: far (>3000 uu) / medium (>1500) / near (>600) / very near.
- `target_awareness`: how aware the target is of you.
- `pursuer_near`: a weak cue that your hunter is within stalk range, with no direction.
- `being_chased`, `exposure`, `escaping`.

Direction and distance are hidden during the reacquire delay (4 s) after spawning or
reassignment. During a chase the hunter keeps tracking. **Final identification stays human**:
players share the civilian body pool (masked party guests), there are no nameplates or
outlines, and NPCs move with the same movement code at walk speed.

## Exposure (information leaks)

`ExposureEvent`: Sprint, PowerUse (Blink / Dark Vision), WeaponDraw (pistol/crossbow in hand),
Attack, Parkour (mantle / high jumps), RestrictedMovement (reserved). Exposure decays at
0.35/s, accumulates per event, and is capped at 3. It drives:
- the target's speed of noticing a visible hunter
- chase start (an exposed hunter within 1200 uu)
- post-kill escape eligibility

## Awareness and chase

Each player perceives their pursuer (LOS, 120° FOV, 2500 uu). Awareness rises faster for
close and exposed hunters and decays out of sight. The levels are
Unaware → Suspicious (0.6) → Aware (1.5) → **Pursuit** (≥ 2.0, visible, < 1200 uu and either
exposed or < 400 uu). Being damaged by your pursuer makes you Aware at once.

During a chase the target regenerates mana faster (+6/s; it uses Blink, a faithful mechanic,
as the escape affordance) and the hunter keeps tracking. The chase ends when the target
breaks line of sight for 8 s, which awards the target +100 (chase escape).

## Kill quality (`KillScore` components)

| Component | Rule |
|---|---|
| base | pistol 100, crossbow 150, sword 250, assassination 400, drop assassination 450, Windblast 100 |
| stealth_bonus | target Unaware +150, Suspicious +60. Full for blade kills, half for crossbow, **none for pistol** (information cost) |
| proximity_bonus | +50 for blade kills |
| stalk_bonus | 5 per second spent within 1500 uu of the target, capped at 100, blade kills only |
| escape_bonus | +150 if, 15 s after the kill, the killer is alive, ≥ 900 uu from the kill spot and exposure < 1 |

Other outcomes: killing your own pursuer is a defensive kill (+100). Killing any other player
is an unauthorised kill (−100). Killing an NPC costs −150 and maxes your exposure.

Measured (test and harness): silent back assassination of an unaware target = 603; a loud
pistol kill of the same target = 200.

## World response

- Guards (original Boyle guard spawners, patrolling nearby original nav points) enter Combat on
  visible criminal acts (sword swing, firing), witnessed murders, and investigate
  violent sounds or Blink.
- Civilians (original AI points; social spots for groups) flee from violent sounds and
  witnessed murders. They recover after 8 s, so panic stays local.
- Gameplay sound events (`SimEvent::Sound` with category and radius) are the only hearing
  input. Raw audio is never used.

## Spawn rules

Spawn candidates are the map's PlayerStarts plus original AI points, scored against:
- distance from other humans (penalty under 1200 uu)
- being in their view (heavy penalty)
- nearby civilians (bonus)

A deterministic tiebreak follows.

## Core product test (human)

Can two people enter a populated space and hesitate because they're unsure who is human?
Run it with:

```
dishonored_server --mode contracts --port 27015
dishonored_runtime --connect HOST:27015 --name you
```

Players should walk (C), stay near groups, and avoid sprinting, powers and drawn weapons.
If the answer is "no", the brief says to improve NPC locomotion and player animation,
visual similarity, target information, blending and exposure before adding features.
`harness contracts` gives the automated half (rules, invariants, determinism, network).
Ambiguity itself needs human playtests (UNKNOWNS U-020).
