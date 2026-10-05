//! [`ItemsConfig`] from the install: inventory and elixir rules, Possession, Bend Time,
//! Devouring Swarm, passive powers (DefaultPlayer.ini), bone charms and gadgets.
//!
//! Every value falls back to the documented original value when a tweak is missing, and its
//! source goes into the gameplay provenance.

use crate::content::Content;
use crate::ini::Ini;
use crate::tweaks::{TMap, TVal, TweakDb, get, get_f32};
use dishonored_ir::items::*;
use std::collections::BTreeMap;

const CORVO: &str = "Twk_Pawn_Corvo.Twk_Pawn_Corvo_Release";
const NORMAL: &str = "m_fBaseValue2_Normal";

struct L<'a> {
    db: &'a TweakDb,
    prov: &'a mut BTreeMap<String, String>,
}

impl L<'_> {
    fn obj(&self, path: &str) -> TMap {
        self.db.try_resolve(path).unwrap_or_default()
    }
    /// Read `field` of `m` (from `src`) into `out`, keeping `default` when absent.
    fn f(&mut self, m: &TMap, src: &str, field: &str, out: &str, default: f32) -> f32 {
        match get_f32(m, field) {
            Some(v) => {
                self.prov.insert(out.into(), format!("{src} :: {field} = {v}"));
                v
            }
            None => {
                self.prov.insert(out.into(), format!("{src} :: {field} absent, original default {default}"));
                default
            }
        }
    }
    fn attr(&mut self, m: &TMap, src: &str, field: &str, out: &str, default: f32) -> f32 {
        self.f(m, src, &format!("{field}.{NORMAL}"), out, default)
    }
}

fn level_key(base: &str, i: usize, field: &str) -> String {
    if i == 0 { format!("{base}.{field}") } else { format!("{base}[{i}].{field}") }
}

fn costs(m: &TMap, d: [i32; 2]) -> [i32; 2] {
    let c = |k: &str, d: i32| get_f32(m, k).map(|v| v as i32).unwrap_or(d);
    [c("m_RuneCosts", d[0]), c("m_RuneCosts[1]", d[1])]
}

