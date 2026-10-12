# The in-game HUD, popups and wheel

How the original draws and times the in-game HUD, decoded from the movies' ActionScript 2
(`swf_dump <package> <movie> - --actions <class sprite>`; the root timelines' DoAction with a
DefineFunction2-aware disassembler) and from `Dishonored.exe` (`oracle index fn/class/xref`).
Everything below is in the port's own words with addresses and constants; no decompiled code
is kept in the repository.

Port: the movies play on a small GFx stage (`crates/dishonored_runtime/src/gfx_stage.rs`), the
classes are rewritten against it (`hud_widgets.rs`, `hud_wheel.rs`, `hud_global.rs`) and the
native drivers follow the exe (`gfx_hud.rs`, `hud_popups.rs`, `hud_dialogue.rs`).

## Movies

| Movie | Package | Driver (native class) | Port |
|---|---|---|---|
| `UI_HUD.HUD` (1280x720, 30 fps) + `Common_assets.lib` (Startup) | `UI_HUD_SF` | `DisGFxMoviePlayerHUD` (tick 0x00BB2280) | `gfx_hud.rs`, `hud_widgets.rs` |
| `UI_HUDFX.HUDFX` | `DishonoredGame` | `DisGFxMoviePlayerHUDFX` | `hud_widgets.rs` (HUDFX section), drawn under the HUD |
| `UI_PowerWheel.powerwheel` + the lib | `UI_PowerWheel_SF` | `DisGFxMoviePlayerPowerWheel` | `hud_wheel.rs`, drawn above the HUD |
| `UI_Global.Global` | `DishonoredGame` | `DisGFxMoviePlayerGlobal` | `hud_global.rs` (saving icon only) |
| `UI_MissionStats.MissionStats` (+ `UI_MissionStatsBg_*`) | `UI_MissionStats_SF` | `DisGFxMoviePlayerMissionStats` | not yet (classes decoded, "Mission stats" below) |
| `UI_Journal` | `UI_Journal_SF` | journal player | not yet |
| `tutorialWindow_mc` / `tutorialMsg_mc` (HUD movie) | `UI_HUD_SF` | `DisGFxMoviePlayerHUD` (0x00BAD2F0 / 0x00BAD060) | `hud_widgets.rs` (`TutorialWindow`, `TutorialMessage`), `gfx_hud.rs` |
| `UI_Tuto_*` (the Help list of the pause menu) | `UI_Tuto_*_SF` | tutorial notes | not yet |

## The stage

What the classes need from Scaleform and how the port plays it:

- Per-instance timelines with frame scripts (`stop`, `play`, `gotoAndStop/Play` by label or
  number, `_visible`), display-list reconcile per frame, `clipDepth` masks and `setMask`
  (rectangular crop), properties set by script detach the clip from its timeline.
- `tweenTo` / `tweenFrom` / `tweenEnd(jump)` (`gfx.motion.Tween`: one tween per clip, time
  based, applied once per movie frame; `onTweenComplete`), intervals (`setInterval`, one-shot in
  every class that uses them), `attachMovie` from the movie or the imported library,
  `createEmptyMovieClip`, `removeMovieClip`.
- Easing (`mx.transitions.easing`): Strong (quintic), Back (s = 1.70158, in-out s x 1.525),
  Regular (quadratic), None.
- Text fields measured with the `DisFonts_SF` advances (1024 EM): `autoSize`, `wordWrap`,
  letter spacing, `textWidth`.
- `_common.ImgLoader`: the image centred on the clip, resized (by maxH / h) only when
  `bDisableAutoFit === false`; external textures `img://Package.Texture` (cooked groups found
  in Startup / UI_HUD_SF).
- `_rotation` reads back normalised to -180..180 (rotation tweens to 360 loop cleanly).
- Not reproduced: filters, blend modes, gradients' exact ramps, skew, the 3D properties
  (`_xrotation`, `_yrotation`, `_z`), BitmapData noise.

Drawn through a pool of Bevy UI nodes (image + text per slot), "show all" scaling of the
1280x720 stage. The movie clip table of the HUD is indexed `(offset - 8) / 16`
(`EDisHUDMovieClip`).

## Screen layout and aspect ratios (0x00BA5840, 0x00B96250)

The movie space (`DisMovieSpaceInfo`, `m_MovieSpaceInfoOnLastPreRender` at +0x1D8; computed by
0x00B87860 for the other players and inline by the HUD's 0x00BA5840 when the viewport size
changes, `m_ViewportSizeX/Y` at +0x204): the 1280x720 stage is fitted whole and centred. A
viewport wider than 16:9 keeps the stage height (720) and shows `emptySpaceX = (720 x w / h -
1280) / 2` stage pixels on each side (21:9, 2560x1080: 213.3); a narrower one keeps the width
and shows `emptySpaceY` above and below (5:4, 1280x1024: 152). The empty space is part of the
movie (not clipped). `Stage.width` / `Stage.height` read the visible extent (1706.7 x 720 at
21:9, 1280 x 1024 at 5:4): `_utils.ScreenPosition.ComputeMovieSpaceInfo` derives the empty
space from them and `GetMovieSpaceCoord(ratio)` returns the safe area as `coordMin = -empty +
(scene + empty) x (1 - ratio) / 2`, `coordMax = scene + empty - (same)` (the class takes the
margin on the stage plus one side's empty space; LIKELY that `Stage.width` is the visible
extent: the native layout and the classes are only consistent with it, U-993). A viewport of
aspect 3.65 or more (`m_bTripleScreenMode`, +0x1FC bit 21) gives the HUD the middle third and
computes the full-screen values separately for the HUDFX clips; not reproduced.

