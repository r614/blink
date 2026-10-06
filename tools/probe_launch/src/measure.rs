//! Movement measurements on the running original game, driven through the probe:
//! keyboard through the game's own input path, the pawn sampled once per rendered frame.
//!
//! measure boot                  menu -> `open L_Boyle_Ext_P` -> past the briefing screen
//! measure jump [N]              standing jumps: apex, rise time, airtime
//! measure fall H1 H2 ...        drops from H uu above the current floor: impact speed, damage
//! measure run                   run, sprint and slide from a standstill (yaw from --yaw DEG)
//! measure trace FRAMES          raw samples as CSV
//! Common options: --out DIR (CSV of every run, default: no files), --yaw DEG.
use anyhow::{Context, Result, bail};
use dishonored_probe::{Client, Sample, physics_name};
use std::fmt::Write as _;
use std::path::PathBuf;
use std::time::{Duration, Instant};

const VK_SPACE: &str = "SpaceBar";
const VK_SHIFT_L: &str = "LeftShift";
const VK_W: &str = "W";
const VK_C: &str = "C";
const VK_RETURN: &str = "Enter";
const VK_ESCAPE: &str = "Escape";

/// Pawn fields (class_layout: Actor / Pawn).
const ACTOR_ROTATION: u32 = 0xd0;
const PAWN_HEALTH: u32 = 0x344;
const PAWN_HEALTH_MAX: u32 = 0x348;

struct Opts {
    out: Option<PathBuf>,
    yaw: Option<f32>,
    rest: Vec<String>,
}

fn opts(args: &[String]) -> Opts {
    let mut o = Opts { out: None, yaw: None, rest: Vec::new() };
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--out" => {
                o.out = args.get(i + 1).map(PathBuf::from);
                i += 1;
            }
            "--yaw" => {
                o.yaw = args.get(i + 1).and_then(|s| s.parse().ok());
                i += 1;
            }
            a => o.rest.push(a.to_string()),
        }
        i += 1;
    }
    o
}

fn status(c: &mut Client) -> Result<std::collections::HashMap<String, String>> {
    Ok(c.status()?.lines().filter_map(|l| l.split_once('=')).map(|(k, v)| (k.to_string(), v.to_string())).collect())
}

fn hex(s: &str) -> u32 {
    u32::from_str_radix(s.trim_start_matches("0x"), 16).unwrap_or(0)
}

fn wait_for(c: &mut Client, what: &str, timeout: Duration, mut pred: impl FnMut(&std::collections::HashMap<String, String>) -> bool) -> Result<()> {
    let t0 = Instant::now();
    loop {
        if let Ok(s) = status(c)
            && pred(&s) {
                return Ok(());
            }
        if t0.elapsed() > timeout {
            bail!("timed out waiting for {what}");
        }
        std::thread::sleep(Duration::from_millis(500));
    }
}

fn tap(c: &mut Client, vk: &str, frames: u32) -> Result<()> {
    c.key(vk, true)?;
    c.wait_ticks(frames)?;
    c.key(vk, false)?;
    Ok(())
}

fn pawn(c: &mut Client) -> Result<u32> {
    let p = hex(status(c)?.get("pawn").map(String::as_str).unwrap_or("0"));
    if p == 0 {
        bail!("no player pawn");
    }
    Ok(p)
}

fn samples(c: &mut Client, frames: u32) -> Result<Vec<Sample>> {
    c.wait_ticks(frames)?;
    Ok(c.sample_fetch()?)
}

fn csv(s: &[Sample]) -> String {
    let mut out = String::from("tick,time,delta,x,y,z,vx,vy,vz,speed2d,physics,health\n");
    for x in s {
        let _ = writeln!(
            out,
            "{},{:.4},{:.4},{:.2},{:.2},{:.2},{:.2},{:.2},{:.2},{:.2},{},{}",
            x.tick,
            x.time_seconds,
            x.delta,
            x.location[0],
            x.location[1],
            x.location[2],
            x.velocity[0],
            x.velocity[1],
            x.velocity[2],
            (x.velocity[0].powi(2) + x.velocity[1].powi(2)).sqrt(),
            physics_name(x.physics),
            x.health
        );
    }
    out
}

fn save(o: &Opts, name: &str, s: &[Sample]) -> Result<()> {
    if let Some(dir) = &o.out {
        std::fs::create_dir_all(dir)?;
        std::fs::write(dir.join(format!("{name}.csv")), csv(s))?;
    }
    Ok(())
}

