//! Oracle measurements of sword combat (see oracle.rs).
//!
//! measure sword [npc=ADDR] [--swings N] [--dist D] [--gap SECS] [--defend SECS] [--block]
//!     The player stands `--dist` uu in front of an NPC, facing it, sword drawn, and swings N
//!     times (`LeftMouseButton` taps `--gap` s apart); then stays `--defend` s (holding block
//!     with `--block`). Every call of the pawn damage path (0x00B5CCD0) is logged: the damaged
//!     pawn, the amount, the instigator, the momentum and the damage type, with the health
//!     before; presses are logged with the world time.
use crate::lab;
use crate::oracle::*;
use crate::oracle_powers::{hdist, place, set_view};
use anyhow::{Context, Result};
use dishonored_probe::{Client, HookDeref};
use std::fmt::Write as _;

fn f32le(b: &[u8], off: usize) -> f32 {
    b.get(off..off + 4).map(|w| f32::from_le_bytes(w.try_into().unwrap())).unwrap_or(f32::NAN)
}
fn i32le(b: &[u8], off: usize) -> i32 {
    b.get(off..off + 4).map(|w| i32::from_le_bytes(w.try_into().unwrap())).unwrap_or(0)
}

/// Face `target` (an NPC's location) from `me` (the player's): yaw, and the pitch from the eye
/// (64 uu above the player's centre) to the NPC's chest (30 uu above its centre).
pub fn aim_at(c: &mut Client, me: [f32; 3], target: [f32; 3]) -> Result<()> {
    let h = hdist(me, target).max(1.0);
    let pitch = ((target[2] + 30.0) - (me[2] + 64.0)).atan2(h).to_degrees();
    set_view(c, pitch, (target[1] - me[1]).atan2(target[0] - me[0]).to_degrees())
}

pub fn damage_hook(c: &mut Client) -> Result<u32> {
    Ok(c.hook_set(FN_PAWN_DAMAGE.0, FN_PAWN_DAMAGE.1, 7, 10_000, &[HookDeref::arg(0, 0, 4), HookDeref::arg(5, 0, 12), HookDeref::ecx(PAWN_HEALTH, 4)])?)
}

pub fn print_damage(c: &mut Client, recs: &[dishonored_probe::HookRecord], t0: f32, player: u32, npc: u32, out: &mut String) -> Result<()> {
    for r in recs {
        let dt = c.name_of(r.args[6]).unwrap_or_default();
        let who = if r.ecx == player { "player".to_string() } else if r.ecx == npc { "npc".to_string() } else { format!("{:#x}", r.ecx) };
        let by = if r.args[1] == 0 { "-".to_string() } else { c.name_of(r.args[1]).unwrap_or_default().rsplit('.').next().unwrap_or("").to_string() };
        let line = format!(
            "{:.3},{who},{},{},{:.0},{:.0},{:.0},{by},{}",
            r.time - t0,
            i32le(&r.data, 0),
            i32le(&r.data, 16),
            f32le(&r.data, 4),
            f32le(&r.data, 8),
            f32le(&r.data, 12),
            dt.rsplit('.').next().unwrap_or("")
        );
        println!("  damage {line}");
        let _ = writeln!(out, "{line}");
    }
    Ok(())
}

