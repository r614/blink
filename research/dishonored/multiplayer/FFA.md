# Free-for-all (debug mode)

Code: `crates/dishonored_multiplayer/src/ffa.rs` (rules), `dishonored_content::ffa_spawns`
(spawn points), `crates/dishonored_bots` (test bots). Tests:
`crates/dishonored_multiplayer/tests/ffa.rs`, `crates/dishonored_bots/tests/hunt.rs`,
`crates/dishonored_content/tests/world_deaths.rs`; harness: `harness ffa` (headless) and
`harness net` (dedicated server and bot clients over UDP).

Dishonored is single-player: free-for-all is the port's mode for validating the simulation,
prediction and replication. The mechanics underneath are the original's (`dishonored_sim`); the
mode's rules are design, kept as close to the engine's own as the data allows.

## Scoring: kills score, world deaths and suicides cost nothing

A kill gives the killer one point. A death without a killer, or with the victim as killer
(`KillZ`, a fall, drowning, a pain volume, the pawn's own grenade), counts as a death and changes
no score.

Evidence (2026-10-07, Engine.upk and DishonoredGame.upk decompiled with `uc_decompile.ps1`,
`package_dump --names`, `class_layout`, strings of `Dishonored.exe`):

- Stock UE3 scores in `GameInfo.ScoreKill`: a suicide or a death with no killer takes a point
  from the victim, a kill gives the killer one, then `CheckScore`. Dishonored's shipped
  `Engine.GameInfo` has neither function (nor `ModifyScoreKill`); `GoalScore` and `MaxLives`
  remain as unused config fields.
- `GameInfo.Killed` only calls the empty `BroadcastDeathMessage` (for a player), `DiscardInventory`
  and `NotifyKilled`. `DishonoredGameInfo` adds no scoring (`PreventDeath` and its native half
  only).
- There is no `PlayerReplicationInfo` class in the cooked packages (`class_layout Engine
  PlayerReplicationInfo`: not found), so there is no `Score`, `Deaths` or `Kills` field to keep.
  `GameReplicationInfo` keeps `GoalScore` and `Winner`, set by nothing.
- `Mutator.ScoreKill` survives as a pass-through to the next mutator; no script calls it, and
  the executable holds no `ScoreKill` name.
- `Pawn.SetKillInstigator` (Engine script) would credit a death by a `bCausedByWorld` damage type
  (`KillZDamageType`, `DmgType_Fell`) to `LastHitBy`, and `Pawn.Landed` clears `LastHitBy`. The
  native damage path does not call it by name (no `SetKillInstigator` string in
  `Dishonored.exe`, while `FellOutOfWorld`, `PreventDeath`, `NotifyKilled` and
  `OutsideWorldBounds` are there), so the port does not credit pushed falls (U-642).

The port used to take a point for a world death (stock UE3's rule, which Dishonored removed); a
5-minute playtest showed a player at -3.

## Spawn points

`ffa_spawns`: the map's PlayerStarts that stand on the original navmesh, in the largest group a
pawn can walk between (navmesh paths). Boyle Ext has five PlayerStarts:

| Tag | Position | Navmesh | FFA |
|---|---|---|---|
| `PlayerStart_SamuelDropOff` (mission start) | (-10092, 11378, -2222) | none: the canal under the visual water, outside the sea's water volume | no |
| `PlayerStart_Garden` (on `BlockingVolume_5`) | (-2663, -1879, 98) | estate grounds | yes |
| `PlayerStart_FrontDoors` | (1355, 1445, 93) | estate grounds | yes |
| `PlayerStart_RoofPatio` | (-1775, -1940, 1113) | an island (4 polys) | no |
| `PlayerStart_RiverGrate` (on `BlockingVolume_2`) | (-921, 1160, -1835) | an island (6 polys) | no |

The navmesh has eight islands (largest: the street outside the estate, 554 polys, no start; the
estate grounds, 353 polys). The original links them only by drops, climbs and Blink, which the
AI navmesh does not encode.

## Test bots

`dishonored_bots::Bot::think` (they only read player states a client receives and only send
`PlayerInput`):

- Every client receives every player's replicated position (no relevancy culling in
  `dishonored_net`), as a human reads it off the scoreboard and minimap. A bot hunts the nearest
  opponent it can reach (hysteresis: it switches when another is 30 % nearer) along the navmesh
  (`nav::Route`: `NavWorld::find_path`, corners pushed 30 uu out of the turn because the player's
  box is 34 uu wide against the navmesh agent's 31, a corner passed once a box sweep to the next
  waypoint is clear). A target with no route is skipped for 5 s; with nobody reachable the bot
  roams to random navmesh polys.
- It fights an opponent in the clear (vision and projectile line): duelists close in (straight
  when level and within 500 uu, else along the path; Blink between 600 and 1050 uu) and use the
  sword; gunners shoot from 400 to 1200 uu and walk the path when further.
- Every wanted move passes `hazard::safe_ahead`: floor within 260 uu below a point 70 uu ahead,
  not deep water, not a pain volume (a wall in the way is harmless). Refused strafes flip side;
  other refused moves stop; a goal that keeps being refused is dropped. A swimming bot heads for
  the nearest navmesh poly at the water surface (mantle height) and holds jump while its head is
  under, never swimming out of the volume over nothing.

## Results (Boyle Ext, 3 bots, 5 minutes)

| | player kills | world deaths | drowning |
|---|---|---|---|
| Before (playtest, 3 rendered `--autopilot` clients) | 0 | 5 | 1 |
| Before (`harness ffa`, headless, seed 1) | 0 | 10 | 1 |
| After (`harness ffa`, seeds 1 / 2 / 3) | 62 / 62 / 63 | 0 | 0 |
| After (release server, 3 rendered `--autopilot` clients at 960x540) | 57 | 0 | 0 |

Headless "before" deaths: five through the missing front door onto the strip past the doorway,
then the sea, then out of its east edge to `KillZ`; one off a roof edge at z 1100 into the sea,
then out of its south-east edge; four off the end of the canal floor next to the Samuel drop-off
start; one drowned walking from the canal floor into the sea volume under water.