fn set_yaw(c: &mut Client, deg: f32) -> Result<()> {
    // Controller.Rotation drives the move direction; Unreal rotators are 65536 per turn.
    let st = status(c)?;
    let pc = hex(st.get("local_pc").map(String::as_str).unwrap_or("0"));
    let yaw = (deg / 360.0 * 65536.0) as i32;
    c.write(pc + ACTOR_ROTATION + 4, &yaw.to_le_bytes())?;
    Ok(())
}

pub fn run(args: &[String]) -> Result<()> {
    let o = opts(args);
    let what = o.rest.first().cloned().unwrap_or_default();
    let mut c = crate::connect()?;
    match what.as_str() {
        "boot" => boot(&mut c),
        "jump" => jump(&mut c, &o),
        "fall" => fall(&mut c, &o),
        "run" => run_sprint_slide(&mut c, &o),
        "sdf" => sdf(&mut c),
        "views" => views(&mut c, &o),
        "gun" => crate::oracle::gun(&mut c, &crate::oracle::Opts { out: o.out.clone(), rest: args.to_vec() }),
        "blink" => crate::oracle_powers::blink(&mut c, &crate::oracle::Opts { out: o.out.clone(), rest: args.to_vec() }),
        "mana" => crate::oracle_powers::mana(&mut c, &crate::oracle::Opts { out: o.out.clone(), rest: args.to_vec() }),
        "windblast" => crate::oracle_powers::windblast(&mut c, &crate::oracle::Opts { out: o.out.clone(), rest: args.to_vec() }),
        "detect" => crate::oracle_stealth::detect(&mut c, &crate::oracle::Opts { out: o.out.clone(), rest: args.to_vec() }),
        "vis" => crate::oracle_stealth::vis(&mut c, &crate::oracle::Opts { out: o.out.clone(), rest: args.to_vec() }),
        "sword" => crate::oracle_combat::sword(&mut c, &crate::oracle::Opts { out: o.out.clone(), rest: args.to_vec() }),
        "trace" => {
            let n: u32 = o.rest.get(1).and_then(|s| s.parse().ok()).unwrap_or(120);
            c.sample_start(100_000)?;
            let s = samples(&mut c, n)?;
            print!("{}", csv(&s));
            Ok(())
        }
        _ => bail!("measure boot|jump|fall|run|trace"),
    }
}

/// Main menu -> Boyle exterior, in control of Corvo.
fn boot(c: &mut Client) -> Result<()> {
    let st = status(c)?;
    if st.get("map").is_some_and(|m| m == "L_Boyle_Ext_P") {
        // Already in the level (e.g. after the player died): open it again and wait for the
        // new pawn.
        let old = st.get("pawn").cloned().unwrap_or_default();
        println!("reopening L_Boyle_Ext_P");
        c.console("open L_Boyle_Ext_P")?;
        wait_for(c, "the reloaded level", Duration::from_secs(300), |s| s.get("map").is_some_and(|m| m == "L_Boyle_Ext_P") && s.get("pawn").is_some_and(|p| *p != old && p != "0x0"))?;
    } else {
        wait_for(c, "the front-end map", Duration::from_secs(240), |s| s.get("map").is_some_and(|m| m == "DishonoredGameFull_P") && s.get("presents").and_then(|p| p.parse::<u32>().ok()).unwrap_or(0) > 200)?;
        println!("front end up; opening L_Boyle_Ext_P");
        c.console("open L_Boyle_Ext_P")?;
        wait_for(c, "L_Boyle_Ext_P", Duration::from_secs(300), |s| s.get("map").is_some_and(|m| m == "L_Boyle_Ext_P"))?;
    }
    println!("level loaded; dismissing the briefing");
    // Skipping the start screen leaves LocalPlayer.ControllerId (+0x5C) at -1, and the
    // viewport client then routes no key to the player: claim controller 0 like the start
    // screen's "press any key" does.
    let st = status(c)?;
    let engine = hex(st.get("gengine").map(String::as_str).unwrap_or("0"));
    let players = c.read_u32(engine + 0x480)?;
    let lp = c.read_u32(players)?;
    c.write(lp + 0x5c, &0i32.to_le_bytes())?;
    // The briefing waits for any key; then the pawn walks normally.
    let t0 = Instant::now();
    loop {
        tap(c, VK_RETURN, 3)?;
        c.wait_ticks(60)?;
        // In control when holding W moves the pawn.
        c.sample_start(1000)?;
        c.key(VK_W, true)?;
        c.wait_ticks(20)?;
        c.key(VK_W, false)?;
        let s = samples(c, 10)?;
        if let (Some(a), Some(b)) = (s.first(), s.last()) {
            let d = ((b.location[0] - a.location[0]).powi(2) + (b.location[1] - a.location[1]).powi(2)).sqrt();
            if d > 20.0 {
                println!("in control at {:?} (moved {d:.1} uu)", b.location);
                return Ok(());
            }
        }
        if t0.elapsed() > Duration::from_secs(120) {
            bail!("still not in control");
        }
    }
}