The clip table (0x00B95650: `_root.<name>` of the HUD movie, `blackShroud_mc`,
`directionalDamages_mc[._vignette_mc.U/D/L/R_mc]`, `keyHole_mc` and the two
`_black_rectangle*_mc` of the HUDFX movie) maps to `EDisHUDMovieClip` (DishonoredGame.upk):
0 `_dot_mc`, 1 (the aspect slot), 2 `crosshairInfosTxt_mc`, 3 `masterHUD_mc`, 4
`playerStatesIcon_mc`, 5 `blackShroud_mc`, 6 `interactions_window_mc`, 7 `interactions_txt_mc`,
8 `interactions_ic_mc`, 9 `interactions_icQTE_mc`, 10 `oxygenGauge_mc`, 11
`interactionGauge_mc`, 12 `grenadeCooking_mc`, 13 `pickupLog_mc`, 14 `gameMsg_mc`, 15
`tutorialMsg_mc`, 16 `tutorialWindow_mc`, 17 `location_mc`, 18 `objectives_mc`, 19
`targetNotification_mc`, 20 `directionalDamages_mc`, 21..24 its vignette's U / D / L / R, 25
`keyHole_mc`, 26 / 27 the black rectangles, 28 `blackStripes_mc`, 29 `subtitles_mc`, 30
`playerChoice_mc`, 31 `skipCutScene_mc`. The init also keeps `m_ContextInteractionsOffset`
(`interactions_txt_mc` - `masterHUD_mc`), `m_PickupLogOffset` (`pickupLog_mc` - `objectives_mc`)
and `m_fLocationDiscoveryVerticalAlignment` (`location_mc._y / 720`), all on the authored
movie.

The layout 0x00B96250 (every viewport change), with the safe area `m_fSafeAreaRatio_PC` 0.9
(tweaks offset 0xBC, the class default of `DisTweaks_GFxMoviePlayerHUD`; the console value is
0.85) on the whole visible extent: `left = -emptySpaceX + W x 0.05`, `top = -emptySpaceY + H x
0.05`, `right = W - emptySpaceX - W x 0.05`, `bottom = H - emptySpaceY - H x 0.05` (W, H the
visible extent):

| Clip | Position / size |
|---|---|
| `masterHUD_mc` | (left, top) |
| `playerStatesIcon_mc` | (left, bottom) |
| `interactions_txt_mc` | (left, top) + `m_ContextInteractionsOffset` |
| `interactions_ic_mc` | (right, bottom) |
| `skipCutScene_mc` | (left, 720 - `_stripeH`) |
| `pickupLog_mc` | (right - 54, top) + `m_PickupLogOffset` |
| `gameMsg_mc`, `tutorialMsg_mc`, `subtitles_mc` | (640, 720 - `_stripeH`) |
| `playerChoice_mc` | y 720 - `_stripeH` |
| `tutorialWindow_mc` | (left + 8, bottom) |
| `location_mc` | (640, alignment x H - emptySpaceY) (`Location` puts `_y` back to its authored value on each `SetLocation`) |
| `objectives_mc`, `targetNotification_mc` | (right, top) |
| `blackShroud_mc` (HUDFX) | `_width` W, `_height` H |
| vignette `U_mc` / `D_mc` | (0, -/+ (360 + emptySpaceY)), `_width` max(W, 1280) |
| vignette `L_mc` / `R_mc` | (-/+ (640 + emptySpaceX), 0), `_height` max(H, 1280) |
| `_black_rectangle0/1_mc` | sides: (-emptySpaceX, 0) / (1280, 0), emptySpaceX x 720; top / bottom: (0, -emptySpaceY) / (0, 720), 1280 x emptySpaceY; 16:9: (-2, -2), 1 x 1 |
| `blackStripes_mc._stripeUp_mc` / `_stripeDown_mc` | x -emptySpaceX |

`_stripeH` is read from the movie (70). The stripes' class (`BlackStripes.SetStripesPosition`,
also its `Stage.onResize` listener) makes both bars `Stage.width` wide and `70 +
emptySpaceY` high, the upper one at the top of the visible extent, the lower one at 650: the
cinematic bars are the HUD movie's own clips, drawn by the movie over the whole screen width
(not native letterboxing). The movie is authored for the console safe area (0.85: the gauges at
(95.5, 54), the objectives at (1184, 54)); at 16:9 the PC layout puts them at (64, 36) and
(1216, 36), and the bottom-centre texts at 650 instead of 665. `DirectionalDamages.TakeDamage`
gets W and H as its screen size (0x00B970F0), so the hits reach the screen's edges.

Other movies: the wheel's PC dock is attached at (`Stage.width / 2`, `Stage.height`) when the
movie starts (right of the stage centre by `emptySpaceX` at 21:9; its `_y` is set again on
every `Open` to `GetMovieSpaceCoord(1).coordMax._y`), its elixir counters at
`GetMovieSpaceCoord(0.9).coordMax`, the saving icon in game at `GetMovieSpaceCoord(0.85).coordMax`
(in a menu at the same point of the bare stage). The menu players with
`m_bDrawBlackStripesWhileActive` (class defaults: main menu, pause menu, journal, mission
statistics, note, store) show the Global movie's `g_BlackStripes` (`SetStripes`, on
`Stage.onResize`; switched by the Global player's tick 0x00BAA610): black fills drawn by script
over the empty space, `emptySpaceX x Stage.height` at `coordMin._x` and 1280 on a wide screen,
`Stage.width x emptySpaceY` at `coordMin._y` and 720 on a narrow one, none at 16:9; the menus
are pillarboxed / letterboxed. Which flag of the Global player (+0x1B8 bit 0) the menus set is
not traced (LIKELY the flag above, U-994).

