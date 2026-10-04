# Canonical Target: Dishonored: Definitive Edition (GOG, Windows)

All binary-level findings in this repository apply to the build fingerprinted here
unless explicitly stated otherwise. Record the `Dishonored.exe` SHA-256 next to any
address or offset.

## Build identity

| Field | Value |
|---|---|
| Product | Dishonored: Definitive Edition (GOG offline/Galaxy release) |
| GOG gameId / productId | 1701063787 |
| GOG buildId | 56034905455963308 |
| GOG registry `ver` | `334700 2022` |
| Install language | English (en-US) |
| Install path | `C:\Program Files\GOG Galaxy\Games\Dishonored` |
| Primary executable | `Binaries\Win32\Dishonored.exe` (32-bit; launched with working dir `Binaries\Win32`) |
| `Dishonored.exe` FileVersion / ProductVersion | `1, 0, 0, 0` / `1, 0, 0, 0` (not informative) |
| `Dishonored.exe` CompanyName | ZeniMax Media Inc. |
| `Dishonored.exe` size | 18,041,856 bytes |
| **`Dishonored.exe` SHA-256** | **`70da96138fe2171cca18888a0adbb731447cd15758e55535672625f09f6bc6fb`** |
| Install size | ~17 GB, 14,362 files |
| Save path | `%USERPROFILE%\Documents\My Games\Dishonored` |
| Registry (game) | `HKCU\Software\Arkane\Dishonored` |
| Registry (GOG) | `HKLM\SOFTWARE\WOW6432Node\GOG.com\Games\1701063787` |

Engine fingerprint (from every cooked package header, see `PACKAGE_FORMAT.md`):

| Field | Value |
|---|---|
| UE3 package file version | 801 |
| Licensee version | 30 |
| Engine version | 9098 |
| Cooker version | 133 |
| Package compression | LZO (flag `0x2`), chunked |
| Cooked platform dir | `CookedPCConsole` |

## Directory structure

```
Dishonored/
├── Binaries/
│   ├── Redist/
│   └── Win32/                 Dishonored.exe, PhysX 2.x + APEX DLLs, Bink, Galaxy.dll,
│                              SteamAPIUpdater.dll, wxWidgets (editor remnants), vorbis/ogg
├── DishonoredGame/
│   ├── Config/                Default*.ini (gameplay tuning; see below)
│   ├── CookedPCConsole/       4,811 files: 470 .upk, 311 Wwise .pck, 3 .tfc, localized .INT/.DEU/...
│   ├── DLC/PCConsole/
│   │   ├── DLC05/             177 files  (challenge maps: L_DLC05_Arena_P, _Race_P, _DropAttack_P, ...)
│   │   ├── DLC06/             1,402 files (The Knife of Dunwall: DaudBase, Slaughter, Timsh, Tower)
│   │   └── DLC07/             1,799 files (The Brigmore Witches: Brigmore, Coldridge, Draper*)
│   ├── Localization/          CZE DEU ESN FRA HUN INT ITA POL RUS
│   └── Movies/                34 .bik
├── Engine/
│   ├── Config/                Base*.ini, DLC05/06/07
│   ├── Localization/
│   └── Stats/
├── __redist/                  DirectX, MSVC 2005/2008/2017, PhysX (legacy) installers
└── goggame-1701063787.*       GOG metadata (.info, .script, .hashdb)
```

## Configuration files (`DishonoredGame/Config`)

Gameplay is heavily data-driven through these INIs. They are first-class evidence
sources for movement, power and combat constants (CONFIRMED values once read).

`DefaultAI.ini DefaultAILocomotion.ini DefaultCamera.ini DefaultCompat.ini
DefaultContactSystem.ini DefaultConversation.ini DefaultDLC06.ini DefaultDLC07.ini
DefaultDebugMenu.ini DefaultEditor.ini DefaultEditorKeyBindings.ini
DefaultEditorUserSettings.ini DefaultEngine.ini DefaultGadget.ini DefaultGame.ini
DefaultInput.ini DefaultItem.ini DefaultLevel.ini DefaultLightmass.ini DefaultNPC.ini
DefaultNPCContent.ini DefaultNPCGuard.ini DefaultPawn.ini DefaultPlayer.ini
DefaultPlayerState.ini DefaultPower.ini DefaultStat.ini DefaultStealable.ini
DefaultUI.ini DefaultWeapon.ini DefaultWeaponRanged.ini`

