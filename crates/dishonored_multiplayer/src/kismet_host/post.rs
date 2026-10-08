//! `DisSeqAct_UberPostProcess` / `DisSeqAct_PostProcess`: the Kismet screen grades.
//!
//! The original hands the action's `m_Parameters` (an `ArkUberPpParameters`: the DOF, colour
//! balance and HDR groups, each with `m_bOverride*` flags) plus `m_fWeight`, `m_fFadeInTime` and
//! `m_fFadeOutTime` to the post manager's single Kismet slot (`FUN_00be7e90` ->
//! `DisPostProcessManager.m_KismetPPParams` +0x224 / `m_KismetPPWeight` +0x220 /
//! `m_KismetPPFadeInTime` +0x21c / `m_KismetPPFadeOutTime` +0x218) and marks effect 0x12
//! (`Epp_UberKismet`) required (`FUN_00be7dc0`: `m_RequiredEffects[0x12]` +0x1b0 = 1; Stop,
//! `FUN_00be7df0`, clears it). Both entry points are 0x00b91570 (Uber) / 0x00b91450 (plain).
//!
//! The manager's per-tick state machine is `FUN_00bef9b0` (`m_EffectStates[0x12]` +0x1ce,
//! `m_KismetStateDuration` +0x214 = the timer): stopped / warming / running / cooling, weight
//! `W x min(t / fade_in, 1)` while warming, `W` while running, `W x (1 - min(t / fade_out, 1))`
//! while cooling, and a Start during a cool-down (or a Stop during a warm-up) resumes from the
//! current level. [`KismetPost::step`] is that function. The weighted override is the same as
//! the powers' (`ark_post::ArkPostSource::power_post`): fields whose override flag is set are
//! blended from the level's value to the action's.
//!
//! The defaults of the flags and fields are the `Default__DisSeqAct_UberPostProcess`
//! `m_Parameters` (DOF, colour balance and exposure / gamma overrides on; post-desaturation,
//! film grain, brightness and contrast overrides off): cooked actions omit fields that equal them.
//!
//! `DisSeqAct_PostProcess` sets `m_RequiredEffects[m_Effect]` (Start) or clears it (Stop); the
//! effects it names in the retail maps (`Epp_Knocked`, `Epp_Weepers`, `Epp_MaskOn`) are screen
//! materials of the chain whose shaders the port cannot rebuild yet, so only the state is kept.

use super::H;
use dishonored_kismet::runtime::Ctx;
use dishonored_kismet::Value;
use dishonored_mission::runtime::PostFxView;
use serde::{Deserialize, Serialize};

/// One `ArkUberPpParameters` group: (struct name, group override flag, scalar fields (name,
/// default, default override flag), vector fields (name, default override flag)).
type Group = (&'static str, &'static str, &'static [(&'static str, f32, bool)], &'static [(&'static str, bool)]);

/// `Default__DisSeqAct_UberPostProcess.m_Parameters`.
const GROUPS: &[Group] = &[
    ("m_DOFParameters", "m_bOverrideDOFParameters", &[("FocusDistance", 1000.0, true), ("InFocusRadius", 300.0, true), ("FarBlurAmount", 0.0, true)], &[]),
    (
        "m_CBParameters",
        "m_bOverrideCBParameters",
        &[("Opacity", 1.0, true), ("PreDesaturation", 0.0, true), ("PostDesaturation", 0.0, false)],
        &[("CrMgYbShadTones", true), ("CrMgYbMidTones", true), ("CrMgYbHighTones", true)],
    ),
    (
        "m_HDRParameters",
        "m_bOverrideHDRParameters",
        &[("Exposure", 0.0, true), ("GammaAdjustment", 1.0, true), ("FilmGrainNoise", 0.0, false), ("GimpBrightness", 0.0, false), ("GimpContrast", 0.0, false)],
        &[],
    ),
];

/// The overriding fields of an `ArkUberPpParameters` value: ArkPp scalar and vector settings by
/// their `m_*` name (the keys `ark_post` reads).
pub fn overrides(params: Option<&Value>) -> (Vec<(String, f32)>, Vec<(String, [f32; 3])>) {
    let mut values = Vec::new();
    let mut vectors = Vec::new();
    let flag = |v: Option<&Value>, name: &str, default: bool| v.and_then(|v| v.field(name)).and_then(Value::as_bool).unwrap_or(default);
    for (group, group_flag, scalars, vecs) in GROUPS {
        if !flag(params, group_flag, true) {
            continue;
        }
        let g = params.and_then(|p| p.field(group));
        for (name, default, on) in *scalars {
            if flag(g, &format!("m_bOverride{name}"), *on) {
                let v = g.and_then(|g| g.field(&format!("m_{name}"))).and_then(Value::as_float).unwrap_or(*default);
                values.push((format!("m_{name}"), v));
            }
        }
        for (name, on) in *vecs {
            if flag(g, &format!("m_bOverride{name}"), *on) {
                let v = match g.and_then(|g| g.field(&format!("m_{name}"))) {
                    Some(Value::Vector(v)) => *v,
                    _ => [0.0; 3],
                };
                vectors.push((format!("m_{name}"), v));
            }
        }
    }
    (values, vectors)
}

