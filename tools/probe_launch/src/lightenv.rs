//! The player's stealth light environment, read live (U-121).
//!
//! The visibility component's raw light is `FUN_004E1F40(m_pStealthLightingDLE)`: with the
//! component's state (`+0x6C`, an `FDynamicLightEnvironmentState`), 0x004E1DB0 sums the two SH
//! vectors at state `+0x450` and `+0x4E0` (each RGB: R, G, B at +0x00 / +0x30 / +0x60, 12 floats
//! per channel), takes the luminance (0.3, 0.59, 0.11) of band 0 times 2 sqrt(pi) and returns
//! `x^(1/2.2)` (0 below 0.0001; -1 when state `+0x5A4` bit 0 is set). The full update 0x005034D0
//! writes the static environment at `+0x300` / `+0x390` (copied to `+0x60` / `+0xF0`) and the
//! dynamic one at `+0x1B0` / `+0x240` (0x00503140).
//!
//! `session lightenv [lights=R] [toggle=true] [settle=N]`: the band-0 RGB of every SH vector of
//! the state, the value recomputed from them, the visibility component's values, and with
//! `lights` the light components within R uu of the pawn (plus directional and sky lights): class,
//! Brightness, LightColor as stored (FColor bytes) and as the engine converts it
//! (`(byte / 255)^2.2`), radius, falloff, distance, channels, flags; with `toggle` each light's
//! contribution measured by switching it off (`LightComponent.SetEnabled`, the environment reset
//! with `ResetEnvironment`, `settle` ticks, read, switched back on), and the value with all of
//! them off (what the precomputed light volume and the rest give).
//!
//! `session lights [class=...]`: every live light component of the given classes (default the
//! movable point / spot lights a layout builds) with the same fields.
use crate::session::Session;
use anyhow::Result;
use serde_json::{Value, json};

pub const STATE: u32 = 0x6c;
pub const SH_SUM: [u32; 2] = [0x450, 0x4e0];
pub const SH_ALL: [(&str, u32); 8] = [
    ("render_a_0x450", 0x450),
    ("render_b_0x4e0", 0x4e0),
    ("static_0x300", 0x300),
    ("static2_0x390", 0x390),
    ("dynamic_0x1b0", 0x1b0),
    ("dynamic2_0x240", 0x240),
    ("static_copy_0x60", 0x60),
    ("static2_copy_0xf0", 0xf0),
];
const LIGHT_CLASSES: [&str; 10] = [
    "PointLightComponent",
    "SpotLightComponent",
    "DominantPointLightComponent",
    "DominantSpotLightComponent",
    "DirectionalLightComponent",
    "DominantDirectionalLightComponent",
    "SkyLightComponent",
    "SphericalHarmonicLightComponent",
    "DisPointLightComponent",
    "DisSpotLightComponent",
];
const FIELDS: [&str; 14] = [
    "Brightness",
    "LightColor",
    "Radius",
    "FalloffExponent",
    "InnerConeAngle",
    "OuterConeAngle",
    "LowerBrightness",
    "LowerColor",
    "CastShadows",
    "CastStaticShadows",
    "bEnabled",
    "LightingChannels",
    "m_LightProbe",
    "LightEnvironment",
];

fn f32_at(b: &[u8], off: usize) -> f32 {
    f32::from_le_bytes(b[off..off + 4].try_into().unwrap())
}

/// (R, G, B) band 0 of the SH vector at `state + off`.
fn band0(raw: &[u8], off: u32) -> [f32; 3] {
    let o = off as usize;
    [f32_at(raw, o), f32_at(raw, o + 0x30), f32_at(raw, o + 0x60)]
}

fn lum(c: [f32; 3]) -> f32 {
    0.3 * c[0] + 0.59 * c[1] + 0.11 * c[2]
}

/// The visibility light from the two rendered vectors (0x004E1DB0, luminance mode).
pub fn value_of(raw: &[u8]) -> (f32, f32) {
    let a = band0(raw, SH_SUM[0]);
    let b = band0(raw, SH_SUM[1]);
    let l = lum([a[0] + b[0], a[1] + b[1], a[2] + b[2]]) * 2.0 * std::f32::consts::PI.sqrt();
    (l, if l > 1e-4 { l.powf(1.0 / 2.2) } else { 0.0 })
}

impl Session {
    pub fn stealth_dle(&mut self) -> Result<u32> {
        let pawn = crate::lab::pawn(&mut self.c)?;
        let pl = self.r.place(&mut self.c, pawn, "m_pStealthLightingDLE")?;
        let dle = self.c.read_u32(pl.addr)?;
        anyhow::ensure!(dle != 0, "the pawn has no m_pStealthLightingDLE");
        Ok(dle)
    }