Port: `gfx_stage::MovieSpace` (each stage's `space`, from the window), `HudWidgets::layout` /
`HudFxWidgets::layout` run by `gfx_hud::update_gfx_hud` on a window size change, the stripes'
`set_stripes_position`, the wheel's and the saving icon's `GetMovieSpaceCoord`, and
`menu_art::black_stripes` over the layers marked `DrawsBlackStripes` (launcher, pause menu,
journal, mission statistics). The task / awareness markers' area (`m_fMarkerAreaRatio_PC` 0.9)
is still taken on the stage, not the visible extent (U-995).

## Popups and the native queue (`objectives_mc`, `targetNotification_mc`)

`ObjectivesNotification` (hud_objW): `Open()` brings the backdrop in from alpha 0, x + 250,
scale 150 % to its saved props in 0.2 s (Strong out); the name strip and state icon follow;
task lines appear after `_tasksApparitionDelay` 700 ms, one every `_taskLifeDuration_const`
3500 ms (`_utils.DelayedAnimation`); `Close()` fades the backdrop (alpha 0, x + 100, 0.2 s)
and the strip (alpha 0, x - 50, 0.25 s); hidden 500 ms later (`_opened` false). Without tasks
it closes after 3500 ms. Sounds (theme `DisUISoundTheme_0` of `Twk_GFxMoviePlayerHUD`):
obj_windowOpen -> UI_Objective_Window, obj_taskAdded / Updated -> UI_Objective_New_Task,
obj_taskCompleted -> UI_Objective_Task_Succes, obj_taskFailed -> UI_Objective_Task_Fail. These
events are in `Bank_UI_Objective` (their `RequiredBank`), not `Bank_UI`.

`TargetNotification` (hud_tgt): the title's letters 25 ms apart (`Back.easeInOut` 0.45 s each),
the portrait open, `OnOpened` after 1 s, then `m_fTargetNotificationDuration` 5 s, `Close()`
alpha 0 in 0.25 s, hidden 1 s later. Sounds H_targetAssassinated / Neutralized / Rescued.

Native queue (`DisGFxMoviePlayerHUD`):

- Tick 0x00BAFE70: `m_fPopupTimer` starts at `m_fPopupDelayOnGameStart` 2 s; a popup is shown
  when the timer is strictly below 0 and the active clip's `_opened` is false; the queue is
  emptied when the popups option is off. Paused (pause menu, loading) stops the timer.
- Enqueue rules: objective added 0x00BB2480; task added 0x00BB27C0 (merged into a queued
  Added / Updated popup of the same objective); task updated 0x00BB28E0 (merged into Updated);
  task completed 0x00BB2A00 (into Updated / Completed); task failed 0x00BB2B20 (into Updated /
  Failed; dropped when the objective's Completed is queued); objective completed / failed
  0x00BB2500 / 0x00BB2660 remove queued Updated popups that carry tasks; merging replaces the
  same task (0x00B9EE60).
- `SetObjectives` 0x00B98A40: tasks as "description (progress)"; a failed objective passes no
  tasks. `SetNotification` 0x00B990D0: the chapter target's `m_TargetName`, portrait
  "img://" + `m_TargetPortraitPath`, type = state + 1.

Port: `hud_popups.rs` (`PopupQueue`) observes the mission view and replaces the old
restart-on-reopen banner.

## Player status (`masterHUD_mc`, class PlayerGauges)

- `FillHealthGauge(h x 100 / max, small, large, h <= 30, (h + regen) x 100 / max)` and
  `FillManaGauge(m x 100 / max, upgraded, available, hidden, m <= 19, regen)` (0x00B9F6E0 /
  0x00B97D40 / 0x00B97F80; `m_LowHealthThreshold` 30, `m_LowManaThreshold` 19), sent only on
  change. The fill mask's `_yscale` tweens to the value in `_meterFillDuration` 0.45 s
  (Strong out), the indicator to the top of the fill (hidden at 100). Upgrade frames: default,
  upgd2 (small), upgd4 (large: Vitality), upgd6 (both). The bits (0x00B9F6E0, `FUN_00C7E1C0`
  asks an attribute whether it carries a modifier of a given name): small = `Attribute_HealthMax`
  has a "Whale Bone Charm Attribute_HealthMax" modifier, large = a "Vitality 1 (0)" or "Vitality 2
  (0)" one; the mana gauge's upgrade bit is the same for `Attribute_ManaMax` and "Whale Bone Charm
  Attribute_ManaMax". The port asks `dishonored_sim::powers::charm_modifies` (U-887, resolved).
- Glow loop: 2.25 s between alpha 15 and 40; low: 0.55 s between 35 and 75.
- Contextual visibility (`FUN_009EA160`): shown `m_fPlayerInfoDisplayDuration` 8 s after a
  change and while low; `Open()` / `Close()` 0.35 s alpha tweens; gated until the HUD ticked
  once.
- Elixir drink: `refill_mc` plays "anim", sound H_quickPotion.
- Equipment (0x00B981F0): `SetWeapon(item, loaded + reserve, loaded)` / `SetPower(item, active,
  enoughMana)` / `SetEmptyEquipment()`; icons from `DisGlobalUIManager.m_EquipmentIcons`
  (DefaultUI.ini) into `ic._imgLoader_mc`; the pistol's loaded rounds as
  `itemIcons_bulletCount` frame loaded + 1.

## Crosshair (`_dot_mc` and the attached `_aspect_mc`)

0x00B9FF30: `m_CrosshairSettings[EDisPowerWheelItem]` (DefaultUI.ini) gives the symbol, the dot
and the dispersion use; interaction crosshairs from `m_InteractionCrosshairSettings`; the dot
state (0 default, 1 empty, 2 over); `SetDispersion` when it moved by 0.1 or more; the
crosshair slides to the focused interactable at `m_fCrosshairInterpolationSpeed` 4.5. Gun and
crossbow symbols spread their parts by the decoded formulas (`Crosshair_Gun`,
`Crosshair_Crossbow`).

## Interactions, messages, subtitles, choices

- `interactions_window_mc.SetInteractions(name, lines)` (0x00BA6150): the interaction texts
  joined with newlines; key tags converted by 0x00BA4620 to "[key]" (mouse buttons are
  `$LMouseButton$` icons in the original; the port writes their names).
- The lines are `Twk_GFxMoviePlayerHUD.m_InteractionTexts[EDisUIInteraction]` (enum
  `DisGFxMoviePlayerHUD.EDisUIInteraction`, groups `EDisUIInteractionGroup` Crosshair / Context /
  Special) or the focused interactable's own `m_InteractText`. `DUI_Special_Choke` (3) is
  "`GBA_Block` [Hold]", the choke's line; `DUI_Crosshair_Carry` (32) "Hold `GBA_Use` Carry",
  `DUI_Crosshair_Pickup` (31) "`GBA_Use` Loot". The port shows the target the host's crosshair
  rule chose (`MatchView::focus`, `dishonored_sim::crosshair`) with its prompt, then the choke
  line when `crosshair::choke_target` holds (`hud_popups::focused_interaction`). Which line the
  original shows for a body or a world pickup (the HUD's text or the pickup's interactable) is
  LIKELY the HUD's.
- `gameMsg_mc.SetGameMessage(msg, m_fGameMessageDuration 5 s)` (DisSeqAct_ShowHUDMessage).
- `AddLogItem` 0x00B98630: "name (count)" when count != 1, only with the pickup log option;
  `PickupLog`: 3 slots, fade in / out 0.4 s, 2 s life, item steps y 0 / -50 / -100 / -150 at
  alpha 100 / 60 / 20 / 0 in 0.25 s, extra items queued.
- `Subtitles`, `CrosshairInfosText`, `TutorialMessage`: 0.25 s fades.
- `PlayerChoice` (a `_common.GenericMenu`): buttons open 50 ms apart, `BTN_OVER` (alpha 100,
  x 15, scale 110) / `BTN_OUT` (85, 0, 100), confirm after 150 ms.
- Stance icon (0x00B95ED0): "sneak" / "slide".
- `location_mc.SetLocation(name, m_fLocationDiscoveryDuration 5 s)` (0x00B95160,
  DisSeqAct_ShowLocationDiscovery; the native also posts `Musics.Discovery`): big first and
  last letters, a mask widening from 10 px over 0.95 s, the middle letters 15 ms apart, 0.6 s
  fade at the end. The port routes the Kismet action as `Action::Location` /
  `MissionView.location`.
- Cinematic stripes (`blackStripes_mc.SetBlackStripes`, `_stripeH` 70).

## HUDFX

- `BlackShroud`: open while the player sneaks (0x00B96050 -> 0x00B8D490), 0.35 s fades.
- `DirectionalDamages.TakeDamage(angle, clamp(damage / m_fMaxDamage 70) x 100, w, h)`
  (0x00B970F0): a pooled `dirHitSprite` (10) and the edge arrow placed by
  `GetClippedPosition` (vertical branch: y = cos < 0 ? -ratio : ratio; horizontal: x = sin < 0 ?
  -ratio : ratio; ratios 1 and 0.7), the hit fading over 1.2 s after max(35 i, 1500) ms, the
  red vignette + (0.01 i^2 + 0.4 i - 1) in 0.15 s, held max(40 i, 2500) ms, 3.5 s out.

## Power wheel (`UI_PowerWheel_SF`)

Classes: `_powerWheel.WheelHandler` (`wheel_mc`), `PowerWheelBase` (attaches the PC dock
`shPC_` as `shortcutsPC_mc` at (Stage.width / 2, Stage.height)), `PC_ShortcutsHandler` over
`ShortcutsHandler`, `PC_PotionsCount` over `PotionsCount`, `_utils.MovieClipCreator`,
`_utils.ScreenPosition` (`GetMovieSpaceCoord(ratio)`: the safe area, no empty space at 16:9).

Items (`_powerWheel.WheelItems`, `EDisPowerWheelItem`): regularBolt 1, sleepDart 2, flareBolt 3,
heart 4, springRazor 5, regularGrenade 6, stickyGrenade 7, regularBullet 8, explosiveBullet 9,
firstPowerIndex 20, darkVision 21, windBlast 22, devouringSwarm 23, blink 24, bendTime 25,
possession 26. Schema frames: crossbow, heart, spring_razor, regular_grenade, sticky_grenade,
gun, outsider. Display order: weapons flare, bolt, sleep, heart, razor, grenade, sticky,
bullet, explosive; powers windBlast, darkVision, blink, swarm, bendTime, possession.

Layout (`FillWheelIcons`): item range 360 / n; the weapons span `n_w x 360 / n` centred on the
top (`ComputeWeaponAngle`: idx / n_w x range_w + range / 2 - range_w / 2), the powers the rest
in reverse order (`ComputePowerAngle`: range_w / 2 + range_p - (idx / n_p x range_p +
range / 2)). Items at scale 75, placed at sin / -cos x (diameter / 2 - width / 2), diameter =
`wheel_circle._width`; the ammo count beside the item; empty items "stop_empty"; powers
"stop_pow". Separators every 360 / n at alpha 10, frames 1..7 in turn.

`Open()`: items `tweenFrom` x 0, y 0, alpha 0 (0.35 s Back out); the wheel from alpha 0,
rotation -20, scale 155 % (0.25 s Strong); white stroke from rotation -50 / 150 % (0.8 s),
black stroke 160 / 250 % (0.9 s), indicator stroke 90 / 450 % (0.6 s), background -60 / 15 %
(0.6 s); separators `tweenFrom` rotation 0 to -120 (0.45 s); the circle from 20 % / alpha 0
(0.45 s). `Close()`: alpha 0 in 0.25 s, then `OnClosed` (icons removed, native told).

Selection (`SetWheelSelectionAngle(angle)`): on the first one the schema and the infos fade in
over 0.85 s (Regular out) and the indicator over 0.35 s; the black stroke turns by the total
angle / 80, the white stroke by -total / 20, the indicator stroke by total / 5, the indicator
to the angle; the current item stays selected within its half range + 6 degrees; otherwise
the item under the angle (weapons: (angle + range_w / 2) mod 360 within range_w; powers:
angle - (180 - range_p / 2) within range_p, reversed). `setItemFocus`: the old item out
(`out` / `out_pow` / `out_empty`, glow "out", scale 75 in 0.25 s Back), the new one in
(`over*`, glow "over", scale 105 in 0.35 s Strong), `onSelectionChanged(item)`, sound
itemSelection. `UpdateItemInfos(name, ammo)`: name upper case; "no_ammo" frame when neither
an ammo type nor a count; else "ammo (count)".

Background (root timeline): `OpenBkgd` fades the glass in (0.25 s) and the white layer to 10;
the glass pieces fly in from 800-1200 px out, 30-80 degrees turned, alpha 50 (`randRange`
1-1.2 s, Strong); the two glass layers drift forever (layer 0: x 100-120 alpha 50 in 5 s, x
-20-0 alpha 100 in 3 s; layer 1: x -100-100 alpha 80 in 6 s, x -20-0 alpha 100 in 6 s); the
separators' pieces drift (y -50..-25, rotation -5..5 in 5 s; y -5..0, rotation -4.5..3.5 in
2.5 s). `randRange(a, b)` = floor(random x (b - a + 1)) + a.

PC dock (`PC_ShortcutsHandler`): ten `shPC_item` slots (`MovieClipCreator.CreateElements`,
centre origins, `_offsetOriginY` -25, `_offsetX` -5), key labels 1..9, 0; scale 90 %. In the
wheel (`Open(true)`): from y + 40 to the bottom edge, alpha 100 (0.2 s Back), background 100,
slots to scale 100 / 110 (selected) in 0.25 s, the elixir counters open. Alone (`Open()`,
number keys): y + 10, background 30, the pressed slot selected; `Close()` fades the other
slots (0.25 s), after 200 ms the selected slot plays "select" and rises 20 px, 350 ms later the
dock fades and drops (0.2 s) and is hidden. Assigning (a number key while the wheel is open):
the selection goes to the slot (sound P_AssignShortcut) or leaves it when the slot already holds
it (P_UnassignShortcut); assigned slots tween from 110 % to 100 %, the icon to 47.5 %; empty
slots to alpha 55 / 95 %, the icon to 0 / 60 %.

Elixir counters (`PC_PotionsCount`): at `GetMovieSpaceCoord(0.9).coordMax` (1216, 684); open
slides health (0.45 s) and mana (0.55 s) in from 150 px right; counts with the key labels from
`FormatText` of `GBA_HealthElixir` / `GBA_ManaElixir` ("[KEY]" with the brackets dropped);
an empty count locks the icon and the key to x 0.4 colour and alpha; drinking plays
`stroke_mc` "consume".

Native (`DisGFxMoviePlayerPowerWheel`, `Twk_GFxMoviePlayerPowerWheel`):

- Bindings: `GBA_Wheel` = MiddleMouseButton, `Dis_Wheel_Open FALSE | OnRelease
  Dis_Wheel_Close FALSE` (controller 0x00AA07A0 / 0x00AA07F0); `GBA_Shortcut_n` =
  `Dis_WheelShortcuts_KeyboardSelect n | OnRelease Dis_WheelShortcuts_KeyboardUse n`
  (0x00AA0900 / 0x00AA0950); mouse wheel `Dis_WheelShortcuts_MouseNext / Previous`.
- Start (0x00BE4280): mode 1 the wheel (0x00BE25C0: `SetPotionsCount` 0x00BC3FE0 with the
  inventory's elixir counts, the shortcut layout `SetShortcuts` 0x00BE1310 with unavailable
  items cleared, then `OpenWheel(content)`), mode 2 the dock alone (0x00BE2BB0), mode 4 the
  quick potion. `m_fWheelInputDelay` 0.25 s applies to the gamepad only; the first mouse input
  after opening resets the cursor.
- Content (0x00BD5530): for each item of the inventory that is a weapon (item type not 0 / 1, and
  `FUN_00C0C680(2)` or type 0x0D / 0x10 / 0x13 / 0x19), every ammo type in the weapon's ammo list
  that `FUN_00C1C9D0` accepts (all but type 1, explosive bullets, which needs a non-zero
  `Attribute_ExplosiveBulletUnlock`, and type 7, sticky grenades, `Attribute_StickyGrenadeUnlock`;
  an ammo type with a zero count is still listed and shows "stop_empty"), then the learned powers
  (the second list); `itemQuantity` the ammo count ("--" with infinite ammo). Nothing is gated on
  "ammo held once" (U-883, resolved; the port has no weapon items, so the Spring Razor and grenade
  groups appear once one was held).
- Tick (0x00BD5250 with 0x00BCD660): the mouse delta accumulates into `m_MousePosition`,
  clamped to `m_fMouseZoneSize` 0.1 x the viewport size; a frame whose delta exceeds
  `m_fMouseDeadZone` 0.01 of the zone sends the position / zone; beyond `m_fStickDeadZone` 0.4
  its angle (acos, clockwise from up, 360 - a on the left) goes to `SetSelectionAngle`.
- `onSelectionChanged(item)` (0x00BCD800) keeps `m_CurItemIdx` and answers `UpdateItemInfos`:
  a power 0x00BC3B00 (name `[DisUISelectionType] <Name>`, text `<Name>_<level>_ShortDescription`
  with the level of the power, e.g. "Short Teleport"; nothing for a power not learned; 0x00C4C6B0 /
  0x00C45930), the Heart 0x00BC3E30 (`Heart_ShortDescription`), a weapon 0x00BC3940 (name, then the
  ammo name of `[DisAmmoType]`, or for the Spring Razor `SpringRazor_ShortDescription`,
  "Proximity Mine"; "no_ammo" when it has neither). U-885, resolved. Closing (`CloseWheel`,
  0x00BBD1D0) keeps the item: it is the one equipped.
- While open the world runs at `m_fBendTimeFactor` 0.05 (see "World time" below).

Shortcuts (controller tick 0x00AB7D30, `m_fShortcutsInputTimer` 0.33 s; U-882):

- A number key `KeyboardSelect n` (0x00AA0900) sets `+0x648` / `+0x64C`; the tick shows the dock
  alone (start mode 2, `OpenShortcuts` after `SetShortcuts`) with slot n selected
  (`SetShortcutSelection`); `KeyboardUse n` (0x00AA0950, on release) makes the slot's item the
  current one (0x00BBD090: `m_CurItemIdx`, `m_LastUsedShortcutIndex` = n), closes the dock with
  `use` and equips it (0x00BCD400). Pressing a number key with the wheel open assigns the current
  item to the slot (0x00BC3400: the slot takes it, or empties when it already holds it; the slot's
  bit in `m_ShortcutPlayerAssignFlags` is set; every other slot holding the item is emptied).
- The mouse wheel (`GBA_NextItem` = MouseScrollUp, `GBA_PrevItem` = MouseScrollDown, 0x00AA0A10,
  not while a key is held or within `m_fShortcutsInputTimer` of the wheel closing): each step
  counts (`+0x654`) and restarts the timer (`+0x658`); the first step brings up the dock with the
  selection `count` filled slots from the last used one (0x00BBD0F0 skips the empty slots,
  wraps), the others move it (0x00BC36C0); when the timer runs out the shown slot becomes current
  (0x00BBD180) and is equipped as the dock closes. Ignored while the wheel is open.
- `SetShortcuts` (0x00BE1310, on every start): slots whose item is not in the wheel content are
  emptied, then the auto-assignment 0x00BDC610 fills the free slots that the player did not
  assign or empty himself: slots from the highest `m_AutoAssign_KeyboardShortcut[i].m_Priority`
  down (10 .. 1 for slots 0 .. 9), each taking the first item of `m_AutoAssign_KeyboardItemOrder`
  (class default: regular bullet, bolt, grenade, spring razor, Heart, Blink, Dark Vision,
  Windblast, Swarm, Bend Time, Possession, sleep dart, flare bolt, sticky grenade, explosive
  bullet, then the DLC06 items) that is available, not on a slot yet and of the kind the slot
  accepts (`m_bWeapon` slots 0 .. 4, `m_bHeart` slot 5, `m_bPower` slots 6 .. 9; the tweak
  `Twk_GFxMoviePlayerPowerWheel`). With everything available keys 1 .. 0 hold pistol, crossbow,
  grenade, Spring Razor, sleep dart, Heart, Blink, Dark Vision, Windblast, Swarm.

World time (U-881, resolved): start mode 1 (0x00BE4280) calls the dilation helper 0x009FA0D0
with the wheel's channel (6) and `m_fBendTimeFactor` for the caster, the world and the third rate
(`m_fTimeTransition` has no value: the rate changes at once); 0x00BD4FC0 sets 1 back. Port: the
client sets `buttons::WHEEL` in the player's input while the wheel is open, and
`dishonored_sim::step_world` keeps one `DilationLayer` (`wheel: true`) per human at 0.05 (the
Bend Time clock is not slowed by it); the simulation is deterministic and the input travels with
the tick like any button. Only a world with a single human pawn honours it (the original needs the
GameInfo, present on the server / standalone): in multiplayer with two or more humans the button
does nothing, so one player cannot slow the others (a dedicated server with one connected human
slows for him, like a listen server host).
- Sounds: `m_pShowSound` Snd_UI_Ingame_Wheel_Hand_In, `m_pHideSound` ..._Hand_Out, the theme's
  itemSelection -> Snd_UI_Ingame_Wheel_Select, itemValidation -> ..._Validation (bank
  `Bank_UI_Ingame_Wheel`), P_AssignShortcut / P_UnassignShortcut (Bank_UI).

Port: `hud_wheel.rs`; the wheel takes the mouse and the number keys while open
(`LocalInput.wheel_open`, which also slows the world); the shortcut table (`ShortcutTable`) is
filled by `SetShortcuts` / the auto-assignment, the number keys and the mouse wheel equip through
the dock (`input_map.rs` no longer equips on a key press); the content comes from the exe's rule
(`hud_wheel::wheel_content`). Not reproduced: the gamepad wheel and d-pad shortcuts, the Heart
(the sim has none), DLC06 items.

## Saving notification (`UI_Global.Global`)

`savingNotificationIc_mc` (`g_SavingNotification_Icon` over `g_SavingNotification`) at
`GetMovieSpaceCoord(0.85).coordMax` (1184, 666): `ShowSavingNotification` attaches a fresh
`gl_sIcon_ic` at (-50, -45) with every piece hidden; `Open`: the outer circle from alpha 0 /
150 % (0.25 s Back), the clockwork from 0 / 130 % (0.3 s Strong), the blades slide in 20 px
(0.25 s), the gears from 20-25 px away (0.3 s), the inner circles 50 ms later (from 20 % and
120 %, 0.3 s Back); the gears start turning after 0.3 s (clockwork 10 s a turn, gears 2.5 s,
10 s, 3.5 s the other way), the circles after another 0.3 s (inner 10 s the other way, middle
10 s, outer 35 s). `Close`: the clip fades over 0.45 s, the clockwork to 120 % (0.5 s), the
inner circle to 120 % (0.3 s), the outer to 60 % turned 10 degrees back (0.25 s).

Native (0x00A17890 engine tick, 0x00BAA390 / 0x00BAA3D0 / 0x00BAA610): a game save sets
`m_GameSaveState` 2 and `m_fGameSaveTimer` = `m_fMinSaveUIDuration` 1.0 s and shows the icon
(`ShowSavingNotification`, the checkpoint text closed); when it ends the state is 1 until the
timer runs out, then the icon closes (unless a profile save is showing it). The checkpoint
text (`g_SavingNotification_Text`, "t_Checkpoint" sliding 75 px in 0.3 s) is never opened by
these paths.

## Awareness markers

Symbol `awarenessIndicator` (`m_AwarenessMarkerSettings`, in `markers_mc`): two halves
`left_mc` / `right_mc` (labels default, fadeIn, visible, quickFadeOut), each with three chevrons
`mc1..mc3` (labels default, fillIn, emptyOut, full, empty, busted_1..3); frame 2 of the symbol
is the off-screen look. Settings (Twk_GFxMoviePlayerHUD over the class defaults): scale
variation end value 75, alpha 100 at 200 uu down to 0 at 150 uu; `DisAwarenessMarkerSettings`
`m_fDisplay_PlayerProxyMaxDist` 200, `m_fDisplay_NPCMaxDist_Vision` 10000,
`m_fDisplay_NPCMaxDist_NoVision` 10000, `m_fFadeOutDelay_InitiallyHidden` 0,
`m_fFadeOutDelay_NowHidden` 1.0, `m_fFadeOutAnimDuration` 3.0, `m_fMinScaleOutsideFOV` 50,
`m_fMaxScaleOutsideFOV` 150, `m_AllowedAIAwareness` (six entries). The detection stinger
`Musics.Music_Player_Detected` plays at `DAL_TurnToFace`.

Marker class (vtable at file offset 0xD61FC0: init 0x00BC5B40, release 0x00BC5C10, tick
0x00BB9FF0, update 0x00BBD630, chevrons 0x00BBA030; base placement 0x00BBD430):

- Level = the NPC's attention level toward the player (`EDisAttentionLevel` 0 Unaware,
  1 HeadTrack, 2 TurnToFace, 3 Investigate, 4 Busted).
