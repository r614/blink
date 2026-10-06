//! `harness fidelity`: renders fixed viewpoints on Boyle Ext with each visual feature on
//! and off and writes before/after pairs into the report.
//!
//! ```text
//! harness fidelity [--map L_Boyle_Ext_P] [--size 1280x720] [--configs full,no-lightmaps,...]
//!                  [--reference DIR]
//! ```
//! Every config is one `dishonored_runtime --inspect --script` run (one map load) that visits
//! all viewpoints. For each feature an `A|B` pair (feature off | full) is composed with ffmpeg.
//! `--reference DIR` (or `DISHONORED_REFERENCE_DIR`) holds original-game captures named
//! `<viewpoint>.png`; when present, SSIM of every render against them is reported. No such
//! captures exist yet (UNKNOWNS U-011), so this is the hook for the similarity metric.

use crate::Ctx;
use crate::report::Report;
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const S: &str = "Visual fidelity (Boyle Ext)";

/// (name, Dishonored-space position, yaw, pitch)
pub const VIEWPOINTS: &[(&str, [f32; 3], f32, f32)] = &[
    ("aerial", [-6000.0, -6000.0, 6000.0], 45.0, -35.0),
    ("street", [-2663.0, -1879.0, 170.0], 0.0, 5.0),
    ("garden", [-3805.0, -1879.0, -70.0], 180.0, -8.0),
    ("spawn1", [-2663.0, -1879.0, 162.0], 180.0, 0.0),
    ("mansion", [1355.0, 1445.0, 157.0], 180.0, 0.0),
    ("sewer", [-921.0, 1160.0, -1771.0], 180.0, 0.0),
    ("moon", [-3805.0, -1879.0, -70.0], -76.0, 29.0),
    ("skyline", [-2663.0, -1879.0, 400.0], 90.0, 12.0),
];

/// (config name, runtime flags, description). `full` must come first.
pub const CONFIGS: &[(&str, &[&str], &str)] = &[
    ("full", &[], "all features"),
    ("baseline", &["--baseline", "--legacy-geometry"], "pre-fidelity renderer: diffuse only, placeholder sun/ambient, old package/actor coverage"),
    ("no-lightmaps", &["--no-lightmaps"], "baked lighting off"),
    ("no-lightmap-specular", &["--no-lightmap-specular"], "directional lightmap specular term off (U-320)"),
    ("no-materials", &["--no-materials"], "diffuse-only materials, no cubemap"),
    ("no-fx", &["--no-fx"], "translucent FX cards hidden"),
    ("no-dynamic-lights", &["--no-dynamic-lights"], "level lights off (placeholder sun)"),
    ("no-fog", &["--no-fog"], "DisFog off"),
    ("no-post", &["--no-post"], "ArkPp post stack off"),
    ("no-bloom", &["--no-bloom"], "material bloom parts off"),
    ("no-sky", &["--no-sky"], "sky treated as ordinary geometry"),
    ("legacy-geometry", &["--legacy-geometry"], "prefix-matched sublevels, static mesh actors only"),
];

