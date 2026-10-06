//! Oracle measurements of powers and mana (see oracle.rs).
//!
//! measure blink [at=X,Y,Z] [--yaw D] [--pitch D] [--mana M] [--aim SECS] [--frames N]
//! measure mana [--from M] [--frames N]
//! measure windblast [--npc ADDR|nearest] [--dist D] [--mana M]
use crate::lab;
use crate::oracle::*;
use anyhow::{Context, Result, bail};
use dishonored_probe::{Client, HookDeref};
use std::fmt::Write as _;

pub const PAWN_MANA: u32 = 0xa60;
pub const BLINK_TARGET: u32 = 0x60;
pub const POWER_OWNER: u32 = 0x7c;
pub const POWER_LEVEL: u32 = 0x88;
/// `DisItemPowers.m_pCurrentActivePower` (after the inventory item fields).
pub const ITEM_POWERS_CURRENT: u32 = 0x114;

fn f32le(b: &[u8], off: usize) -> f32 {
    b.get(off..off + 4).map(|w| f32::from_le_bytes(w.try_into().unwrap())).unwrap_or(f32::NAN)
}
fn i32le(b: &[u8], off: usize) -> i32 {
    b.get(off..off + 4).map(|w| i32::from_le_bytes(w.try_into().unwrap())).unwrap_or(0)
}

pub fn set_view(c: &mut Client, pitch_deg: f32, yaw_deg: f32) -> Result<()> {
    let pc = lab::pc(c)?;
    let r = [(pitch_deg / 360.0 * 65536.0) as i32 & 0xffff, (yaw_deg / 360.0 * 65536.0) as i32 & 0xffff, 0];
    let mut b = Vec::new();
    for v in r {
        b.extend(v.to_le_bytes());
    }
    c.write(pc + ACTOR_ROTATION, &b)?;
    Ok(())
}

/// Put the pawn at `at` (cylinder centre) and let it settle on the floor.
pub fn place(c: &mut Client, at: [f32; 3]) -> Result<()> {
    let p = lab::pawn(c)?;
    let mut b = Vec::new();
    for v in at {
        b.extend(v.to_le_bytes());
    }
    c.write(p + ACTOR_LOCATION, &b)?;
    c.write(p + ACTOR_VELOCITY, &[0u8; 12])?;
    c.key("W", true)?;
    c.wait_ticks(1)?;
    c.key("W", false)?;
    c.wait_ticks(30)?;
    Ok(())
}

pub fn parse_at(o: &Opts) -> Option<[f32; 3]> {
    let s = o.rest.iter().find_map(|a| a.strip_prefix("at="))?;
    let v: Vec<f32> = s.split(',').filter_map(|x| x.parse().ok()).collect();
    (v.len() == 3).then(|| [v[0], v[1], v[2]])
}

/// The player's component of `comp_class`.
pub fn power_comp(c: &mut Client, comp_class: &str) -> Result<u32> {
    let p = lab::pawn(c)?;
    for a in c.find("", comp_class)? {
        if c.read_u32(a + POWER_OWNER).ok() == Some(p) {
            return Ok(a);
        }
    }
    bail!("no {comp_class} on the player")
}

/// Select an active power in the left hand: equip `DisItemPowers` as the secondary item and
/// point its current power at the pawn's component.
pub fn select_power(c: &mut Client, comp_class: &str) -> Result<u32> {
    let comp = power_comp(c, comp_class)?;
    let lvl = c.read_u32(comp + POWER_LEVEL)? as i32;
    if lvl <= 0 {
        bail!("{comp_class} has level {lvl} (lab power first)");
    }
    lab::equip(c, "DisItemPowers", 2)?;
    c.wait_ticks(20)?;
    let item = player_item(c, "DisItemPowers")?;
    c.write(item + ITEM_POWERS_CURRENT, &comp.to_le_bytes())?;
    c.wait_ticks(20)?;
    Ok(comp)
}

pub fn hdist(a: [f32; 3], b: [f32; 3]) -> f32 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2)).sqrt()
}