    fn state_raw(&mut self, dle: u32) -> Result<(u32, Vec<u8>)> {
        let st = self.c.read_u32(dle + STATE)?;
        anyhow::ensure!(st != 0, "the stealth light environment has no state");
        Ok((st, self.c.read(st, 0x640)?))
    }

    fn env_summary(&mut self, dle: u32) -> Result<Value> {
        let (st, raw) = self.state_raw(dle)?;
        let mut vecs = serde_json::Map::new();
        for (n, off) in SH_ALL {
            let c = band0(&raw, off);
            vecs.insert(n.into(), json!({"rgb": c, "lum_x_2sqrtpi": lum(c) * 2.0 * std::f32::consts::PI.sqrt()}));
        }
        let (linear, value) = value_of(&raw);
        let vis = crate::oracle_stealth::vis_component(&mut self.c)?;
        let v = self.c.read(vis + crate::oracle_stealth::VIS_VALUES, 20)?;
        Ok(json!({
            "state": format!("{st:#x}"),
            "flag_0x5a4": raw[0x5a4] & 1,
            "band0": vecs,
            "linear": linear,
            "value": value,
            "visibility": {"light": f32_at(&v, 0), "light_norm": f32_at(&v, 4), "speed": f32_at(&v, 8), "raw_light": f32_at(&v, 12), "raw_speed": f32_at(&v, 16)},
        }))
    }

    /// The fields of one light component, its position (LightToWorld row 3, or the owner's
    /// location) and the colour as the engine converts it.
    pub fn light_info(&mut self, lc: u32) -> Result<Value> {
        let mut o = serde_json::Map::new();
        o.insert("component".into(), crate::reflect::object_value(&mut self.c, lc)?);
        for f in FIELDS {
            if let Ok(pl) = self.r.place(&mut self.c, lc, f)
                && let Ok(v) = self.r.read_place(&mut self.c, &pl)
            {
                o.insert(f.into(), v);
            }
        }
        if let Some(col) = o.get("LightColor").cloned() {
            let lin = |k: &str| (col[k].as_f64().unwrap_or(0.0) / 255.0).powf(2.2);
            let b = o.get("Brightness").and_then(Value::as_f64).unwrap_or(0.0);
            let (r, g, bb) = (lin("R"), lin("G"), lin("B"));
            o.insert("color_linear".into(), json!([r, g, bb]));
            o.insert("intensity_linear".into(), json!([b * r, b * g, b * bb]));
            o.insert("intensity_lum".into(), json!(b * (0.3 * r + 0.59 * g + 0.11 * bb)));
        }
        if let Ok(pl) = self.r.place(&mut self.c, lc, "LightToWorld")
            && let Ok(m) = self.r.read_place(&mut self.c, &pl)
        {
            let w = &m["WPlane"];
            o.insert("position".into(), json!([w["X"], w["Y"], w["Z"]]));
            let z = &m["ZPlane"];
            o.insert("axis".into(), json!([z["X"], z["Y"], z["Z"]]));
        }
        Ok(Value::Object(o))
    }

    fn lights_of(&mut self, classes: &[&str]) -> Result<Vec<u32>> {
        let mut out = Vec::new();
        for c in classes {
            out.extend(self.c.find("", c)?);
        }
        Ok(out)
    }

    fn reset_env(&mut self, dle: u32, settle: u32) -> Result<()> {
        let _ = self.r.call(&mut self.c, dle, "ResetEnvironment", &json!([]));
        self.c.step(settle, 60_000)?;
        Ok(())
    }

