//! Helpers that set up a scenario in the running original (items, powers, NPC listing) and
//! raw access to the DLL's native instruments (entry hooks, native calls, event traces).
//!
//! lab give <TweakObject> [noequip]   DishonoredPawn.OnAddInventoryItem with the class default of
//!                                    DisSeqAct_AddInventoryItem pointing at the tweak
//! lab power <Name> <level>           DishonoredPlayerPawn.OnAddPower (Blink, Windblast, ...)
//! lab equip <ItemClass> <1|2>        DishonoredPlayerController.Dis_EquipItemByType (1 right, 2 left)
//! lab npcs [radius]                  NPC pawns: address, tweak, health, location, distance
//! lab hook <addr> <prologue> <nargs> <max> [deref...]   entry logger (see `parse_deref`)
//! lab hookfetch <id>                 records of a hook
//! lab ncall <func> <this|0> [args...] [--float]
//! lab trace <Class.Func>... [--parms N] [--max M]  /  lab tracefetch
use anyhow::{Context, Result, bail};
use dishonored_probe::{Client, HookDeref};

pub fn parse_u32(s: &str) -> Result<u32> {
    Ok(match s.strip_prefix("0x") {
        Some(h) => u32::from_str_radix(h, 16)?,
        None => s.parse::<i64>()? as u32,
    })
}

pub fn status_u32(c: &mut Client, key: &str) -> Result<u32> {
    let v = c.status_value(key)?.unwrap_or_default();
    Ok(u32::from_str_radix(v.trim_start_matches("0x"), 16).unwrap_or(0))
}

pub fn pawn(c: &mut Client) -> Result<u32> {
    let p = status_u32(c, "pawn")?;
    if p == 0 {
        bail!("no player pawn");
    }
    Ok(p)
}

pub fn pc(c: &mut Client) -> Result<u32> {
    let p = status_u32(c, "local_pc")?;
    if p == 0 {
        bail!("no player controller");
    }
    Ok(p)
}

/// The class default object `Default__<Class>`.
pub fn cdo(c: &mut Client, class: &str) -> Result<u32> {
    c.find(&format!("Default__{class}"), class)?.first().copied().with_context(|| format!("no Default__{class}"))
}

/// The first live object named `name` (any class).
pub fn object(c: &mut Client, name: &str) -> Result<u32> {
    c.find(name, "")?.first().copied().with_context(|| format!("no object {name}"))
}

/// Give the player an inventory item by its tweak object (`Twk_Inv_PistolCorvo`, ...): the
/// Kismet handler `OnAddInventoryItem` with the action's class default as the parameter
/// (`m_pItemToAdd` +0xF8, `m_bEquipNewItem` +0xFC bit 0).
pub fn give(c: &mut Client, tweak: &str, equip: bool) -> Result<()> {
    let item = object(c, tweak)?;
    let act = cdo(c, "DisSeqAct_AddInventoryItem")?;
    c.write(act + 0xf8, &item.to_le_bytes())?;
    c.write(act + 0xfc, &(equip as u32).to_le_bytes())?;
    let p = pawn(c)?;
    c.call(p, "DishonoredPawn.OnAddInventoryItem", &act.to_le_bytes())?;
    Ok(())
}

/// Raise an active or passive power to `level` (`OnAddPower`: `m_PowerName` +0xF8 FString,
/// `m_PowerLevel` +0x104).
pub fn power(c: &mut Client, name: &str, level: i32) -> Result<()> {
    let act = cdo(c, "DisSeqAct_AddPower")?;
    let fs = c.fstring(name)?;
    let mut b = Vec::new();
    for v in fs {
        b.extend(v.to_le_bytes());
    }
    b.extend(level.to_le_bytes());
    c.write(act + 0xf8, &b)?;
    let p = pawn(c)?;
    c.call(p, "DishonoredPlayerPawn.OnAddPower", &act.to_le_bytes())?;
    Ok(())
}

/// `Dis_EquipItemByType(class, usage)`; usage 1 = primary (right hand), 2 = secondary (left).
pub fn equip(c: &mut Client, class: &str, usage: u8) -> Result<()> {
    let cls = c.find(class, "Class")?.first().copied().with_context(|| format!("no class {class}"))?;
    let pcx = pc(c)?;
    let mut parms = cls.to_le_bytes().to_vec();
    parms.push(usage);
    c.call(pcx, "DishonoredPlayerController.Dis_EquipItemByType", &parms)?;
    Ok(())
}

