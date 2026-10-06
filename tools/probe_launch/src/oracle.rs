//! Oracle measurements for open approximations (research/dishonored/HOOK_TRIAGE.md): weapons,
//! combat, powers and stealth on the running original, through entry hooks on the native
//! functions the decompilation named, per-frame samples and memory reads.
//!
//! measure gun [--shots N] [--rapid SECS] [--out DIR]
//!     Pistol shots: the dispersion offset of every shot (0x00AA2B10 entry: offset in 720-line
//!     pixels, aim point, the weapon's m_fDispersion), the traced shot (0x00BE1560 from the
//!     pistol's fire 0x00C2A3E0: start / end) against the camera rotation, and the reload
//!     timeline (fire request, reload start 0x00C2AA80, timed stage 0x00C23620, magazine load
//!     0x00C28C50, context end 0x00C236A0). `--rapid` taps fire every other frame for SECS.
use crate::lab;
use anyhow::{Context, Result, bail};
use dishonored_probe::{Client, HookDeref, HookRecord};
use std::fmt::Write as _;
use std::path::PathBuf;
use std::time::{Duration, Instant};

pub struct Opts {
    pub out: Option<PathBuf>,
    pub rest: Vec<String>,
}

impl Opts {
    pub fn num<T: std::str::FromStr>(&self, flag: &str, default: T) -> T {
        self.rest.iter().position(|a| a == flag).and_then(|i| self.rest.get(i + 1)).and_then(|s| s.parse().ok()).unwrap_or(default)
    }
    pub fn has(&self, flag: &str) -> bool {
        self.rest.iter().any(|a| a == flag)
    }
    pub fn save(&self, name: &str, text: &str) -> Result<()> {
        if let Some(d) = &self.out {
            std::fs::create_dir_all(d)?;
            std::fs::write(d.join(name), text)?;
        }
        Ok(())
    }
}

// --- native functions (Ghidra addresses = runtime addresses, no ASLR) and their prologues ---
/// Screen offset -> shot direction (`(offset px, aim NDC, out dir)`, ecx unused).
pub const FN_OFFSET_DIR: (u32, u8) = (0x00AA2B10, 6);
/// Line / extent trace (`(actor, hit, start, end, flags, ...)`).
pub const FN_TRACE: (u32, u8) = (0x00BE1560, 6);
/// Pistol fire (`DishonoredWepPistol`, thiscall).
pub const FN_PISTOL_FIRE: u32 = 0x00C2A3E0;
/// Reload context start / continue (slot 101), timed stage start, end (slot 104).
pub const FN_RELOAD_START: (u32, u8) = (0x00C2AA80, 6);
pub const FN_RELOAD_TIMED: (u32, u8) = (0x00C23620, 7);
pub const FN_RELOAD_END: (u32, u8) = (0x00C236A0, 9);
/// Magazine load (weapon, thiscall).
pub const FN_MAG_LOAD: (u32, u8) = (0x00C28C50, 10);
/// Pawn damage path (thiscall pawn, `(int* damage, instigator, hit location, momentum..., damage
/// type class at arg 6)`).
pub const FN_PAWN_DAMAGE: (u32, u8) = (0x00B5CCD0, 6);

pub const ACTOR_LOCATION: u32 = 0xc4;
pub const ACTOR_ROTATION: u32 = 0xd0;
pub const ACTOR_PHYSICS: u32 = 0x104;
pub const ACTOR_VELOCITY: u32 = 0x1b4;
pub const PAWN_HEALTH: u32 = 0x344;
pub const PAWN_INVENTORY: u32 = 0x59c;
pub const PC_CAMERA: u32 = 0x384;
/// `PlayerCamera.CameraCache.POV` rotation (CameraCache +0x32C: time, location, rotation, FOV).
pub const CAMERA_POV_ROT: u32 = 0x33c;
pub const CAMERA_POV_FOV: u32 = 0x348;
/// `DishonoredWeapon_Ranged`: loaded rounds, `m_fDispersion`.
pub const RANGED_LOADED: u32 = 0x124;
pub const RANGED_DISPERSION: u32 = 0x12c;
/// `DishonoredInventory.m_AmmoInfo` (TArray of (count, capacity); bullets first).
pub const INV_AMMO_INFO: u32 = 0xbc;