/// The `harness fidelity` viewpoints (tools/harness/src/fidelity.rs): name, eye position,
/// yaw, pitch (degrees, Unreal axes).
const VIEWPOINTS: &[(&str, [f32; 3], f32, f32)] = &[
    ("aerial", [-6000.0, -6000.0, 6000.0], 45.0, -35.0),
    ("street", [-2663.0, -1879.0, 170.0], 0.0, 5.0),
    ("garden", [-3805.0, -1879.0, -70.0], 180.0, -8.0),
    ("spawn1", [-2663.0, -1879.0, 162.0], 180.0, 0.0),
    ("mansion", [1355.0, 1445.0, 157.0], 180.0, 0.0),
    ("sewer", [-921.0, 1160.0, -1771.0], 180.0, 0.0),
    ("moon", [-3805.0, -1879.0, -70.0], -76.0, 29.0),
    ("skyline", [-2663.0, -1879.0, 400.0], 90.0, 12.0),
];

/// Fixed-viewpoint captures: the pawn is frozen (PHYS_None) where its camera lands on the
/// viewpoint, the controller rotation is set, and the back buffer is saved as
/// `<out>/<name>.bmp`. The camera position actually reached is read back from
/// `PlayerCamera.CameraCache.POV` (Camera +0x32C: time, location, rotation, FOV).
fn views(c: &mut Client, o: &Opts) -> Result<()> {
    let out = o.out.clone().context("--out DIR")?;
    std::fs::create_dir_all(&out)?;
    let only: Vec<&String> = o.rest[1..].iter().collect();
    let _ = c.console("ShowHUD");
    for (name, eye, yaw, pitch) in VIEWPOINTS {
        if !only.is_empty() && !only.iter().any(|n| n.as_str() == *name) {
            continue;
        }
        let st = status(c)?;
        let pc = hex(st.get("local_pc").map(String::as_str).unwrap_or("0"));
        let p = pawn(c)?;
        let cam = c.read_u32(pc + 0x384)?;
        let rot = |c: &mut Client| -> Result<()> {
            let r = [(pitch / 360.0 * 65536.0) as i32 & 0xffff, (yaw / 360.0 * 65536.0) as i32 & 0xffff, 0];
            let mut b = Vec::new();
            for v in r {
                b.extend(v.to_le_bytes());
            }
            c.write(pc + ACTOR_ROTATION, &b)?;
            Ok(())
        };
        let mut target = *eye;
        target[2] -= 64.0;
        let mut pov = [0.0f32; 3];
        for _ in 0..3 {
            let mut b = Vec::new();
            for v in target {
                b.extend(v.to_le_bytes());
            }
            c.write(p + 0xc4, &b)?;
            c.write(p + 0x1b4, &[0u8; 12])?;
            c.write(p + 0x104, &[0u8])?;
            rot(c)?;
            c.wait_ticks(20)?;
            let b = c.read(cam + 0x32c + 4, 12)?;
            for k in 0..3 {
                pov[k] = f32::from_le_bytes(b[k * 4..k * 4 + 4].try_into()?);
            }
            for k in 0..3 {
                target[k] += eye[k] - pov[k];
            }
        }
        let mut b = Vec::new();
        for v in target {
            b.extend(v.to_le_bytes());
        }
        c.write(p + 0xc4, &b)?;
        c.write(p + 0x104, &[0u8])?;
        rot(c)?;
        c.wait_ticks(30)?;
        let b = c.read(cam + 0x32c + 4, 12)?;
        for k in 0..3 {
            pov[k] = f32::from_le_bytes(b[k * 4..k * 4 + 4].try_into()?);
        }
        let cap = c.capture()?;
        crate::write_bmp(&out.join(format!("{name}.bmp")), &cap)?;
        let fov = c.read_f32(cam + 0x32c + 0x1c)?;
        println!("{name}: camera at ({:.1}, {:.1}, {:.1}) for ({}, {}, {}), yaw {yaw} pitch {pitch}, FOV {fov:.1}", pov[0], pov[1], pov[2], eye[0], eye[1], eye[2]);
    }
    let _ = c.console("ShowHUD");
    Ok(())
}