fn script() -> String {
    let mut steps = vec![r#"{"wait_ticks":120}"#.to_string()];
    for (name, p, yaw, pitch) in VIEWPOINTS {
        steps.push(format!(r#"{{"camera":{{"pos":[{},{},{}],"yaw":{yaw},"pitch":{pitch}}}}}"#, p[0], p[1], p[2]));
        steps.push(r#"{"wait_ticks":45}"#.into());
        steps.push(format!(r#"{{"screenshot":"{name}"}}"#));
    }
    steps.push(r#"{"quit":true}"#.into());
    format!("{{\"steps\":[{}]}}", steps.join(","))
}

fn ffmpeg() -> String {
    std::env::var("FFMPEG").unwrap_or_else(|_| "ffmpeg".into())
}

/// SSIM of `img` against `reference` via ffmpeg's ssim filter (None if unavailable).
pub fn ssim(img: &Path, reference: &Path) -> Option<f64> {
    let out = Command::new(ffmpeg())
        .args(["-hide_banner", "-i"])
        .arg(img)
        .arg("-i")
        .arg(reference)
        .args(["-lavfi", "[0][1]scale2ref[a][b];[a][b]ssim", "-f", "null", "-"])
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&out.stderr);
    let all = text.rsplit("All:").next()?;
    all.split_whitespace().next()?.parse().ok()
}

pub fn run(ctx: &Ctx, rep: &mut Report) -> Result<()> {
    let map = ctx.opt("--map").unwrap_or_else(|| "L_Boyle_Ext_P".into());
    let size = ctx.opt("--size").unwrap_or_else(|| "1280x720".into());
    let wanted: Option<Vec<String>> = ctx.opt("--configs").map(|s| s.split(',').map(str::to_string).collect());
    let reference = ctx.opt("--reference").or_else(|| std::env::var("DISHONORED_REFERENCE_DIR").ok()).map(PathBuf::from);
    let root = ctx.out.join("fidelity");
    std::fs::create_dir_all(&root)?;
    let script_path = root.join("viewpoints.json");
    std::fs::write(&script_path, script())?;

    let mut done: Vec<&str> = Vec::new();
    for (name, flags, desc) in CONFIGS {
        if *name != "full" && wanted.as_ref().is_some_and(|w| !w.iter().any(|x| x == name)) {
            continue;
        }
        let dir = root.join(name);
        let t0 = std::time::Instant::now();
        let log = std::fs::File::create(root.join(format!("{name}.log")))?;
        let status = Command::new(ctx.bin("dishonored_runtime"))
            .args(["--inspect", "--map", &map, "--size", &size, "--script"])
            .arg(&script_path)
            .arg("--out")
            .arg(&dir)
            .args(*flags)
            .stdout(Stdio::from(log.try_clone()?))
            .stderr(Stdio::from(log))
            .status()
            .with_context(|| format!("running dishonored_runtime for {name}"))?;
        let shots = VIEWPOINTS.iter().filter(|v| dir.join(format!("{}.png", v.0)).exists()).count();
        rep.check(S, &format!("render config `{name}` ({desc})"), status.success() && shots == VIEWPOINTS.len(), format!("{shots}/{} viewpoints in {:.0?}", VIEWPOINTS.len(), t0.elapsed()));
        done.push(name);
    }

    // Before/after pairs: <config> (left) | full (right).
    let mut rows = Vec::new();
    for (view, ..) in VIEWPOINTS {
        let full = root.join("full").join(format!("{view}.png"));
        let mut row = vec![format!("**{view}**"), format!("![](fidelity/full/{view}.png)")];
        for name in done.iter().filter(|n| **n != "full") {
            let other = root.join(name).join(format!("{view}.png"));
            let pair = root.join("pairs").join(format!("{view}__{name}_vs_full.png"));
            std::fs::create_dir_all(pair.parent().unwrap())?;
            let ok = full.exists()
                && other.exists()
                && Command::new(ffmpeg())
                    .args(["-y", "-loglevel", "error", "-i"])
                    .arg(&other)
                    .arg("-i")
                    .arg(&full)
                    .args(["-filter_complex", "hstack=inputs=2,scale=1280:-1"])
                    .arg(&pair)
                    .status()
                    .is_ok_and(|s| s.success());
            row.push(if ok { format!("[{name} \\| full](fidelity/pairs/{view}__{name}_vs_full.png)") } else { "-".into() });
        }
        rows.push(row);
    }
    let mut header = vec!["viewpoint", "full"];
    let names: Vec<String> = done.iter().filter(|n| **n != "full").map(|n| n.to_string()).collect();
    header.extend(names.iter().map(String::as_str));
    rep.table("Viewpoints: full render and before|after pairs (feature off | full)", &header, rows);

    // Similarity against original captures (U-011): hook only until captures exist.
    match &reference {
        Some(dir) if dir.is_dir() => {
            let mut rows = Vec::new();
            for (view, ..) in VIEWPOINTS {
                let r = dir.join(format!("{view}.png"));
                if !r.exists() {
                    continue;
                }
                let mut row = vec![view.to_string()];
                for name in &done {
                    let s = ssim(&root.join(name).join(format!("{view}.png")), &r);
                    row.push(s.map(|v| format!("{v:.4}")).unwrap_or_else(|| "-".into()));
                }
                rows.push(row);
            }
            let mut h = vec!["viewpoint"];
            h.extend(done.iter().copied());
            rep.table("SSIM against original-game captures", &h, rows);
        }
        _ => rep.check(S, "similarity vs original captures", true, "skipped: no reference captures (UNKNOWNS U-011); pass --reference DIR with <viewpoint>.png"),
    }
    Ok(())
}