pub fn load_items_config(content: &Content, db: &TweakDb, prov: &mut BTreeMap<String, String>) -> ItemsConfig {
    let mut l = L { db, prov };
    let mut cfg = ItemsConfig::default();

    // --- inventory (Corvo) ---
    let corvo = l.obj(CORVO);
    let attrs_path = format!("{CORVO}.pAttributeTweaksNormal");
    let attrs = l.obj(&attrs_path);
    let input_path = "Twk_Pawn_DefaultPlayer.Twk_Pawn_PlayerInput";
    let input = l.obj(input_path);
    let inv = &mut cfg.inventory;
    let d = InventoryConfig::default();
    let caps = [
        "m_BulletCapacity",
        "m_ExplosiveBulletCapacity",
        "m_ArrowCapacity",
        "m_SleepArrowCapacity",
        "m_FlareArrowCapacity",
        "m_SpringRazorCapacity",
        "m_GrenadeCapacity",
        "m_StickyGrenadeCapacity",
    ];
    for (i, c) in caps.iter().enumerate() {
        inv.ammo_capacity[i] = l.attr(&attrs, &attrs_path, c, &format!("items.inventory.ammo_capacity[{i}]"), d.ammo_capacity[i] as f32) as u16;
    }
    inv.health_elixir_value = l.f(&corvo, CORVO, "m_nHealthElixirValue", "items.inventory.health_elixir_value", d.health_elixir_value);
    inv.mana_elixir_value = l.f(&corvo, CORVO, "m_nManaElixirValue", "items.inventory.mana_elixir_value", d.mana_elixir_value);
    inv.max_health_elixirs = l.f(&corvo, CORVO, "m_nMaxHealthElixir", "items.inventory.max_health_elixirs", d.max_health_elixirs as f32) as u8;
    inv.max_mana_elixirs = l.f(&corvo, CORVO, "m_nMaxManaElixir", "items.inventory.max_mana_elixirs", d.max_mana_elixirs as f32) as u8;
    inv.max_equipped_charms = l.f(&corvo, CORVO, "m_MaxActivatedBoneCharmCount", "items.inventory.max_equipped_charms", d.max_equipped_charms as f32) as u8;
    let interact = l.obj("Default__DisTweaks_InteractableInterface");
    inv.interact_distance = l.f(&interact, "Default__DisTweaks_InteractableInterface", "m_fMaxDistance", "items.inventory.interact_distance", d.interact_distance);
    inv.choke_time = l.attr(&attrs, &attrs_path, "m_ChokeTime", "items.inventory.choke_time", d.choke_time);
    l.prov.insert("items.inventory.choke_anim_time".into(), "Startup.upk Ply_Sword_Choke_as Sword_Choke_Loop_Master length 5.033 s (anim state 0x5f mapping LIKELY, U-101)".into());
    inv.choke_hold_to_auto_carry = l.f(&input, input_path, "m_fChokeHoldToAutoCarry", "items.inventory.choke_hold_to_auto_carry", d.choke_hold_to_auto_carry);
    inv.assassinate_hold_to_auto_carry = l.f(&input, input_path, "m_fAssassinateHoldToAutoCorpseCarry", "items.inventory.assassinate_hold_to_auto_carry", d.assassinate_hold_to_auto_carry);
    inv.carry_ground_speed = l.attr(&attrs, &attrs_path, "m_GroundSpeedCarryingCorpse", "items.inventory.carry_ground_speed", d.carry_ground_speed);
    inv.carry_jump_z = l.attr(&attrs, &attrs_path, "m_JumpZCarryingCorpse", "items.inventory.carry_jump_z", d.carry_jump_z);
    inv.throw_strength = l.attr(&attrs, &attrs_path, "m_ThrowStrength", "items.inventory.throw_strength", d.throw_strength);
    let xbow = "Twk_Inv_PlayerSpecific.Twk_Inv_CrossbowCorvo.DisTweaks_InventoryItem_Attributes_21";
    let xbow_m = l.obj(xbow);
    inv.crossbow_ranged_damage = l.attr(&xbow_m, xbow, "m_RangedDamage", "items.inventory.crossbow_ranged_damage", d.crossbow_ranged_damage);
    let pistol = "Twk_Inv_PlayerSpecific.Twk_Inv_PistolCorvo.DisTweaks_InventoryItem_Attributes_25";
    let pistol_m = l.obj(pistol);
    inv.pistol_ranged_damage = l.attr(&pistol_m, pistol, "m_RangedDamage", "items.inventory.pistol_ranged_damage", d.pistol_ranged_damage);

    {
        let ini = Ini::load_layered(&content.root, "Player");
        let sec = "DishonoredGame.DishonoredPlayerPawn";
        for (i, k) in ["m_fMinSpeedRayScale", "m_fRaySpeedScale", "m_fRaySpeedScale_Max"].iter().enumerate() {
            if let Some(v) = ini.get_f32(sec, k) {
                cfg.inventory.melee_ray_speed[i] = v;
            }
        }
        l.prov.insert("items.inventory.melee_ray_speed".into(), format!("DefaultPlayer.ini [{sec}] = {:?}", cfg.inventory.melee_ray_speed));
    }

    // --- Possession ---
    let pp = "Twk_Powers_Possess.Twk_Powers_Possess";
    let pm = l.obj(pp);
    let dp = PossessConfig::default();
    let p = &mut cfg.possess;
    p.mana_cost = l.f(&pm, pp, "m_ManaCost", "items.possess.mana_cost", dp.mana_cost);
    p.rune_costs = costs(&pm, dp.rune_costs);
    for i in 0..2 {
        p.range[i] = l.f(&pm, pp, &level_key("m_Levels", i, "m_fPossessRange"), &format!("items.possess.range[{i}]"), dp.range[i]);
        p.bend_time_extension[i] = get_f32(&pm, &level_key("m_Levels", i, "m_fBendTimeExtension")).unwrap_or(0.0);
    }
    p.cancel_cooldown = l.f(&pm, pp, "m_fCancelCooldown", "items.possess.cancel_cooldown", dp.cancel_cooldown);
    p.magical_appearance_delay = l.f(&pm, pp, "m_fMagicalAppearanceDelay", "items.possess.magical_appearance_delay", dp.magical_appearance_delay);
    p.intro_duration = l.f(&pm, pp, "m_IntroDuration", "items.possess.intro_duration", dp.intro_duration);
    p.warn_before_exit = l.f(&pm, pp, "m_WarnApparitionInSecondsBeforeForceExit", "items.possess.warn_before_exit", dp.warn_before_exit);
    p.exit_fade = l.f(&pm, pp, "m_ExitFadeDuration", "items.possess.exit_fade", dp.exit_fade);
    let possessable = |l: &mut L, path: &str, name: &str, d: &PossessableConfig| -> PossessableConfig {
        let m = l.obj(path);
        let mut c = d.clone();
        for i in 0..2 {
            c.duration[i] = get_f32(&m, &level_key("m_Levels", i, "m_fPossessDuration")).unwrap_or(0.0);
        }
        l.prov.insert(format!("items.possess.{name}.duration"), format!("{path} :: m_Levels[].m_fPossessDuration = {:?}", c.duration));
        c.required_level = l.f(&m, path, "m_RequiredPossessPowerLevel", &format!("items.possess.{name}.required_level"), d.required_level as f32) as u8;
        c.ground_speed = l.f(&m, path, "m_fGroundSpeedWhilePossessed", &format!("items.possess.{name}.ground_speed"), d.ground_speed);
        for (i, a) in c.awareness.iter_mut().enumerate() {
            let k = if i == 0 { "m_AwarenessSettings.m_bCanBePossessed".to_string() } else { format!("m_AwarenessSettings[{i}].m_bCanBePossessed") };
            *a = matches!(get(&m, &k), Some(TVal::B(true)));
        }
        c.disallow = matches!(get(&m, "m_bDisallowPossession"), Some(TVal::B(true)));
        c
    };
    p.npc = possessable(&mut l, "Twk_Possessable_BaseNPC.Twk_Possessable_BaseNPC", "npc", &dp.npc);
    p.rat = possessable(&mut l, "Twk_Possessable_Rat.Twk_Possessable_Rat", "rat", &dp.rat);

    // --- Bend Time ---
    let bp = "Twk_Powers.BendTime.Twk_BendTime";
    let bm = l.obj(bp);
    let db = BendTimeConfig::default();
    cfg.bend_time.mana_cost = l.f(&bm, bp, "m_ManaCost", "items.bend_time.mana_cost", db.mana_cost);
    cfg.bend_time.rune_costs = costs(&bm, db.rune_costs);
    for i in 0..2 {
        let k = |f: &str| level_key("m_Levels", i, f);
        let dl = db.levels[i];
        cfg.bend_time.levels[i] = BendTimeLevel {
            world_dilation: get_f32(&bm, &k("m_WorldDilation")).unwrap_or(dl.world_dilation),
            player_dilation: get_f32(&bm, &k("m_PlayerDilation")).unwrap_or(dl.player_dilation),
            duration: get_f32(&bm, &k("m_TimeDuration")).unwrap_or(dl.duration),
            dilation_warmup: get_f32(&bm, &k("m_fTimeDilationWarmupTime")).unwrap_or(dl.dilation_warmup),
            dilation_cooldown: get_f32(&bm, &k("m_fTimeDilationCooldownTime")).unwrap_or(dl.dilation_cooldown),
            projectile_early_wakeup: get_f32(&bm, &k("m_fProjectileEarlyWakeupTime")).unwrap_or(dl.projectile_early_wakeup),
        };
    }
    l.prov.insert("items.bend_time.levels".into(), format!("{bp} :: m_Levels = {:?}", cfg.bend_time.levels));

    // --- Devouring Swarm ---
    let sp = "Twk_Powers_DevouringSwarm.Twk_DevouringSwarm";
    let sm = l.obj(sp);
    let ds = SwarmConfig::default();
    cfg.swarm.mana_cost = l.f(&sm, sp, "m_ManaCost", "items.swarm.mana_cost", ds.mana_cost);
    cfg.swarm.rune_costs = costs(&sm, ds.rune_costs);
    cfg.swarm.spawn_delay = l.f(&sm, sp, "m_fRatSwarmSpawnDelay", "items.swarm.spawn_delay", ds.spawn_delay);
    for i in 0..2 {
        let k = |f: &str| level_key("m_Levels", i, f);
        let dl = ds.levels[i];
        let rat_path = match get(&sm, &k("m_pRatSwarmTweaks")) {
            Some(TVal::Obj(o)) => o.clone(),
            _ => format!("Twk_RatSwarm.Twk_DevouringSwarm_Lvl{}", i + 1),
        };
        let rm = l.obj(&rat_path);
        let r = |f: &str, d: f32| get_f32(&rm, f).unwrap_or(d);
        cfg.swarm.levels[i] = SwarmLevel {
            max_distance: get_f32(&sm, &k("m_fMaxDistance")).unwrap_or(dl.max_distance),
            duration: get_f32(&sm, &k("m_fDuration")).unwrap_or(dl.duration),
            rat_count: get_f32(&sm, &k("m_RatSpawnerSettings.SpawnNum")).map(|v| v as u16).unwrap_or(dl.rat_count),
            detection_radius: r("m_fPawnDetectionRadius", dl.detection_radius),
            initial_damage_delay: r("m_fInitialDelayBeforeDealingDamage", dl.initial_damage_delay),
            damage_interval: r("m_fDamageTimeInterval", dl.damage_interval),
            damage_per_bite: r("m_fDamagePerBite", dl.damage_per_bite),
            min_damage: r("m_fMinDamage", dl.min_damage),
            max_damage: r("m_fMaxDamage", dl.max_damage),
            eat_startup: r("m_fEatStartupDuration", dl.eat_startup),
            eat_per_limb: r("m_fEatPerLimbDuration", dl.eat_per_limb),
            eat_delay_after_kill: r("m_fEatStartupDelayAfterKill", dl.eat_delay_after_kill),
        };
        l.prov.insert(format!("items.swarm.levels[{i}]"), format!("{sp} m_Levels[{i}] + {rat_path}"));
    }

    // --- Shadow Kill (NPC pawn tweak base) ---
    let npc = l.obj("Default__DisTweaks_NPCPawn");
    let dk = ShadowKillConfig::default();
    cfg.shadow_kill.delay = l.f(&npc, "Default__DisTweaks_NPCPawn", "m_fShadowKillDelay", "items.shadow_kill.delay", dk.delay);
    cfg.shadow_kill.fade_duration = l.f(&npc, "Default__DisTweaks_NPCPawn", "m_fShadowKillFadeToBlackDuration", "items.shadow_kill.fade_duration", dk.fade_duration);

    // --- gadgets ---
    let g = &mut cfg.gadgets;
    let rp = "Twk_Inv_SpringRazor.Twk_Inv_SpringRazorPlaced";
    let rm = l.obj(rp);
    let dr = SpringRazorConfig::default();
    let mut f = |name: &str, field: &str, d: f32| l.f(&rm, rp, field, &format!("items.gadgets.springrazor.{name}"), d);
    g.springrazor = SpringRazorConfig {
        arming_time: f("arming_time", "m_fArmingTime", dr.arming_time),
        detection_radius: f("detection_radius", "m_fDetectionCylinderRadius", dr.detection_radius),
        detection_height: f("detection_height", "m_fDetectionCylinderHeight", dr.detection_height),
        detonation_radius: f("detonation_radius", "m_fDetonationCylinderRadius", dr.detonation_radius),
        detonation_height: f("detonation_height", "m_fDetonationCylinderHeight", dr.detonation_height),
        trigger_reaction_time: f("trigger_reaction_time", "m_fTriggerReactionTime", dr.trigger_reaction_time),
        damage_reaction_time: f("damage_reaction_time", "m_fDamageReactionTime", dr.damage_reaction_time),
        damage_delay: f("damage_delay", "m_fDamageActorDelayTime", dr.damage_delay),
        max_damage_distance: f("max_damage_distance", "m_fMaxDamageDistance", dr.max_damage_distance),
        non_npc_damage: f("non_npc_damage", "m_NonNPCDamage[1]", dr.non_npc_damage),
        immune_npc_damage: f("immune_npc_damage", "m_ImmuneNPCDamage", dr.immune_npc_damage),
        impulse: f("impulse", "m_fImpulseForce", dr.impulse),
        sensor_offset: f("sensor_offset", "m_fSensorOffsetDistance", dr.sensor_offset),
    };
    let throw_p = "Twk_Inv_GrenadeCorvo.Twk_Inv_GrenadeCorvo.DisTweaks_InventoryItem_PlayerSpecific_0.DisTweaks_ThrowGrenade_0";
    let throw = l.obj(throw_p);
    let grenade = |l: &mut L, proj: &str, comp: &str, name: &str, sticky: bool| -> GrenadeConfig {
        let d = GrenadeConfig::default();
        let pm = l.obj(proj);
        let cm = l.obj(comp);
        let expl_path = match get(&cm, "m_pExplosionTweaks") {
            Some(TVal::Obj(o)) => o.clone(),
            _ => "Twk_Projectiles.Explosions.Twk_ExplosionBase".into(),
        };
        let em = l.obj(&expl_path);
        let de = ExplosionConfig::default();
        let pre = format!("items.gadgets.{name}");
        GrenadeConfig {
            throw_speed: l.f(&throw, throw_p, "m_fThrowSpeed", &format!("{pre}.throw_speed"), d.throw_speed),
            throw_angle: l.f(&throw, throw_p, "m_fThrowAngle", &format!("{pre}.throw_angle"), d.throw_angle),
            gravity_mult: l.f(&pm, proj, "m_fGravityMultiplier", &format!("{pre}.gravity_mult"), d.gravity_mult),
            // Class default 0: the cooked tweak does not set it (see U-102).
            fuse: l.f(&cm, comp, "m_fDetonationDelay", &format!("{pre}.fuse"), 0.0),
            min_delay_after_hit: l.f(&cm, comp, "m_fMinDetonationDelayAfterHit", &format!("{pre}.min_delay_after_hit"), d.min_delay_after_hit),
            bounciness: get_f32(&pm, "m_fBounciness").unwrap_or(d.bounciness),
            floor_bounciness: get_f32(&pm, "m_fFloorBounciness").unwrap_or(if sticky { 0.0 } else { d.floor_bounciness }),
            sticky,
            explosion: ExplosionConfig {
                radial_damage: l.f(&em, &expl_path, &format!("m_RadialDamage.{NORMAL}"), &format!("{pre}.explosion.radial_damage"), de.radial_damage),
                damage_radius: l.f(&em, &expl_path, "m_fDamageRadius", &format!("{pre}.explosion.damage_radius"), de.damage_radius),
                full_damage_radius: l.f(&em, &expl_path, "m_fFullDamageRadius", &format!("{pre}.explosion.full_damage_radius"), de.full_damage_radius),
                player_damage_radius: l.f(&em, &expl_path, "m_fPlayerDamageRadius", &format!("{pre}.explosion.player_damage_radius"), de.player_damage_radius),
                player_full_damage_radius: l.f(&em, &expl_path, "m_fPlayerFullDamageRadius", &format!("{pre}.explosion.player_full_damage_radius"), de.player_full_damage_radius),
                radial_momentum: l.f(&em, &expl_path, "m_fRadialMomentum", &format!("{pre}.explosion.radial_momentum"), de.radial_momentum),
                extra_up_force: l.f(&em, &expl_path, "m_fExtraUpForce", &format!("{pre}.explosion.extra_up_force"), de.extra_up_force),
                attached_damage: l.f(&em, &expl_path, "m_fAttachedDamage", &format!("{pre}.explosion.attached_damage"), de.attached_damage),
            },
        }
    };
    g.grenade = grenade(&mut l, "Twk_Proj_Grenade.Twk_Proj_Grenade", "Twk_Proj_Grenade.Twk_Proj_Grenade.DisTweaks_GrenadeComponent_0", "grenade", false);
    g.sticky_grenade = grenade(&mut l, "Twk_Proj_StickyGrenade.Twk_Proj_StickyGrenade", "Twk_Proj_StickyGrenade.Twk_Proj_StickyGrenade.DisTweaks_GrenadeComponent_1", "sticky_grenade", true);
    let bolt = |l: &mut L, path: &str, name: &str, d: BoltConfig| -> BoltConfig {
        let m = l.obj(path);
        let pre = format!("items.gadgets.{name}");
        BoltConfig {
            damage_mult: l.f(&m, path, "m_fDamageMultiplier", &format!("{pre}.damage_mult"), d.damage_mult),
            speed_mult: l.f(&m, path, "m_fSpeedMultiplier", &format!("{pre}.speed_mult"), d.speed_mult),
            gravity_mult: l.f(&m, path, "m_fGravityMultiplier", &format!("{pre}.gravity_mult"), d.gravity_mult),
            limb_damage_cap: l.f(&m, path, "m_fLegsAndArmsDamageCap", &format!("{pre}.limb_damage_cap"), d.limb_damage_cap),
            kill_on_headshot: get(&m, "m_bKillOnHeadshot").map(|v| matches!(v, TVal::B(true))).unwrap_or(d.kill_on_headshot),
        }
    };
    let dg = GadgetConfig::default();
    g.bolt = bolt(&mut l, "Twk_Projectiles.Twk_Proj_Arrow", "bolt", dg.bolt);
    g.sleep_dart = bolt(&mut l, "Twk_Projectiles.Twk_Proj_Arrow_Sleep", "sleep_dart", dg.sleep_dart);
    g.incendiary_bolt = bolt(&mut l, "Twk_Projectiles.Twk_Proj_Arrow_Flare", "incendiary_bolt", dg.incendiary_bolt);
    let flare = l.obj("Twk_Projectiles.Twk_Proj_Arrow_Flare");
    g.immolation_time = l.f(&flare, "Twk_Projectiles.Twk_Proj_Arrow_Flare", "m_fNPCImmolationTime", "items.gadgets.immolation_time", dg.immolation_time);

    // --- rune costs of the active powers ---
    for (name, path) in [
        ("Blink", "Twk_Powers.Blink.Twk_Blink"),
        ("WindBlast", "Twk_Powers.Windblast.Twk_WindBlast"),
        ("DarkVision", "Twk_Powers.DarkVision.Twk_DarkVision"),
        ("Possess", pp),
        ("BendTime", bp),
        ("DevouringSwarm", sp),
    ] {
        let m = l.obj(path);
        cfg.active_rune_costs.push((name.into(), costs(&m, [0, 0])));
    }
    l.prov.insert("items.active_rune_costs".into(), "m_RuneCosts of each power tweak".into());

    // --- bone charms ---
    if let Some(TVal::Obj(list)) = get(&corvo, "m_pCharmListTweaks") {
        let m = l.obj(list);
        if let Some(TVal::List(charms)) = m.get("m_Charms") {
            for c in charms {
                let attribute = match c.get("m_AttributeName") {
                    Some(TVal::Name(n)) => n.clone(),
                    _ => continue,
                };
                let mut name = String::new();
                let mut values = Vec::new();
                if let Some(TVal::List(levels)) = c.get("m_Levels") {
                    for lv in levels {
                        if name.is_empty() {
                            if let Some(TVal::Str(s)) = lv.get("m_Name") {
                                name = s.trim().to_string();
                            }
                        }
                        values.push(get_f32(lv, "m_fValue").unwrap_or(0.0));
                    }
                }
                cfg.charms.push(BoneCharm { name, attribute, values });
            }
        }
        l.prov.insert("items.charms".into(), format!("{list} :: m_Charms ({} charms)", cfg.charms.len()));
    }

    // --- passive powers: DishonoredPowersComponent.m_Powers in DefaultPlayer.ini ---
    let ini = Ini::load_layered(&content.root, "Player");
    for v in ini.get_all("DishonoredGame.DishonoredPowersComponent", "m_Powers") {
        if let Some(p) = parse_power(&v) {
            cfg.passive_powers.push(p);
        }
    }
    l.prov.insert("items.passive_powers".into(), format!("DefaultPlayer.ini [DishonoredGame.DishonoredPowersComponent] m_Powers ({} powers)", cfg.passive_powers.len()));
    cfg
}

