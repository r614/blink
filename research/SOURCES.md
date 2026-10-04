# Sources

Primary evidence comes from the local GOG installation (`dishonored/GAME_FILES.md`).
External references are knowledge sources only. No code from GPL or AGPL projects is
copied into this repository.

| Source | Used for | License | How used |
|---|---|---|---|
| Local install: cooked `.upk`, `.tfc`, `Config/*.ini` | All format and gameplay evidence | Retail, owned copy | Read-only, never committed |
| UEViewer / UModel (gildor2/UEViewer) | Cross-check of the UE3 v801 serialization paths and Dishonored branches | MIT (bundled LZO is GPL-2.0, not used) | Read for knowledge; layouts independently re-verified against hex |
| UE Explorer / UELib (EliotVU/Unreal-Library) | Dishonored `UClass` quirks for script packages | MIT | Knowledge |
| lzokay-native (crates.io) | LZO1X decompression | MIT | Dependency |
| Bevy | Rendering and runtime | MIT/Apache-2.0 | Dependency |

See `dishonored/EXISTING_WORK.md` for the full prior-art survey.
