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

#### Opening and input timing

Read from the AS2 bytecode (`swf_dump Dishonored_MainMenu MainMenu - --actions <sprite>`):

- `MainMenu.PlayOpenAnimation` (class sprite 291) runs whenever the main menu opens: after
  the start screen and from `BackToMainMenu` (return from a sub-screen). It plays
  `M_OpenMainMenu`, tweens the menu to its start properties (0.85 s, `Strong.easeOut`),
  opens the splatters, logo, blades, Corvo and compass, and sets a 180 ms interval to
  `OnOpened`.
- `OnOpened` calls `OpenMenu`: `_menu_mc` (the button band) is set to alpha 0 and moved
  `_width + 150` px right of its rest place, then tweened back over 0.35 s
  (`Strong.easeOut`); `SetMenu` builds the buttons and calls `Open`
  (`_common.GenericMenu`, sprite 281).
- `MainMenuButtonBar.Open` (sprite 289) tweens its lines (0.25 s) and sets a 400 ms interval
  to its `OnOpened`, which calls `InitInputs` (`GenericMenu.InitInputs`: `EnableInputs(true)`,
  the accept / back inputs, and the selection handler's `_handlerEnabledState = true`). Only
  then does the button bar take keys, pad or mouse selection.

So the original's main menu takes input 180 + 400 = 580 ms after it opens. The port plays
the whole opening (see "Front end motion" below) and the same input lock
(`launcher.rs` `OPEN_INPUT_DELAY`), counted from the frame the menu opens.

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
handlers. `SoundHandler.PlaySound(name)` has no table of its own: it calls
`gfxProcessSound(null, 'Default', name)` and the movie player's theme resolves the name, so the
movies' mixed spellings (`Validation` / `validation`, `Back` / `back`, `Change` / `change`) hit
the same binding (an `FName` lookup, case-insensitive; the port matches the same way,
`menu_audio.rs`). The global movie's own theme
(`Twk_GFxMoviePlayerGlobal.DisUISoundTheme_1`) adds `G_MsgBoxOpen` (message box), the pause
movie's (`Twk_GFxMoviePlayerPauseMenu.DisUISoundTheme_2`) `GO_OpenGameOverMenu`.

## How the motion was decoded

The menus' behaviour is AS2 in the movies' `DoInitAction` class sprites plus one-line frame
scripts (`stop()`, `play()`, `gotoAndPlay("loop")`) in the timelines. Two research helpers
(committed: `research/scripts/swf_actions.ps1` lists every sprite's frame labels and frame
scripts; `research/scripts/as2_decompile.ps1` turns the class bytecode into readable
statements; both take the raw movie that `swf_dump <pkg> <movie> <out.gfx>` writes) gave the
class sources. Rules that hold in every movie (classes 249 `gfx.motion.Tween`, 259 `Strong`,
260 `Back`, 268 `None`, compiled into each movie):

- `clip.tweenTo(d, props, ease)` snapshots the named properties and interpolates
  `start + ease(t) * (target - start)` with `t = elapsed / d` in real time (`getTimer`), then
  snaps to the target. One tween per clip: a new `tweenTo` replaces the running one (properties
  only the old one named freeze). A plain assignment during a tween is overwritten by the tween
  on the next frame. `tweenFrom` swaps: the clip jumps to the given values and tweens back.
  `tweenEnd(true)` jumps to the end, `tweenEnd(false)` freezes.
- `Strong` is quintic (`easeOut = 1 - (1 - t)^5`); `Back` overshoots with `s = 1.70158`
  (`easeInOut` with `s * 1.525`).
- `setInterval(obj, 'Fn', ms)` is always cleared in `Fn`: a one-shot delay.
  `_utils.DelayedAnimation(target, first, last, ms, cb, endCb, dec)` calls `cb(first)` at once,
  the next index every `ms`, and `endCb` one interval after the last.
- State labels (`over`, `out`, `selected`, ...) are played with `gotoAndPlay` and run until the
  frame script's `stop()`, at 30 fps. The settled look of `over` on the main menu button
  (sprite 121) is frame 13 (stop), not the frame before the next label (17). Looping states
  (`loop_over`, the selected tab's glow, the focused arrows) `gotoAndPlay` back to their label.
- Sprites whose first frame does not `stop()` animate on their own from creation (the Corvo
  mask of the main menu, sprite 241, loops frames 9-278; the compass `loop` 8-46).
- PC keys (`InputsManager.InputManager`, same in every movie): A = Enter / Numpad 0,
  B = Esc / Backspace / Numpad 1, X = Shift / Numpad 2, Y = Alt / Numpad 3, LB / LT / RB / RT =
  Numpad 4 / 5 / 7 / 8, directions = arrows (repeat while held), wheel = scroll. There is no
  Q / E or R in the movies.
- `_global.gfxExtensions = true` in every movie: the classes also set and tween the GFx 3D
  properties `_z`, `_xrotation`, `_yrotation` (e.g. the main menu button's over props
  `_z = -650`, the logo's `_z = -650`). The port does not draw them (U-951).

The port reproduces this in `menu_anim.rs` (the tween / easing / interval / timeline model)
and `menu_art.rs` (frame scripts parsed from the movie; `States` plays a label's timeline for
the seconds since it was sent; per-clip property overrides; free-running timelines).

## Front end motion (decoded)

All times are from the call that starts the sequence. Coordinates are in the parent clip.

### Start screen (`StartScreen` 279, `startScreen_mc` at (640, 372.1))

`Open`: the clip's alpha 0 -> 100 in 0.45 s, `_logo_mc` 0 -> 100 in 0.85 s, `_txt_mc`
(`t_PressAnyKey`, `$TitleFont` 25 px) 0 -> 100 in 0.25 s (all Strong.easeOut). Any key:
`M_PressStart`, the clip fades out (0.25 s) and the main menu opens (`PlayOpenAnimation`).
B on the main menu goes back to it (`MainMenuButtonBar.BPressed`: the menu closes, the start
screen opens).

### Main menu (`MainMenu` 291, `MainMenuButtonBar` 289, `MainMenuButton` 271, `GenericMenu` 281)

`PlayOpenAnimation`:

| t | clip | from | to | d | ease |
|---|---|---|---|---|---|
| 0 | `mainMenu_mc` | current (after a close: alpha 0, y + 50) | start props, alpha 100 | 0.85 | Strong.easeOut |
| 0 | `_logo_mc` | alpha 0, rot -25, scale 450, x - 150 | props | 0.2 | Strong.easeOut |
| 0, +75 ms each | logo `line0..4_mc` | scale 350, alpha 0, y -/+ 150 | props | 0.25 | Strong.easeOut |
| 150 ms | logo `_bkgd_mc` | alpha 0, rot 10, scale 250 | props | 0.2 | Strong.easeOut |
| 350 ms | logo `_logo_mc`, `_logoGlow_mc` | scale 250 / alpha 0 | props / alpha 100 | 0.2 | Strong.easeOut |
| 10 ms, +50 ms each | `splatter0..2_mc` | scale 30, alpha 0, x -/+ 250, y -/+ 550, rot - 50 | props | 0.35 | Strong.easeInOut |
| 0 | `_corvo_mc` | x + 350, rot 5 | props | 0.3 | Strong.easeInOut |
| 0 | `compass0/1_mc`, `compass2_mc` | scale 150 / 250, alpha 0 | props | 0.35 / 0.25 | Back.easeOut / Strong.easeOut |
| 0 | blades (`AnimatedBlades`) | +-5 px, -5.5..7.5 deg off | props | 0.55..1.05 (random) | Strong.easeOut; then every 2.7 s a 3.5 s drift to new random offsets |
| 180 ms | `_menu_mc` (band) | alpha 0, x + width + 150 | props | 0.35 | Strong.easeOut |
| 180 ms | band `_bkgd_mc` | alpha 0, x 250 | alpha 100, x 0 | 0.25 | Strong.easeOut |
| 180 ms | `_lineUp_mc`, `_lineDown_mc` | scale 250 | props | 0.25 | Strong.easeOut |
| 180 ms, +50 ms each | buttons | scale 250, alpha 0 | over props (the selected one: scale 110, y 5) or out props (scale 100, y 0) | 0.35 | Back.easeInOut |
| 580 ms | | | inputs on (`InitInputs`), compass `loop` | | |

Layout (`GenericMenu.SetMenu`, horizontal, origin centre): button i is as wide as its label
(`$TitleFont` 25 px, autoSize `textWidth + 4`) plus 50 (its top / bottom lines); the buttons
are packed edge to edge and the row is right-aligned at `-(10 + 0.1 * Stage.width)` = -138 in
the band; the band's dark strip (`_bkgd_mc`, solid #17191C) is stretched to the row width
+ 128. Entries (`SetMenu`): Continue (when a save exists), New Game, Missions, Load (when saves
exist), Options, Quit Game (PC).

Selection (left / right, mouse hover; the bar loops): `playSelectionOn` - side lines out
(x -/+ 25, alpha 0, 0.2 s), top / bottom lines in (from x -/+ 30, alpha 0, to props, 0.35 s),
the button to scale 110 / y 5 (0.25 s Back.easeOut) and `over` (frames 7-13).
`playSelectionOut` - side lines back (0.25 s Back.easeOut), top / bottom to x -/+ 40 alpha 0
(0.2 s), the button to scale 100 / y 0 (0.15 s Strong.easeOut) and `out` (28-37).
`onSelectionUpdated(i)` with `p = i * 100 / last`: `compass0` rotation -10.25 i, `compass1`
7.5 i (0.25 s); Corvo x - 0.01 p, y - 0.02 p; blades x - 0.07 p, y - 0.085 p (0.25 s).

Confirm (A / click): inputs off, `Validation`, the choice runs 150 ms later
(`OnPlayerChoiceConfirm`): Continue keeps the menu (`bSkipCloseMenu`); the others close it and
open their screen at once. `MainMenu.Close`: Corvo x + 350 alpha 0 (0.3 s), splatters x + 350
alpha 0 (0.25 s), the buttons one every 50 ms to alpha 0, scale 80, x - 30 (0.2 s), the whole
menu alpha 0, y + 50 (0.35 s). Quit Game asks `t_Q_QuitGame` (Yes quits, No reopens the menu).

### New game screen (`NewGameScreen` 285, `NewGameMenu` 282)

The port's Play screen uses it (modes instead of difficulties, the same widgets). Buttons
(`m_nGame_btn`) vertical at `x = 10 + 0.075 * Stage.width`, `y = i * (h + 2) + h / 2`
(h 51.5); one every 50 ms from scale 250 to the out props (0.35 s Back.easeInOut). Panel art
(`_bkgd_mc`) at 150 ms (scale 250, rot -5 -> props, 0.25 s), description at 300 ms (scale 450 ->
100, 0.25 s), Corvo and the texture from x - 500 (0.25 s), title bar, help bar
(`SecondaryScreen`) at 550 ms; input at 160 ms. Focus: x 25 (0.25 s Back.easeOut) + `over`;
out: x 0 (0.25 s) + `out`; the portrait shows frame `idx + 1`; the description swaps at once.
B: the buttons close one by one (50 ms), the screen hides at once.

### Missions screen (`MissionsScreen` 278, `MissionsMenu` / `SlideMenu`)

Cards (`m_missions_item`, about 482 px wide) on `_slide_mc` (-566.1, -69.65, -5 deg):
`SetItemsSize(cur)` gives scale / alpha 100 to the current card (raised 35 px), 80 to its
neighbours, then 75, then 70; each card advances by its width at the previous card's scale
minus 100 at that scale. `Open`: the cards snap (alpha 100), the strip slides from the last
card to the first in `n * 0.08` s, the slide fades in over 2.25 s, the selection and inputs at
150 ms. Selection: the card plays `over`, its strokes fade in (0.25 s), every card tweens to
its new size / place (0.2 s), the strip centres the card (0.35 s). Left / right do not wrap
(`Stop`). The card picture is `UI_Mission_Large.MissionsScreen_<m_ImageName>_Large`, the
`m_ImageName` of the map in `m_MapConfig`; number `00`-style, name upper-cased. The port uses
this screen for its custom missions and for free roam (`menu_maps.rs`: the 42 base maps).

### Confirmation box (`UI_Global` `gl_msgBox`, `_common.MessageBox` 176)

`InvokeMessageBox(cb, target, msg, b0, b1)` -> engine `ShowMessageBox` -> the box at
(640, 360): `G_MsgBoxOpen`; vignette scale 120 -> 100 (0.65 s), box art alpha 0 / scale 50 ->
100 (0.3 s Back.easeOut), box shape scale 350 -> 100 (0.3 s), text scale 300 -> 100 (0.3 s),
buttons bar scale 250 -> 100 (0.3 s), backdrop black 60 % / white 20 % (0.25 s); edge particles
fly in from 1..500 px outside (0.45 s); the box then drifts (scale 105 over 18 s, 102.5 over
8 s; the art +-20 px). Buttons (`gl_msgBox_btn`, about 285 px) side by side, centred, at the
bar (-16.15, 136.2); focus button 0 (Yes); left / right do not wrap (`Stop`); Enter confirms
(`Confirmation`); Esc does nothing on PC. Inputs at 250 ms. Close: alpha 0 / scale 105
(0.45 s), the box is removed and the answer sent at 250 ms (0 = Yes, 1 = No, -1 = timer).

### Title bar and help bar (`Common_assets.lib`)

Title bar (`lib_titleMc`, `_common.TitleBar` 324; `SetTitle` upper-cases into the text and its
shadows): the clip snaps to its props; `_bkgd_mc` from alpha 0, x - 120, scale 250, rot -20
(0.3 s Strong.easeInOut); the cart to alpha 80 (0.6 s); at 150 ms the lines (`tweenFrom`
x -350 / 1250, scale 150, alpha 0; 0.3 / 0.35 s) and shadow 0, shadow 1 at 250 ms, the text at
350 ms (alpha 0, x - 20, scale 200, rot -5 -> props; shadows Back.easeInOut, text
Strong.easeOut). Shadow 2 never fades in (off by one in `OpenTitleShadow`). Close: alpha 0,
scale 110, x - 150 (0.25 s). The text clip is drawn dark (#17191C), the shadows light.

Help bar (`lib_dynamicHelpBar`, `_common.HelpBar` 330, attached at (1184, 651)): entries right
to left (`Options_main`: X `t_RestoreSettings`, B `t_Back`; `SecondaryScreen`: A `t_Validate`,
B `t_Back`; `SaveList`; `MissionsScreen`), each a `lib_<btn>_Multi` icon (frame `PC`: Enter,
Esc, Shift, Alt keys) at 90 % and the upper-cased text (`$NormalFont` 24 px); the last entry
at `-width - 10`, each earlier one 20 px further left; background L / tiled middle / R.
`OpenHelp`: from x + 20, alpha 0 (0.3 s); `CloseHelp`: alpha 0 (0.35 s).

### Options screen (`OptionsScreen` 244 and the list / widget classes)

Layout relative to `options_mc` at (640, 360): title bar (-545, -295, -4 deg); main tabs
(`lib_mTabs_tab_s`, 256 px, 206 px step, centred) at (0, -210, -4 deg), shoulders (LB / RB
icons) at -/+(W/2 + 15); sub tabs (`SubTab`, as wide as their text, 50 px apart, centred) at
(0, -150, -4 deg) on their strip; the list at (-450, -80) (-135 without sub tabs), -4 deg,
lines every 48 px (`o_options_line`, strip variant `i mod 3`), 8 visible (9 without sub tabs),
the widget at x 680; the scroll bar (`lib_scrollBar`) left of the list when it overflows.

Open: backdrop texture alpha 0 -> 100 (0.25 s), title bar, help bar `Options_main`, tabs one
every 50 ms from scale 200 to their props (0.25 s Back.easeOut; the selected one raised 20 px),
the shoulders after the last tab (alpha 0 / scale 230 -> 100, 0.25 s Back.easeOut) together
with the first list. List open (`OptionsList.Open(side)`): the list clip alpha 0 -> 100
(0.3 s), the content from x -/+ 35 (0.25 s), each line from x + 20 / alpha 0 (0.25 s), 15 ms
apart; line 0 is selected when the stagger ends. Tab change (LB / RB, Numpad 4 / 7, or a
click; wraps; `ChangeMainTab`): the old list moves -/+ 25 and fades (0.25 s), the new one
opens 150 ms later from the side the tabs moved to (forward = from the right). Sub tabs (LT /
RT, Numpad 5 / 8; `ChangeSubTab`) swap the same way; a new category's sub tabs slide in
(x + 250, 0.3 s; strip x - 250, 0.35 s). Close (B, `Back`): title close, lines fade bottom
first (20 ms apart, 0.2 s) while the content fades over 0.5 s without moving, tabs close in
reverse (alpha 0, scale 120, 0.25 s), backdrop x - 150 alpha 0, help closes; the main menu
(or the pause panel) opens at once; the screen is removed at 500 ms.

Lines and widgets: focus plays the line's `over` and moves it x 20 (0.25 s; key binding rows
0.2 s), the widget's `over` (arrows loop while focused; at an end the arrow shows `locked`).
Up / down stop at the ends (`Stop`). Steppers and booleans clamp at their ends (`Stop`; the
list creates every stepper with `isLoop` false); a stepper's text slides out (0.15 s
Strong.easeInOut, 20 px) and the new value slides in 150 ms later (0.1 s Back.easeOut) with
`Change`; booleans are two halves (Off, On) that swap `selected` / `unselected`; sliders move
the thumb (`x = -138 + ratio * 276`, 0.1 s) and play `Change` twice (key handler and
`UpdateValue`); the thumb shows the value only for integer ranges. Links swap their value
text for the action text on focus (0.25 s) and open their screen on A. Key binding: A plays
`bind_in` (the key blinks) and waits; the text does not change until the new key arrives
(`bind_out`). X (Shift): `Reset`, then `t_Q_RestoreSettings` with the category (or
"Category -> Sub-category"); Yes restores, the list fades back in with nothing selected.

Secondary screens: video settings (`o_videoSettings`): options screen to alpha 50, the panel
from alpha 0 / scale 120 (0.25 s), veil 0 -> 90 %, 3 lines (`o_options_line_s`, widget x 400),
Accept (`t_Apply`) / Cancel (`t_Cancel`), the help bar closes; Accept with changes applies them
and asks `t_Q_VideoSettings` (No / timeout restores the previous values; the timer length is
native, U-952). Brightness (`o_Options_gammaSetting`, no veil): options screen to alpha 0, the
panel from y + 150 / alpha 0 (0.25 s); A keeps, B restores the start value. Controller scheme
(`o_Options_padMapping`, frame `PC`): veil 0 -> 90 %. None of them has a close animation (the
close interval is 0.5 ms): they hide and the options screen returns to alpha 100 in 0.2 s.

### Pause menu (`PauseScreen` 149, `PauseMenu` 148, `PauseMenuButton` 137)

`PauseScreen.Open` (Esc; also on every return from a sub screen, with `OpenScreen`):

| t (ms) | what |
|---|---|
| 0 | `_lineH_mc` / `_lineV_mc` from alpha 0, x / y + 200 (0.35 s Back.easeInOut); compass scale 250 alpha 0 -> props (0.25 s) + `loop`; texture x - 200 alpha 0 (0.25 s); vignette alpha 0 scale 110 -> 100 (0.25 s), then it flickers 95 / 100 % every 100 ms |
| 30 | title (`t_Pause`, rotated -100.8 deg): bkgd from x - 320 (0.3 s), lines and shadow 0 at +150, text at +250 (shadow 1 never fades in) |
| 80 | strokes `_line0/1/2_mc` from y - 150 / + 250 / + 250 (0.25 / 0.45 / 0.25 s) |
| 150 | inputs on |
| 205 | splatters scale 120 / 130 / 180 -> props (0.25 / 0.35 / 0.4 s) |
| 220 | mask portrait scale 280, alpha 0, x - 10, y + 200 -> props (0.25 s) |
| 260 | blades fly in (`AnimatedBlades.Open`) |
| 275 | `_menu_mc` alpha 100 (0.25 s); items one every 50 ms from scale 200, x - 520 -> scale 100, x 0, alpha 100 (0.25 s Back.easeOut) |

Entries (`SetPauseMenu`): Resume, Save Game, Load Game, Options, Help, Back to Main Menu, Back
to Windows (PC). Save / Load / Help are struck through (`stop_lock`, A plays `Failure`) when not
allowed. Vertical list bottom-aligned in `_menu_mc` (48.15, 229.2; 90 %, -10 deg), items 48 px
on a 53 px pitch, strip frame `i mod 4`, clipped by `_maskMenu_mc`. Focus (up / down, loops):
x 30 (0.25 s Back.easeOut) + `over` (`over_lock`); out: x 0 (0.25 s) + `out`. Parallax with
`p = i * 100 / last`: compass rotations -10.25 i / 7.5 i, texture x - 0.3 p / y - 0.085 p,
the panel x + 0.3 p / y - 0.085 p, the portrait x - 0.3 p / y - 0.085 p / scale - 0.1 p, the
blades x - 0.08 p / y - 0.085 p / scale - 0.1 p (0.25 s). Confirm: vignette `flash`,
`Validation`, 150 ms, the panel closes and the entry runs. Esc: `back`, resume.
`Close(bBackToGame)`: lines out (0.35 s), menu alpha 0 x - 50 (0.2 s), compass / texture
x - 150 alpha 0 (0.25 s), portrait / splatters / strokes fade (0.25-0.4 s), title close; when
going back to the game also `CloseScreen`, background and vignette out; hidden at 500 ms, the
game resumes then (`OnMenuClosed`). Back to Main Menu asks `t_Q_BackToMainMenu`, Back to
Windows `t_Q_BackToWindows`.

Save / load lists (`UI_LoadGame`): rows `m_loadGameList_item` (date `$TitleFont` 23 px,
chapter `$NormalFont` 25 px) on a 71.5 px pitch, 7 visible, at (-515, -205) -2.5 deg; open:
list from x - 40 alpha 0 (0.3 s), rows from x + 20 (0.25 s), 20 ms apart; focus x 15 (0.25 s);
questions `t_Q_LoadGame` (`t_Q_LoadGameMainMenu` from the main menu), `t_Q_SaveGame`
(overwrite; a new slot saves at once).

### Journal (`UI_Journal_SF` `Journal`, `DisGFxMoviePlayerJournal`)

Port: `crates/dishonored_runtime/src/journal.rs`. Opened by `GBA_Journal` (`J`,
`Dis_ToggleJournal` 0x009EE550 -> 0x00AA3290 -> 0x00BD7940) and `DisSeqAct_OpenJournal`
(`m_Tab`, `EDisJournalTab` 0 objectives .. 4 inventory, 5 the objectives' chapter notes). The
movie's start (0x00BD11E0) sets `_root.StartTabIndex` (powers / charms fall back to 0 when
unavailable), `StartSubTabIndex`, `PowersTabAvailable`, `BoneCharmsTabAvailable`, `PlayerCoins`,
then fires the player's `DisSeqEvent_JournalViewed` (0x00BCFAC0, class from 0x00BBC040). Class
defaults: `bPauseGameWhileActive`, `m_bShowHUDWhileActive` false, tweak `Twk_UI_Journal`.

- `JournalBase` attaches `Journal` (67: `_bkgdTabs_mc` at (-492.6, -260), alpha 45 %) at
  (640, 360) over `bkgd_mc` (`p_bkgd`), plus `m_Vignette`. `JournalTabsOrder`: objectives 0,
  logs 1, powers 2, bone charms 3, inventory 4 (stats 5 unused). Tabs `lib_mTabs_tab_s` at
  (0, -260), `_offsetTabX` -60. Content clips `j_obj_mc`, `j_logs_mc`, `j_powers_mc`,
  `j_boneCharms_mc`, `j_inventory_mc`; `OpenTabContent` slides 250 px (0.25 s),
  `CloseJournal` 0.3 s tweens and `closeJournal` after 0.75 s.
- Sub tabs (`JournalTabContent_tabs`): `j_SubTabsBkgd` / `j_SubTabs` at (-250, -185), `SubTab`s
  left-aligned at -544, 25 px apart; sub-tab change hides the content in 0.15 s.
  Objectives: Tasks / Mission clues (`t_ChapterNotes`) / Mission items; logs: Written notes /
  Books / Audiographs; inventory (`j_InventoryScreen`): Resources, Key ring, Gadgets, Ammo,
  Upgrades (`req_Inventory_*`, builders 0x00BD1C70 / 0x00BD19E0 / 0x00BD1F00 / 0x00BD20C0 /
  0x00BD2340, not decoded further).
- `j_ItemsListScreen`: list at (-512, -155), details at (355, -280).
- `Req_ObjectivesList` (0x00BD79B0): an active objective is listed (inactive ones with the
  history); objective state 0/1/2 -> `not_done` / `done` / `failed`, `bIsOptional` = objective
  +0x48 bit 1; tasks listed when open, finished ones only with the history (task state 0/1
  `not_done`, 2 `done`, 3 `failed`); primary objectives first (0x00BD0A40), optional appended
  (0x00BC9630). `j_ObjectivesList`: section `t_PrimaryTasks` / `t_SecondaryTasks` whenever
  `bIsOptional` changes, tasks at `_startOriginTaskX` 55, 20 px after a section, `Y` toggles
  the history (`OnToggleObjectivesListHistory`), `X` the HUD marker.
- Logs: `Req_Logs_Notes` / `_Books` / `_Audio` (vslot 144 0x00BD99F0 -> 0x00BD15E0) list the
  held abstract items of `EDisJournalItemSection` 8 / 12 / 7 (`DJIS_Note`, `DJIS_Book`,
  `DJIS_AudioLog`); class defaults `DisAbstractItem` `DJIS_Mission`, `DisAbstractItemAudioLog`
  `DJIS_AudioLog`. Items live in `<Package>_SF.upk` (e.g. `WrittenNotes_twk_SF`), localized in
  `<Package>.int` `[<Group.Name> <Class>]`.
- PC keys (`InputsManager`): A Enter / Numpad 0, B Esc / Backspace / Numpad 1, X Shift / Numpad 2,
  Y Alt / Numpad 3.

### Mission statistics (`UI_MissionStats_SF` `MissionStats`, `DisGFxMoviePlayerMissionStats`)

Port: `crates/dishonored_runtime/src/mission_stats.rs`. Data: `DisTweaks_MissionStats`
(`Twk_MissionStats.Twk_M0_Prison` .. `Twk_M8_Lighthouse` in `DishonoredGameFull_P`): 12
`m_MissionStats` rows (Hostiles Killed `HostileKill`, Civilians Killed `CivilianKill`, Alarms
Rung, Bodies Found, Overall Chaos `ChaosLevel`, Didn't Kill Anyone `HostileKill + CivilianKill`
checkbox, Ghost `NPCsAlerted` checkbox, Runes / Bone Charms / Shrines / Paintings / Coins
found), `m_MissionStatsMaxValues`, `m_MissionSummaryItems` (story flag tests),
`m_BackgroundImagePath` (`UI_MissionStatsBg_*_SF`). Texts `Twk_UI_MissionStats`.

- `DisSeqAct_MissionStatsTracking` (0x009FE090): input 0 empties the pawn's tracked list
  (+0xB7C, 0x00AB1B00) and adds the tweak's stats with their max values (0x00AB1B50); every stat
  increment (0x00AB9560) adds to its tracked entry (0x00AA97E0); input 1 reads them (0x00AA9700),
  the chaos row 1 when the darkness level reaches `m_ChaosLevelThreshold`, the summary bits from
  the story flags, stores the record (0x009376D0, mission index `m_MissionNumber` + 9 / + 12 for
  the DLCs) and saves.
- `SetMissionStats` (vslot 120, 0x00BD3CA0) -> `_root.missionStats_mc.SetMissionStats(list,
  pickups, missionName, img, bOpenedInMenu, bReplayAvailable)`; the name is
  `DishonoredGameInfo.m_MissionNames[n]`. Rows (0x00BCBEA0): value `ROUND(v*2+0.5) >> 1`;
  `ChaosLevel` -> `m_HighChaos` / `m_LowChaos`; checkbox -> boolean; stats 0x21 .. 0x25, 0x27 go
  to the pickups list with `maxValue` and `pickUpImg` (0x00BB9BD0: coins, runes, boneCharms,
  outsider, paintings, crackedCharms); summary lines (0x00BCBC00) or `m_EmptyMissionSummaryItem`.
- Stats sources: kill 0x00B73990 (`NumKills`, then `HostileKill` / `CivilianKill` by
  `Twk_PlayerStats.m_HostileNPCInfo` / `m_CivilianNPCInfo`, 0x00ACC4B0), knock-out 0x00B738F0
  (`*PutToSleep`), bell 0x00A3D080 (`NumAlarmsTriggered`), corpse 0x00A98950, sighting
  0x00B0BE40 (`NPCsAlerted` when the player is busted), abstract pickup 0x00C4BD60 (`GoldFound` /
  `RuneFound` by quantity, `m_bCountForStats`), bone charm 0x00ABBA90 (`BoneCharmFound`).
- `ms_statsScreen` (108): `_bkgd_mc` (`MS_Background`) at (640, 360), `_title_mc` (117.3, 164.6),
  `_list_mc` (138.7, 177.25), `_pickUps_mc` (1185.95, 378.15). `MS_StatsScreen.Open`: black veil
  in 0.75 s, then `OpenElements` (`OpenScreen`, pickups from +100 / +50 in 1.25 s, background,
  list), `OnOpened` at 1.3 s. `MS_StatsList`: sections 25 px apart, rows at x 30, 4 px apart,
  a row every 40 ms. Help `MissionsStats` (X replay, A continue), `MissionsStats_replayLocked`
  (A), `MissionsStats_inMenu` (B).

## Objective popups (HUD `objectives_mc`, `targetNotification_mc`)

Decoded from the `UI_HUD_SF` "HUD" ActionScript (`swf_dump UI_HUD_SF HUD - --actions <id>`; class
registration in the export sprites' DoInitAction: `hud_objW` -> `ObjectivesNotification`,
`hud_tgt` -> `TargetNotification`) and `Twk_InGameUI.Twk_GFxMoviePlayerHUD`:

- `ObjectivesNotification.Open()`: backdrop from alpha 0, x + 250, scale 150 % to its authored
  props in 0.2 s (`Strong.easeOut`); task lines start after `_tasksApparitionDelay` 700 ms, one
  every `_taskLifeDuration_const` 3500 ms (`_utils.DelayedAnimation`), then `Close()`: backdrop
  alpha 0 / x + 100 in 0.2 s, name strip alpha 0 / x - 50 in 0.25 s, `OnClosed` hides it 500 ms
  later. Without tasks it closes after 3500 ms.
- `TargetNotification.Open()`: portrait open timeline, `OnOpened` after 1000 ms, then the
  notification's duration (`m_fTargetNotificationDuration` 5 s), `Close()`: alpha 0 in 0.25 s,
  hidden 1000 ms later.
- The native side queues popups (`m_PopupQueue`, `m_fPopupTimer`); the first waits
  `m_fPopupDelayOnGameStart` 2 s. Titles: "New Objective", "Objective Updated", "Objective
  Completed", "Objective Failed".
- No key shows them: `DefaultInput.ini` has no objectives binding; the objectives are read in
  the journal (`GBA_Journal`, J, `Dis_ToggleJournal`). `PSI_HUD_bShowObjectivePopups` turns them off.

Port: the HUD movie itself plays (`gfx_hud.rs`, `hud_widgets.rs`) and the native queue with its
merging rules is reproduced (`hud_popups.rs`); see HUD.md for the full decode.

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

### Every original setting, and what the port does with it

Columns: the original's type / range / step / default (`Default__ArkProfileSettings`), and
the port: **Applied** (where; "live" = takes effect at once from the options screen in the
launcher or the pause menu; "load" = at the next level load), **Saved** (every option is
persisted to `settings.ron` on each change, `GameSettings::commit`), or **Not honoured**
with the reason (the feature does not exist in the port yet). Nothing the port cannot do is
faked; the options screen shows every option as the original does.

| Tab / sub-tab | Id | Key | Type / values (step) | Default | Port |
|---|---|---|---|---|---|
| General / Gameplay | 104 | `PSI_Gameplay_bAutoUseManaElixir` | bool | On (`Twk_Pawn_PlayerInput.m_bAutoConsumeManaElixir=true`) | Not honoured: when the original drinks automatically is not decoded (U-954) |
| | 105 | `PSI_Gameplay_KillCamMode` | Off / Normal / Frequent | Normal | Not honoured: no kill cam |
| | 106 | `PSI_Gameplay_Difficulty` | Easy / Normal / Hard / Very Hard | -1 (asked at New Game); port: Normal | Not honoured: the sim imports the Normal column of every difficulty attribute (`DIFFICULTY_NORMAL`); a choice needs the four columns and a server-owned setting |
| | 107 | `PSI_Gameplay_bAutoSaveInMenu` | bool | On | Not honoured: no journal screen |
| | 108 | `PSI_Gameplay_HeadBobAmount` | 0.0-1.0 (0.1) | 1.0 | Applied live: weight of the `ADD_Head_*` additives (`viewmodel.rs`) |
| | 109 | `PSI_Gameplay_CameraRelativeClimbing` | bool | On | Not honoured: `ClimbConfig.camera_relative` is global sim config (needs a per-player flag) |
| General / User Interface | 87 | `PSI_HUD_Visibility` (gauges) | Off / Contextual / Always | Always | Applied live (`hud_settings.rs`) |
| | 88 | `PSI_HUD_bShowObjectivePopups` | bool | On | Applied live (`hud_popups.rs`) |
| | 89 | `PSI_HUD_bShowTutorialNotifications` | bool | On | Not honoured: tutorial messages are not implemented |
| | 90 | `PSI_HUD_bShowInteractions` | bool | On | Applied live |
| | 91 | `PSI_HUD_bShowHighlight` | bool | On | Not honoured: no object highlight |
| | 92 | `PSI_HUD_bShowPickupLog` | bool | On | Not honoured: no pickup log |
| | 93 | `PSI_HUD_bShowContextualIcons` | bool | On | Not honoured: no contextual icons |
| | 94 | `PSI_HUD_bShowPlayerStance` | bool | On | Applied live |
| | 95 | `PSI_HUD_bShowObjectiveMarkers` | bool | On | Applied live |
| | 96 | `PSI_HUD_bShowGrenadeMarkers` | bool | On | Not honoured: no grenade markers |
| | 97 | `PSI_HUD_bShowAwarenessMarkers` | bool | On | Not honoured: no awareness markers |
| | 98 | `PSI_HUD_bShowHeartTargetMarkers` | bool | On | Not honoured: no Heart |
| | 99 | `PSI_HUD_CrosshairStyle` | Off / Simple / Normal | Normal | Applied live |
| | 100 | `PSI_HUD_bCrosshairMovement` | bool | On | Not honoured yet: the crosshair dispersion exists (`gfx_hud.rs`, HUD owner) but does not read it |
| | 101 | `PSI_HUD_CrosshairOpacity` | 0-100 (10) | 100 | Applied live |
| Controls / Keyboard Mapping | 29-64 | `PSI_GBA_*` (36 actions) | key (`m_BindableKeyMapping`) | `DefaultInput.ini` `.m_PCBindings` | Applied live (`settings::Bindings`) for move, jump, sneak, sprint, lean, use, sword, left hand, block, elixirs, quicksave / quickload and shortcuts 1-0; Zoom, Wheel, Next / Previous item, Journal and the four menu keys are saved only (no such screens / actions) |
| Controls / Mouse | 67 | `PSI_Mouse_Sensitivity` | 0.1-3.0 (0.1), multiplier of `MouseSensitivity=13.0` | 1.0 | Applied live (`input_map.rs`) |
| | 68 | `PSI_Mouse_bSmooting` (sic) | bool | On | Applied live (two-frame average, APPROXIMATION) |
| | 69 | `PSI_Mouse_bInvertY` | bool | Off | Applied live |
| | 70 / 71 | `PSI_Mouse_bAutoAim` / `AutoAimStrength` | bool / 0-100 (10) | Off / 100 | Not honoured: no aim assist |
| | 72 / 73 | `PSI_Mouse_bFriction` / `FrictionStrength` | bool / 0-100 (10) | Off / 100 | Not honoured: no aim friction |
| Controls / Gamepad | 76 | `PSI_Gamepad_BindingSet` (link: `t_Modify_ControlsScheme` screen) | Type A-D (`.m_PadBindingSet1..4`) | Type A | Saved; the scheme screen shows the original picture and labels. Not honoured: the port reads no gamepad |
| | 77 | `PSI_Gamepad_bVibration` | bool | On | Not honoured: no gamepad |
| | 78 / 79 | `PSI_Gamepad_LookX/YSensitivity` | 0-100 (10) | -1 (unset); port: 50 | Not honoured: no gamepad |
| | 80 | `PSI_Gamepad_bInvertY` | bool | -1; port: Off | Not honoured: no gamepad |
| | 81 / 82 | `PSI_Gamepad_bAutoAim` / strength | bool / 0-100 (10) | -1, 100; port: On, 100 | Not honoured: no gamepad |
| | 83 / 84 | `PSI_Gamepad_bFriction` / strength | bool / 0-100 (10) | On / 100 | Not honoured: no gamepad |
| Graphics | 112 | `PSI_Graphics_Gamma` (link: `t_Modify_Gamma` screen) | 0.5-5.0 (0.1) (`m_BrightnessValues` 0.998 .. 0.80 for the logos) | 2.2 | Applied live: display gamma of the ArkPp grade (`m_GammaAdjustment / gamma`) or the colour grading |
| | 115 | `PSI_GraphicsPC_Resolution` (Video Settings) | display modes | desktop | Applied live (window) |
| | 116 | `PSI_GraphicsPC_bFullScreen` (Video Settings) | bool | On | Applied live |
| | 117 | `PSI_GraphicsPC_bVSync` (Video Settings) | bool | On | Applied live |
| | 118 | `PSI_GraphicsPC_FOV` | 65-110 (5) (`DefaultCamera.ini m_fDefaultFOV=75`) | 75 | Applied live (`viewmodel::apply_fov`) |
| | 119 | `PSI_GraphicsPC_TextureDetails` | Low / Medium / High | High | Applied at load (import cap 256 / 512 / 1024) |
| | 120 | `PSI_GraphicsPC_ModelDetails` | Normal / High | Normal | Not honoured: only LOD 0 is imported |
| | 121 | `PSI_GraphicsPC_LightShaftEnable` | bool | On | Not honoured: no light-shaft pass |
| | 122 | `PSI_GraphicsPC_AntiAliasingMode` | Off / MLAA / FXAA | MLAA | Applied live: Off, MLAA = SMAA (the MLAA derivative), FXAA; no MSAA (`DISHONORED_MSAA=1` keeps the earlier 4x MSAA) |
| | 123 | `PSI_GraphicsPC_RatShadows` | bool | Off | Not honoured: rat meshes always cast shadows (`world_models.rs` would take `NotShadowCaster`) |
| Audio | 126 | `PSI_Audio_GlobalVolume` | 0-100 (10) | 100 | Applied live (`audio.rs`, `menu_audio.rs`) |
| | 127 | `PSI_Audio_MusicVolume` | 0-100 (10) | 90 | Applied live (level score, menu music) |
| | 128 | `PSI_Audio_SFXVolume` | 0-100 (10) | 90 | Applied live (SFX, ambience, UI) |
| | 129 | `PSI_Audio_VoicesVolume` | 0-100 (10) | 100 | Applied live (`VO_` events) |
| | 130 | `PSI_Audio_SubtitlesMode` | Off / Main Dialogue / All Speeches | -1 (unset); port: Off | Not honoured yet: the dialogue HUD (`hud_dialogue.rs`, HUD owner) shows every line and does not read it |
| | 133 | `PSI_AudioPC_SpeakerConfiguration` | Auto / Stereo / 5.1 | Auto | Not honoured: the audio engine mixes stereo only |

Value changes follow the widgets: steppers, booleans (Off, On) and sliders clamp at their ends
(`Settings::step` returns false there, the menu plays `Stop`); Restore (X / Shift) resets the
current sub-tab after `t_Q_RestoreSettings`. The launcher reloads the settings file when a game
it started exits (the game's pause menu may have changed it), so its own later saves do not
overwrite those changes. `state` of the launcher control socket reports `settings` and each
option's `effects` (Applied / SavedOnly).

The original also has profile ids 1-22 (UE3 generic `OnlineProfileSettings`: vibration,
invert, voice, difficulty, ...), which its menus do not show, and 152, the DLC06 difficulty.


## The port's launcher

Implemented in `crates/dishonored_runtime/src/{launcher,launcher_bg,menu_art,menu_anim,menu_ui,menu_widgets,menu_options,menu_maps,menu_audio,pause_menu,settings,hud_settings}.rs`
and `crates/dishonored_settings`. The approach:

- Screens are rebuilt as display lists (`menu_art.rs`) from the original movies: the movie's
  sprites are walked with the frame each state label's timeline shows (played from the time
  it was sent there, with the frame scripts), named clips take the properties the classes'
  tweens give them (`menu_anim.rs`), and the movie's text fields get the localized strings with
  the field's font, size, colour, alignment and rotation. While anything moves the list is
  rebuilt every frame and the UI entities are updated in place.
- Layouts that ActionScript computes are computed the same way (button widths from the
  `$TitleFont` advances, packed and right-aligned; list pitches; tab steps; card sizes).
- Screens: start screen; main menu bar (Continue, Play, Missions, Free Roam, Load,
  Multiplayer, Options, Quit); Play on the new-game screen (free for all, contracts, spectate,
  the Playground; map and bots lines); Missions (custom missions) and Free Roam (the 42 base
  maps, `menu_maps.rs`) on the Missions card slide; Load on the save list; Multiplayer (join /
  host) in the options line style; Options; the confirmation box. Pause menu: the original's
  entries and screens (`pause_menu.rs`).
- Free roam: `dishonored_runtime --freeroam --map M [--start-event E]` runs the map offline
  with no mission of the port's own, the original Kismet of the map and the global sequence
  (started by the map's entry event; none for the ten story-redirected maps), the map's
  original NPCs and dialog. The launcher remembers the last map (`settings.launcher`), and the
  Play screen's and the server creator's maps (all but `DishonoredGameFull_P`).
- One window, as in the original: games run in a child process, but the launcher shows the
  original loading screen for the map (`loading_screen.rs`), the child opens its window
  hidden at the launcher window's place and size (`--window-pos`, `--size`), shows it once
  it renders and prints a hand-over line, and the launcher hides. Back to Main Menu prints
  the game window's place; the launcher shows its main menu there and the game exits 0.2 s
  later; Back to Windows prints `BLINK_HANDOFF_QUIT` and both quit (`handoff.rs`). A game that
  ends any other way also brings the menu back.
- Settings live in `%APPDATA%/dishonored-rs/settings.ron`. `--settings FILE` or
  `DISHONORED_RS_SETTINGS` overrides the path. The defaults are the values above.

Not reproduced (listed, not approximated): the GFx 3D properties (`_z`, `_xrotation`,
`_yrotation`; U-951); the menu level's camera matinees per screen (`TransitionTo(cameraID)`
sends `To<ID>` / `From<ID>` to the level's Kismet; the camera stays on the main pose, U-953);
the idle trailer after `m_fTrailerTimer` 30 s (U-955); the drop-down list of the resolution
line (left / right step through the modes instead); the help bar's hover underline; the
mouse-hold repeat of the widget arrows; text layout uses Bevy's shaping with the game fonts
(no Scaleform kerning, U-956); timeline masks (`clipDepth`) crop the masked quads to the mask's
bounds (exact for rectangles; text is not cropped); `setMask` masks scripted by the classes are
only kept from being drawn (clips named `*mask*`).

## Screen aspect

The menu movies are fitted whole and centred like the HUD (`gfx_stage::MovieSpace`, HUD.md
"Screen layout and aspect ratios"). The players with `m_bDrawBlackStripesWhileActive` (main
menu, pause, journal, mission statistics) show the Global movie's black stripes over the empty
space while they are up: black bars left and right of the stage at 21:9, above and below it at
5:4, none at 16:9 (`menu_art::black_stripes`; LIKELY, U-989). The classes' layouts that read
`Stage.width` (`MainMenuButtonBar`, `NewGameMenu`) keep their 16:9 values in the port (U-991).

The menus' quads are Bevy UI image nodes stretched to their stage parallelogram
(`NodeImageMode::Stretch`): in the default mode an image node keeps its texture's aspect ratio, so
a quad drawn at another aspect (a solid fill on the 1 x 1 white image, a 512 x 512 vignette over
the screen) came out as a square in the middle of its box. That was the light square on the
confirmation box (the box's mask, `boxPart2_mc` depth 2, a solid 1088 x 132 shape) and the pale
square of the stealth shroud; the box's backdrop (`_bkgdBlack_mc` / `_bkgdWhite_mc`, 1280 x 720
solids) also covered only a 720 x 720 square.

## Parity automation

`fixtures/scripts/menu_parity.json` drives the launcher through every screen, tab, sub-tab and
option and takes 221 screenshots, timed in real seconds:

```
DISHONORED_RS_SETTINGS=<scratch>/settings.ron dishonored_runtime --launcher --size 1280x720 \
    --script fixtures/scripts/menu_parity.json --out <scratch>/menu_parity --exit-after 400
```

(a scratch settings file: the script steps every option). Next to the PNGs it writes
`shots.json`: for each shot the real time, the page and the seconds since the page opened, and
the timing probes `menu_drawn_s` / `input_ready_s` (also in the control socket's `state`
`timing`; `input_ready_s - menu_drawn_s` must be 0.58 s). Script steps: `activate`, `focus`,
`step`, `back`, `button` (X / LB / RB / LT / RT), `text`, `screenshot`, `wait_frames`,
`wait_secs`, `wait_level`, `wait_input` (until the screen takes input), `quit`. The control
socket takes the same as commands (`activate`, `focus`, `step`, `back`, `button`, `text`,
`capture`, `screenshot`, `state`, `quit`). `fixtures/scripts/launcher_screens.json` is the short
version the harness launcher suite runs (17 shots).

Expected look of the shots (compare with captures of the original at the same times):

| Shots | Expected |
|---|---|
| `000_start_t0.0`, `001_start_open` | Start screen fading in over 0.45 s (logo 0.85 s, text 0.25 s); "PRESS ANY KEY" centred low. |
| `010`-`014_main_t*` | Main menu opening: 0.05 s logo shrinking from 450 % / -25 deg, splatters flying in, Corvo sliding from the right; 0.18 s band not yet visible; 0.35 s band sliding in from the right, buttons popping from 250 %; 0.58 s buttons settled, the first one at 110 % with its lines; 1.0 s all settled, blades drifting. |
| `020`-`031_main_focus_*` | The focused button at 110 % and 5 px down, top / bottom lines in, side lines out; 0.12 s mid-way (Back.easeOut overshoot); compass needles turned by -10.25 / 7.5 deg per index, Corvo and blades shifted. |
| `040_main_close_t0.15` .. `042_play_t0.7` | Main menu fading and dropping 50 px; new-game screen: buttons popping one every 50 ms, panel art at 150 ms, description at 300 ms, title bar swinging in. |
| `043_play_mode*` | The focused mode 25 px right with `over`; the portrait and description change at once. |
| `045`, `046` | Play closing (buttons shrinking one by one), main menu reopening (the 0.85 s root tween from alpha 0 / +50 px). |
| `050`-`053_missions*`, `055`-`062_freeroam*` | Card slide: the strip sliding from the last card to the first, the slide fading in over 2.25 s; the current card at 100 % raised 35 px with its strokes, neighbours at 80 / 75 / 70 %; mission pictures from `UI_Mission_Large`. |
| `070`, `071` | Multiplayer join / host lines on the options backdrop with the title and help bars. |
| `080`-`082_options*` | Options opening: backdrop fading in, tabs popping one every 50 ms, the list lines sliding in 15 ms apart, line 0 selected; help bar (Shift Restore Settings, Esc Back) at the bottom right. |
| `1xx_opt_<tab>_tabswap_t0.05`, `1xx_opt_<tab>` | Tab swap: the old list moving away and fading, the new one entering 150 ms later from the side of the move; then settled. |
| `1xx_opt_<id>_focus / _left / _right` | The focused line 20 px right with `over`, the widget focused (arrows looping); after left / right the new value (stepper text sliding in; boolean half selected; slider thumb moved) or the locked arrow at an end. |
| `opt_controls_keyboard`, `opt_bind_*` | Key list (8 lines, scroll bar); a capture shows the key blinking (`bind_in`); the list scrolled to the last binding. |
| `opt_gamepad_scheme*` | Pad picture with the scheme's labels over a 90 % veil; Type B labels after the step. |
| `opt_gamma*` | Brightness: options faded out, the five logos, title and hint, the panel rising from 150 px below; brighter after the step. |
| `opt_video*` | Video settings panel scaling in from 120 % over the options at 50 %, 3 lines, Accept / Cancel. |
| `opt_restore_question*` | Confirmation box opening (box from 50 % with Back.easeOut, text from 300 %), Yes focused. |
| `opt_close_t0.1`, `opt_closed_main` | Options closing (lines fading bottom first, tabs growing out) while the main menu reopens. |
| `quit_question*`, `quit_no_main` | Main menu closed behind the quit question; after No the menu reopens. |
| `back_to_start` | Start screen again (B on the main menu). |