    pub fn light_env(&mut self, a: &Value) -> Result<Value> {
        let dle = self.stealth_dle()?;
        let mut out = json!({"dle": crate::reflect::object_value(&mut self.c, dle)?});
        let pawn = crate::lab::pawn(&mut self.c)?;
        let at = crate::lab::read_vec(&mut self.c, pawn + 0xc4)?;
        out["pawn"] = json!(at);
        for f in ["bDynamic", "bSynthesizeSHLight", "bSynthesizeDirectionalLight", "bForceCompositeAllLights", "bOverrideOwnerBounds", "OverriddenBounds", "LightingChannels", "bIsCharacterLightEnvironment", "NumVolumeVisibilitySamples", "AmbientGlow", "LightDesaturation", "BouncedLightingFactor"] {
            if let Ok(pl) = self.r.place(&mut self.c, dle, f)
                && let Ok(v) = self.r.read_place(&mut self.c, &pl)
            {
                out["dle_fields"][f] = v;
            }
        }
        out["env"] = self.env_summary(dle)?;
        let Some(radius) = a.get("lights").and_then(Value::as_f64) else { return Ok(out) };
        let settle = a.get("settle").and_then(Value::as_u64).unwrap_or(3) as u32;
        let toggle = a.get("toggle").and_then(Value::as_bool).unwrap_or(false);
        let mut near = Vec::new();
        for lc in self.lights_of(&LIGHT_CLASSES)? {
            let info = self.light_info(lc)?;
            let cls = self.c.name_of(lc)?;
            let global = cls.starts_with("Directional") || cls.starts_with("DominantDirectional") || cls.starts_with("SkyLight") || cls.starts_with("SphericalHarmonic");
            let d = info.get("position").and_then(Value::as_array).map(|p| {
                let p: Vec<f32> = p.iter().map(|x| x.as_f64().unwrap_or(0.0) as f32).collect();
                ((p[0] - at[0]).powi(2) + (p[1] - at[1]).powi(2) + (p[2] - at[2]).powi(2)).sqrt()
            });
            let r = info.get("Radius").and_then(Value::as_f64).unwrap_or(0.0) as f32;
            let reach = d.is_some_and(|d| d <= radius as f32 && d <= r.max(1.0));
            if global || reach {
                let mut info = info;
                info["distance"] = json!(d);
                if let Some(d) = d
                    && r > 0.0
                {
                    let fe = info.get("FalloffExponent").and_then(Value::as_f64).unwrap_or(2.0) as f32;
                    info["point_falloff"] = json!((1.0 - d * d / (r * r)).max(0.0).powf(fe));
                }
                near.push((lc, info));
            }
        }
        if toggle {
            let _ = self.c.lockstep(true, 30_000);
            self.reset_env(dle, settle)?;
            let base = self.env_summary(dle)?;
            let base_lin = base["linear"].as_f64().unwrap_or(0.0);
            out["env_reset"] = base.clone();
            for (lc, info) in near.iter_mut() {
                if info.get("bEnabled").and_then(Value::as_bool) == Some(false) {
                    info["delta_linear"] = json!(0.0);
                    continue;
                }
                let off = self.r.call(&mut self.c, *lc, "SetEnabled", &json!([false]));
                if let Err(e) = off {
                    info["toggle_error"] = json!(format!("{e:#}"));
                    continue;
                }
                self.reset_env(dle, settle)?;
                let e = self.env_summary(dle)?;
                info["delta_linear"] = json!(base_lin - e["linear"].as_f64().unwrap_or(0.0));
                info["value_without"] = e["value"].clone();
                let _ = self.r.call(&mut self.c, *lc, "SetEnabled", &json!([true]));
            }
            // Every candidate off at once: what is left (light volume, sky, the rest).
            let mut switched = Vec::new();
            for (lc, info) in &near {
                if info.get("bEnabled").and_then(Value::as_bool) != Some(false) && self.r.call(&mut self.c, *lc, "SetEnabled", &json!([false])).is_ok() {
                    switched.push(*lc);
                }
            }
            self.reset_env(dle, settle)?;
            out["env_all_off"] = self.env_summary(dle)?;
            for lc in switched {
                let _ = self.r.call(&mut self.c, lc, "SetEnabled", &json!([true]));
            }
            self.reset_env(dle, settle)?;
            out["env_restored"] = self.env_summary(dle)?;
        }
        near.sort_by(|a, b| {
            let k = |v: &Value| v.get("delta_linear").and_then(Value::as_f64).or(v.get("intensity_lum").and_then(Value::as_f64)).unwrap_or(0.0);
            k(&b.1).partial_cmp(&k(&a.1)).unwrap_or(std::cmp::Ordering::Equal)
        });
        out["lights"] = Value::Array(near.into_iter().map(|x| x.1).collect());
        Ok(out)
    }

    pub fn list_lights(&mut self, a: &Value) -> Result<Value> {
        let classes: Vec<String> = match a.get("class").and_then(Value::as_str) {
            Some(c) => c.split(',').map(str::to_string).collect(),
            None => vec!["PointLightComponent".into(), "SpotLightComponent".into()],
        };
        let owner_filter = a.get("owner").and_then(Value::as_str).map(str::to_string);
        let mut out = Vec::new();
        for c in &classes {
            for lc in self.c.find("", c)? {
                let owner = self.c.read_u32(lc + 0x58).unwrap_or(0);
                let on = if owner != 0 { self.c.name_of(owner).unwrap_or_default() } else { String::new() };
                if let Some(f) = &owner_filter
                    && !on.contains(f.as_str())
                {
                    continue;
                }
                let mut info = self.light_info(lc)?;
                info["owner"] = json!(on);
                out.push(info);
            }
        }
        Ok(json!({"count": out.len(), "lights": out}))
    }
}
