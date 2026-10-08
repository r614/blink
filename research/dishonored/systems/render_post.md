# Rendering area 3: post-process chain (WIP audit)

Status: audit started, no code changes yet. Branch has `main` and
`worktree-agent-a4c9db721993b0d16` merged (LOG.md / UNKNOWNS.md conflicts resolved by keeping
both sides).

## Class census (package_scan over CookedPCConsole, all packages, export counts)

Counts include class definitions (a count of 1 is usually only the class itself).

| Class | Exports | Notes |
|---|---|---|
| PostProcessVolume | 0 | No UE3 PostProcessVolumes in the retail maps |
| ColorScaleVolume | 1 | Likely class only; confirm |
| PostProcessChain | 6 | `Startup` `AltScreen_Effects.PostProcessChain.Test_PPG` is the game chain; other 5 to identify |
| ArkPpNodeMaterial | 21 | Screen materials in the chain(s) |
| ArkPpNodeSwitch | 13 | Bend Time / Blink / Possession / Adrenaline / KO / Lens switches |
| ArkPpNodeKuwa | 3 | Kuwahara node (U-522: is it in a level chain?) |
| ArkPpNodeAA / Dof / Blur / SceneColor / Target / CommonTarget | 2 / 2 / 2 / 2 / 2 / 4 | Chain plumbing |
| DisSeqAct_UberPostProcess | 15 | Kismet changes of the ArkPp uber settings: not ported |
| DisSeqAct_PostProcess | 10 | Kismet post changes: not ported |
| DisPostProcessManager | 5 | Post manager (PpBridge channels) |
| DisTweaks_PostProcess | 3 | Post tweaks: not read yet |
| DisBlindedPpController | 2 | Blinded (flash / sleep dart?) screen effect: not ported |
| DisOpacityParameterPpController | 2 | Opacity-driven screen material: not ported |
| DisDarkVisionPpController / MeshRenderPpController | 2 / 2 | Dark Vision eyelid + souls (ported) |
| SeqAct_SetDOFParams / SetMotionBlurParams | 1 / 1 | Probably class only |
| ArkPpSettings | 1 | Struct defaults (Engine.upk) |

## Already in the port (before this branch)

| Feature | Data source | Status |
|---|---|---|
| Level ArkPp grade (LUT: exposure, gamma, CrMgYb, desat, brightness, contrast) | WorldInfo `m_ArkDefaultPpSettings` | done (`ark_post.rs`) |
| Far DOF | same | done |
| Film grain | same | done |
| Bloom parts + tint | same + bloom-relevant materials | done (`bloom.rs`), non-generic bases U-521 |
| DisFog layers | DisFog actors | done (selection rules U-321) |
| Distortion pass | particle materials | done (`distortion.rs`) |
| Bend Time field + blur | Test_PPG, captured shaders | done (`power_pp.rs`) |
| Blink lens field | captured shaders + Twk_Blink | done |
| Dark Vision grade, eyelid, souls | Twk_DarkVision, captured shaders | done |

## Left (next steps, highest coverage first)

1. Per-map census of `DisSeqAct_UberPostProcess` / `DisSeqAct_PostProcess` (which maps, which
   fields, blend times); port as Kismet ops writing an override layer on `ArkPostSource`
   (blend like the Dark Vision overrides). Check `oracle index` for their exec functions.
2. Read `DisTweaks_PostProcess` (3) and the 5 `DisPostProcessManager` objects (channels,
   warmup / cooldown) with `tweaks` / `obj_dump`.
3. Identify the other 5 PostProcessChain objects and all 21 ArkPpNodeMaterial screen materials;
   map each switch to its gameplay trigger: Possession (`PPG_PossessionVectors`,
   `POSSESSION_IN/OUT_INST`; rat / fish possession), Adrenaline, KO, Lens, Bend Time underwater,
   `DisBlindedPpController`, `DisOpacityParameterPpController` (damage / low health candidates).
   Shaders for PPG_* materials are not in RefShaderCache: need oracle shader captures.
4. Motion blur / radial blur (global shaders 64-71) and Kuwahara (52-55): only if a chain or
   Kismet action enables them.
5. No light shafts / UE3 uber post found so far; confirm via WorldInfo `DefaultPostProcessSettings`
   and the static index before closing.
6. Settings toggles in `dishonored_settings` matching the original's video options; add views to
   `scripts/render_fixes.sh`.

The class-filtered package_scan (per-package listing for the classes above) was stopped before
finishing; rerun it (it takes > 10 min over all packages).
