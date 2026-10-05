# Menus and settings of the original game

How Dishonored builds its main menu, pause menu and options screens, where their text and
settings come from, and the full list of original settings with defaults. Everything here was
read from the GOG install with the repo's tools (`swf_dump`, `swf_render`, `obj_dump`,
`package_dump`, `find_export`, `audio_dump`). No text or images are copied into the
repository; the launcher reads them at runtime.

## Movies and packages

| Movie | Package | Notes |
|---|---|---|
| `UI_MainMenu.MainMenu` (210 KB, 55 external images) | `Dishonored_MainMenu.upk` | Main menu. It is also the persistent level of the menu map (see below). |
| `UI_OptionsMenu.OptionsMenu` (156 KB, 19 images) | `Startup.upk` | Options screen, video settings, gamma, gamepad scheme. Imported by the main and pause menus. |
| `UI_LoadGame.LoadGame` | `Startup.upk` | Save list. Imported. |
| `Common_assets.lib` | `Startup.upk` | Shared widgets: title bar, shoulder/trigger/face button icons, help bar. |
| `UI_PauseMenu.PauseMenu` (134 KB, 25 images) | `UI_PauseMenu_SF.upk` | Pause / game over / tutorials list. |
| `UI_Gamma.GammaImage` | `UI_Gamma_SF.upk` | Calibration logo (`gamma_logo`), drawn five times by script. |
| `Options_GamepadMapping_Xenon` / `_PS3` (Texture2D) | `UI_GamepadMapping.upk` | Controller picture loaded at runtime by `ImgLoader`. |
| fonts `$NormalFont` / `$TitleFont` | `DisFonts_SF` (`gfxfontlib.swf`) | Imported by every menu. They are "Emerge BF" (body) and "ChaletComprime-CologneEighty" (titles). |

`SwfMovie.ReferencedSwfs` lists the imports. For example, MainMenu lists `common_assets.lib`,
`UI_OptionsMenu.OptionsMenu` and `UI_LoadGame.LoadGame`. The GFx `ImportAssets2` tags (code
71) resolve characters against them. MainMenu imports `o_OptionsScreen`, `o_videoSettings`,
`o_Options_gammaSetting`, `o_Options_padMapping` and `m_loadGame_` from the options and
load-game movies, plus the two fonts. `DefaultUI.ini` `[DishonoredGame.DisGlobalUIManager]`
names the in-game movies (`m_PauseMenuMoviePath="UI_PauseMenu.PauseMenu"`, HUD, journal, ...),
with DLC overrides.

### Main menu (`UI_MainMenu.MainMenu`, stage 1280x720, 30 fps)

Root timeline. Frame 0 holds `mainMenu_mc` (`m_MainMenu`, 242), `newGame_mc` (`m_nGame`, 201),
`startScreen_mc` (`m_StartScreen`, 165, the "press any key" logo), `gamepadMapping_mc` and
`gammaSetting_mc`. Frame 1 adds `missions_mc` (`m_missions`), `loadGame_mc`,
`optionsMenu_mc` and `videoSettings_mc`. These are the imported options-movie screens.

- `mainMenu_mc` holds the background art (sprite 230), `_blades_mc`, `_compass_mc`,
  `_splatters_mc`, `_menu_mc` (the button band, tilted by about 2 degrees), `_logo_mc` (the
  DISHONORED cartouche with glow and lines), `_corvo_mc` and `_DLCButton_mc` (console store).
  Buttons (`m_MainMenu_btn_`, 121) are attached at runtime by the AS2 class
  `MainMenuButtonBar` along the band. Each button has the frame labels `default`, `over`,
  `loop_over`, `out`, `valid`, `stop_lock`, `over_lock` and `out_lock`. Its label field is a
  `$TitleFont` 25 px centred field, colour `#e3f2d6`, with a glow copy.