- "Seen": the NPC rendered within the last 0.2 s (or a second visibility test). Seen: the
  display timer is reset to `m_fFadeOutDelay_NowHidden`; not seen: a rise of the level sets it
  to `m_fFadeOutDelay_InitiallyHidden`. Both timers count down every frame (0x00BB9FF0).
- Shown while the level is 1..3, the display timer is positive and the pawn is not flagged out:
  on first show both halves play "fadeIn" and the chevrons below the level are set "full" (the
  others "empty"); a level change animates the changed chevrons ("fillIn" / "emptyOut").
- Level 4 once: both halves "visible", the chevrons "busted_1..3".
- Otherwise: a shown marker plays "quickFadeOut" and the fade timer starts at
  `m_fFadeOutAnimDuration`; the clip is released when that timer is below 0.
- Placement: shown at the projected point (`_x`, `_y`, visibility only on change); scale
  500 / max(tan(fov / 2) x distance, 1) x the scale variation, alpha from the alpha variation;
  off screen the symbol shows frame 2, turns toward the NPC (angle from the screen centre + 90,
  mirrored on the left) and its scale is clamped to 50..150.

Port (`gfx_hud.rs` `update_awareness`, `hud_widgets.rs` `AwarenessMarker`): the match view carries
`marks` (each NPC's attention entry toward a human, its `EAIAwareness` and the proxy point the
NPC believes the player is at), the marker hangs on the `head_jnt` position the renderer reports
(`EffectSockets.heads`), the allowed list is `awareness::ALLOWED`, and the detection stinger plays
as a marker with TurnToFace or more comes up. Compared with dishonored-bevy's `awareness.rs`
(Eamon): same symbol, halves, chevron labels and 75 % scale / 150-200 uu alpha, but its levels are
read from its own NPC state, it marks hostile NPCs only, and it fades by its own timers; the
exe's state machine above (seen test, display timers, busted look, off-screen frame) is kept.
What the original's `DisSeqAct_OverrideAwarenessDisplay` does for scripted markers is not
modelled (bevy shows them for the NPCs the scripts name). U-886: the manager's anchor and the
allowed list are decoded in the port; the exact visibility test (`FUN_00B4A300`) is approximated
by a line of sight to the head.
## Task markers

