# Rain (`DisSeqAct_SetRainEmitter`, the camera's rain box)

Port: `crates/dishonored_runtime/src/rain.rs` (camera tick and module simulation),
`crates/dishonored_assets/src/particles.rs` (module import), `dishonored_ir::particles::{RainDrops,
RainImpacts, FloatDist, ParticleParam}`.

## Kismet -> camera

`DisSeqAct_SetRainEmitter::Activated` 0x00BA1770 -> 0x00B9ADF0: the first `Target` that is an
`Emitter` whose template's `Emitters[0].LODLevels[0].Modules[0]` is a `DisParticleModuleRainDrops`
gives `m_Extent` (+0x48) to `DishonoredPlayerCamera::SetRainEmitter` 0x00AC0570:

- a different previous `m_pRainBoxEmitter` (+0x4DC) is deactivated (PSC `SetActive(false)`,
  0x008A9880(0));
- `m_RainBoxExtent` (+0x4E0) = extent, `m_pRainBoxEmitter` = the emitter, activated (0x008A9880(1));
- `m_NumRainDrops` (+0x508), `m_fRainImpactsMinDist` / `MaxDist` (+0x50C / +0x510) from the action;
- with a rotation actor: `m_RainDirection` (+0x514) = -(X axis of its rotation), the emitter's
  `Rotation` = the actor's; without: (0, 0, -1) and the default rotator DAT_014353FC (BSS, read as
  the zero rotator).

## Camera tick (end of 0x00AD80A0, camera mode 1)

With E = the rain emitter, V = view direction, C = POV location:

1. `Vb = V` transformed by E's matrix (vtable +0x1BC, 0x004C6CD0: rotation, scale, location; the
   normal transform, local -> world, as compiled); `t = min_i Extent[i] / |Vb[i]|` (0 -> 0x7F7FC99E);
   `E.SetLocation(C + V t)` (0x006539C0).
2. When E has a PSC: covered when the view target pawn has `m_pActiveWaterVolume` (+0x4E4); else
   `P = C + (appFrand*70-35, appFrand*70-35, 0)` and `SingleLineCheck(End = P - Dir*5000, Start = P,
   0x2086)`; uncovered = it returned TRUE (nothing hit).
3. `m_bWasUncovered` (+0x4C0 bit 4) unchanged: `m_fRainSpawnKillRate` (+0x520) < 10000 ->
   `DeltaSeconds * rate * 100` (WorldInfo.DeltaSeconds, 0x00781240); changed: store it, rate = 30.
   `SetFloatParameter("SpawnKillRate", rate)`, `SetFloatParameter("MaxParticles", uncovered ?
   (float)m_NumRainDrops : 0)` (0x008AA690).
4. Impacts: `m_RainImpacts.Num = 0`; with `m_NumRequestedRainImpacts` (+0x500) > 0 and min < max,
   that many tries with appSRand (LCG DAT_0141B1F8 * 0xBB38435 + 0x3619636B, mantissa trick):
   `d = lerp(min, max, r)`, `half = d tan(FOV * pi / 360)`, `x = lerp(-half, half, r)`,
   `y = lerp(-half/aspect, half/aspect, r)`, `W = C + X d + Y x + Z y` (0x00402BC0 POV frame);
   trace 0x00BE1560 (actor, hit, Start = W - Dir*5000, End = W + Dir*5000, 0x2209F); a hit with
   `Dir . N < 0` appends {location, Dir - 2 (Dir . N) N}. Then (always) LastRequested (+0x4FC) =
   requested, requested = 0, Available (+0x504) = impacts, CurrentIndex (+0x4F8) = 0.

## `DisParticleModuleRainDrops`

Fields `m_Extent` +0x48, `m_MaxParticles` +0x54, `m_SpawnKillRate` +0x70, `m_StartVelocity` +0x8C,
`m_StartColor` +0xA8, `m_StartAlpha` +0xC4, `m_StartSize` +0xE0, `m_FadingRate` +0xFC; class
defaults `Default__DisParticleModuleRainDrops` (DishonoredGame): MaxParticles / SpawnKillRate are
`DistributionFloatParticleParameter`s named `MaxParticles` / `SpawnKillRate`, `DPM_Direct`,
constants 15 / 1; FadingRate 8. The level templates (`Streets1_FX.Effects.Soft_Rain`,
`Lighthouse_FX.Effect.Storm_Rain`) serialize only `Constant` on their parameter subobjects: name
and mode come from the archetype.

- Distribution values: every call is `GetValue(EmitterTime +0xE4, Component +8)`. A
  `DistributionFloatParticleParameter` (0x005E1FA0): the PSC's instance parameter (vslot 184) or
  `Constant`; `DPM_Direct` returns it, `DPM_Abs` takes |v|, else clamp to [MinInput, MaxInput] and
  map onto [MinOutput, MaxOutput] (slope 0 if MinInput >= MaxInput). Vector random tables
  (0x0042ACA0) draw one appFrand per axis unless the table's `Type` locks axes.
- Spawn (slot 73, 0x00C00700): 0x00C00470 in the whole box (-Extent .. Extent), RelativeTime 0.
- 0x00C00470 (drop placement): `Location += lerp(min, max, appFrand)` per axis; Velocity =
  BaseVelocity = `m_StartVelocity` transformed by the component's LocalToWorld (normal);
  Color/BaseColor rgb = `m_StartColor`, both alphas = `m_StartAlpha`; Size = BaseSize =
  `m_StartSize`; RelativeTime = the argument; OneOverMaxLifetime = 0 (drops never die by age).