- `newGame_mc` is the difficulty screen. It has a painted background (`_bkgdTexture_mc`), blades,
  `_menu_mc` (a panel with a `_description_mc` multiline 26 px text box), a Corvo portrait,
  and a title from `lib_titleMc`. Its buttons are `m_nGame_btn` (labels `default` / `over` /
  `out` / `valid` / `over_lock` / `out_lock`). The difficulty names and descriptions are
  `t_difficulty0..3` / `t_difficulty0..3Desc`.
- `missions_mc` is a slide of `m_missions_item` cards. Each card has a mission image loaded by
  `ImgLoader` from `UI_Mission_Large` / `UI_Mission_Small`, a number, a name, a lock and a
  selection stroke, plus tabs (`t_Campaign`, DLC).
- AS2 classes in the movie include `MainMenu`, `MainMenuBase`, `MainMenuButton`,
  `MainMenuButtonBar`, `NewGameMenu`, `MissionsMenu`, `MissionsScreen`, `StartScreen`,
  `TransitionHandler`, `SoundHandler` and `_common.*` (`GenericMenu`, `ItemsList`,
  `ScrollBarList`, `SelectionHandler`, `TabsHandler`, `SlideMenu`, `InputsManager`,
  `MessageBoxInvoke`, `ImgLoader`, `Vignette`). They are not executed by the port.

### Options (`UI_OptionsMenu.OptionsMenu`)

Root: `options_mc` (`o_OptionsScreen`: only `_bkgdTexture_mc`; everything else is attached by
script), `gamepadMapping_mc` (`o_Options_padMapping`), `gammaSetting_mc`
(`o_Options_gammaSetting`), `videoSettings_mc` (`o_videoSettings`) and `help`.

The widgets are exports, attached by the classes `OptionsScreen`, `OptionsListWithTabs`,
`OptionsSubTabs`, `OptionsLine`, `KeyboardMappingList`, `VideoSettings`, `GammaSetting`,
`GamepadMapping` and the `_common.P_*` widgets:

| Export | Class | Contents / labels |
|---|---|---|
| `SubTab` (18 -> 17) | tab | `TAB TEXT` ($TitleFont 21 px). States `default`, `over`, `selected`, `unselected`, `out`, `disabled`. Unselected colour `#828a7b`. |
| `o_options_line` (59) | `OptionsLine` | `btn` hit area, `_bkgd_mc` strip, `_txt_mc` label ($NormalFont 25 px, 481 px wide). States `default` / `over` / `out` / `disabled`. |
| `o_options_line_s` (51) | short line | same, 23 px |
| `OptionsStepper_widget` (73) | `P_OptionsStepper` | `_bkgd_mc`, centred value text (220 px, white), `_leftArrow_mc` / `_rightArrow_mc` at -166 / +166. |
| `OptionsStepperB_widget` (84) | `P_OptionsStepperBoolean` | two halves `btn0` / `btn1` (`selected` / `unselected`) |
| `Slider_widget` (90) | `P_Slider` | arrows, `_bkgd_mc` track, `_thumb_mc` with a numeric field (`999`, dark) |
| `DropDownMenu_widget` (103) + `DropDownMenu_list(_item)` | `P_DropDownMenu` | value text and `_btn_mc` (`list_open`) |
| `Link_widget` (97) | `P_Link` | opens a secondary screen |
| `o_KeyBoardMappingList` / `o_MappingList_item` (122) | `KeyboardMappingList` | action name at +50, `widget_mc` (121: `bind_in`, `loop`, `bind_out`) at +680 showing the key |
| `o_videoSettings` (151) | `VideoSettings` | `_black_mc` backdrop (solid `#17191c`), `_widget_mc` = list + `buttonBar_mc` (`accept_mc` / `cancel_mc`, labels `default`, `over`, `out`, `click`, `disabled`) + title |
| `o_Options_gammaSetting` (159) | `GammaSetting` | `_title_txt` ($TitleFont 35), `_desc_txt`, slider line |
| `o_Options_padMapping` (203) | `GamepadMapping` | `_img_mc` (pad picture), 37 label fields, platform labels `Xenon` / `PC` / `PS3` / `PS3_Jap` |

### Pause menu (`UI_PauseMenu.PauseMenu`)