/// Least squares `y = a + b x + c x^2`.
fn fit_parabola(p: &[(f64, f64)]) -> Option<(f64, f64, f64)> {
    if p.len() < 4 {
        return None;
    }
    let mut s = [0.0f64; 5];
    let mut t = [0.0f64; 3];
    for &(x, y) in p {
        let mut xp = 1.0;
        for k in 0..5 {
            s[k] += xp;
            if k < 3 {
                t[k] += xp * y;
            }
            xp *= x;
        }
    }
    let m = [[s[0], s[1], s[2]], [s[1], s[2], s[3]], [s[2], s[3], s[4]]];
    let det = |m: [[f64; 3]; 3]| m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1]) - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0]) + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0]);
    let d = det(m);
    if d.abs() < 1e-12 {
        return None;
    }
    let col = |k: usize| {
        let mut mm = m;
        for r in 0..3 {
            mm[r][k] = t[r];
        }
        det(mm) / d
    };
    Some((col(0), col(1), col(2)))
}

/// Register of a CTAB entry (`Name=c9x1` -> 9).
fn ctab_reg(ctab: &str, name: &str) -> Option<usize> {
    ctab.split(' ').find_map(|e| e.strip_prefix(name).and_then(|r| r.strip_prefix("=c"))).and_then(|r| r.split('x').next()).and_then(|n| n.parse().ok())
}

/// Pixel shader constants of the dominant light's distance-field shadow pass, read at draw
/// time from every created pixel shader that declares `DistanceFieldParameters`.
fn sdf(c: &mut Client) -> Result<()> {
    let shaders: Vec<_> = c.shaders()?.into_iter().filter(|s| s.ctab.contains("DistanceFieldParameters")).collect();
    println!("{} pixel shaders declare DistanceFieldParameters", shaders.len());
    let names = ["DistanceFieldParameters", "LightColorAndFalloffExponent", "DiffuseOverrideParameter", "SpecularOverrideParameter", "ScreenPositionScaleBias"];
    let mut seen = std::collections::BTreeMap::new();
    let got = c.ps_constants(0, 400, "DistanceFieldParameters")?;
    let drawn: std::collections::BTreeSet<u64> = got.iter().map(|g| g.1).collect();
    let drawn = drawn.len();
    for (_, hash, regs) in &got {
        let Some(s) = shaders.iter().find(|s| s.hash == *hash) else { continue };
        {
            // Light policy from the constant table: spot lights bind SpotAngles, point and
            // spot lights LightPositionAndInvRadius; the directional policy neither.
            let kind = if s.ctab.contains("SpotAngles") {
                "spot"
            } else if s.ctab.contains("LightPositionAndInvRadius") {
                "point"
            } else {
                "directional"
            };
            let mut line = format!("[{kind}] ");
            for n in names {
                if let Some(r) = ctab_reg(&s.ctab, n) {
                    let v = regs[r];
                    let _ = write!(line, "{n}=({:.6}, {:.6}, {:.6}, {:.6}) ", v[0], v[1], v[2], v[3]);
                }
            }
            *seen.entry(line).or_insert(0) += 1;
        }
    }
    println!("{drawn} of them drawn in the sampled frames; distinct constant sets:");
    for (l, n) in seen {
        println!("  x{n}: {l}");
    }
    Ok(())
}

fn floor_z(s: &[Sample]) -> f32 {
    s.iter().filter(|x| x.physics == 1).map(|x| x.location[2]).fold(f32::NAN, f32::min)
}

