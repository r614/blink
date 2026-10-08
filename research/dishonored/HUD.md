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
| `UI_MissionStats.MissionStats` (+ `UI_MissionStatsBg_*`) | `UI_MissionStats_SF` | `DisGFxMoviePlayerMissionStats` | not yet |
| `UI_Journal` | `UI_Journal_SF` | journal player | not yet |
| `UI_Tuto_*` | `UI_Tuto_*_SF` | tutorial windows | not yet |

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
  upgd2 (small: a charm raised the maximum), upgd4 (large: Vitality), upgd6 (both).
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
- Content (0x00BD5530): the inventory's weapons with each known ammo type, the Heart, the
  learned powers; `itemQuantity` the ammo count ("--" with infinite ammo).
- Tick (0x00BD5250 with 0x00BCD660): the mouse delta accumulates into `m_MousePosition`,
  clamped to `m_fMouseZoneSize` 0.1 x the viewport size; a frame whose delta exceeds
  `m_fMouseDeadZone` 0.01 of the zone sends the position / zone; beyond `m_fStickDeadZone` 0.4
  its angle (acos, clockwise from up, 360 - a on the left) goes to `SetSelectionAngle`.
- `onSelectionChanged(item)` (0x00BCD800) keeps `m_CurItemIdx` and answers `UpdateItemInfos`
  (0x00BC3B00 weapons, 0x00BC3940 powers, 0x00BC3E30 Heart); closing (`CloseWheel`,
  0x00BBD1D0) keeps the item: it is the one equipped.
- While open the world runs at `m_fBendTimeFactor` 0.05.
- Sounds: `m_pShowSound` Snd_UI_Ingame_Wheel_Hand_In, `m_pHideSound` ..._Hand_Out, the theme's
  itemSelection -> Snd_UI_Ingame_Wheel_Select, itemValidation -> ..._Validation (bank
  `Bank_UI_Ingame_Wheel`), P_AssignShortcut / P_UnassignShortcut (Bank_UI).

Port: `hud_wheel.rs`; the wheel takes the mouse and the number keys while open
(`LocalInput.wheel_open`), the player stands still (the world is not slowed, U-881); the dock's
table drives the number keys (`LocalInput.shortcuts`), starting from the port's default list.

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

## Awareness markers (decoded, not drawn yet)

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

Open for the port: the manager's anchor point and projection for these markers, the allowed
awareness list, and the attention level reaching the HUD (the session view carries no per-NPC
attention; an offline mission could read its brains) (U-886).
## HUD options

`hud_settings.rs` maps the `PSI_HUD_*` options (`dishonored_settings::Hud`): popups, pickup log,
interactions, stance icon, gauge visibility (always / contextual / never), crosshair style and
opacity. The subtitles option is not applied (the port cannot tell unset from off).

## Harness scripts

`fixtures/scripts/hud_playground.json` (playground) and `hud_boyle.json` (`missions/
boyle_original.ron`) use `{"hud": {...}}` steps (`HudTest`): objective, target, pickup,
message, subtitle, choice, interaction, stripes, location, wheel, wheel_angle, shortcut,
saving; screenshots at open / hold / close / gone.

## Not done yet

Awareness markers (decoded above, not drawn), the death screen (pause menu movie,
owned by the menus work), the mission stats screen, the journal, the tutorial windows and rune
or bone-charm messages, the low-health / low-mana post-process, the crosshair focus name, the
task marker's description and distance texts. See research/UNKNOWNS.md U-880..U-887.