/// Parse one `m_Powers=(m_Name="Vitality",...,m_Levels=((m_RuneCost=0,m_Modifiers=),...))` line.
pub fn parse_power(s: &str) -> Option<PassivePower> {
    let name = s.split("m_Name=\"").nth(1)?.split('"').next()?.to_string();
    let levels_at = s.find("m_Levels=(")? + "m_Levels=(".len();
    // Split the level list at depth 0 of its own parentheses.
    let body = &s[levels_at..];
    let mut levels = Vec::new();
    let mut depth = 0i32;
    let mut start = None;
    for (i, ch) in body.char_indices() {
        match ch {
            '(' => {
                if depth == 0 {
                    start = Some(i + 1);
                }
                depth += 1;
            }
            ')' => {
                depth -= 1;
                if depth == 0 {
                    if let Some(st) = start.take() {
                        levels.push(parse_level(&body[st..i]));
                    }
                }
                if depth < 0 {
                    break;
                }
            }
            _ => {}
        }
    }
    Some(PassivePower { name, levels })
}

fn parse_level(s: &str) -> (i32, Vec<AttributeModifier>) {
    let cost = s.split("m_RuneCost=").nth(1).and_then(|r| r.split([',', ')']).next()).and_then(|v| v.trim().parse().ok()).unwrap_or(0);
    let mut mods = Vec::new();
    for part in s.split("m_AttributeName=\"").skip(1) {
        let attribute = part.split('"').next().unwrap_or("").to_string();
        let kind = match part.split("m_ModType=").nth(1).and_then(|r| r.split([',', ')']).next()).unwrap_or("") {
            "eDisAttributeModifierType_AddVal" => ModifierType::AddVal,
            "eDisAttributeModifierType_SetVal" => ModifierType::SetVal,
            "eDisAttributeModifierType_AddBasePercent" => ModifierType::AddBasePercent,
            _ => ModifierType::Other,
        };
        let value = part.split("m_fModValue=").nth(1).and_then(|r| r.split([',', ')']).next()).and_then(|v| v.trim().parse().ok()).unwrap_or(0.0);
        mods.push(AttributeModifier { attribute, kind, value });
    }
    (cost, mods)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_power_line() {
        let s = r#"(m_Name="Vitality",m_eUISelection=eDisUISelectionType_Vitality,m_Levels=((m_RuneCost=0,m_Modifiers=),(m_RuneCost=1,m_Modifiers=((m_AttributeName="Attribute_HealthMax",m_ModType=eDisAttributeModifierType_AddVal,m_fModValue=20.000000))),(m_RuneCost=3,m_Modifiers=((m_AttributeName="Attribute_HealthMax",m_ModType=eDisAttributeModifierType_AddVal,m_fModValue=20.000000),(m_AttributeName="Attribute_HealthRegenInitialDelay",m_ModType=eDisAttributeModifierType_SetVal,m_fModValue=5.000000)))),m_CurrentLevel=-1)"#;
        let p = parse_power(s).unwrap();
        assert_eq!(p.name, "Vitality");
        assert_eq!(p.levels.len(), 3);
        assert_eq!(p.levels[1].0, 1);
        assert_eq!(p.levels[2].1.len(), 2);
        assert_eq!(p.levels[2].1[1].kind, ModifierType::SetVal);
        assert_eq!(p.levels[2].1[1].value, 5.0);
    }
}