fn f32le(b: &[u8], off: usize) -> f32 {
    f32::from_le_bytes(b[off..off + 4].try_into().unwrap())
}
fn i32le(b: &[u8], off: usize) -> i32 {
    i32::from_le_bytes(b[off..off + 4].try_into().unwrap())
}

/// The player's inventory item of `class` (owning inventory +0x54 = the pawn's inventory).
pub fn player_item(c: &mut Client, class: &str) -> Result<u32> {
    let p = lab::pawn(c)?;
    let inv = c.read_u32(p + PAWN_INVENTORY)?;
    // Prefer the equipped instance (`m_CurSocket` +0x59 = ItemSocket_Equipped): giving an
    // item that is already held leaves a second, unequipped copy.
    let mut first = None;
    for a in c.find("", class)? {
        if c.read_u32(a + 0x54).ok() == Some(inv) {
            if c.read(a + 0x59, 1)?.first() == Some(&1) {
                return Ok(a);
            }
            first.get_or_insert(a);
        }
    }
    first.with_context(|| format!("the player holds no {class}"))
}

pub fn world_time(c: &mut Client) -> Result<f32> {
    let pc = lab::pc(c)?;
    let wi = c.read_u32(pc + 0x160)?;
    Ok(c.read_f32(wi + 0x38c)?)
}

fn refill_bullets(c: &mut Client) -> Result<()> {
    let p = lab::pawn(c)?;
    let inv = c.read_u32(p + PAWN_INVENTORY)?;
    let arr = c.read_u32(inv + INV_AMMO_INFO)?;
    c.write(arr, &10i32.to_le_bytes())?;
    Ok(())
}

fn tap(c: &mut Client, key: &str, frames: u32) -> Result<()> {
    c.key(key, true)?;
    c.wait_ticks(frames)?;
    c.key(key, false)?;
    Ok(())
}

/// Unreal rotator units to radians.
fn rot_rad(u: i32) -> f64 {
    (u as f64) * std::f64::consts::TAU / 65536.0
}

/// Unit view vector of an Unreal (pitch, yaw) in rotator units.
fn view_dir(pitch: i32, yaw: i32) -> [f64; 3] {
    let (p, y) = (rot_rad(pitch), rot_rad(yaw));
    [p.cos() * y.cos(), p.cos() * y.sin(), p.sin()]
}

fn angle_between(a: [f64; 3], b: [f64; 3]) -> f64 {
    let d = a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
    let na = (a[0] * a[0] + a[1] * a[1] + a[2] * a[2]).sqrt();
    let nb = (b[0] * b[0] + b[1] * b[1] + b[2] * b[2]).sqrt();
    (d / (na * nb)).clamp(-1.0, 1.0).acos()
}

/// Port reference (`left_hand::offset_direction`): the angle of a 720-line pixel offset of
/// radius r through a 75 degree horizontal FOV on 16:9.
fn port_angle(r: f64) -> f64 {
    let k = 2.0 * (75f64.to_radians() * 0.5).tan() / (720.0 * 16.0 / 9.0);
    (r * k).atan()
}