`m_TaskMarkers` (0x00BAD550 builds one per active task target of the player; the ctor 0x00BCE750
sets the `_description_mc.txt` field once to the target's localised `m_TargetName`):
`m_TaskMarkerSettings` / `m_OptionalTaskMarkerSettings` (the secondary symbol for an optional
task): `m_fWorldOffsetZ` 10 uu, scale 90 % at 10000 uu growing to 150 % at 500 uu, alpha 100 from
1000 uu down to 0 at the target, `m_bCanFocus`, `m_fMarkerFocusDistance` 225 stage px: the marker
nearest the middle within that distance is focused and shows its description. No
`m_bKeepBBInMarkerArea`: a marker outside the marker area (`m_fMarkerAreaRatio_PC` 0.9) or
behind the camera is not drawn (the Heart's rune / bone charm markers and the grenade indicator
are kept at the edge). `m_TaskMarkerSpecificSettings.m_bShowDistance` is true and
`m_VanishPreset_VeryClose` (alpha 100 from 400 uu to 0) / `_Never` are chosen by
`DisSeqAct_UpdateTaskTarget.m_MarkerVanishPreset`; the exe has no distance format string next
to the marker code (U-964), so the port follows dishonored-bevy's `markers.rs` (Eamon) for the
text: the target name, then the distance in metres (100 uu): "Name (12 m)", or "12 m" without a
name. The marker list is the Kismet's `UpdateTaskTarget` targets with their `m_TargetName`
(`MissionView.task_markers`); the vanish preset is not applied (the one alpha variation above is).

## Tutorials

- `TutorialMessage` (`tutorialMsg_mc`): `DisSeqAct_SetTutorialMessage` (`m_TutorialMessage` for
  `m_fDuration` seconds, else `m_fTutorialDuration` 10 s; `m_bShowOnce` / `m_bAllowCycling` /
  `m_fTutorialCycleDelay` 5 s are not modelled) reaches the view as `MissionView.tutorial`.
  The pawn tweaks' own messages (`m_AcquiredElixirTutorialMessage`, `m_HeartTargetTutorialMessage`,
  `m_BlockTutorialMessage` ...) come from the HUD's tutorial stack (0x00BAFA60, `m_TutorialStack`),
  not decoded (U-965); bevy raises the Heart one and the elixir ones from its own events.
