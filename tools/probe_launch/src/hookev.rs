//! Trace events from native entry hooks (the paths that do not pass through ProcessEvent):
//!
//! - `Sound`: every Wwise event post, 0x009B2BE0 (thiscall on the audio device, `(UAkEvent*,
//!   AActor* source, ...)`; `Actor.PostAkEvent`'s exec 0x005C1050 reaches it through 0x006CD730,
//!   native callers directly). Object = the source actor, name = the AkEvent (`Package.Event`).
//! - `Effect`: every `UParticleSystemComponent::ActivateSystem` (0x008B1F80, thiscall on the
//!   component; the pooled effects of `EmitterPool.SpawnEmitter` too). Object = the
//!   component's Owner (+0x48), name = its Template (+0x1C4), detail `comp=0x...`.
//! - `Noise`: every AI noise the noise manager dispatches, 0x00C5A760 (thiscall, `(event*)`:
//!   location +0x14, loudness `EAINoiseLoudness` +0x20, context `EAINoiseContext` +0x21,
//!   source +0x24, damping +0x38). Object = the source, name = the loudness, detail has the
//!   context, location and damping.
//!
//! Each record carries the world time; the tick is the first frame whose `world.time` reaches it.
use crate::session::Session;
use anyhow::Result;
use dishonored_oracle::trace::TraceEvent;
use dishonored_probe::HookDeref;
use std::collections::HashMap;

/// The inner post every Wwise event goes through (0x009B2BE0 and the by-id / by-name /
/// positional variants 0x009B2C40, 0x009B2E40, 0x009B3090, 0x009B31B0, 0x009B3440 all call it;
/// footsteps and material contacts post through the variants): `(u32 akID, float*, AActor*
/// source, ...)`; the event is named from the AkEvent objects' `m_akID` (+0x40).
#[allow(dead_code)]
pub const FN_AK_POST: (u32, u8) = (0x009b_2150, 6);
/// The Wwise SDK entries themselves (cdecl `(id | wchar_t* | char*, AkGameObjectID, flags, ...)`):
/// every post, including the ones that bypass the engine wrapper (footsteps, material
/// contacts).
pub const FN_AK_SDK_ID: (u32, u8) = (0x00d1_7670, 5);
pub const FN_AK_SDK_W: (u32, u8) = (0x00d1_6cd0, 5);
pub const FN_AK_SDK_A: (u32, u8) = (0x00d1_6d60, 5);
pub const FN_PSC_ACTIVATE: (u32, u8) = (0x008b_1f80, 7);
pub const FN_AI_NOISE: (u32, u8) = (0x00c5_a760, 10);

#[derive(Default, Clone, Copy)]
pub struct Want {
    pub sounds: bool,
    pub effects: bool,
    pub noises: bool,
}

