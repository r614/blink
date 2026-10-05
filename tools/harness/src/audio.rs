//! Audio verification: Wwise media decode to sane PCM, and every simulation-event
//! mapping resolves to at least one decodable original sound.

use crate::Ctx;
use crate::report::Report;
use anyhow::Result;
use dishonored_audio::cues;
use dishonored_audio::library::{Leaf, SoundLibrary};
use dishonored_audio::vorbis::CodebookLibrary;
use dishonored_audio::wem::{Pcm, Riff};
use std::collections::BTreeMap;
use std::path::PathBuf;

fn install() -> PathBuf {
    std::env::var_os("DISHONORED_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"C:\Program Files\GOG Galaxy\Games\Dishonored"))
}

/// Non-silent, finite, plausible rate/duration, and as long as the header says.
fn sane(p: &Pcm, expected_frames: Option<usize>) -> Result<(), String> {
    if p.samples.iter().any(|s| !s.is_finite()) {
        return Err("non-finite samples".into());
    }
    if p.peak() < 1e-3 {
        return Err(format!("silent (peak {:.5})", p.peak()));
    }
    if !(8000..=96000).contains(&p.sample_rate) || p.channels == 0 || p.channels > 8 {
        return Err(format!("implausible format {} ch {} Hz", p.channels, p.sample_rate));
    }
    let d = p.duration();
    if !(0.005..=1800.0).contains(&d) {
        return Err(format!("implausible duration {d:.3}s"));
    }
    if let Some(n) = expected_frames {
        if n > 0 && p.frames() != n {
            return Err(format!("{} frames decoded, header says {n}", p.frames()));
        }
    }
    Ok(())
}

pub fn run(_ctx: &Ctx, rep: &mut Report) -> Result<()> {
    let root = install();
    let sec = "audio";
    let t0 = std::time::Instant::now();
    let cb = CodebookLibrary::from_install(&root);
    rep.check(
        sec,
        "Wwise Vorbis codebook table located in Dishonored.exe",
        cb.as_ref().is_ok_and(|c| c.len() >= 500),
        match &cb {
            Ok(c) => format!("{} codebooks ({:.1}s)", c.len(), t0.elapsed().as_secs_f32()),
            Err(e) => format!("{e:#}"),
        },
    );
    let mut lib = SoundLibrary::new(&root)?;
    let mut banks: Vec<&str> = cues::PLAYER_BANKS.to_vec();
    let amb = cues::map_ambience(dishonored_content::DEFAULT_MAP);
    if let Some(a) = &amb {
        banks.extend_from_slice(a.banks);
    }
    for b in &banks {
        let r = lib.load_bank(b);
        rep.check(sec, &format!("load {b}"), r.is_ok(), r.err().map(|e| format!("{e:#}")).unwrap_or_default());
    }

    // 1. Media per codec class: decode every embedded file of the loaded banks and a
    //    sample of streamed files; all must be sane.
    let mut classes: BTreeMap<String, (usize, usize, f32, String)> = BTreeMap::new();
    for (bi, b) in lib.banks.iter().enumerate() {
        // DIDX entries that also exist in the stream table are prefetch heads (the
        // bank keeps only the first packets); the full file is decoded from the stream.
        let embedded = b.bank.media.iter().filter(|m| !b.pck.streams.iter().any(|s| s.id == m.id)).map(|m| (m.id, false));
        let streamed = b.pck.streams.iter().take(4).map(|s| (s.id, true));
        for (id, streamed) in embedded.chain(streamed) {
            let key = dishonored_audio::library::MediaKey { bank: bi, id, streamed };
            let Ok(bytes) = lib.media_bytes(key) else { continue };
            let (class, frames) = match Riff::parse(&bytes) {
                Ok(r) => {
                    let frames = dishonored_audio::vorbis::vorb_info(&r).ok().map(|v| v.sample_count as usize);
                    (format!("{} {} ch {}", r.codec_name(), r.channels, if streamed { "streamed" } else { "embedded" }), frames)
                }
                Err(_) => ("unparsed".to_string(), None),
            };
            let e = classes.entry(class).or_insert((0, 0, 0.0, String::new()));
            match lib.decode(key).map_err(|e| format!("{e:#}")).and_then(|p| sane(&p, frames).map(|_| p)) {
                Ok(p) => {
                    e.0 += 1;
                    e.2 += p.duration();
                }
                Err(err) => {
                    e.1 += 1;
                    if e.3.is_empty() {
                        e.3 = format!("{} media {id}: {err}", b.name);
                    }
                }
            }
        }
    }
    let mut rows = Vec::new();
    for (k, (ok, bad, secs, first)) in &classes {
        rep.check(sec, &format!("decode {k}"), *bad == 0 && *ok > 0, format!("{ok} sane, {bad} bad, {secs:.0} s audio {first}"));
        rows.push(vec![k.clone(), ok.to_string(), bad.to_string(), format!("{secs:.1}")]);
    }
    rep.table("Decoded media by codec (loaded banks)", &["class", "sane", "bad", "seconds"], rows);

    // 2. Every simulation-event mapping resolves to >= 1 decodable sound.
    let check_event = |lib: &SoundLibrary, ev: &str| -> Result<(usize, usize), String> {
        let cue = lib.resolve(ev).ok_or_else(|| "does not resolve".to_string())?;
        let mut leaves: Vec<Leaf> = Vec::new();
        cue.leaves(&mut leaves);
        let mut ok = 0;
        let mut err = String::new();
        for l in &leaves {
            match lib.decode(l.media) {
                Ok(p) => match sane(&p, None) {
                    Ok(()) => ok += 1,
                    Err(e) => err = e,
                },
                Err(e) => err = format!("{e:#}"),
            }
        }
        if ok == 0 {
            return Err(format!("no decodable leaf ({} leaves) {err}", leaves.len()));
        }
        Ok((ok, leaves.len()))
    };
    let mut rows = Vec::new();
    for (key, ev) in cues::SIM_CUES {
        let r = check_event(&lib, ev);
        rep.check(sec, &format!("cue {key} -> {ev}"), r.is_ok(), match &r {
            Ok((ok, n)) => format!("{ok}/{n} leaves decode"),
            Err(e) => e.clone(),
        });
        rows.push(vec![key.to_string(), ev.to_string(), match r {
            Ok((ok, n)) => format!("{ok}/{n}"),
            Err(e) => e,
        }]);
    }
    rep.table("Simulation cues", &["cue", "original event", "decodable leaves"], rows);

    // 3. Footsteps / landings: coverage across surfaces (stone is the fallback and must exist).
    let all = cues::all_player_events();
    let steps: Vec<&String> = all.iter().filter(|e| e.starts_with("FS_")).collect();
    let resolved = steps.iter().filter(|e| check_event(&lib, e).is_ok()).count();
    rep.check(sec, "footstep/landing events resolve", resolved * 10 >= steps.len() * 8, format!("{resolved}/{} surface x gait events", steps.len()));
    for g in [cues::Gait::Sneak, cues::Gait::Run, cues::Gait::Sprint] {
        let ev = cues::footstep_event("Stone", g, false);
        rep.check(sec, &format!("fallback {ev}"), check_event(&lib, &ev).is_ok(), "");
    }

    // 4. Level ambience for the default map.
    if let Some(a) = &amb {
        for s in a.state_events {
            rep.check(sec, &format!("state event {s}"), lib.apply_state_event(s), "");
        }
        for ev in std::iter::once(a.bed).chain(a.music) {
            let r = check_event(&lib, ev);
            rep.check(sec, &format!("ambience {ev}"), r.is_ok(), match r {
                Ok((ok, n)) => format!("{ok}/{n} leaves decode"),
                Err(e) => e,
            });
        }
    }
    crate::audio_playback::run(rep, &root)?;
    rep.check(sec, "audio checks finished", true, format!("{:.1}s", t0.elapsed().as_secs_f32()));
    Ok(())
}