- GetSpawnAmount (slot 101, 0x00C01370): `need = ((int)MaxParticles - Active - OldLeftover) / dt`,
  `cap = SpawnModule.Rate * max(SpawnModule.RateScale, 0)`, Rate = min(need, cap), Number =
  (int)(Rate dt + OldLeftover), returns FALSE (no regular spawn rate).
- Update (slot 74, 0x00C00790): average Velocity of the live drops (BaseColor.A > 0); shift
  `S = (Location - OldLocation) - avg dt` (emitter instance +0x2C / +0xE8); per axis cut `c = S >= 0
  ? E - S : -E - S`; 8 boxes split at c (bit k of the index = upper part on axis k), volumes, the box
  of index (S.x<0) + 2(S.y<0) + 4(S.z<0) (still covered) weighted 0, CDF over the rest.
  `max = (int)MaxParticles`; budget = live != max ? appCeil(SpawnKillRate * DeltaSeconds) : 0;
  fade = FadingRate * DeltaSeconds. Per particle (last to first): RelativeTime > 0: += fade, >= 1
  -> 0 and Color.A = BaseColor.A, else Color.A = BaseColor.A * rt; < 0: -= fade, <= -1 -> dead
  (Color.A = BaseColor.A = 0, rt = 0, Velocity = BaseVelocity = 0), else Color.A = (rt + 1)
  BaseColor.A. Then (component WorldToLocal, the 0x00402550 inverse) a live drop outside the open
  box starts fading out (rt > 0 -> -rt; rt == 0 -> -fade), one inside while fading out fades back
  in (-rt). A dead drop, while budget > 0 and live < max: slab = first CDF entry >= appSRand,
  Location = emitter Location, 0x00C00470 in that slab with RelativeTime = fade, budget--,
  OldLocation = Location.
- slot 82 (0x00C012E0) is the editor's box.

## `DisParticleModuleRainImpacts`

`m_bFixedLocation` +0x48 (default true); per-instance data 4 bytes (vslot 77 -> 4).

- GetSpawnAmount (0x00C01470, with the camera from 0x00B515C0): rate = max(RateScale, 0) * Rate;
  `acc = frac(A) + rate dt` (A the instance float, truncation), the camera's Requested += (int)acc;
  with Available > 0: Number = appRound(Available / LastRequested * (int)A_old), Available -=
  Number, LastRequested -= (int)A_old, Rate = (Number - OldLeftover) / dt (so exactly Number
  spawn); else Number = 0. A = acc.
- Spawn (0x00C04450): the camera's impact `m_RainImpacts[CurrentIndex++]`: Location = its
  position; with `m_bFixedLocation` BaseVelocity and Velocity += its direction, Flags |= 4.
  (Without a camera, an editor preview position.)
- FinalUpdate (0x00C01670): OldLocation = Location - Velocity (the velocity-aligned sprite stands
  along the reflected rain).

## Templates

`Streets1_FX.Effects.Soft_Rain` (L_Distillery_P, L_Ovrsr_P, L_Ovrsr_Back_P, L_Streets1_P) and
`Lighthouse_FX.Effect.Storm_Rain` (L_LightH_HighChaos_FX `Emitter_8`): two drop emitters
(`DisParticleModuleRainDrops` first, Extent 500, `rain_droplets_inst`, `PSA_Velocity`,
`ParticleModuleSizeMultiplyVelocity` on Y only: 0.02 / 0.015 x |v| makes the streak) and one
impact emitter (`DisParticleModuleRainImpacts`, Spawn Rate 30, Lifetime 0.25-0.35, Size 15-20,
`rain_ground_droplet`).

## Port notes

- The port moves the level's auto-activated instance of the placed rain `Emitter`
  (`effects::PlacedEmitter`), spawning one only if none lives. The emitter's rotation is
  `RainView::rotation` (the rotation actor's rotator), else the zero rotator, times the placed
  actor's scale.
- Instance parameters: `FxInstance::set_float_param` (UE3 `SetFloatParameter`); the drop module's
  `FloatDist`s read them through `ParticleParam::value` (0x005E1FA0).
- Drops are the port's `Particle`s with life = infinity plus `rain::Drop` (RelativeTime,
  BaseColor.A, BaseColor, BaseSize); they bypass `present_rules::MAX_PARTICLES`.
- `ParticleModuleSizeMultiplyVelocity` is applied in `effects::particle_look` for every emitter
  (it was an unsupported module before). Velocity-aligned and rectangle sprites now take their
  height from Size.Y (only `PSA_Square` draws Size.X by Size.X).
- Deactivation is the port's `FxInstance::stop`; the port's one-shot rule then removes the instance
  at once (its emitter time is past `ONE_SHOT_LIMIT`). The original keeps ticking a deactivated
  PSC; whether its drop module still respawns drops there is not decoded.
- LIKELY: the impacts module's spawn flag (Flags |= 4 with `m_bFixedLocation`) keeps those
  particles in place; the port does not move them.
- Open: water (`m_pActiveWaterVolume`) is read as swimming / eye under water; the traces use the
  PAWN channel without the complex-collision and `TRACE_Others` bits of 0x2209F / 0x2086;
  `PostSpawn`'s emitter-motion interpolation is not applied (only `Location += SpawnTime x
  Velocity`); the camera-mode test (mode 1) is not applied; `appFrand` / `appSRand` streams are
  the port's own.
- Importer: an omitted `LookupTable` is taken from the archetype only for the rain modules
  (`dist_ex`). UE3 serializes struct members against the archetype everywhere, so the general
  importer differs from the engine when a template leaves a table at its default (for example
  `AlphaOverLife` = 1, Spawn `Rate` = 20).