fn jump(c: &mut Client, o: &Opts) -> Result<()> {
    let n: usize = o.rest.get(1).and_then(|s| s.parse().ok()).unwrap_or(3);
    println!("| run | z0 | apex (uu) | rise (s) | airtime (s) | launch vz | land vz |");
    println!("|---|---|---|---|---|---|---|");
    for k in 0..n {
        c.wait_ticks(40)?;
        c.sample_start(100_000)?;
        c.wait_ticks(10)?;
        tap(c, VK_SPACE, 3)?;
        let s = samples(c, 150)?;
        save(o, &format!("jump{k}"), &s)?;
        let Some(i0) = s.iter().position(|x| x.physics == 2) else {
            println!("| {k} | no jump (physics stayed {}) |", physics_name(s.last().map(|x| x.physics).unwrap_or(0)));
            continue;
        };
        let z0 = s[..i0].iter().map(|x| x.location[2]).next_back().unwrap_or(f32::NAN);
        let t_off = s[i0 - 1].time_seconds;
        let (ia, apex) = s.iter().enumerate().skip(i0).map(|(i, x)| (i, x.location[2])).fold((i0, f32::MIN), |a, b| if b.1 > a.1 { b } else { a });
        let land = s.iter().skip(i0).position(|x| x.physics != 2).map(|i| i + i0);
        let air = land.map(|i| s[i].time_seconds - t_off);
        let land_vz = land.map(|i| s[i - 1].velocity[2]);
        // Least-squares parabola through the airborne samples: z = a + b t + c t^2.
        let air_s: Vec<(f64, f64)> = s[i0..land.unwrap_or(s.len())].iter().map(|x| ((x.time_seconds - t_off) as f64, x.location[2] as f64)).collect();
        if let Some((a, b, cc)) = fit_parabola(&air_s) {
            let g = -2.0 * cc;
            let disc = b * b - 4.0 * cc * (a - z0 as f64);
            let t_land = if disc >= 0.0 { (-b - disc.sqrt()) / (2.0 * cc) } else { f64::NAN };
            println!("|   | fit: launch vz {:.1}, gravity {:.1}, apex {:.2} at {:.3} s, back to z0 at {:.3} s |", b, g, a - z0 as f64 + b * b / (2.0 * g), b / g, t_land);
        }
        println!(
            "| {k} | {:.2} | {:.2} | {:.3} | {} | {:.1} | {} |",
            z0,
            apex - z0,
            s[ia].time_seconds - t_off,
            air.map(|a| format!("{a:.3}")).unwrap_or("-".into()),
            s[i0].velocity[2],
            land_vz.map(|v| format!("{v:.1}")).unwrap_or("-".into())
        );
        let _ = floor_z(&s);
    }
    Ok(())
}

fn fall(c: &mut Client, o: &Opts) -> Result<()> {
    let heights: Vec<f32> = o.rest[1..].iter().filter_map(|s| s.parse().ok()).collect();
    if heights.is_empty() {
        bail!("measure fall H1 H2 ...");
    }
    println!("| drop (uu) | fall time (s) | impact vz (uu/s) | health before | after | damage |");
    println!("|---|---|---|---|---|---|");
    for (k, &h) in heights.iter().enumerate() {
        let p = pawn(c)?;
        let hm = i32::from_le_bytes(c.read(p + PAWN_HEALTH_MAX, 4)?[..4].try_into()?);
        c.write(p + PAWN_HEALTH, &hm.to_le_bytes())?;
        c.wait_ticks(20)?;
        c.sample_start(10)?;
        let s0 = samples(c, 3)?;
        let start = s0.last().context("no sample")?.location;
        // Actor.SetLocation refuses the player pawn (returns false) and SetPhysics(PHYS_Falling)
        // is ignored, so Location.Z is written and a one-frame step forward lets
        // physWalking find no floor and start the fall itself.
        c.sample_start(100_000)?;
        c.write(p + 0xcc, &(start[2] + h).to_le_bytes())?;
        c.wait_ticks(2)?;
        tap(c, VK_W, 1)?;
        let s = samples(c, 240)?;
        save(o, &format!("fall{k}_{h}"), &s)?;
        let top = s.first().map(|x| x.location[2]).unwrap_or(0.0);
        let i0 = s.iter().position(|x| x.physics == 2);
        let land = i0.and_then(|i0| s.iter().skip(i0).position(|x| x.physics != 2).map(|i| i + i0));
        let (Some(i0), Some(li)) = (i0, land) else {
            println!("| {h} | did not fall/land (z {top:.1}) |");
            continue;
        };
        let drop = top - s[li].location[2];
        let impact = s[..li].iter().map(|x| x.velocity[2]).fold(0.0f32, f32::min);
        let hb = s0.last().map(|x| x.health).unwrap_or(0);
        let ha = s.last().map(|x| x.health).unwrap_or(0);
        println!("| {:.1} (asked {h}) | {:.3} | {:.1} | {hb} | {ha} | {} |", drop, s[li].time_seconds - s[i0].time_seconds, impact, hb - ha);
        // Back to full health for the next drop.
        let p = pawn(c)?;
        c.write(p + PAWN_HEALTH, &hm.to_le_bytes())?;
    }
    Ok(())
}

