//! `--profile`: frame-time statistics and the render passes' GPU / CPU times
//! (`RenderDiagnosticsPlugin`), logged as `PROFILE` lines.
//!
//! - Every 5 s (or at each inspect-script screenshot, labelled with the shot's name): frame
//!   time mean / median / p95 / p99 / max over the frames since the last report, the entity
//!   count, and the most expensive render passes averaged over the same frames.
//! - Per-system CPU spans need a build with `--features bevy/trace_chrome` (Bevy writes a
//!   Chrome trace JSON); see RENDERING.md "Performance".

use bevy::diagnostic::DiagnosticsStore;
use bevy::prelude::*;
use std::collections::HashMap;

/// `DISHONORED_PERF_BASELINE=1`: redo the per-frame work the performance pass removed (the
/// environment map filtered every frame, every particle sprite mesh rebuilt every frame, HUD
/// nodes and texts written every frame), for before / after timings with one build.
pub fn perf_baseline() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("DISHONORED_PERF_BASELINE").is_some())
}

#[derive(Resource, Default)]
pub struct Profile {
    /// Frame times (ms) since the last report.
    frames: Vec<f32>,
    /// Sums of each render diagnostic over those frames (path -> (sum, count)).
    passes: HashMap<String, (f64, u32)>,
    last_report: f32,
    /// A labelled report is wanted at the next frame (inspect screenshots).
    pub mark: Option<String>,
}

pub struct ProfilePlugin;

impl Plugin for ProfilePlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(bevy::render::diagnostic::RenderDiagnosticsPlugin).init_resource::<Profile>().add_systems(Last, collect);
    }
}

fn pct(sorted: &[f32], p: f32) -> f32 {
    if sorted.is_empty() {
        return 0.0;
    }
    sorted[((sorted.len() - 1) as f32 * p).round() as usize]
}

fn collect(mut prof: ResMut<Profile>, diag: Res<DiagnosticsStore>, time: Res<Time<Real>>) {
    let dt = time.delta_secs() * 1000.0;
    if dt > 0.0 {
        prof.frames.push(dt);
    }
    for d in diag.iter() {
        let p = d.path().as_str();
        if p.ends_with("elapsed_gpu") || p.ends_with("elapsed_cpu") {
            if let Some(v) = d.value() {
                let e = prof.passes.entry(p.to_string()).or_insert((0.0, 0));
                e.0 += v;
                e.1 += 1;
            }
        }
    }
    let now = time.elapsed_secs();
    let label = prof.mark.take();
    if label.is_none() && now - prof.last_report < 5.0 {
        return;
    }
    prof.last_report = now;
    let mut f = std::mem::take(&mut prof.frames);
    let passes = std::mem::take(&mut prof.passes);
    if f.is_empty() {
        return;
    }
    f.sort_by(f32::total_cmp);
    let mean = f.iter().sum::<f32>() / f.len() as f32;
    let entities = diag.get(&bevy::diagnostic::EntityCountDiagnosticsPlugin::ENTITY_COUNT).and_then(|d| d.value()).unwrap_or(0.0);
    let mut gpu: Vec<(String, f64)> = Vec::new();
    let mut cpu: Vec<(String, f64)> = Vec::new();
    for (p, (s, n)) in passes {
        let avg = s / n.max(1) as f64;
        let name = p.trim_start_matches("render/").trim_end_matches("/elapsed_gpu").trim_end_matches("/elapsed_cpu").to_string();
        if p.ends_with("elapsed_gpu") { gpu.push((name, avg)) } else { cpu.push((name, avg)) }
    }
    gpu.sort_by(|a, b| b.1.total_cmp(&a.1));
    cpu.sort_by(|a, b| b.1.total_cmp(&a.1));
    let fmt = |v: &[(String, f64)]| v.iter().take(12).map(|(p, t)| format!("{p}={t:.2}")).collect::<Vec<_>>().join(" ");
    info!(
        "PROFILE [{}] frames {} mean {:.2} ms ({:.0} fps) p50 {:.2} p95 {:.2} p99 {:.2} max {:.2} entities {:.0}\n  GPU ms: {}\n  CPU ms: {}",
        label.unwrap_or_else(|| format!("t={now:.0}s")),
        f.len(),
        mean,
        1000.0 / mean,
        pct(&f, 0.5),
        pct(&f, 0.95),
        pct(&f, 0.99),
        f[f.len() - 1],
        entities,
        fmt(&gpu),
        fmt(&cpu)
    );
}