- `TutorialWindow` (`tutorialWindow_mc`, class sprite 812; 0x00BAD2F0 -> `SetTutorialWindow(text,
  image)`): in from the left at 250 % (0.2 s `Back.easeOut`), the picture (200 %), strokes and text
  settle 150 ms later, then the parts drift (`Anim` 5 s, `AnimLoop` 4.25 s, each re-armed 0.5 s
  early); the native side keeps it `m_fTutorialWindowDuration` 10 s, then `Close()` (0.4 s) and
  `OnClosed`; windows queue one behind the other. Pictures: 0 gear, 1 note, 2 rune, 3 bone charm.
  Texts (HUD tweak): `m_PressKeyToBuyPowers`, `m_PressKeyToEquipBoneCharms` for the first rune /
  bone charm, `m_PressKeyToReadChapterNoteMessage` when the Kismet adds a chapter log item
  (`DisSeqAct_AddAbstractItem`) - the triggers are bevy's (`interact.rs`, `campaign.rs`), the
  native's tests (`m_NumRunesPickedUp`, `m_bHasNewBoneCharm`) are not traced. Sound
  `H_windowNotification`. Option `PSI_HUD_bShowTutorialNotifications` empties the queue.

## Crosshair texts

- The interaction window's title is the interactable's name (`m_InteractableName`): a talker's
  `NpcDef.name` (`MissionView.npc_names`).
