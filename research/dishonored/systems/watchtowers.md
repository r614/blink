# Watch towers (`DisWatchTower`)

Decoded from `Dishonored.exe` with the static index. Port: `crates/dishonored_ai/src/watchtower.rs`
(state machine), `crates/dishonored_sim/src/launchers.rs` (`fire_tower`, shots share the launcher
table at `TOWER_SHOT_BASE + index`), `crates/dishonored_assets/src/interactables.rs` (`watchtower`).

## Placement and tweak

Three ship: `L_Streets2_Script:DisWatchTower_1`, `DisWatchTower_2` and
`L_TowerRtrn_Yard_Script:DisWatchTower_0`. All use the same `DisTweaks_WatchTower`
(`m_pWatchTowerTweaks`):

| field | value |
|---|---|
| faction | `Faction_Guard_Default`; hacked: guards Enemy, Corvo Neutral (`m_HackedDispositions`) |
| vision (`m_pVisionTweak`) | inner 20 deg, 6000 uu; no outer cone |
| hacked vision | inner 360 deg / 4000 uu, outer 360 deg / 1000 uu |
| tower / light rotation | 40 deg/s each; light pitch 70 deg below, 60 deg above the horizon |
| attack | initial delay 2 s, volleys of 3 shots 0.2 s apart, 4 s between volleys, spread 200 uu, timeout 7 s, warning 1.5 s, attack FOV x1.5 |
| frantic sweep | speed 0.5, range 1000 uu |
| projectile | `WatchTower_Twk.WatchTowerExplosiveArrow_twk`, 2250 uu/s, gravity x0.25, `m_ProjectileDamage` unset (0): the explosion (30 radial, 500 uu; players 400 uu) does the damage |
| noise | `EAINoiseLoudness4`; allies' attention on the tracked target `DAL_Busted` |

The light is the vision bone's socket (`Light_Socket`) and the muzzle `m_ProjectileSocket`
(`Arrow_Socket`) of the tower mesh at its bind pose, about 1090 uu above the base.

## Update (0x00A3C640, tick vslot 103 at 0x00A3F890)

States: 1 Explore, 2 Attack, 3 Deactivated.

- **Explore**: the light follows `m_pExploreTracked`, an `InterpActor` driven by the level's
  matinee, turning at the rotation speeds (pitch clamped to the limits).
- **Detection**: `DisCpntType_VisionWatchtower` with the vision tweak from the light. A live
  enemy (by faction, or by the hacked dispositions) inside a cone with a clear line switches to
  Attack. A corpse in view fires the ally/enemy-corpse-seen output once per body.
- **Attack**: the cones scale by the attack FOV multiplier. After the initial delay it fires
  volleys at the target, each shot spread by `m_fFireSpread`. When the target is lost, the light
  sweeps frantically (a sine of `m_fFranticSpeed` over `m_fFranticRange`) and
  `m_fStateAttackTimeout` returns the tower to Explore.
- **Deactivated**: nothing runs. A whale-oil receptacle removal or Kismet TurnOff gets here.

## Kismet

- `DisSeqAct_WatchTower`: the action follows the input index:
  - 0 TurnOff
  - 1 TurnOn
  - 2 Toggle
  - 3 SetExploreTracked
  - 4 SetAlertTracked
  - 5 SwitchPolarity (hack)
- `DisSeqAct_WatchTowerShootAtTarget`: `m_pTargetActor` becomes the scripted target.
- `DisSeqEvent_WatchTower` outputs:
  - 0 activated
  - 1 deactivated
  - 2 neutral corpse seen
  - 3 ally corpse seen
  - 4 enemy corpse seen
  - 5 hacked

  Streets2 fires output 2 at the start: its placed plague corpses lie in a beam.

## Not ported

- The light's material parameters (`DisWTLightMaterial`).
- The tower's sounds.
- Mesh rotation in the renderer (presentation).
