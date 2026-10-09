# Walls of light (`DisWallOfLight`)

Decoded from `Dishonored.exe` with the static index. The port is in
`crates/dishonored_sim/src/walls.rs`; its module doc lists the rules with their addresses. The
data comes from `crates/dishonored_assets/src/world_actors.rs` (`wall_of_light`), and the retail
test is `crates/dishonored_assets/tests/walls_of_light_retail.rs`.

## Placement data (all maps)

24 walls ship, on 12 maps:

- Boyle Int
- Bridge 1c and 2
- Flooded FGate
- the Lighthouse (both chaos variants)
- Streets1 and Streets2
- Tower Return Int and Yard

Every wall shares the same tweak values:

- `m_Damage` 100
- `m_bJib` false
- `m_ActivationTime` and `m_ActivationTimeForPawnEffect` both 0.1 s

Their collision is a 20 uu thick box from the tweak's `m_pStaticMesh`. The tweaks interface's apply
(0x00A29110) puts that mesh on the component (`SetStaticMesh`); a placed component on the Flooded
gate has no mesh of its own. The box is scaled by `DrawScale3D`, typically 330 to 660 uu wide and
390 to 1230 uu high.

Friendly factions come from the tweak, or from the placed `m_FriendlyFactionsOverride`:

| Level | Friendly factions |
|---|---|
| Default | `Faction_Guard_Default` |
| Boyle | `Neutral_Guard` and `Guard_Default` |
| Bridge | Sokolov, his servants and guards |
| Lighthouse | Martin's and Pendleton's armies and Martin |
| Tower | the tower guards and torturers, plus the Regent's safe room |

Each wall is powered by a `DisWhaleOilReceptacle`, resolved as follows:

1. the wall's own `m_pAttachedReceptacle`;
2. otherwise the receptacle listing the wall in `m_aAttachedActors`.

Bridge 2's wall 0 and Tower Int's wall 1 have no battery placed in their receptacle, so they start
unpowered.

## Rules

The `walls.rs` module doc has the decoded detail. In summary:

- **Power.** A wall is placed hidden with no collision.
  - The receptacle's plug and unplug call the wall's consumer callbacks: 0x00A40230 and 0x00A3EE40.
  - Powering on shows the wall and fires output 0. Powering off hides it, removes collision and
    fires output 1.
  - The tick restores `TouchAll` collision while the battery has a charge.
  - While Bend Time stops the world, the wall has no collision, so it can be walked through.
- **Touch** (live pawns).
  - A pawn of a friendly faction passes. On a reversed (rewired) wall the rule inverts: friendlies
    and the player's enemies die, and a player-controlled pawn never does.
  - While the 0.1 s kill effect runs, any pawn that starts touching dies.
  - Each kill costs the battery's pawn charge.
  - NPCs burn to ash. The player takes `TakeDamage(100)`, an energy damage type with no damage
    reduction.
- **Overlap tick.**
  - Rats are shocked.
  - Bodies are disintegrated to ash. That includes a carried or thrown body pushed into the wall.
  - Both cost the battery's ambient-animal charge.
- **Kismet.**
  - `DisSeqAct_WallofLightControl` inputs:
    - 0 TurnOff
    - 1 TurnOn
    - 2 Toggle: it passes the current state to the setter, so it is a no-op as decompiled.
    - 3 SwitchPolarity
  - `DisSeqEvent_WallOfLight` outputs:
    - 0 activated
    - 1 deactivated
    - 2 polarity default
    - 3 polarity reversed
    - 4 kill effect on
    - 5 kill effect off

## Open

- **Presentation.** The view needs to hide the mesh while the wall is unpowered. The beam and spark
  emitters (`m_Beams`, `m_SparkEffects`) and the sounds also need toggling.
- **Projectiles and melee.** A powered wall blocks projectile and melee traces (`ShouldTrace`
  0x00A21B30), and thrown objects are disintegrated (slot 232, 0x00A3EA80).
- **Windblast rebound** (0x00A3BEF0). A blast whose force reaches `m_fWindblastMinimumForce` costs a
  charge.
- **Special-class exemption** in the kill function 0x00A3B940. It is an exact class match against
  `DAT_0145F1A8`, which is unresolved.