- `crosshairInfosTxt_mc` over someone asleep: the pawn tweak's `m_AsleepCrosshairText`
  ("(Unconscious)", its value in the retail pawn tweaks per dishonored-bevy `crosshair.rs`); the
  probe (`DisUIInteractionProbe`) is approximated by the view ray passing within 45 uu of the
  body's chest within 300 uu (U-966).

## Game over (pause movie)

`PauseScreen.ShowGameOverMenu(reason)` (0x00BCCD80): the pause movie with `_bIsGameOverMenu`:
title `t_GameOver`, `_gameOver_mc` (class `GameOverTitle`, the reason text in from 150 px right,
the backing from 350 px left, 0.25 s `Strong.easeOut`), the mask portrait plays `anim_slice`,
sound `GO_OpenGameOverMenu` (not `OpenScreen`); entries `t_ContinueFromLastSave` (locked without a
save; `Dis_Load 0`), `t_LoadASpecificSave`, `t_BackToMainMenu`, `t_BackToWindows`; no resume. The
reason is `m_GameOverMessage` or the pause tweak's `m_PlayerDiedMessage` ("You have met your
demise."). Port: `pause_menu.rs` (`PauseMenu.game_over`) shows it when the local player is dead in
an offline mission that does not respawn him, `GAME_OVER_DELAY` (2 s) after the death (the native
trigger and delay are not traced, U-967).

