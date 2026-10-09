# NPC teleport spell (`DisItemContext_NPCTeleportSpell`)

The whalers' and Daud's teleport, decoded from `Dishonored.exe`. The port is
`crates/dishonored_sim/src/teleport_spell.rs`; its module doc has the state machine with addresses.
The tweaks are read by `crates/dishonored_assets/src/ai_level.rs` (`teleport_spell`), and the
Kismet action is in `crates/dishonored_multiplayer/src/kismet_host.rs`
(`DisSeqAct_NPCDoTeleportSpell`).

## Data

The tweak is reached from the pawn as follows:

1. The pawn tweak's `m_ContentInventoryLoadout` lists its items. For example,
   `Pwn_Assassin_MTall_1` lists `Twk_Inv_SwordAssassin` and `Twk_Inv_AssassinHand`.
2. The hand item's `m_pNPCSpecificTweaks` holds `m_ContextSlots_Direct` entries: "Teleport Spell"
   and "Attract Spell".
3. The "Teleport Spell" entry points at the `DisTweaks_NPCTeleportSpell`.

Values of `Twk_Inv_AssassinHand` (Daud's hand falls back to it and keeps the same values):

| field | value |
|---|---|
| `m_fDisappearanceFadeOutTime` / `m_fReappearanceFadeInTime` | 0.45 / 0.3 s |
| `m_TeleportDelay` | class default (0, 0) |
| `m_DistanceFromTarget` | 300..800 uu |
| `m_ContextCooldown` / when unreachable | 15..20 / 5..10 s |
| `m_fIdealContextRange` / `m_fMaxContextRange` | 500 / 3000 uu |
| `m_fAllowedAngle` / `m_fAllowedAngleFromTargetDir` | 180 / 45 deg |

## Kismet (`DisSeqAct_NPCDoTeleportSpell`)

**Inputs:** Start, Abort.

**Outputs:**

- **Out (Instant, Non-Aborted):** fires at once.
- **Success (Last):** fires when the last pawn ends having teleported.
- **Interrupted (Any):** fires for each pawn that did not teleport.

**Variables:**

- **Target:** the pawns.
- **Destination** (`m_pDestinationActor`): a TargetPoint or PathTargetPoint. The pawn takes its
  location and rotation.

**Properties:**

- `m_fReappearanceDelay`: the custom delay; a negative value rolls the tweak's delay.
- `m_bSetNewHomeActor`: makes the destination the pawn's new home.
- `m_bAlwaysOutOfBendTime`: not ported.

**Where it's used:**

- Flooded: the assassins, Daud's combat points and bend-time points, and "Send Them All Away".
- The Refinery intro.
- The Tower soirée, where it removes the assassins' bodies.

## Open

- **AI use.** The combat and escape intentions are not ported. These cover target distance, angle,
  cooldowns and the escape distance; the AI's item-context selection does not exist yet.
- **Attract Spell.** The whalers' pull is not ported.
- **Spot search.** The fit test of `FindSpot` (0x00BF10A0) is not ported, so the destination is
  used as placed.
- **Presentation.** The fade's material parameter is not rendered; the pawn is hidden only while
  fully faded out. The effects and sounds are not ported either.