fn speed2d(x: &Sample) -> f32 {
    (x.velocity[0].powi(2) + x.velocity[1].powi(2)).sqrt()
}

fn run_sprint_slide(c: &mut Client, o: &Opts) -> Result<()> {
    if let Some(y) = o.yaw {
        set_yaw(c, y)?;
    }
    if let Some(at) = o.rest.iter().find_map(|a| a.strip_prefix("at=")) {
        // at=X,Y,Z: put the pawn there first (Location written directly; it settles onto
        // the floor below within a few frames).
        let v: Vec<f32> = at.split(',').filter_map(|s| s.parse().ok()).collect();
        if v.len() == 3 {
            let p = pawn(c)?;
            let mut b = Vec::new();
            for x in &v {
                b.extend(x.to_le_bytes());
            }
            c.write(p + 0xc4, &b)?;
            tap(c, VK_W, 1)?;
            c.wait_ticks(60)?;
        }
    }
    c.wait_ticks(20)?;
    // Run.
    // Frame counts assume ~30 fps; the analysis uses the game time of each sample.
    let fr = |secs: f32| (secs * 30.0) as u32;
    let run_s: f32 = o.rest.get(1).and_then(|s| s.parse().ok()).unwrap_or(1.5);
    let sprint_s: f32 = o.rest.get(2).and_then(|s| s.parse().ok()).unwrap_or(2.0);
    c.sample_start(100_000)?;
    c.key(VK_W, true)?;
    let run = samples(c, fr(run_s))?;
    save(o, "run", &run)?;
    // Sprint (W held).
    c.sample_start(100_000)?;
    c.key(VK_SHIFT_L, true)?;
    let sprint = samples(c, fr(sprint_s))?;
    save(o, "sprint", &sprint)?;
    // Slide: sneak while sprinting, W and Shift kept down.
    c.sample_start(100_000)?;
    c.wait_ticks(1)?;
    tap(c, VK_C, 2)?;
    let slide = samples(c, fr(1.8))?;
    c.key(VK_SHIFT_L, false)?;
    c.key(VK_W, false)?;
    save(o, "slide", &slide)?;
    // The speed is clamped at the stance's maximum: the plateau is the top speed reached.
    let plateau = |s: &[Sample]| s.iter().map(speed2d).fold(0.0f32, f32::max);
    let t_to = |s: &[Sample], v: f32| s.iter().find(|x| speed2d(x) >= v).map(|x| x.time_seconds - s[0].time_seconds);
    let rv = plateau(&run);
    let sv = plateau(&sprint);
    println!("run plateau {rv:.1} uu/s (90 % after {:?} s)", t_to(&run, 0.9 * rv));
    println!("sprint plateau {sv:.1} uu/s (90 % of the gain after {:?} s)", t_to(&sprint, rv + 0.9 * (sv - rv)));
    // Slide: from the first frame the speed leaves the sprint plateau (or the C press) to the
    // frame the speed settles.
    let x0 = slide.first().context("no slide samples")?;
    let mut dist = 0.0;
    let mut settled = None;
    for w in slide.windows(2) {
        let d = ((w[1].location[0] - w[0].location[0]).powi(2) + (w[1].location[1] - w[0].location[1]).powi(2)).sqrt();
        dist += d;
        if settled.is_none() && w[1].time_seconds - x0.time_seconds > 0.3 && (speed2d(&w[1]) - speed2d(&w[0])).abs() < 0.5 && speed2d(&w[1]) < sv - 50.0 {
            settled = Some((w[1].time_seconds - x0.time_seconds, dist, speed2d(&w[1])));
        }
    }
    println!("slide: speed profile (t, speed):");
    for x in slide.iter().step_by(6) {
        println!("  {:.3} {:.1} z {:.1} {}", x.time_seconds - x0.time_seconds, speed2d(x), x.location[2], physics_name(x.physics));
    }
    match settled {
        Some((t, d, v)) => println!("slide settled after {t:.3} s, {d:.1} uu, at {v:.1} uu/s"),
        None => println!("slide did not settle within the window ({dist:.1} uu)"),
    }
    let _ = (VK_ESCAPE, VK_SPACE);
    Ok(())
}