pub fn blink(c: &mut Client, o: &Opts) -> Result<()> {
    let comp = select_power(c, "DishonoredActivePowerComponent_Blink")?;
    if let Some(at) = parse_at(o) {
        place(c, at)?;
    }
    set_view(c, o.num("--pitch", 0.0), o.num("--yaw", 0.0))?;
    let p = lab::pawn(c)?;
    let mana: i32 = o.num("--mana", 100);
    c.write(p + PAWN_MANA, &mana.to_le_bytes())?;
    c.wait_ticks(10)?;
    c.watch(&[(PAWN_MANA, 4)])?;
    c.watch_abs(&[(comp + BLINK_TARGET, 12), (comp + 0xa0, 4)])?;
    c.sample_start(1_000_000)?;
    c.wait_ticks(3)?;
    let pc = lab::pc(c)?;
    let cam = c.read_u32(pc + PC_CAMERA)?;
    c.key("RightMouseButton", true)?;
    c.wait_ticks((o.num("--aim", 0.5f32) * 30.0) as u32)?;
    let eye = lab::read_vec(c, cam + 0x32c + 4)?;
    let target = lab::read_vec(c, comp + BLINK_TARGET)?;
    c.key("RightMouseButton", false)?;
    c.wait_ticks(o.num("--frames", 150))?;
    let s = c.sample_fetch()?;
    c.watch(&[])?;
    c.watch_abs(&[])?;
    let mut csv = String::from("time,x,y,z,vx,vy,vz,physics,mana,tx,ty,tz,active\n");
    for x in &s {
        let e = &x.extra;
        let _ = writeln!(
            csv,
            "{:.4},{:.2},{:.2},{:.2},{:.1},{:.1},{:.1},{},{},{:.1},{:.1},{:.1},{}",
            x.time_seconds,
            x.location[0],
            x.location[1],
            x.location[2],
            x.velocity[0],
            x.velocity[1],
            x.velocity[2],
            x.physics,
            i32le(e, 0),
            f32le(e, 4),
            f32le(e, 8),
            f32le(e, 12),
            i32le(e, 16) & 1
        );
    }
    o.save("blink.csv", &csv)?;
    let start = s.first().context("no samples")?;
    println!(
        "eye ({:.1}, {:.1}, {:.1}), pawn ({:.1}, {:.1}, {:.1}); target ({:.1}, {:.1}, {:.1}): {:.1} uu from the eye horizontally, dz {:.1}",
        eye[0], eye[1], eye[2], start.location[0], start.location[1], start.location[2], target[0], target[1], target[2], hdist(eye, target), target[2] - eye[2]
    );
    // The move: PHYS_Flying (4) samples.
    let first = s.iter().position(|x| x.physics == 4);
    let last = s.iter().rposition(|x| x.physics == 4);
    if let (Some(fi), Some(li)) = (first, last) {
        let pre = &s[fi.saturating_sub(1)];
        let fin = &s[(li + 1).min(s.len() - 1)];
        println!(
            "move: {} flying samples, {:.3} s from the last pre-move sample to the first after; horizontal {:.1} uu, dz {:.1}; final pawn to target: horizontal {:.1}, dz {:.1}; end z {:.1}",
            li - fi + 1,
            fin.time_seconds - pre.time_seconds,
            hdist(pre.location, fin.location),
            fin.location[2] - pre.location[2],
            hdist(fin.location, target),
            fin.location[2] - target[2],
            s.last().unwrap().location[2]
        );
        let steps: Vec<String> = s[fi.saturating_sub(1)..=(li + 1).min(s.len() - 1)].windows(2).filter(|w| w[1].time_seconds > w[0].time_seconds).map(|w| format!("{:.0}/{:.3}", hdist(w[0].location, w[1].location), w[1].time_seconds - w[0].time_seconds)).collect();
        println!("  steps between ticks (uu/dt): {}", steps.join(" "));
    } else {
        println!("no Blink move seen (physics never Flying)");
    }
    let m0 = i32le(&start.extra, 0);
    let m_min = s.iter().map(|x| i32le(&x.extra, 0)).min().unwrap_or(m0);
    let low_t = s.iter().find(|x| i32le(&x.extra, 0) == m_min).map(|x| x.time_seconds).unwrap_or(0.0);
    let regen0 = s.iter().find(|x| x.time_seconds > low_t && i32le(&x.extra, 0) > m_min).map(|x| x.time_seconds - low_t);
    let m_end = i32le(&s.last().unwrap().extra, 0);
    println!("mana {m0} -> {m_min} (cost {}), first regen {:?} s after the spend, {m_end} at the end", m0 - m_min, regen0);
    Ok(())
}

pub fn mana(c: &mut Client, o: &Opts) -> Result<()> {
    let p = lab::pawn(c)?;
    let from: i32 = o.num("--from", 5);
    c.watch(&[(PAWN_MANA, 4), (0x1224, 8)])?;
    c.sample_start(1_000_000)?;
    c.write(p + PAWN_MANA, &from.to_le_bytes())?;
    c.wait_ticks(o.num("--frames", 150))?;
    let s = c.sample_fetch()?;
    c.watch(&[])?;
    let mut last = -1;
    let t0 = s.first().map(|x| x.time_seconds).unwrap_or(0.0);
    let mut csv = String::from("time,mana,countdown,pool\n");
    for x in &s {
        let m = i32le(&x.extra, 0);
        let _ = writeln!(csv, "{:.4},{m},{:.3},{}", x.time_seconds, f32le(&x.extra, 4), i32le(&x.extra, 8));
        if m != last {
            println!("  {:.3} s: mana {m} (countdown {:.3}, pool {})", x.time_seconds - t0, f32le(&x.extra, 4), i32le(&x.extra, 8));
            last = m;
        }
    }
    o.save("mana.csv", &csv)?;
    Ok(())
}