## Mission stats (decoded, not drawn)

`UI_MissionStats.MissionStats` classes (`swf_dump UI_MissionStats_SF MissionStats - --actions N`):
`MissionStatsBase` (137: attaches `ms_statsScreen` `missionStats_mc`, `ms_BlackVeil`,
`m_Vignette`), `MS_StatsScreen` (150: `SetMissionStats(list, pickUps, name, image, inMenu,
replay)`; opens the black veil 0.75 s, `OpenElements` plays `OpenScreen`, the pick-ups panel
(1.25 s from +100, +50, `_yrotation` -10), the background and the list; `OnOpened` at 1.3 s starts
the title / list / pick-ups drift 5.25 s / 4.25 s; A -> `OnContinueClicked` + `Close`, X ->
`OnReplayMissionClicked` after `t_Q_ReplayMission`, B in the menu), `MS_StatsList` (139: sections
`ms_statList_section`, rows `ms_statList_item` 4 px apart, sections 25 px, rows open 40 ms apart
from x + 20, 150 %, alpha 0 over 0.25 s; boolean values show `checkBox_mc` frame 2),
`MS_PickUpsList` (138: `t_Found`, lines "name value / max", `ic_mc` frame), `MS_Background` (148:
Corvo / image / blades / left / right panels in over 0.9 s, drift 6.25 s / 5.05 s),
`MS_StatsScreenTitle` over `_common.TitleBar` (147 / 145), `_common.AnimatedBackground` /
`AnimatedBlades` / `Vignette` (143 / 144 / 151). The rows are the `DisTweaks_MissionStats` of the
mission (`m_MissionStats` rows with `m_Description`, `m_Stat` (`ePlayerStat_*`), `m_bCheckbox`,
`m_bCheckIfNonZero`, `m_bAddOtherStat`, `m_MissionStatsMaxValues`, `m_MissionSummaryItems` with a
story flag); the port's `ShowMissionStats` op fires "Closed" at once (the server would have to
hold it until the screen is closed) and keeps only a few of the `ePlayerStat` counters, so the
screen is not drawn (U-968). dishonored-bevy's `mission.rs` draws the same screen by hand from the
same rows (Eamon).

## HUD options

`hud_settings.rs` maps the `PSI_HUD_*` options (`dishonored_settings::Hud`): popups, pickup log,
interactions, stance icon, gauge visibility (always / contextual / never), crosshair style and
opacity. The subtitles option is not applied (the port cannot tell unset from off).

## Harness scripts

`fixtures/scripts/hud_playground.json` (playground) and `hud_boyle.json` (`missions/
boyle_original.ron`) use `{"hud": {...}}` steps (`HudTest`): objective, target, pickup,
message, subtitle, choice, interaction, stripes, location, wheel, wheel_angle, shortcut,
saving; screenshots at open / hold / close / gone. `hud_wheel2.json` covers the wheel's content
and auto-assigned dock, assign / unassign (`assign`), the number key (`shortcut`) and the mouse
wheel (`shortcut_step`); `hud_screens.json` the tutorial window and line (`tutorial_window`,
`tutorial`), task markers placed ahead of the player (`task_markers`), the interaction title and
the game over screen (`game_over`). The scripts have not been run in this branch (no run was
allowed); the awareness markers and the world slow-down have `cargo test` coverage in the sim
(`wheel_slows_the_world_in_a_single_human_world_only`).

## Not done yet

The mission stats screen (decoded above; needs the server to hold the op), the journal (an
interactive screen over notes, powers and charms: needs the notes and the rune spending in the
sim), the pause menu's tutorial notes (`UI_Tuto_*`), the Heart's wheel item and markers, the
low-health / low-mana post-process, the vanish presets of task markers, the pawn tweaks'
tutorial messages. See research/UNKNOWNS.md U-880..U-887 and U-964..U-968.
