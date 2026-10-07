//! Oracle measurements of presentation details (HOOK_TRIAGE.md planned rows).
//!
//! measure lean [at=X,Y,Z] [--yaw DEG] [--hold FRAMES] [--out DIR]
//!     Holds Q (lean left) then E (lean right) while standing still and samples the camera
//!     (`PlayerCamera.CameraCache.POV` location / rotation) every frame: the lean offset to the
//!     side and down relative to the standing eye, and its time course. Run at a fixed step
//!     (`lab fixedstep 60`) for tick-exact times.
use crate::oracle::{ACTOR_LOCATION, ACTOR_PHYSICS, ACTOR_ROTATION, ACTOR_VELOCITY, Opts, PC_CAMERA};
use anyhow::{Context, Result};
use dishonored_probe::Client;
use std::fmt::Write as _;

/// `CameraCache.POV`: time at +0x32C, then location (12), rotation (12), FOV.
const CAMERA_POV_LOCATION: u32 = 0x32c + 4;

fn status_hex(c: &mut Client, key: &str) -> Result<u32> {
    let s = c.status()?;
    let v = s.lines().find_map(|l| l.strip_prefix(&format!("{key}="))).context(key.to_string())?;
    Ok(u32::from_str_radix(v.trim_start_matches("0x"), 16)?)
}

fn ticks(c: &mut Client) -> Result<u32> {
    let s = c.status()?;
    Ok(s.lines().find_map(|l| l.strip_prefix("ticks=")).context("ticks")?.parse()?)
}

fn f3(b: &[u8], off: usize) -> [f32; 3] {
    [0, 1, 2].map(|k| f32::from_le_bytes(b[off + k * 4..off + k * 4 + 4].try_into().unwrap()))
}

fn i3(b: &[u8], off: usize) -> [i32; 3] {
    [0, 1, 2].map(|k| i32::from_le_bytes(b[off + k * 4..off + k * 4 + 4].try_into().unwrap()))
}

pub fn lean(c: &mut Client, o: &Opts) -> Result<()> {
    let pc = status_hex(c, "local_pc")?;
    let pawn = status_hex(c, "pawn")?;
    let cam = c.read_u32(pc + PC_CAMERA)?;
    let hold: u32 = o.num("--hold", 90);
    if let Some(at) = o.rest.iter().find_map(|a| a.strip_prefix("at=")) {
        let v: Vec<f32> = at.split(',').filter_map(|s| s.parse().ok()).collect();
        let mut b = Vec::new();
        for x in v.iter().take(3) {
            b.extend(x.to_le_bytes());
        }
        c.write(pawn + ACTOR_LOCATION, &b)?;
        c.write(pawn + ACTOR_VELOCITY, &[0u8; 12])?;
        // PHYS_Falling so it lands and walks again after a frozen viewpoint.
        c.write(pawn + ACTOR_PHYSICS, &[2u8])?;
        c.wait_ticks(60)?;
    }
    if let Some(y) = o.rest.iter().position(|a| a == "--yaw").and_then(|i| o.rest.get(i + 1)).and_then(|s| s.parse::<f32>().ok()) {
        let r = [0i32, (y / 360.0 * 65536.0) as i32, 0];
        let mut b = Vec::new();
        for v in r {
            b.extend(v.to_le_bytes());
        }
        c.write(pc + ACTOR_ROTATION, &b)?;
        c.wait_ticks(20)?;
    }
    c.watch_abs(&[(cam + CAMERA_POV_LOCATION, 24)])?;
    c.sample_start(100_000)?;
    c.wait_ticks(30)?;
    let mut marks = Vec::new();
    for key in ["Q", "E"] {
        marks.push((key, true, ticks(c)?));
        c.key(key, true)?;
        c.wait_ticks(hold)?;
        marks.push((key, false, ticks(c)?));
        c.key(key, false)?;
        c.wait_ticks(hold)?;
    }
    let s = c.sample_fetch()?;
    c.watch_abs(&[])?;
    let mut csv = String::from("tick,time,pawn_x,pawn_y,pawn_z,cam_x,cam_y,cam_z,pitch,yaw,roll\n");
    for x in &s {
        if x.extra.len() < 24 {
            continue;
        }
        let l = f3(&x.extra, 0);
        let r = i3(&x.extra, 12);
        let _ = writeln!(csv, "{},{:.4},{:.2},{:.2},{:.2},{:.2},{:.2},{:.2},{},{},{}", x.tick, x.time_seconds, x.location[0], x.location[1], x.location[2], l[0], l[1], l[2], r[0], r[1], r[2]);
    }
    o.save("lean.csv", &csv)?;
    // Summary: offsets relative to the first sample, in the view's right / up frame.
    let Some(first) = s.iter().find(|x| x.extra.len() >= 24) else { return Ok(()) };
    let base = f3(&first.extra, 0);
    let yaw = (i3(&first.extra, 12)[1] as f32) / 65536.0 * std::f32::consts::TAU;
    let right = [-yaw.sin(), yaw.cos()];
    let t0 = first.time_seconds;
    println!("marks (key, down, tick): {marks:?}");
    println!("time,side,down,roll_deg");
    for x in s.iter().filter(|x| x.extra.len() >= 24).step_by(3) {
        let l = f3(&x.extra, 0);
        let side = (l[0] - base[0]) * right[0] + (l[1] - base[1]) * right[1];
        let roll = i3(&x.extra, 12)[2] as f32 / 65536.0 * 360.0;
        println!("{:.3},{:.2},{:.2},{:.2}", x.time_seconds - t0, side, base[2] - l[2], roll);
    }
    Ok(())
}