Root: `bkgd_mc` (blades and particles), `tutorials_mc`, `pauseMenu_mc` (`p_pauseMenu`). Frame 1
adds the imported `optionsMenu_mc`, `loadGame_mc`, `saveGame_mc`, `gamepadMapping_mc`,
`gammaSetting_mc` and `videoSettings_mc`.

`pauseMenu_mc` holds the textures, blades, splatters, compass, lines, the mask portrait
(`_corvo_mc`, a 383-frame animation), `_menu_mc` + `_maskMenu_mc` (the list area),
`_title_mc` (title bar rotated about -100 degrees with a "PAUSE" $TitleFont 65 px field and
two shadow copies) and `_gameOver_mc`. Entries are `p_pauseMenu_item` (37): a `_bkgd_mc`
strip (`n_menuLine0..2`, three random variants), a `_txt_mc` ($TitleFont 25), and a
`_strike_mc` with a mask. Its labels are `default`, `over`, `out`, `over_lock`, `out_lock` and
`stop_lock`. Entry texts come from `[DisGFxMoviePlayerMenuBase_Texts]`: `t_ResumeGame`,
`t_SaveGame`, `t_LoadGamePauseMenu`, `t_Options`, `t_Help`, `t_BackToMainMenu` (and
`t_Q_BackToMainMenu`), `t_QuitGame`.

### Menu level

`Dishonored_MainMenu` is a level: `TheWorld` plus the streamed sublevels
`Dishonored_MainMenu_Env` and `_FX`. `map_dump Dishonored_MainMenu` imports it with the regular
importer. It has 4 lights (a dominant directional, two spots and a point), a cubemap,
post-process settings (exposure 0.6, desaturation 0.2, film grain) and two fog volumes.
Kismet `Main_Sequence` runs one matinee per screen (`SeqAct_Interp` comments: `MainMenu`,
`NewGame`, `StartCam_Reverse`, ...), each driving one of 11 `CameraActor`s. Six cameras
share the main pose: location (53.6, -4835.0, 3036.1), rotator (4011, 12916, -5), that is
pitch 22.0 and yaw 71.0 degrees. `DefaultEngine.ini` `m_MapConfig` loads the map with the
`LoadingSavingNotification` movie.

### Sounds and music

`Twk_UI_MainMenu.Twk_GFxMoviePlayerMainMenu` (`DisTweaks_GFxMoviePlayerMainMenu`, in
`Dishonored_MainMenu.upk`):

- `m_pStartMusicEvent = Musics.Music_Menu` and `m_pStopMusicEvent = Musics.Music_Menu_Stop`.
  These are in bank `Bank_Music_Menu`. `Music_Menu` resolves to one looping 129.6 s stream.
- `m_SoundThemes` "Default": an own theme (`M_PressStart` -> `Snd_UI_Ingame.UI_M_PressStart`,
  `M_OpenMainMenu` -> `UI_M_OpenMainMenu`) plus the external theme
  `Twk_InGameUI.UISoundTheme_Common` (in `DishonoredGame.upk`). The common theme maps
  `OpenScreen`, `CloseScreen`, `Move`, `validation`, `Confirmation`, `back`, `Failure`,
  `change`, `ChangeMainTab`, `ChangeSubTab`, `Stop`, `ToHome`, `ToEnd` and `Reset` to
  `Snd_UI_Ingame.UI_<name>` events in `Bank_UI`. `UI_Move` is -24 dB.

The movies' `SoundHandler` class posts these names; the port posts the same names from its
handlers.

## Where the text comes from

All labels are UE3 localization (`DishonoredGame/Localization/INT/*.int`, UTF-16LE). The
movies' own text fields only hold placeholders (`TEXT`, `TAB TEXT`, `W`, `999`), so the
port fills them from:

| File / section | Keys |
|---|---|
| `Settings.int` `[ProfileSettingIDs]` | one label per option: `PSI_Mouse_Sensitivity`, ..., `PSI_GBA_*` (key-binding actions) |
| `Settings.int` `[ProfileSettingValues]` | value names: `BP_True/False`, `HV_*`, `CS_*`, `KCM_*`, `EDifficulty_*`, `TextureDetails_*`, `ModelDetails_*`, `AntialiasingMode_*`, `SubtitlesMode_*`, `SpeakerConfiguration_*`, `PBS_1..4` |
| `Settings.int` `[Keys]` | key display names (`SpaceBar` -> "Space Bar", `Unbound_Game`) |
| `Twk_InGameUI.int` `[Twk_GFxMoviePlayerMenuBase DisTweaks_GFxMoviePlayerMenuBase]` | tabs `m_SettingsCategory_General/Controls/Graphics/System/Audio`; sub-tabs `m_SettingsSubCategory_GameplaySettings` ("Gameplay"), `_HUDSettings` ("User Interface"), `_KeyboardMapping`, `_MouseSettings`, `_GamepadSettings`; `m_SettingsSubMenu_VideoSettings`, `_DeviceSelection`; pad labels `m_GamepadMapping1..4[i]` |
| `DishonoredGame.int` `[DisGFxMoviePlayerBase_Texts]` | `t_Options`, `t_Back`, `t_Select`, `t_Confirm`, `t_Validate` ("ACCEPT"), `t_Cancel`, `t_Apply`, `t_EmptyList`, `t_Loading` |
| `DishonoredGame.int` `[DisGFxMoviePlayerMenuBase_Texts]` | `t_RestoreSettings`, `t_Q_RestoreSettings`, `t_Modify_ControlsScheme`, `t_Modify_VideoSettings`, `t_Modify_Gamma`, `t_GammaHint`, `t_Q_VideoSettings`, pause entries, `t_QuitGame`, `t_BackToWindows` |
| `DishonoredGame.int` `[DisGFxMoviePlayerMainMenu_Texts]` | `t_PressAnyKey`, `t_NewGame`, `t_Missions`, `t_Extras`, `t_difficulty*`, `t_MissionsScreenDesc`, `t_Campaign` |

The category tabs on PC are General, Controls, Graphics and Audio. System holds the
storage device, which is console-only.

## Settings model

`DefaultEngine.ini` `[Engine.UIDataStore_OnlinePlayerData]` sets
`ProfileSettingsClassName="Engine.ArkProfileSettings"`. Its class default object
`Engine.upk` `Default__ArkProfileSettings` (77,812 bytes) holds:

- `ProfileMappings` (124 entries): id, mapping type (`PVMT_IdMapped`, `PVMT_Ranged`,
  `PVMT_Boolean`, `PVMT_RawValue`), value ids with their value-label keys, and min / max /
  increment for ranges.
- `DefaultSettings` (113 entries): id, data type and default value.
- `m_BindableKeyMapping` (108 key names) and the enums `EBindableKey`, `EPadBindingSets`,
  `EBooleanParameter`, `EHUDVisibility`, `ECrosshairStyle`, `EKillCamMode`, `EDifficulty`,
  `ETextureDetails`, `EModelDetails`, `EAntialiasingMode`, `ESubtitlesMode` and
  `ESpeakerConfigurationProfile`.

The ids are not named in the data; the mapping to `PSI_*` is by order. Ids 29-64 are the 36
key bindings in `Settings.int` order. Ids 65/66, 74/75, 85/86, 102/103, 110/111, 113/114,
124/125 and 131/132 are gaps, one pair between each `Settings.int` group. Each group's
types and ranges match its labels exactly: for example 118 is ranged 65-110 step 5 with
default 75 (FOV), 112 is 0.5-5.0 with default 2.2 (gamma), and 99 has values
`CS_Off/CS_Simple/CS_Normal`. Confidence: LIKELY. Values are stored in the player profile.
Ids 135-153 are save/progress data. Resolution, fullscreen and vsync also exist as
`DefaultEngine.ini` `[SystemSettings]` `ResX=1920 ResY=1080 UseVsync=True Fullscreen=True`.
The user copy would be `Documents/My Games/Dishonored/DishonoredGame/Config/DishonoredEngine.ini`,
but this install has not written one yet.