/// The post manager's Kismet slot.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct KismetPost {
    pub values: Vec<(String, f32)>,
    pub vectors: Vec<(String, [f32; 3])>,
    /// `m_RequiredEffects[Epp_UberKismet]`.
    pub required: bool,
    /// `m_EffectStates[Epp_UberKismet]`: 0 stopped, 1 warming, 2 running, 3 cooling.
    pub state: u8,
    /// `m_KismetStateDuration`.
    pub timer: f32,
    pub fade_in: f32,
    pub fade_out: f32,
    /// `m_KismetPPWeight`.
    pub weight_max: f32,
    /// The weight `FUN_00bef9b0` last wrote.
    pub weight: f32,
}

impl KismetPost {
    /// `FUN_00bef9b0`: advance the state machine by `dt`.
    pub fn step(&mut self, dt: f32) {
        if self.state == 0 && !self.required {
            return;
        }
        self.timer += dt;
        let t = self.timer;
        let (fin, fout, w) = (self.fade_in, self.fade_out, self.weight_max);
        let tiny = |x: f32| x.abs() < 1e-8;
        if self.required || self.state == 0 {
            if !self.required {
                return;
            }
            match self.state {
                0 => {
                    if tiny(fin) {
                        self.state = 2;
                        self.weight = w;
                    } else {
                        self.state = 1;
                        self.timer = 0.0;
                    }
                }
                1 => {
                    if t < fin {
                        self.weight = w * (t / fin).min(1.0);
                    } else {
                        self.state = 2;
                        self.weight = w;
                    }
                }
                3 => {
                    // Started again while cooling: warm up from the current level.
                    if !tiny(fin) {
                        let u = (t / fout).min(1.0);
                        self.state = 1;
                        let level = (1.0 - u) * w;
                        self.timer = if w != 0.0 { level / w * fin } else { 0.0 };
                        self.weight = level;
                    } else {
                        self.state = 2;
                        self.weight = w;
                    }
                }
                _ => self.weight = w,
            }
        } else {
            match self.state {
                2 => {
                    if tiny(fout) {
                        self.state = 0;
                    } else {
                        self.state = 3;
                        self.timer = 0.0;
                        self.weight = w;
                    }
                }
                1 => {
                    // Stopped while warming: cool down from the current level.
                    if !tiny(fout) {
                        let u = (t / fin).min(1.0);
                        self.state = 3;
                        self.timer = (1.0 - u) * fout;
                        self.weight = u * w;
                    } else {
                        self.state = 0;
                    }
                }
                _ => {
                    if fout <= t {
                        self.state = 0;
                    } else {
                        self.weight = (1.0 - (t / fout).min(1.0)) * w;
                    }
                }
            }
        }
    }

    /// The client's view while the effect is on.
    pub fn view(&self) -> Option<PostFxView> {
        (self.state != 0 && self.weight > 0.0).then(|| PostFxView { values: self.values.clone(), vectors: self.vectors.clone(), weight: self.weight })
    }
}

/// `DisSeqAct_UberPostProcess::Activated` (0x00b91570): input 0 Start, input 1 Stop.
pub(super) fn uber(h: &mut H, cx: &Ctx) {
    if cx.input() == Some(1) {
        h.st.post.required = false;
        return;
    }
    let (values, vectors) = overrides(cx.prop("m_Parameters").as_ref());
    let p = &mut h.st.post;
    p.values = values;
    p.vectors = vectors;
    p.weight_max = cx.prop_float("m_fWeight");
    p.fade_in = cx.prop_float("m_fFadeInTime");
    p.fade_out = cx.prop_float("m_fFadeOutTime");
    p.required = true;
}

/// `DisSeqAct_PostProcess::Activated` (0x00b91450): Start requires `m_Effect`, Stop clears it.
/// `Epp_Knocked` (0x0b) also sets the knock-out timer (`m_KnockOutTimer` +0x208 = time + 1),
/// and every effect but `Epp_FakeBlink` (0x14) starts in the running state at once
/// (`FUN_00be7dc0` with state 0); `Epp_FakeBlink` warms up.
pub(super) fn effect(h: &mut H, cx: &Ctx) {
    let name = cx.prop_str("m_Effect");
    if cx.input() == Some(1) {
        h.st.post_effects.remove(&name);
    } else {
        h.st.post_effects.insert(name);
    }
}