Notable early observations:
- `DefaultPlayer.ini` `[DishonoredGame.DishonoredPlayerPawn] m_fMaxFallSpeed=2000.0`
- `DefaultPlayer.ini` `[DishonoredGame.DishonoredPowersComponent] m_Powers=...` encodes power
  levels as attribute modifiers (e.g. Agility/"Celerity": `Attribute_JumpZ_PowerJump`,
  `Attribute_GroundSpeedSprint`, `Attribute_MaxSpeedBeforeFallingDamage=2550`,
  `Attribute_MaxSpeedBeforeFallingDeath=3500`).
- Player-specific offscreen melee damage reduction `m_fOffscreenMeleeDamageReduction[0..3]=0.5`.

Engine-level `Engine/Config/Base*.ini` are the standard UE3 base layer.

## Cooked content (`DishonoredGame/CookedPCConsole`)

| Kind | Count | Notes |
|---|---|---|
| `.upk` | 470 | All open with `package_scan` (0 failures) |
| `.pck` | 311 | Wwise sound banks (`Bank_*`) |
| `.tfc` | 3 | Texture file caches: `Textures.tfc` (1,209,025,724 B), `Lighting.tfc` (548,298,402 B), `CharTextures.tfc` (301,325,850 B) |
| localized | 670 × 6 | `.INT .DEU .ESN .FRA .ITA .RUS` |
| other | | `Init.bnk`, `MISSIONAUDIOCACHINGLIST.AKMAP`, `GlobalShaderCache-PC-D3D-SM3.bin`, `PersistentCookerShaderData.bin`, `COOKEDCL`, `COOKGUID`, `COOKEDLANGUAGES` |

Script/class packages: `Core.upk` (45,534 B), `Engine.upk` (2,506,430 B),
`GameFramework.upk` (46,500 B), `DishonoredGame.upk` (5,928,252 B), `IpDrv.upk`,
`GFxUI.upk`, `AkAudio.upk`, `OnlineSubsystem*.upk`. `DishonoredGame.upk` contains
the Dishonored UnrealScript classes and is the main static-analysis target for
gameplay semantics.

### Level naming

Maps are split into a persistent package `L_<Map>_P.upk` plus streamed sublevels by
discipline: `_Env` (art geometry), `_Block`, `_Light`, `_Nav`, `_Script`, `_Audio`,
`_FX`, `_Stealth`, `_HighChaos`, etc.

Persistent levels (base game): ArtDealer, Boyle_Ext, Boyle_Int, Bridge_Part1a/1b/1c/2,
Brothel, Distillery, Distillery2, Flooded_FAssassins/FGate/FIntro/FRefinery/FStreets,
Galvani1/2, Isl_HighChaos/LowChaos, LightH_HighChaos/LowChaos, Out*, OutsiderDream,
Ovrsr, Ovrsr_Back, Ovrsr_Kennel, Prison, PrsnSewer, Pub_Assault/Day/Dusk/Morning/Night,
Streets1, Streets2, Streetsewer, Tower, TowerRtrn_Int/Yard, plus `DishonoredGameFull_P`.

### Global class histogram (top entries, all 470 packages)

```
165771 StaticMeshComponent    113242 AnimSequence     52179 Texture2D
 48037 ShadowMap2D             24479 MaterialInstanceConstant
 21625 StaticMesh              19921 RB_BodySetup     19626 LightMapTexture2D
  3635 Model (BSP)              3092 StaticMeshActor   3434 DishonoredNavPoint
```

The StaticMeshComponent/StaticMeshActor ratio shows that cooked levels store most
static geometry in `StaticMeshCollectionActor`s.

## Hashes (SHA-256)

### Binaries/Win32

```
70da96138fe2171cca18888a0adbb731447cd15758e55535672625f09f6bc6fb  Dishonored.exe
33f90c63b20e7c38c3246e32e45ec96a432a6007447099a76633100434975564  Galaxy.dll
db12fc7ff25cb4924b6a84245fe5b8c7366c1c1e62c7023d3fdd6732b305dce0  PhysXCore.dll
a273d799e2af1bea0c705df3cd540c39ad6a929ce29fc72173b9c7677f5a98b0  PhysXCooking.dll
767419a814f63c5da1d94e947a3ceffb9cd963cf3e4bc9352fe146e4a011e30d  NxCharacter.dll
1c7d514d002ab50afa6b479c35d25c6c628b0a8b3173719299d3a4fda55c52bc  PhysXLoader.dll
917334fa5f47b66617a8c0e8d9bbf570a40521b4ee54f9b35986ff808a62672a  PhysXExtensions.dll
96a626f69cc65c3c8922867aff45df1342e7001d8477c3b7fbe1de9c9f878883  APEX_release.dll
a4ddcf8d78eac388cbc85155ef37a251a77f50de79d0b975ab9bb65bd0375698  binkw32.dll
```