pub fn gun(c: &mut Client, o: &Opts) -> Result<()> {
    // --weapon crossbow: the crossbow (DisWepCrossbow, minimum dispersion 0) instead of the pistol.
    let crossbow = o.rest.iter().position(|a| a == "--weapon").and_then(|i| o.rest.get(i + 1)).is_some_and(|w| w == "crossbow");
    let (class, dmin) = if crossbow { ("DisWepCrossbow", 0.0f32) } else { ("DishonoredWepPistol", 15.0f32) };
    let pistol = player_item(c, class).context("give the weapon first: lab give Twk_Inv_PistolCorvo / Twk_Inv_CrossbowCorvo")?;
    let pc = lab::pc(c)?;
    let cam = c.read_u32(pc + PC_CAMERA)?;
    let fov = c.read_f32(cam + CAMERA_POV_FOV)?;
    println!("pistol {pistol:#x}, camera FOV {fov:.2}");
    let shots: usize = o.num("--shots", 40);
    let rapid: f32 = o.num("--rapid", 0.0);
    let h_off = c.hook_set(FN_OFFSET_DIR.0, FN_OFFSET_DIR.1, 3, 10_000, &[HookDeref::arg(0, 0, 8), HookDeref::arg(1, 0, 8), HookDeref { src: HookDeref::ABS, pre: u32::MAX, off: pistol + RANGED_DISPERSION, len: 4 }])?;
    let h_trace = c.hook_set(
        FN_TRACE.0,
        FN_TRACE.1,
        5,
        100_000,
        &[HookDeref::arg(2, 0, 12), HookDeref::arg(3, 0, 12), HookDeref { src: HookDeref::ABS, pre: pc + PC_CAMERA, off: CAMERA_POV_ROT, len: 12 }],
    )?;
    let h_start = c.hook_set(FN_RELOAD_START.0, FN_RELOAD_START.1, 0, 10_000, &[])?;
    let h_timed = c.hook_set(FN_RELOAD_TIMED.0, FN_RELOAD_TIMED.1, 0, 10_000, &[])?;
    let h_end = c.hook_set(FN_RELOAD_END.0, FN_RELOAD_END.1, 1, 10_000, &[])?;
    let h_load = c.hook_set(FN_MAG_LOAD.0, FN_MAG_LOAD.1, 0, 10_000, &[HookDeref::ecx(RANGED_LOADED, 4)])?;
    c.watch_abs(&[(pistol + RANGED_LOADED, 4), (pistol + RANGED_DISPERSION, 4)])?;
    c.sample_start(1_000_000)?;
    let mut presses = Vec::new();
    let mut trace_recs: Vec<HookRecord> = Vec::new();
    let t0 = Instant::now();
    if rapid > 0.0 {
        let end = world_time(c)? + rapid;
        let mut k = 0;
        while world_time(c)? < end {
            if k % 8 == 0 {
                refill_bullets(c)?;
            }
            k += 1;
            tap(c, "RightMouseButton", 1)?;
            c.wait_ticks(1)?;
            trace_recs.extend(c.hook_fetch(h_trace)?.into_iter().filter(|r| (FN_PISTOL_FIRE..FN_PISTOL_FIRE + 0x1000).contains(&r.ret)));
        }
    } else {
        for k in 0..shots {
            // Ready: a round loaded and the dispersion settled at its minimum.
            let t = Instant::now();
            loop {
                let b = c.read(pistol + RANGED_LOADED, 12)?;
                if i32le(&b, 0) > 0 && (f32le(&b, 8) - dmin).abs() < 0.01 {
                    break;
                }
                if t.elapsed() > Duration::from_secs(8) {
                    bail!("shot {k}: the pistol did not settle (loaded {}, dispersion {})", i32le(&b, 0), f32le(&b, 8));
                }
                std::thread::sleep(Duration::from_millis(30));
            }
            refill_bullets(c)?;
            let _ = c.hook_fetch(h_trace)?;
            presses.push(world_time(c)?);
            tap(c, "RightMouseButton", 2)?;
            c.wait_ticks(3)?;
            trace_recs.extend(c.hook_fetch(h_trace)?.into_iter().filter(|r| (FN_PISTOL_FIRE..FN_PISTOL_FIRE + 0x1000).contains(&r.ret)));
        }
        // Let the last reload finish.
        c.wait_ticks(90)?;
    }
    let samples = c.sample_fetch()?;
    println!("{} presses in {:.1} s", presses.len(), t0.elapsed().as_secs_f32());
    let offs = c.hook_fetch(h_off)?;
    let starts = c.hook_fetch(h_start)?;
    let timed = c.hook_fetch(h_timed)?;
    let ends = c.hook_fetch(h_end)?;
    let loads: Vec<HookRecord> = c.hook_fetch(h_load)?.into_iter().filter(|r| r.ecx == pistol).collect();
    for (h, a) in [(h_off, FN_OFFSET_DIR), (h_trace, FN_TRACE), (h_start, FN_RELOAD_START), (h_timed, FN_RELOAD_TIMED), (h_end, FN_RELOAD_END), (h_load, FN_MAG_LOAD)] {
        // Disable (max 0) so the hooks cost nothing between measurements.
        let _ = c.hook_set(a.0, a.1, 0, 0, &[]);
        let _ = h;
    }
    c.watch_abs(&[])?;
    // --- the shots ---
    let mut csv = String::from("time,dx,dy,r,dispersion,r_over_d,angle_deg,aim_x,aim_y\n");
    let mut ratios = Vec::new();
    let mut angles = Vec::new();
    let mut shot_times = Vec::new();
    let mut disp_at_shot = Vec::new();
    for r in &offs {
        let (dx, dy, ax, ay, d) = (r.f32_at(0), r.f32_at(4), r.f32_at(8), r.f32_at(12), r.f32_at(16));
        let rad = (dx * dx + dy * dy).sqrt();
        let ang = dy.atan2(dx).to_degrees();
        let _ = writeln!(csv, "{:.4},{dx:.4},{dy:.4},{rad:.4},{d:.3},{:.4},{ang:.2},{ax:.4},{ay:.4}", r.time, rad / d);
        if d > 0.0 {
            ratios.push((rad / d) as f64);
        }
        angles.push(ang as f64);
        shot_times.push(r.time);
        disp_at_shot.push(d);
    }
    o.save("gun_shots.csv", &csv)?;
    let n = ratios.len().max(1) as f64;
    let mean = ratios.iter().sum::<f64>() / n;
    let var = ratios.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / n;
    let maxr = ratios.iter().cloned().fold(0.0, f64::max);
    // Kolmogorov-Smirnov distance of r/d against U(0, 1) (uniform radius).
    let mut sorted = ratios.clone();
    sorted.sort_by(|a, b| a.total_cmp(b));
    let ks = sorted.iter().enumerate().map(|(i, &x)| ((i + 1) as f64 / n - x).abs().max((x - i as f64 / n).abs())).fold(0.0, f64::max);
    let amean = angles.iter().sum::<f64>() / angles.len().max(1) as f64;
    let quad = [0, 1, 2, 3].map(|q| angles.iter().filter(|&&a| ((a + 180.0) / 90.0).floor() as i32 == q).count());
    println!("shots {}: r/d mean {mean:.3} (uniform radius 0.5), var {var:.4} (1/12 = 0.0833), max {maxr:.3}, KS {ks:.3} (5% critical {:.3})", ratios.len(), 1.36 / n.sqrt());
    println!("  angle mean {amean:.1} deg, per quadrant {quad:?}; dispersion at the shot: {:?}", disp_at_shot.iter().map(|d| format!("{d:.1}")).collect::<Vec<_>>().join(" "));
    // --- traced direction vs the camera ---
    let mut tcsv = String::from("time,cam_pitch,cam_yaw,dir_x,dir_y,dir_z,angle_from_view_deg,offset_r,port_angle_deg\n");
    let mut errs = Vec::new();
    for t in &trace_recs {
        let s = [t.f32_at(0) as f64, t.f32_at(4) as f64, t.f32_at(8) as f64];
        let e = [t.f32_at(12) as f64, t.f32_at(16) as f64, t.f32_at(20) as f64];
        let rot = [t.u32_at(24) as i32, t.u32_at(28) as i32];
        let dir = [e[0] - s[0], e[1] - s[1], e[2] - s[2]];
        let a = angle_between(dir, view_dir(rot[0], rot[1])).to_degrees();
        // The offset logged just before this trace.
        let off = offs.iter().rev().find(|r| r.time <= t.time + 1e-4);
        let (r, pa) = match off {
            Some(r) => {
                let rr = ((r.f32_at(0) as f64).powi(2) + (r.f32_at(4) as f64).powi(2)).sqrt();
                (rr, port_angle(rr).to_degrees())
            }
            None => (f64::NAN, f64::NAN),
        };
        if pa.is_finite() {
            errs.push(a - pa);
        }
        let _ = writeln!(tcsv, "{:.4},{},{},{:.5},{:.5},{:.5},{a:.4},{r:.3},{pa:.4}", t.time, rot[0], rot[1], dir[0], dir[1], dir[2]);
    }
    o.save("gun_traces.csv", &tcsv)?;
    if !errs.is_empty() {
        let me = errs.iter().sum::<f64>() / errs.len() as f64;
        let mx = errs.iter().map(|e| e.abs()).fold(0.0, f64::max);
        println!("traced shots {}: angle from the view minus the port's angle for the same offset: mean {me:.4} deg, max |.| {mx:.4} deg", errs.len());
    }
    // --- reload timeline ---
    let mut tl = String::from("event,time\n");
    for (name, v) in [("press", presses.iter().map(|&t| t).collect::<Vec<_>>()), ("shot", shot_times.clone()), ("reload_start", starts.iter().map(|r| r.time).collect()), ("timed_stage", timed.iter().map(|r| r.time).collect()), ("load", loads.iter().map(|r| r.time).collect()), ("context_end", ends.iter().map(|r| r.time).collect())] {
        for t in v {
            let _ = writeln!(tl, "{name},{t:.4}");
        }
    }
    o.save("gun_timeline.csv", &tl)?;
    println!("per shot (s after the shot): reload start / timed stage / magazine load / context end, next shot");
    for (i, &st) in shot_times.iter().enumerate() {
        let next = shot_times.get(i + 1).copied().unwrap_or(f32::INFINITY);
        let first = |v: &[HookRecord]| v.iter().map(|r| r.time).find(|&t| t >= st && t < next).map(|t| format!("{:.3}", t - st)).unwrap_or("-".into());
        println!("  {:.3}: {} / {} / {} / {}, next {}", st, first(&starts), first(&timed), first(&loads), first(&ends), if next.is_finite() { format!("{:.3}", next - st) } else { "-".into() });
    }
    // Dispersion after the load: time to settle at the minimum.
    let mut scsv = String::from("time,loaded,dispersion\n");
    for s in &samples {
        if s.extra.len() >= 8 {
            let _ = writeln!(scsv, "{:.4},{},{:.3}", s.time_seconds, i32le(&s.extra, 0), f32le(&s.extra, 4));
        }
    }
    o.save("gun_samples.csv", &scsv)?;
    for l in &loads {
        let after: Vec<_> = samples.iter().filter(|s| s.time_seconds >= l.time && s.extra.len() >= 8).collect();
        let settled = after.iter().find(|s| (f32le(&s.extra, 4) - dmin).abs() < 0.01).map(|s| s.time_seconds - l.time);
        // Slope while narrowing: the first two samples strictly between max and min.
        let mid: Vec<_> = after.iter().filter(|s| { let d = f32le(&s.extra, 4); d < 44.9 && d > 15.1 }).take(2).collect();
        let slope = if mid.len() == 2 { (f32le(&mid[0].extra, 4) - f32le(&mid[1].extra, 4)) / (mid[1].time_seconds - mid[0].time_seconds) } else { f32::NAN };
        println!("  load at {:.3}: dispersion at min after {:?} s, slope {slope:.1}/s", l.time, settled.map(|s| (s * 1000.0).round() / 1000.0));
    }
    Ok(())
}
