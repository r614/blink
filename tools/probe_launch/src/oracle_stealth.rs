//! Oracle measurements of NPC perception (see oracle.rs).
//!
//! measure detect [npc=ADDR] [--dists 300,600,...] [--stances stand,crouch] [--secs S]
//!     The player is put `d` uu in front of an NPC (along its facing), facing it, still; the
//!     NPC's `PlayerAttnInfo` (`DisAttentionInfo_Complex`) is reset and sampled every frame:
//!     the accumulator value, level, flags, normalized LOS distance, the vision debug values
//!     (raw distance, final visibility level clamped / unclamped, accumulation rate) and the
//!     player's `DishonoredVisibilityComponent` light and speed values.
//! measure vis [at=X,Y,Z]...   the player's visibility component values at points (stand and
//!                             crouch), for comparing the light probe.
use crate::lab;
use crate::oracle::*;
use crate::oracle_powers::{place, set_view};
use anyhow::{Context, Result, bail};
use dishonored_probe::Client;
use std::fmt::Write as _;

/// `DisAttentionInfo_Complex` fields (class_layout).
pub const ATTN_TWEAKS: u32 = 0x3c;
pub const ATTN_ACC: u32 = 0x8c; // value, leak rate, min, max
pub const ATTN_LEVEL: u32 = 0xac;
pub const ATTN_FLAGS: u32 = 0x510;
pub const ATTN_LOS_DIST: u32 = 0x514; // .. +0x528: raw distance, final vis clamped, unclamped, accumulation rate, drain rate
/// `DishonoredVisibilityComponent` light value, normalized, speed value, raw light, raw speed.
pub const VIS_VALUES: u32 = 0x68;

fn f32le(b: &[u8], off: usize) -> f32 {
    b.get(off..off + 4).map(|w| f32::from_le_bytes(w.try_into().unwrap())).unwrap_or(f32::NAN)
}

pub fn attention_info(c: &mut Client, npc: u32) -> Result<u32> {
    let ctrl = c.read_u32(npc + 0x26c)?;
    if ctrl == 0 {
        bail!("NPC {npc:#x} has no controller");
    }
    let cname = c.name_of(ctrl)?;
    let cname = cname.rsplit('.').next().unwrap_or("").to_string();
    for a in c.find("PlayerAttnInfo", "DisAttentionInfo_Complex")? {
        let n = c.name_of(a)?;
        if n.contains(&format!(".{cname}.")) {
            return Ok(a);
        }
    }
    bail!("no PlayerAttnInfo under {cname}")
}

pub fn vis_component(c: &mut Client) -> Result<u32> {
    let p = lab::pawn(c)?;
    for a in c.find("VisComponent", "DishonoredVisibilityComponent")? {
        if c.read_u32(a + 0x58).ok() == Some(p) {
            return Ok(a);
        }
    }
    bail!("no visibility component on the player")
}

/// Reset the accumulator value and level (the profile's min stays).
fn reset_attention(c: &mut Client, info: u32) -> Result<()> {
    c.write(info + ATTN_ACC, &0f32.to_le_bytes())?;
    c.write(info + ATTN_LEVEL, &[0u8])?;
    Ok(())
}