pub fn sword(c: &mut Client, o: &Opts) -> Result<()> {
    let npc = match o.rest.iter().find_map(|a| a.strip_prefix("npc=")) {
        Some(a) => lab::parse_u32(a)?,
        None => lab::npcs(c)?.into_iter().find(|n| n.health > 0 && n.tweak.contains("Guard")).context("no guard")?.addr,
    };
    let p = lab::pawn(c)?;
    let dist: f32 = o.num("--dist", 110.0);
    let nl = lab::read_vec(c, npc + ACTOR_LOCATION)?;
    let yaw = (c.read_u32(npc + ACTOR_ROTATION + 4)? as i32) as f32 / 65536.0 * std::f32::consts::TAU;
    let at = [nl[0] + yaw.cos() * dist, nl[1] + yaw.sin() * dist, nl[2] + 20.0];
    place(c, at)?;
    let me = lab::read_vec(c, p + ACTOR_LOCATION)?;
    aim_at(c, me, nl)?;
    // Full health for both.
    let hm = lab::read_i32(c, p + 0x348)?;
    c.write(p + PAWN_HEALTH, &hm.to_le_bytes())?;
    c.wait_ticks(10)?;
    println!("NPC {npc:#x} health {}/{}, horizontal distance {:.0}", lab::read_i32(c, npc + PAWN_HEALTH)?, lab::read_i32(c, npc + 0x348)?, hdist(nl, lab::read_vec(c, p + ACTOR_LOCATION)?));
    let h = damage_hook(c)?;
    // Script-side damage and death events (TakeDamage is a script event; the native damage
    // path above is the player's).
    let mut evf = Vec::new();
    for f in ["DishonoredNPCPawn.TakeDamage", "DishonoredPawn.TakeDamage", "Pawn.TakeDamage", "Pawn.Died", "DishonoredPawn.Died", "DishonoredNPCPawn.Died", "Controller.NotifyTakeHit"] {
        if let Some(a) = c.find(f, "Function")?.first() {
            evf.push(*a);
        }
    }
    c.event_trace(&evf, 52, 10_000)?;
    c.watch(&[])?;
    c.watch_abs(&[(npc + PAWN_HEALTH, 4), (npc + ACTOR_LOCATION, 12)])?;
    c.sample_start(1_000_000)?;
    let t0 = world_time(c)?;
    let mut log = String::from("event,time,detail\n");
    let swings: u32 = o.num("--swings", 3);
    let gap: f32 = o.num("--gap", 1.5);
    for k in 0..swings {
        // Keep facing the NPC.
        let nl = lab::read_vec(c, npc + ACTOR_LOCATION)?;
        let me = lab::read_vec(c, p + ACTOR_LOCATION)?;
        aim_at(c, me, nl)?;
        let t = world_time(c)? - t0;
        let _ = writeln!(log, "press,{t:.3},swing {k} at {:.0} uu", hdist(nl, me));
        c.key("LeftMouseButton", true)?;
        c.wait_ticks(2)?;
        c.key("LeftMouseButton", false)?;
        let until = world_time(c)? + gap;
        let real = std::time::Instant::now();
        while world_time(c)? < until && real.elapsed().as_secs_f32() < gap * 20.0 + 10.0 {
            c.wait_ticks(2)?;
        }
    }
    let defend: f32 = o.num("--defend", 0.0);
    if defend > 0.0 {
        let block = o.rest.iter().any(|a| a == "--block");
        if block {
            c.key("LeftControl", true)?;
            let _ = writeln!(log, "block_down,{:.3},", world_time(c)? - t0);
        }
        let until = world_time(c)? + defend;
        let real = std::time::Instant::now();
        while world_time(c)? < until && real.elapsed().as_secs_f32() < defend * 20.0 + 10.0 {
            let nl = lab::read_vec(c, npc + ACTOR_LOCATION)?;
            let me = lab::read_vec(c, p + ACTOR_LOCATION)?;
            aim_at(c, me, nl)?;
            c.wait_ticks(2)?;
        }
        if block {
            c.key("LeftControl", false)?;
            let _ = writeln!(log, "block_up,{:.3},", world_time(c)? - t0);
        }
    }
    let s = c.sample_fetch()?;
    let recs = c.hook_fetch(h)?;
    let _ = c.hook_set(FN_PAWN_DAMAGE.0, FN_PAWN_DAMAGE.1, 0, 0, &[]);
    c.watch_abs(&[])?;
    print!("{log}");
    let mut dmg = String::from("time,who,amount,health_before,mx,my,mz,instigator,type\n");
    print_damage(c, &recs, t0, p, npc, &mut dmg)?;
    let evs = c.event_fetch()?;
    c.event_trace(&[], 0, 0)?;
    let mut ev = String::from("time,function,object,damage,instigator,hx,hy,hz,mx,my,mz,type\n");
    for e in &evs {
        let f = c.name_of(e.func)?.rsplit(' ').next().unwrap_or("").to_string();
        let o_name = c.name_of(e.object)?.rsplit('.').next().unwrap_or("").to_string();
        let b = &e.parms;
        let line = if f.ends_with("TakeDamage") {
            let inst = u32::from_le_bytes(b[4..8].try_into().unwrap());
            let dt = u32::from_le_bytes(b[32..36].try_into().unwrap());
            format!(
                "{:.4},{f},{o_name},{},{},{:.0},{:.0},{:.0},{:.0},{:.0},{:.0},{}",
                e.time - t0,
                i32le(b, 0),
                if inst == 0 { "-".into() } else { c.name_of(inst)?.rsplit('.').next().unwrap_or("").to_string() },
                f32le(b, 8),
                f32le(b, 12),
                f32le(b, 16),
                f32le(b, 20),
                f32le(b, 24),
                f32le(b, 28),
                if dt == 0 { "-".into() } else { c.name_of(dt)?.rsplit('.').next().unwrap_or("").to_string() }
            )
        } else {
            format!("{:.4},{f},{o_name},,,,,,,,,", e.time - t0)
        };
        println!("  event {line}");
        let _ = writeln!(ev, "{line}");
    }
    o.save("sword_events.csv", &ev)?;
    let mut hl = String::from("time,npc_health,player_health\n");
    let mut last = (i32::MIN, i32::MIN);
    for x in &s {
        let v = (i32le(&x.extra, 0), x.health);
        if v != last {
            let _ = writeln!(hl, "{:.3},{},{}", x.time_seconds - t0, v.0, v.1);
            println!("  health at {:.3}: npc {} player {}", x.time_seconds - t0, v.0, v.1);
            last = v;
        }
    }
    o.save("sword_log.csv", &log)?;
    o.save("sword_damage.csv", &dmg)?;
    o.save("sword_health.csv", &hl)?;
    Ok(())
}