pub fn read_vec(c: &mut Client, addr: u32) -> Result<[f32; 3]> {
    let b = c.read(addr, 12)?;
    Ok([0, 1, 2].map(|i| f32::from_le_bytes(b[i * 4..i * 4 + 4].try_into().unwrap())))
}

pub fn read_i32(c: &mut Client, addr: u32) -> Result<i32> {
    Ok(c.read_u32(addr)? as i32)
}

pub struct Npc {
    pub addr: u32,
    pub name: String,
    pub tweak: String,
    pub health: i32,
    pub health_max: i32,
    pub loc: [f32; 3],
    pub dist: f32,
}

pub fn npcs(c: &mut Client) -> Result<Vec<Npc>> {
    let p = pawn(c)?;
    let me = read_vec(c, p + 0xc4)?;
    let mut out = Vec::new();
    for a in c.find("", "DishonoredNPCPawn")? {
        let full = c.name_of(a)?;
        if full.starts_with("Class ") {
            continue;
        }
        let tw = c.read_u32(a + 0x748)?;
        let tweak = if tw != 0 { c.name_of(tw)?.rsplit(' ').next().unwrap_or("").to_string() } else { String::new() };
        let loc = read_vec(c, a + 0xc4)?;
        let d = ((loc[0] - me[0]).powi(2) + (loc[1] - me[1]).powi(2) + (loc[2] - me[2]).powi(2)).sqrt();
        out.push(Npc {
            addr: a,
            name: full.rsplit('.').next().unwrap_or("").to_string(),
            tweak,
            health: read_i32(c, a + 0x344)?,
            health_max: read_i32(c, a + 0x348)?,
            loc,
            dist: d,
        });
    }
    out.sort_by(|a, b| a.dist.total_cmp(&b.dist));
    Ok(out)
}

/// `a<i>:<off>:<len>` stack argument i, `c:<off>:<len>` ecx, `m:<addr>:<len>` absolute,
/// `a<i>*<pre>:<off>:<len>` / `c*<pre>:...` through a pointer at base + pre first.
pub fn parse_deref(s: &str) -> Result<HookDeref> {
    let mut it = s.split(':');
    let head = it.next().context("deref")?;
    let off = parse_u32(it.next().context("deref offset")?)?;
    let len = parse_u32(it.next().context("deref length")?)?;
    let (src, pre) = match head.split_once('*') {
        Some((s, p)) => (s, parse_u32(p)?),
        None => (head, u32::MAX),
    };
    let src = match src {
        "c" => HookDeref::ECX,
        "m" => HookDeref::ABS,
        a => a.strip_prefix('a').context("deref source a<i>, c or m")?.parse()?,
    };
    Ok(HookDeref { src, pre, off, len })
}