pub fn detect(c: &mut Client, o: &Opts) -> Result<()> {
    let npc = match o.rest.iter().find_map(|a| a.strip_prefix("npc=")) {
        Some(a) => lab::parse_u32(a)?,
        None => lab::npcs(c)?.into_iter().find(|n| n.health > 0 && n.tweak.contains("Guard")).context("no guard")?.addr,
    };
    let info = attention_info(c, npc)?;
    let vis = vis_component(c)?;
    let tw = c.read_u32(info + ATTN_TWEAKS)?;
    println!("NPC {npc:#x} ({}), attention info {info:#x}, profile {}", c.name_of(npc)?.rsplit('.').next().unwrap_or(""), c.name_of(tw).unwrap_or_default());
    let dists: Vec<f32> = o.rest.iter().position(|a| a == "--dists").and_then(|i| o.rest.get(i + 1)).map(|s| s.split(',').filter_map(|x| x.parse().ok()).collect()).unwrap_or(vec![300.0, 500.0, 700.0, 900.0, 1200.0, 1600.0]);
    let stances: Vec<String> = o.rest.iter().position(|a| a == "--stances").and_then(|i| o.rest.get(i + 1)).map(|s| s.split(',').map(str::to_string).collect()).unwrap_or(vec!["stand".into(), "crouch".into()]);
    let secs: f32 = o.num("--secs", 1.5);
    let mut all = String::from("stance,dist,time,value,leak,level,flags,los_norm,raw_dist,vis_clamped,vis_unclamped,acc_rate,drain_rate,light,light_norm,speed,raw_light,raw_speed,px,py,pz\n");
    println!("| stance | d (set) | raw dist | LOS norm | vis unclamped | vis clamped | acc rate /s | measured rise /s | light | light norm | level |");
    println!("|---|---|---|---|---|---|---|---|---|---|---|");
    let p = lab::pawn(c)?;
    for stance in &stances {
        for &d in &dists {
            // Stand in front of the NPC along its facing.
            let nl = lab::read_vec(c, npc + ACTOR_LOCATION)?;
            let yaw = (c.read_u32(npc + ACTOR_ROTATION + 4)? as i32) as f32 / 65536.0 * std::f32::consts::TAU;
            let at = [nl[0] + yaw.cos() * d, nl[1] + yaw.sin() * d, nl[2] + 20.0];
            place(c, at)?;
            if stance == "crouch" {
                c.key("C", true)?;
                c.wait_ticks(2)?;
                c.key("C", false)?;
                c.wait_ticks(20)?;
            }
            let me = lab::read_vec(c, p + ACTOR_LOCATION)?;
            let face = (nl[1] - me[1]).atan2(nl[0] - me[0]).to_degrees();
            set_view(c, 0.0, face)?;
            c.wait_ticks(5)?;
            reset_attention(c, info)?;
            c.watch_abs(&[(info + ATTN_ACC, 16), (info + ATTN_LEVEL, 1), (info + ATTN_FLAGS, 4), (info + ATTN_LOS_DIST, 24), (vis + VIS_VALUES, 20), (npc + ACTOR_LOCATION, 12)])?;
            c.sample_start(1_000_000)?;
            let t0 = world_time(c)?;
            let real = std::time::Instant::now();
            while world_time(c)? < t0 + secs {
                c.wait_ticks(5)?;
                if real.elapsed().as_secs_f32() > 10.0 + secs * 10.0 {
                    bail!("the world does not advance (paused by a tutorial or menu?)");
                }
            }
            let s = c.sample_fetch()?;
            c.watch_abs(&[])?;
            if stance == "crouch" {
                c.key("C", true)?;
                c.wait_ticks(2)?;
                c.key("C", false)?;
                c.wait_ticks(10)?;
            }
            let mut rows = Vec::new();
            for x in &s {
                let e = &x.extra;
                if e.len() < 77 {
                    continue;
                }
                let r = (x.time_seconds, f32le(e, 0), f32le(e, 4), e[16], u32::from_le_bytes(e[17..21].try_into().unwrap()), [f32le(e, 21), f32le(e, 25), f32le(e, 29), f32le(e, 33), f32le(e, 37), f32le(e, 41)], [f32le(e, 45), f32le(e, 49), f32le(e, 53), f32le(e, 57), f32le(e, 61)], {
                    let n = [f32le(e, 65), f32le(e, 69), f32le(e, 73)];
                    let d3 = ((n[0] - x.location[0]).powi(2) + (n[1] - x.location[1]).powi(2) + (n[2] - x.location[2]).powi(2)).sqrt();
                    (n, d3)
                });
                let _ = writeln!(
                    all,
                    "{stance},{d},{:.4},{:.4},{:.4},{},{:#x},{:.4},{:.1},{:.4},{:.4},{:.4},{:.4},{:.4},{:.4},{:.4},{:.4},{:.4},{:.1},{:.1},{:.1},{:.1},{:.1},{:.1},{:.1}",
                    r.0, r.1, r.2, r.3, r.4, r.5[0], r.5[1], r.5[2], r.5[3], r.5[4], r.5[5], r.6[0], r.6[1], r.6[2], r.6[3], r.6[4], x.location[0], x.location[1], x.location[2], r.7 .0[0], r.7 .0[1], r.7 .0[2], r.7 .1
                );
                rows.push(r);
            }
            // Measured rise rate: value change over time while below the clamp, from the first
            // sample to the first at or above 1 (or the last).
            let first = rows.first();
            let top = rows.iter().find(|r| r.1 >= 0.999).or(rows.last());
            let rise = match (first, top) {
                (Some(a), Some(b)) if b.0 > a.0 => (b.1 - a.1) / (b.0 - a.0),
                _ => f32::NAN,
            };
            let mid = rows.get(rows.len() / 2);
            if let Some(m) = mid {
                println!(
                    "| {stance} | {d} | {:.0} / {:.0} | {:.3} | {:.3} | {:.3} | {:.3} | {rise:.3} | {:.3} | {:.3} | {} |",
                    m.7 .1, m.5[1], m.5[0], m.5[3], m.5[2], m.5[4], m.6[0], m.6[1], rows.last().map(|r| r.3).unwrap_or(0)
                );
            }
            // Leave: back to the start and let the attention leak away.
            reset_attention(c, info)?;
        }
    }
    o.save("detect.csv", &all)?;
    Ok(())
}

pub fn vis(c: &mut Client, o: &Opts) -> Result<()> {
    let comp = vis_component(c)?;
    let pts: Vec<[f32; 3]> = o
        .rest
        .iter()
        .filter_map(|a| a.strip_prefix("at="))
        .filter_map(|s| {
            let v: Vec<f32> = s.split(',').filter_map(|x| x.parse().ok()).collect();
            (v.len() == 3).then(|| [v[0], v[1], v[2]])
        })
        .collect();
    let p = lab::pawn(c)?;
    println!("| point | pawn | stance | light | light norm | speed | raw light | raw speed |");
    println!("|---|---|---|---|---|---|---|---|");
    for at in pts {
        for crouch in [false, true] {
            place(c, at)?;
            if crouch {
                c.key("C", true)?;
                c.wait_ticks(2)?;
                c.key("C", false)?;
            }
            c.wait_ticks(30)?;
            let b = c.read(comp + VIS_VALUES, 20)?;
            let me = lab::read_vec(c, p + ACTOR_LOCATION)?;
            println!(
                "| ({:.0}, {:.0}, {:.0}) | ({:.0}, {:.0}, {:.0}) | {} | {:.4} | {:.4} | {:.4} | {:.4} | {:.4} |",
                at[0], at[1], at[2], me[0], me[1], me[2], if crouch { "crouch" } else { "stand" }, f32le(&b, 0), f32le(&b, 4), f32le(&b, 8), f32le(&b, 12), f32le(&b, 16)
            );
            if crouch {
                c.key("C", true)?;
                c.wait_ticks(2)?;
                c.key("C", false)?;
                c.wait_ticks(10)?;
            }
        }
    }
    Ok(())
}