### Core packages

```
f432acd67890bb7b02d355543e2eba2830b0ffde5cc4f2416312769bb49981c9  Core.upk
8bfb32268205fb1ab94d8aede034bf13977fa6496a2701e979c24af9900e7ae8  Engine.upk
b67b38702e5f7c1f4151f1002da39d2a1f093ca89d17a095a2ca33ffb02eb9cc  GameFramework.upk
ec64bd36be12d7063199cf2722a2b8f280100f1ed3f3c63404ac34980d7141e2  DishonoredGame.upk
af48295452d46b7f5cccb81d623c75190f27937d7c3b4f52308d9650a23050f9  DishonoredGameFull_P.upk
7b82ff1c3560f9b867d57ef5219f0fe9d10378de705ee92a68fb9b2d1877bbba  Startup.upk
```

### Target map: Lady Boyle's Last Party (see `maps/TARGET_MAP.md`)

```
0365506f2f50e08749c28b8f97f5b7e423b077ee97800a343c743c604fddcfbd  L_Boyle_Ext_P.upk
01606245853fedacb337e6b7d2d030620e60e597fbb55bed0eeee5156ef3ff0b  L_Boyle_Ext_Env.upk
25998d7b9e6ea4839af0f52ef98f3a66ef4d3067ff7152e30c0e37635cbc1f4e  L_Boyle_Ext_Light.upk
95052143f734f692d5310096932638c0b361de07c67c0bea5cc0af953b0962ca  L_Boyle_Ext_Nav.upk
c7f03c95142f29b8015995579ea39b27b8999a7b24414c2b51b6ff5a2c0cc134  L_Boyle_Ext_Script.upk
98e4d108b335e0acc88f9e11298490c9a9e44757df24892275dd47eb0bbe9888  L_Boyle_Ext_Audio.upk
2ab560a528c8ccc3915f6ec68271ec2dffa60a2630796eb4f5148ad96d390cf0  L_Boyle_Ext_Fx.upk
94a4ef9683da2b84059a8981e49a517f7533a3c6c96fd66aba9ac83dba6d5bce  L_Boyle_Int_P.upk
75a01d0dd5a1b9e404dec967c45fa7e9f8be37899c518d54f56d87aaafd2f0fb  L_Boyle_Int_Env.upk
eb7acb6fd4a08e90ed20f5fa7b7b276c8cc0d57955c3b319e185230e4a326b32  L_Boyle_Int_Basement.upk
e0d8ce6a5760857e2fa715e34bf7c3b5aad189c8a865a23c89c2123ca1158de8  L_Boyle_Int_Light.upk
e812538c72438041d6f80116f1ebb3a85b857be776fec921e879cc677fd1d205  L_Boyle_Int_Nav.upk
17be4c7bd18f4e400dc053e0fcf61590240b38b0c3666f2f224676ffdfaccb86  L_Boyle_Int_Script.upk
03127105a5326d4a4511a5c05bac192058bf3a0e8eca81afb2ae580350988f78  L_Boyle_Int_Stealth.upk
d4cb094f5970693226f12cca479fd96d7e18c962f1f126d6b26f4217d1a6fad4  L_Boyle_Int_Audio.upk
c8ba1227ee8d1f26a84732e68dc6cd7419c4c8aef072d0d11dbdad5b52881b00  L_Boyle_Int_Fx.upk
```

### Secondary (movement test bed candidate): Hound Pits pub

```
92843e44fc60e2ed73d2560058470b5b677cf995d53d6e290e64a7b75188eab0  L_Pub_Day_Env.upk
775705a96671a0caada01044073b7278f5159462328891129272caca995332c4  L_Pub_Day_Light.upk
a68b879c538b4f77dcef96cf7b520d2c16c4fd4482947ec9fa26d683239cc130  L_Pub_Day_Nav.upk
```

## Handling rules

- The install directory is read-only for this project. Tools open packages read-only.
- Nothing from this directory is committed. `.gitignore` excludes `*.upk`, `*.tfc`, `*.bin`.
- The runtime locates the install via `DISHONORED_DIR` or the GOG registry key above.