pub struct Hooks {
    ids: Vec<(&'static str, u32, (u32, u8))>,
}

impl Want {
    pub fn any(&self) -> bool {
        self.sounds || self.effects || self.noises
    }
}

pub fn install(s: &mut Session, w: Want) -> Result<Hooks> {
    let mut ids = Vec::new();
    if w.sounds {
        ids.push(("Sound", s.c.hook_set(FN_AK_SDK_ID.0, FN_AK_SDK_ID.1, 2, 100_000, &[])?, FN_AK_SDK_ID));
        ids.push(("SoundW", s.c.hook_set(FN_AK_SDK_W.0, FN_AK_SDK_W.1, 2, 100_000, &[HookDeref::arg(0, 0, 160)])?, FN_AK_SDK_W));
        ids.push(("SoundA", s.c.hook_set(FN_AK_SDK_A.0, FN_AK_SDK_A.1, 2, 100_000, &[HookDeref::arg(0, 0, 80)])?, FN_AK_SDK_A));
    }
    if w.effects {
        ids.push(("Effect", s.c.hook_set(FN_PSC_ACTIVATE.0, FN_PSC_ACTIVATE.1, 1, 100_000, &[HookDeref::ecx(0x48, 4), HookDeref::ecx(0x1c4, 4)])?, FN_PSC_ACTIVATE));
    }
    if w.noises {
        ids.push(("Noise", s.c.hook_set(FN_AI_NOISE.0, FN_AI_NOISE.1, 1, 100_000, &[HookDeref::arg(0, 0x14, 12), HookDeref::arg(0, 0x20, 4), HookDeref::arg(0, 0x24, 4), HookDeref::arg(0, 0x38, 4)])?, FN_AI_NOISE));
    }
    // Drop what came before the run.
    for (_, id, _) in &ids {
        s.c.hook_fetch(*id)?;
    }
    Ok(Hooks { ids })
}

/// An AkGameObjectID: the engine registers objects by their address; 0 / -1 is global.
fn game_object(s: &mut Session, go: u32) -> String {
    if go == 0 || go == u32::MAX {
        return "global".into();
    }
    match s.c.name_of(go) {
        Ok(n) if !n.trim().is_empty() => short(&n),
        _ => format!("go:{go:#x}"),
    }
}

fn short(full: &str) -> String {
    let path = full.split_once(' ').map(|x| x.1).unwrap_or(full);
    match path.split_once('.') {
        Some((pkg, rest)) => format!("{pkg}:{}", rest.trim_start_matches("TheWorld.PersistentLevel.")),
        None => path.to_string(),
    }
}

/// Fetch the records, disable the hooks, and turn the records into events. `times` is the
/// world time of each frame (index = tick).
pub fn collect(s: &mut Session, h: Hooks, times: &[f64]) -> Result<Vec<TraceEvent>> {
    let mut names: HashMap<u32, String> = HashMap::new();
    let mut name = |s: &mut Session, a: u32| -> String {
        if a == 0 {
            return "None".into();
        }
        names.entry(a).or_insert_with(|| s.c.name_of(a).unwrap_or_default()).clone()
    };
    let le = s.c.find("EAINoiseLoudness", "Enum")?.first().copied().unwrap_or(0);
    let loud = s.r.enum_names(&mut s.c, le).cloned().unwrap_or_default();
    let ce = s.c.find("EAINoiseContext", "Enum")?.first().copied().unwrap_or(0);
    let ctx = s.r.enum_names(&mut s.c, ce).cloned().unwrap_or_default();
    // akID -> AkEvent name.
    let mut ak: HashMap<u32, String> = HashMap::new();
    if h.ids.iter().any(|x| x.0 == "Sound") {
        let evs = s.c.find("", "AkEvent")?;
        let paths: Vec<dishonored_probe::ReadPath> = evs.iter().map(|a| dishonored_probe::ReadPath { base: *a, offsets: vec![0x40], len: 4 }).collect();
        let ids = s.c.reads(&paths)?;
        let names_ = s.c.names_of(&evs)?;
        for ((b, n), _) in ids.iter().zip(&names_).zip(&evs) {
            if let Some(b) = b {
                ak.entry(u32::from_le_bytes(b[..4].try_into().unwrap())).or_insert_with(|| n.split_once(' ').map(|x| x.1).unwrap_or(n).to_string());
            }
        }
    }
    let mut out = Vec::new();
    for (kind, id, f) in h.ids {
        let recs = s.c.hook_fetch(id)?;
        let _ = s.c.hook_set(f.0, f.1, 0, 0, &[]);
        for r in recs {
            let tick = times.iter().position(|t| *t + 1e-4 >= r.time as f64).unwrap_or(times.len()) as u32;
            let ev = match kind {
                "Sound" => {
                    let akid = r.args.first().copied().unwrap_or(0);
                    let e = ak.get(&akid).cloned().unwrap_or_else(|| format!("akid:{akid:#010x}"));
                    let go = r.args.get(1).copied().unwrap_or(0);
                    TraceEvent { tick, kind: "Sound".into(), object: game_object(s, go), name: e, detail: format!("akid={akid:#010x}") }
                }
                "SoundW" | "SoundA" => {
                    let text: String = if kind == "SoundW" {
                        let w: Vec<u16> = r.data.chunks(2).map(|c| u16::from_le_bytes([c[0], c[1]])).take_while(|&c| c != 0).collect();
                        String::from_utf16_lossy(&w)
                    } else {
                        r.data.iter().take_while(|&&c| c != 0).map(|&c| c as char).collect()
                    };
                    let go = r.args.get(1).copied().unwrap_or(0);
                    TraceEvent { tick, kind: "Sound".into(), object: game_object(s, go), name: text, detail: "by-name".into() }
                }
                "Effect" => {
                    let owner = name(s, r.u32_at(0));
                    let tpl = name(s, r.u32_at(4));
                    TraceEvent { tick, kind: "Effect".into(), object: short(&owner), name: tpl.split_once(' ').map(|x| x.1).unwrap_or(&tpl).to_string(), detail: format!("comp={:#x}", r.ecx) }
                }
                _ => {
                    let src = name(s, r.u32_at(16));
                    let l = r.data.get(12).copied().unwrap_or(0) as usize;
                    let c = r.data.get(13).copied().unwrap_or(0) as usize;
                    TraceEvent {
                        tick,
                        kind: "Noise".into(),
                        object: short(&src),
                        name: loud.get(l).cloned().unwrap_or_else(|| l.to_string()),
                        detail: format!("context={} x={:.1} y={:.1} z={:.1} damping={:.3}", ctx.get(c).cloned().unwrap_or_else(|| c.to_string()), r.f32_at(0), r.f32_at(4), r.f32_at(8), r.f32_at(20)),
                    }
                }
            };
            out.push(ev);
        }
    }
    Ok(out)
}