/// Windblast at an NPC `--dist` uu ahead: the damage path's records (damage, momentum,
/// damage type) and the NPC's motion.
pub fn windblast(c: &mut Client, o: &Opts) -> Result<()> {
    let comp = select_power(c, "DishonoredActivePowerComponent_WindBlast")?;
    let npc = match o.rest.iter().find_map(|a| a.strip_prefix("npc=")) {
        Some(a) => lab::parse_u32(a)?,
        None => lab::npcs(c)?.into_iter().find(|n| n.health > 0).context("no NPC")?.addr,
    };
    let p = lab::pawn(c)?;
    let dist: f32 = o.num("--dist", 400.0);
    // Stand `dist` from the NPC, facing it.
    let nl = lab::read_vec(c, npc + ACTOR_LOCATION)?;
    let me = lab::read_vec(c, p + ACTOR_LOCATION)?;
    let (dx, dy) = (me[0] - nl[0], me[1] - nl[1]);
    let n = (dx * dx + dy * dy).sqrt().max(1.0);
    let at = [nl[0] + dx / n * dist, nl[1] + dy / n * dist, nl[2] + 10.0];
    place(c, at)?;
    let nl = lab::read_vec(c, npc + ACTOR_LOCATION)?;
    let me = lab::read_vec(c, p + ACTOR_LOCATION)?;
    let yaw = (nl[1] - me[1]).atan2(nl[0] - me[0]).to_degrees();
    set_view(c, o.num("--pitch", 0.0), yaw)?;
    let mana: i32 = o.num("--mana", 100);
    c.write(p + PAWN_MANA, &mana.to_le_bytes())?;
    let h = c.hook_set(FN_PAWN_DAMAGE.0, FN_PAWN_DAMAGE.1, 7, 1000, &[HookDeref::arg(0, 0, 4), HookDeref::arg(5, 0, 12), HookDeref::ecx(PAWN_HEALTH, 4)])?;
    c.watch(&[(PAWN_MANA, 4)])?;
    c.watch_abs(&[(npc + ACTOR_LOCATION, 12), (npc + ACTOR_VELOCITY, 12), (npc + PAWN_HEALTH, 4), (npc + ACTOR_PHYSICS, 1), (comp + 0x90, 1)])?;
    c.sample_start(1_000_000)?;
    c.wait_ticks(5)?;
    let t_press = world_time(c)?;
    c.key("RightMouseButton", true)?;
    c.wait_ticks(2)?;
    c.key("RightMouseButton", false)?;
    c.wait_ticks(o.num("--frames", 90))?;
    let s = c.sample_fetch()?;
    let recs = c.hook_fetch(h)?;
    let _ = c.hook_set(FN_PAWN_DAMAGE.0, FN_PAWN_DAMAGE.1, 0, 0, &[]);
    c.watch(&[])?;
    c.watch_abs(&[])?;
    println!("NPC {npc:#x} at {:.0} uu (horizontal {:.0}), press at {t_press:.3}", ((nl[0] - me[0]).powi(2) + (nl[1] - me[1]).powi(2) + (nl[2] - me[2]).powi(2)).sqrt(), hdist(nl, me));
    for r in &recs {
        let dt = c.read_u32(r.args[6]).ok().and_then(|_| c.name_of(r.args[6]).ok()).unwrap_or_default();
        println!(
            "  damage {:.3} s after the press: pawn {:#x}{} amount {} momentum ({:.0}, {:.0}, {:.0}) |m| {:.0} health {} type {}",
            r.time - t_press,
            r.ecx,
            if r.ecx == npc { " (target)" } else { "" },
            i32le(&r.data, 0),
            f32le(&r.data, 4),
            f32le(&r.data, 8),
            f32le(&r.data, 12),
            (f32le(&r.data, 4).powi(2) + f32le(&r.data, 8).powi(2) + f32le(&r.data, 12).powi(2)).sqrt(),
            i32le(&r.data, 16),
            dt.rsplit('.').next().unwrap_or("")
        );
    }
    let mut csv = String::from("time,mana,nx,ny,nz,nvx,nvy,nvz,nhealth,nphys,mode\n");
    for x in &s {
        let e = &x.extra;
        let _ = writeln!(csv, "{:.4},{},{:.1},{:.1},{:.1},{:.1},{:.1},{:.1},{},{},{}", x.time_seconds, i32le(e, 0), f32le(e, 4), f32le(e, 8), f32le(e, 12), f32le(e, 16), f32le(e, 20), f32le(e, 24), i32le(e, 28), e.get(32).copied().unwrap_or(0), e.get(33).copied().unwrap_or(0));
    }
    o.save("windblast.csv", &csv)?;
    let m0 = s.first().map(|x| i32le(&x.extra, 0)).unwrap_or(0);
    let m1 = s.iter().map(|x| i32le(&x.extra, 0)).min().unwrap_or(0);
    let vmax = s.iter().map(|x| (f32le(&x.extra, 16).powi(2) + f32le(&x.extra, 20).powi(2)).sqrt()).fold(0.0f32, f32::max);
    let moved = match (s.first(), s.last()) {
        (Some(a), Some(b)) => ((f32le(&a.extra, 4) - f32le(&b.extra, 4)).powi(2) + (f32le(&a.extra, 8) - f32le(&b.extra, 8)).powi(2)).sqrt(),
        _ => 0.0,
    };
    println!("mana {m0} -> {m1}; NPC max horizontal speed {vmax:.0}, moved {moved:.0} uu");
    Ok(())
}