pub fn run(args: &[String]) -> Result<()> {
    let mut c = crate::connect()?;
    let a = |i: usize| args.get(i).map(String::as_str).unwrap_or("");
    match a(0) {
        "give" => give(&mut c, a(1), a(2) != "noequip"),
        "power" => power(&mut c, a(1), a(2).parse().unwrap_or(1)),
        "equip" => equip(&mut c, a(1), a(2).parse().unwrap_or(1)),
        "setup" => {
            // The measurement kit: tutorials off (`DisGlobalUIManager.m_bEnableTutorials` /
            // `m_bEnableBaseTutorials`, +0x304 bits 3-4: a tutorial window pauses the world),
            // the player kept alive (`DishonoredPawn.m_MinimumScriptedHealth` +0x594 = 1;
            // damage is still applied and logged), pistol, sword, Blink and Windblast level 2,
            // and the fixed time step (Hz, 0: off) given as the first argument.
            for a in c.find("pGlobalUIManager", "DisGlobalUIManager")? {
                if c.name_of(a)?.contains("PersistentLevel") {
                    let v = c.read_u32(a + 0x304)? & !0x18;
                    c.write(a + 0x304, &v.to_le_bytes())?;
                }
            }
            let p = pawn(&mut c)?;
            c.write(p + 0x594, &1i32.to_le_bytes())?;
            give(&mut c, "Twk_Inv_PistolCorvo", true)?;
            give(&mut c, "Twk_Inv_SwordCorvo", true)?;
            power(&mut c, "Blink", 2)?;
            power(&mut c, "Windblast", 2)?;
            let hz: f64 = a(1).parse().unwrap_or(60.0);
            if hz > 0.0 {
                c.write(0x012696b0, &(1.0 / hz).to_le_bytes())?;
                c.write(0x0141b27c, &1u32.to_le_bytes())?;
            }
            Ok(())
        }
        "fixedstep" => {
            // GUseFixedTimeStep (0x0141B27C) and GFixedDeltaTime (0x012696B0, double): the
            // engine's time update (0x009DDDF0) then advances every tick by the fixed step,
            // as with -benchmark (which also drops the ini files and hangs at the menu).
            let hz: f64 = a(1).parse().unwrap_or(0.0);
            if hz > 0.0 {
                c.write(0x012696b0, &(1.0 / hz).to_le_bytes())?;
                c.write(0x0141b27c, &1u32.to_le_bytes())?;
            } else {
                c.write(0x0141b27c, &0u32.to_le_bytes())?;
            }
            Ok(())
        }
        "cdo" => {
            println!("{:#010x}", cdo(&mut c, a(1))?);
            Ok(())
        }
        "npcs" => {
            let r: f32 = a(1).parse().unwrap_or(1e9);
            for n in npcs(&mut c)?.into_iter().filter(|n| n.dist <= r) {
                println!("{:#010x} {:<22} {:<32} {:>3}/{:<3} ({:.0}, {:.0}, {:.0}) d {:.0}", n.addr, n.name, n.tweak, n.health, n.health_max, n.loc[0], n.loc[1], n.loc[2], n.dist);
            }
            Ok(())
        }
        "hook" => {
            let derefs = args[5..].iter().map(|s| parse_deref(s)).collect::<Result<Vec<_>>>()?;
            let id = c.hook_set(parse_u32(a(1))?, a(2).parse()?, a(3).parse()?, a(4).parse()?, &derefs)?;
            println!("hook {id}");
            Ok(())
        }
        "hookfetch" => {
            for r in c.hook_fetch(a(1).parse()?)? {
                let args: Vec<String> = r.args.iter().map(|v| format!("{v:#x}")).collect();
                let f: Vec<String> = r.data.chunks(4).filter(|w| w.len() == 4).map(|w| {
                    let u = u32::from_le_bytes(w.try_into().unwrap());
                    let f = f32::from_bits(u);
                    if f.is_finite() && f.abs() > 1e-6 && f.abs() < 1e7 { format!("{f:.4}") } else { format!("{u:#x}") }
                }).collect();
                println!("{:.4} ecx {:#x} ret {:#x} args [{}] data [{}]", r.time, r.ecx, r.ret, args.join(" "), f.join(" "));
            }
            Ok(())
        }
        "ncall" => {
            let float = args.iter().any(|s| s == "--float");
            let vals: Vec<u32> = args[3..].iter().filter(|s| !s.starts_with("--")).map(|s| {
                if let Some(f) = s.strip_prefix("f:") { f.parse::<f32>().map(f32::to_bits).map_err(anyhow::Error::from) } else { parse_u32(s) }
            }).collect::<Result<_>>()?;
            let (eax, st0) = c.native_call(parse_u32(a(1))?, parse_u32(a(2))?, &vals, float)?;
            println!("eax {eax:#x} ({}) st0 {st0}", eax as i32);
            Ok(())
        }
        "trace" => {
            let mut funcs = Vec::new();
            let mut parms = 32;
            let mut max = 10000;
            let mut i = 1;
            while i < args.len() {
                match args[i].as_str() {
                    "--parms" => {
                        parms = args[i + 1].parse()?;
                        i += 1;
                    }
                    "--max" => {
                        max = args[i + 1].parse()?;
                        i += 1;
                    }
                    f => funcs.push(c.find(f, "Function")?.first().copied().with_context(|| format!("no function {f}"))?),
                }
                i += 1;
            }
            c.event_trace(&funcs, parms, max)?;
            Ok(())
        }
        "tracefetch" => {
            for r in c.event_fetch()? {
                let f = c.name_of(r.func)?;
                let o = c.name_of(r.object)?;
                let hex: Vec<String> = r.parms.iter().map(|x| format!("{x:02x}")).collect();
                println!("{:.4} {} on {} parms {}", r.time, f.rsplit(' ').next().unwrap_or(""), o.rsplit('.').next().unwrap_or(""), hex.join(""));
            }
            Ok(())
        }
        _ => bail!("lab give|power|equip|cdo|npcs|hook|hookfetch|ncall|trace|tracefetch"),
    }
}