### Every original setting

The "Port" column says whether the port acts on the setting. Every option is listed in
the options screen and saved; the ones the port does not act on are marked
"not yet effective".

| Tab / sub-tab | Id | Key | Type / values | Default | Port |
|---|---|---|---|---|---|
| General / Gameplay | 104 | `PSI_Gameplay_bAutoUseManaElixir` | bool | On (also `Twk_Pawn_PlayerInput.m_bAutoConsumeManaElixir=true`) | saved |
| | 105 | `PSI_Gameplay_KillCamMode` | Off / Normal / Frequent | Normal | saved |
| | 106 | `PSI_Gameplay_Difficulty` | Easy / Normal / Hard / Very Hard | -1 (asked at New Game); port: Normal | saved |
| | 107 | `PSI_Gameplay_bAutoSaveInMenu` | bool | On | saved |
| | 108 | `PSI_Gameplay_HeadBobAmount` | 0.0-1.0 step 0.1 | 1.0 | saved |
| | 109 | `PSI_Gameplay_CameraRelativeClimbing` | bool | On | saved |
| General / User Interface | 87 | `PSI_HUD_Visibility` (gauges) | Off / Contextual / Always | Always | applied |
| | 88 | `PSI_HUD_bShowObjectivePopups` | bool | On | applied |
| | 89 | `PSI_HUD_bShowTutorialNotifications` | bool | On | saved |
| | 90 | `PSI_HUD_bShowInteractions` | bool | On | applied |
| | 91 | `PSI_HUD_bShowHighlight` | bool | On | saved |
| | 92 | `PSI_HUD_bShowPickupLog` | bool | On | saved |
| | 93 | `PSI_HUD_bShowContextualIcons` | bool | On | saved |
| | 94 | `PSI_HUD_bShowPlayerStance` | bool | On | applied |
| | 95 | `PSI_HUD_bShowObjectiveMarkers` | bool (`BP_Default*`) | On | applied |
| | 96 | `PSI_HUD_bShowGrenadeMarkers` | bool | On | saved |
| | 97 | `PSI_HUD_bShowAwarenessMarkers` | bool | On | saved |
| | 98 | `PSI_HUD_bShowHeartTargetMarkers` | bool | On | saved |
| | 99 | `PSI_HUD_CrosshairStyle` | Off / Simple / Normal | Normal | applied |
| | 100 | `PSI_HUD_bCrosshairMovement` | bool | On | saved |
| | 101 | `PSI_HUD_CrosshairOpacity` | 0-100 step 10 | 100 | applied |
| Controls / Keyboard Mapping | 29-64 | `PSI_GBA_*` (36 actions) | key (`m_BindableKeyMapping`) | `DefaultInput.ini` `[Engine.PlayerInput]` `.m_PCBindings` | movement, jump, sneak, sprint, lean, use, sword, left hand, block, shortcuts 1-5 applied; the rest saved |
| Controls / Mouse | 67 | `PSI_Mouse_Sensitivity` | 0.1-3.0 step 0.1 (multiplier; `[Engine.PlayerInput] MouseSensitivity=13.0`) | 1.0 | applied |
| | 68 | `PSI_Mouse_bSmooting` (sic) | bool | On | applied (two-frame average, APPROXIMATION) |
| | 69 | `PSI_Mouse_bInvertY` | bool | Off | applied |
| | 70 / 71 | `PSI_Mouse_bAutoAim` / `AutoAimStrength` | bool / 0-100 | Off / 100 | saved |
| | 72 / 73 | `PSI_Mouse_bFriction` / `FrictionStrength` (aim assist) | bool / 0-100 | Off / 100 | saved |
| Controls / Gamepad | link | `t_Modify_ControlsScheme` | pad picture screen | | shown |
| | 76 | `PSI_Gamepad_BindingSet` | Type A-D (`.m_PadBindingSet1..4`) | Type A | saved |
| | 77 | `PSI_Gamepad_bVibration` | bool | On | saved |
| | 78 / 79 | `PSI_Gamepad_LookX/YSensitivity` | 0-100 step 10 | -1 (unset); port: 50 | saved |
| | 80 | `PSI_Gamepad_bInvertY` | bool | -1; port: Off | saved |
| | 81 / 82 | `PSI_Gamepad_bAutoAim` / strength | bool / 0-100 | -1, 100; port: On, 100 | saved |
| | 83 / 84 | `PSI_Gamepad_bFriction` / strength | bool / 0-100 | On / 100 | saved |
| Graphics | link | `t_Modify_Gamma` -> 112 `PSI_Graphics_Gamma` | 0.5-5.0 step 0.1 (`m_BrightnessValues` 0.998, 0.99, 0.95, 0.90, 0.80 for the logos) | 2.2 | applied (3D colour-grading exponent 2.2/gamma) |
| | link | `t_Modify_VideoSettings` -> 115 `PSI_GraphicsPC_Resolution` | display modes (drop-down) | desktop (`[SystemSettings]` 1920x1080) | applied |
| | 116 | `PSI_GraphicsPC_bFullScreen` | bool | On | applied |
| | 117 | `PSI_GraphicsPC_bVSync` | bool | On | applied |
| | 118 | `PSI_GraphicsPC_FOV` | 65-110 step 5 (`DefaultCamera.ini` `m_fDefaultFOV=75`) | 75 | applied |
| | 119 | `PSI_GraphicsPC_TextureDetails` | Low / Medium / High | High | applied (import cap 256 / 512 / 1024) |
| | 120 | `PSI_GraphicsPC_ModelDetails` | Normal / High | Normal | saved |
| | 121 | `PSI_GraphicsPC_LightShaftEnable` | bool | On | saved |
| | 122 | `PSI_GraphicsPC_AntiAliasingMode` | Off / MLAA / FXAA | MLAA | applied (Off = no MSAA, otherwise MSAA 4x; APPROXIMATION) |
| | 123 | `PSI_GraphicsPC_RatShadows` | bool | Off | saved |
| Audio | 126 | `PSI_Audio_GlobalVolume` | 0-100 step 10 | 100 | applied |
| | 127 | `PSI_Audio_MusicVolume` | 0-100 | 90 | applied (level score, menu music) |
| | 128 | `PSI_Audio_SFXVolume` | 0-100 | 90 | applied (SFX, ambience, UI) |
| | 129 | `PSI_Audio_VoicesVolume` | 0-100 | 100 | applied (`VO_` events) |
| | 130 | `PSI_Audio_SubtitlesMode` | Off / Main Dialogue / All Speeches | -1 (unset); port: Off | saved (exposed to the dialogue HUD through `GameSettings`) |
| | 133 | `PSI_AudioPC_SpeakerConfiguration` | Auto / Stereo / 5.1 | Auto | saved |

The original also has profile ids 1-22 (UE3 generic `OnlineProfileSettings`: vibration,
invert, voice, difficulty, ...), which its menus do not show, and 152, the DLC06 difficulty.

## The port's launcher

Implemented in `crates/dishonored_runtime/src/{launcher,launcher_bg,menu_*,pause_menu,settings,hud_settings}.rs`
and `crates/dishonored_settings`. The approach:

- Screens are rebuilt as display lists. Original sprites are drawn at the frame of their state
  label: the end of the `over` transition for focus, `selected` / `unselected` for tabs, and
  `PC` for the pad screen. The movie's text fields are filled with the localized strings
  using the field's font, size, colour, alignment and rotation.
- Layout positions that ActionScript computes (list rows, button bar spacing, the pause
  entries along the strips) are this port's. Original geometry inside each widget is kept.
- The Missions screen is a list of option-style lines rather than the card slide, because
  custom missions have no card artwork.
- Settings live in `%APPDATA%/dishonored-rs/settings.ron`. `--settings FILE` or
  `DISHONORED_RS_SETTINGS` overrides the path. The defaults are the values above.
