//! NPC perception and behaviour tuning from the install: Arkane AI tweaks plus
//! `DefaultAI.ini` / `DefaultNPC.ini`. Every field that is read records its source in
//! `AiConfig::provenance`; fields not found keep the `AiConfig::default()` value (which was
//! read from the same objects) and are marked as such.

use crate::content::Content;
use crate::ini::Ini;
use crate::tweaks::{TMap, TVal, TweakDb, get_f32};
use dishonored_ir::ai::{AiConfig, AttentionConfig, AttentionIncrease, MeleeContextConfig, VisTuning};

/// Attention profiles: (field, tweak object). The civilian profiles are cooked into level
/// script packages (`L_Boyle_Ext_Script`); without them their fallback (the guard profile)
/// applies, which has the same values.
const GUARD_UNS: &str = "Twk_Attn_Guard_Unsuspecting.Twk_Attn_Guard_Unsuspecting";
const GUARD_SUS: &str = "Twk_Attn_Guard_Suspecting.Twk_Attn_Guard_Suspecting";
const CIV_UNS: &str = "Twk_Attn_Noncombatant_Uns.Twk_Attn_Noncombatant_Uns";
const CIV_SUS: &str = "Twk_Attn_Noncombatant_Sus.Twk_Attn_Noncombatant_Sus";
const NEUTRAL: &str = "Twk_Attn_Default_Neutral.Twk_Attn_Default_Neutral";
const VISION: &str = "Twk_Vision_Default.Twk_Vision_Default";

fn level_index(name: &str) -> Option<u8> {
    Some(match name {
        "DALFI_None" => 0,
        "DALFI_HeadTrack" => 1,
        "DALFI_TurnToFace" => 2,
        "DALFI_Investigate" => 3,
        "DALFI_Busted" => 4,
        "DALFI_MaxAttention" => 5,
        _ => return None,
    })
}

fn increase(m: &TMap, key: &str, default: AttentionIncrease) -> AttentionIncrease {
    match m.get(key) {
        Some(TVal::Struct(s)) => AttentionIncrease {
            amount: get_f32(s, "m_fByAmount").unwrap_or(0.0),
            to_level: match s.get("m_ToThreshold") {
                Some(TVal::Name(n)) => level_index(n).unwrap_or(default.to_level),
                _ => 0,
            },
        },
        _ => default,
    }
}

fn attention(m: &TMap, d: &AttentionConfig) -> AttentionConfig {
    let f = |k: &str, dv: f32| get_f32(m, k).unwrap_or(dv);
    let mut start = d.start;
    let mut stop = d.stop;
    for i in 0..4 {
        let key = if i == 0 { "m_AttentionThresholds".to_string() } else { format!("m_AttentionThresholds[{i}]") };
        if let Some(TVal::Struct(s)) = m.get(&key) {
            start[i] = get_f32(s, "m_fAttentionLevelToStart").unwrap_or(start[i]);
            stop[i] = get_f32(s, "m_fAttentionLevelToStop").unwrap_or(stop[i]);
        }
    }
    let seeing_scalar_player = match m.get("m_SeeingPawnAttentionIncreaseScalar_ToPlayer") {
        Some(TVal::Struct(s)) => get_f32(s, "m_fBaseValue2_Normal").unwrap_or(d.seeing_scalar_player),
        _ => d.seeing_scalar_player,
    };
    let distance_tuning = match m.get("m_DistanceValueTuning") {
        Some(TVal::Struct(s)) => tuning(s, &d.distance_tuning),
        _ => d.distance_tuning.clone(),
    };
    let mut stopping_variance = d.stopping_variance;
    for (i, v) in stopping_variance.iter_mut().enumerate() {
        let key = if i == 0 { "m_AttentionThresholds".to_string() } else { format!("m_AttentionThresholds[{i}]") };
        if let Some(TVal::Struct(s)) = m.get(&key) {
            *v = get_f32(s, "m_fStoppingVariance").unwrap_or(0.0);
        }
    }
    // `m_ReasonSettings[i].m_bCanIncreaseToBusted` (struct default false).
    let mut can_bust = 0u32;
    let mut any_reason = false;
    for i in 0..21 {
        let key = if i == 0 { "m_ReasonSettings".to_string() } else { format!("m_ReasonSettings[{i}]") };
        if let Some(TVal::Struct(s)) = m.get(&key) {
            any_reason = true;
            if matches!(s.get("m_bCanIncreaseToBusted"), Some(TVal::B(true))) {
                can_bust |= 1 << i;
            }
        }
    }
    if !any_reason {
        can_bust = d.can_bust;
    }
    let clamp_target_visibility = match m.get("m_bClampTargetVisibility") {
        Some(TVal::B(b)) => *b,
        _ => d.clamp_target_visibility,
    };
    AttentionConfig {
        leak_rate: f("m_fAttentionLeakRate", d.leak_rate),
        max: f("m_fMaxAttention", d.max),
        start,
        stop,
        danger_sound: increase(m, "m_IncreaseForDangerSound", d.danger_sound),
        threatening_sound: increase(m, "m_IncreaseForThreateningSound", d.threatening_sound),
        anomaly_sound: increase(m, "m_IncreaseForAnomalySound", d.anomaly_sound),
        help_sound: increase(m, "m_IncreaseForHelpSound", d.help_sound),
        alarm_sound: increase(m, "m_IncreaseForAlarmSound", d.alarm_sound),
        death_rattle: increase(m, "m_IncreaseForDeathRattleSound", d.death_rattle),
        attacked_by: increase(m, "m_IncreaseForAttackedBy", d.attacked_by),
        attacked_enemy: increase(m, "m_IncreaseForAttackedEnemy", d.attacked_enemy),
        discovered_corpse: increase(m, "m_IncreaseForDiscoveredCorpse", d.discovered_corpse),
        ally_combat_target: increase(m, "m_IncreaseForAllyCombatTarget", d.ally_combat_target),
        ally_panic_target: increase(m, "m_IncreaseForAllyPanicTarget", d.ally_panic_target),
        ally_search_target: increase(m, "m_IncreaseForAllySearchTarget", d.ally_search_target),
        touching_rate: f("m_fTouchingPawnAttentionIncreaseRate", d.touching_rate),
        seeing_scalar_player,
        seeing_scalar_npc: f("m_fSeeingPawnAttentionIncreaseScalar_ToLiveNPC", d.seeing_scalar_npc),
        distance_to_lose_sight: f("m_fDistanceToLoseSight", d.distance_to_lose_sight),
        distance_tuning,
        clamp_target_visibility,
        stopping_variance,
        can_bust,
        seeing_scalar_dead_npc: f("m_fSeeingPawnAttentionIncreaseScalar_ToDeadNPC", d.seeing_scalar_dead_npc),
        min_range_unsighted_busted_proxy: f("m_fMinRangeToUnsightedBustedProxy", d.min_range_unsighted_busted_proxy),
    }
}

/// A `DisVisCalcTuningParams` struct.
fn tuning(s: &TMap, d: &VisTuning) -> VisTuning {
    let points = match s.get("m_Points") {
        Some(TVal::List(pts)) => pts.iter().map(|p| [get_f32(p, "m_fInputValue").unwrap_or(0.0), get_f32(p, "m_fOutputValue").unwrap_or(0.0)]).collect(),
        _ => d.points.clone(),
    };
    VisTuning {
        multiply: match s.get("m_ApplyOp") {
            Some(TVal::Name(n)) => n == "DVCTPAO_Multiply",
            _ => d.multiply,
        },
        scalar: get_f32(s, "m_fScalar").unwrap_or(d.scalar),
        bias: get_f32(s, "m_fBias").unwrap_or(d.bias),
        exponent: get_f32(s, "m_fExponent").unwrap_or(d.exponent),
        use_points: match s.get("m_bUsePointsInsteadOfFormula") {
            Some(TVal::B(b)) => *b,
            _ => d.use_points,
        },
        points,
    }
}

/// Load the AI tuning. `extra` names level packages holding level-cooked tweaks.
pub fn load_ai_config(content: &Content, extra: &[String]) -> AiConfig {
    let mut out = AiConfig::default();
    let db = match TweakDb::load_with(content, extra) {
        Ok(db) => db,
        Err(e) => {
            out.provenance.insert("ai".into(), format!("defaults (tweak database failed: {e:#})"));
            return out;
        }
    };
    let mut prov = std::mem::take(&mut out.provenance);
    let mut profile = |path: &str, field: &str, d: &AttentionConfig| -> AttentionConfig {
        match db.try_resolve(path) {
            Some(m) => {
                prov.insert(format!("ai.{field}"), path.to_string());
                attention(&m, d)
            }
            None => {
                prov.insert(format!("ai.{field}"), format!("fallback value (not in the loaded packages: {path})"));
                d.clone()
            }
        }
    };
    out.guard_unsuspecting = profile(GUARD_UNS, "guard_unsuspecting", &out.guard_unsuspecting);
    out.guard_suspecting = profile(GUARD_SUS, "guard_suspecting", &out.guard_suspecting);
    out.civilian_unsuspecting = profile(CIV_UNS, "civilian_unsuspecting", &out.guard_unsuspecting);
    out.civilian_suspecting = profile(CIV_SUS, "civilian_suspecting", &out.guard_suspecting);
    out.neutral = profile(NEUTRAL, "neutral", &out.neutral);
    if let Some(v) = db.try_resolve(VISION) {
        let g = |k: &str, d: f32| get_f32(&v, k).unwrap_or(d);
        let d = out.vision.clone();
        out.vision.inner_fov_deg = g("m_fVision_InnerFOV_Deg", d.inner_fov_deg);
        out.vision.outer_fov_deg = g("m_fVision_OuterFOV_Deg", d.outer_fov_deg);
        out.vision.inner_range = g("m_fVision_InnerMaxRange", d.inner_range);
        out.vision.outer_range = g("m_fVision_OuterMaxRange", d.outer_range);
        out.vision.inner_up = g("m_fVision_InnerVerticalFOVScale_Up", d.inner_up);
        out.vision.inner_down = g("m_fVision_InnerVerticalFOVScale_Down", d.inner_down);
        out.vision.outer_up = g("m_fVision_OuterVerticalFOVScale_Up", d.outer_up);
        out.vision.outer_down = g("m_fVision_OuterVerticalFOVScale_Down", d.outer_down);
        prov.insert("ai.vision".into(), VISION.into());
    }
    let beh = |path: &str| db.try_resolve(path);
    if let Some(m) = beh("AI_BehaviorTweaks_Search.TweaksBehavior_Search_Default") {
        out.search.cancel_investigate_wait = [get_f32(&m, "m_fCancelInvestigateWaitTimeMin").unwrap_or(0.5), get_f32(&m, "m_fCancelInvestigateWaitTimeMax").unwrap_or(1.0)];
        out.search.reached_broadcast_range = get_f32(&m, "m_fSearchReachedProxyBroadcastRange").unwrap_or(out.search.reached_broadcast_range);
        prov.insert("ai.search.cancel_investigate_wait".into(), "AI_BehaviorTweaks_Search.TweaksBehavior_Search_Default".into());
    }
    if let Some(m) = beh("AI_BehaviorTweaks_Search.TweaksBehavior_Search_Default.DisTweaks_AISubState_Investigate_0") {
        out.search.proximity = get_f32(&m, "m_fProximityThresholds").unwrap_or(out.search.proximity);
        prov.insert("ai.search.proximity".into(), "TweaksBehavior_Search_Default.DisTweaks_AISubState_Investigate_0.m_fProximityThresholds".into());
    }
    if let Some(m) = beh("AI_BehaviorTweaks_Guard.TweaksBehavior_Guard_Default") {
        out.guard_attention_span = get_f32(&m, "m_fAttentionSpan").unwrap_or(out.guard_attention_span);
        prov.insert("ai.guard_attention_span".into(), "AI_BehaviorTweaks_Guard.TweaksBehavior_Guard_Default.m_fAttentionSpan".into());
    }
    if let Some(m) = beh("AI_BehaviorTweaks_Panic.TweaksBehavior_Panic_Civilian") {
        let p = &mut out.panic;
        p.cower_distance = get_f32(&m, "m_fCowerReactionDistance").unwrap_or(p.cower_distance);
        p.flee_point_claim_radius = get_f32(&m, "m_fFleePointClaimRadius").unwrap_or(p.flee_point_claim_radius);
        p.flee_interception_distance = get_f32(&m, "m_fFleeInterceptionDistance").unwrap_or(p.flee_interception_distance);
        p.free_flight_time = get_f32(&m, "m_fFreeFlightTimer").unwrap_or(p.free_flight_time);
        prov.insert("ai.panic".into(), "AI_BehaviorTweaks_Panic.TweaksBehavior_Panic_Civilian".into());
    }
    if let Some(m) = beh("AI_BehaviorTweaks_Panic.TweaksBehavior_Panic_Civilian.DisTweaks_AISubState_Flee_0") {
        out.panic.forbidden_angle_deg = get_f32(&m, "m_fForbiddenAngle").unwrap_or(out.panic.forbidden_angle_deg);
        out.panic.forbidden_radius = get_f32(&m, "m_fForbiddenRadius").unwrap_or(out.panic.forbidden_radius);
    }
    if let Some(m) = beh("AI_BehaviorTweaks_TriggerAlarm.TweaksBehavior_TriggerAlarm_Party") {
        out.alarm_companion_range = get_f32(&m, "m_fCompanionRange").unwrap_or(out.alarm_companion_range);
        out.alarm_fight_radius = get_f32(&m, "m_fFightRadius").unwrap_or(out.alarm_fight_radius);
        // Unset chances keep the class default 0.
        let ta = &mut out.trigger_alarm;
        let g = |k: &str| get_f32(&m, k).unwrap_or(0.0);
        ta.chance_sighting_alone = g("m_fAlarmChanceUponSighting_Alone");
        ta.chance_sighting_in_group = g("m_fAlarmChanceUponSighting_InGroup");
        ta.chance_found_corpse = g("m_fAlarmChanceUponFoundCorpse");
        ta.chance_witness_death = g("m_fAlarmChanceUponWitnessDeath");
        ta.chance_help = g("m_fAlarmChanceUponHelp");
        ta.fight_radius = out.alarm_fight_radius;
        ta.companion_range = out.alarm_companion_range;
        ta.wait_before_ring = get_f32(&m, "m_fWaitTimeBeforeRing").unwrap_or(ta.wait_before_ring);
        ta.companion_must_be_ally = matches!(m.get("m_bCompanionMustBeAlly"), Some(TVal::B(true)));
        ta.companion_must_match_squad = matches!(m.get("m_bCompanionMustMatchSquad"), Some(TVal::B(true)));
        prov.insert("ai.alarm".into(), "AI_BehaviorTweaks_TriggerAlarm.TweaksBehavior_TriggerAlarm_Party".into());
    }
    if let Some(m) = beh("AI_BehaviorTweaks_EliteGuard.TweaksBehavior_CombatEliteGuard") {
        let rng = |k: &str, d: [f32; 2]| match m.get(k) {
            Some(TVal::Struct(s)) => [get_f32(s, "m_fMinValue").unwrap_or(0.0), get_f32(s, "m_fMaxValue").unwrap_or(d[1])],
            _ => d,
        };
        out.combat.gun_usage_start = rng("m_GunUsageStart", out.combat.gun_usage_start);
        out.combat.gun_usage_cooldown = rng("m_GunUsageCooldown", out.combat.gun_usage_cooldown);
        prov.insert("ai.combat.gun_usage".into(), "AI_BehaviorTweaks_EliteGuard.TweaksBehavior_CombatEliteGuard".into());
    }
    if let Some(m) = beh("AI_BrainTweaks_Elite.BrainTweaks_Guard_Elite_Party").or_else(|| beh("AI_BrainTweaks_Guard.BrainTweaks_Guard")) {
        out.combat.reachable_melee_range = get_f32(&m, "m_fReachableMeleeRange").unwrap_or(out.combat.reachable_melee_range);
        out.combat.reachable_melee_height = get_f32(&m, "m_fReachableMeleeHeight").unwrap_or(out.combat.reachable_melee_height);
        prov.insert("ai.combat.reachable_melee".into(), "guard brain tweak m_fReachableMelee{Range,Height}".into());
    }
    // Player visibility (`DishonoredVisibilityComponent` tuning).
    if let Some(m) = beh("Twk_PlayerVisibility.Twk_PlayerVisibility") {
        let pv = &mut out.player_visibility;
        if let Some(TVal::Struct(s)) = m.get("m_LightValueTuning") {
            pv.light = tuning(s, &pv.light);
        }
        if let Some(TVal::Struct(s)) = m.get("m_SpeedValueTuning") {
            pv.speed = tuning(s, &pv.speed);
        }
        // Unset bonuses keep the class default 0.
        let b = |k: &str| get_f32(&m, k).unwrap_or(0.0);
        pv.bonus_autocrouching = b("m_fVisLevelBonus_Autocrouching");
        pv.bonus_sneaking = b("m_fVisLevelBonus_Sneaking");
        pv.bonus_sliding = b("m_fVisLevelBonus_Sliding");
        pv.bonus_standing = b("m_fVisLevelBonus_Standing");
        pv.bonus_walking = b("m_fVisLevelBonus_Walking");
        pv.bonus_sprinting = b("m_fVisLevelBonus_Sprinting");
        pv.bonus_swimming = b("m_fVisLevelBonus_Swimming");
        pv.zero_speed_sneaking = matches!(m.get("m_bForceZeroSpeedWhileSneaking"), Some(TVal::B(true)));
        prov.insert("ai.player_visibility".into(), "Twk_PlayerVisibility.Twk_PlayerVisibility".into());
    }
    if let Some(m) = beh("Twk_GlobalPlayerVisSettings.Twk_GlobalPlayerVisSettings") {
        out.player_visibility.light_darkest = get_f32(&m, "m_fLightValueDarkestVal").unwrap_or(out.player_visibility.light_darkest);
        out.player_visibility.light_brightest = get_f32(&m, "m_fLightValueBrightestVal").unwrap_or(out.player_visibility.light_brightest);
    }
    // INIs.
    let ai = Ini::load_layered(&content.root, "AI");
    if let Some(v) = ai.get_f32("DishonoredGame.DishonoredGlobalAIManager", "m_fGroupDistance") {
        out.group_distance = v;
        prov.insert("ai.group_distance".into(), "DefaultAI.ini [DishonoredGame.DishonoredGlobalAIManager] m_fGroupDistance".into());
    }
    // Melee contexts: the elite guard attack pattern's cooldowns (cooked into the level script
    // packages; without them the defaults hold the same values) and the combat manager's
    // inhibition times.
    let path = "AI_AttackPatterns_Shared.TwkAttackPattern_BerserkerElite";
    if let Some(m) = beh(path) {
        if let Some(TVal::List(items)) = m.get("m_ItemContextOverrides") {
            let inhibit = |class: &str| -> f32 {
                let key = match class {
                    "DisItemContext_NPCAttackShort" => "m_fInhibitionTimeShortAttack",
                    "DisItemContext_NPCAttackMedium" => "m_fInhibitionTimeMediumAttack",
                    "DisItemContext_NPCAttackStepBack" => "m_fInhibitionTimeStepBackAttack",
                    _ => return 0.0,
                };
                ai.get_f32("DishonoredGame.DisGlobalCombatManager", key).unwrap_or(0.0)
            };
            let mut ctx = Vec::new();
            // Pattern order is the tweak's; the shared pawn only has the short sword swings.
            for it in items {
                let class = match it.get("m_pContextToOverride") {
                    Some(TVal::Obj(o)) => o.rsplit('.').next().unwrap_or(o).to_string(),
                    _ => continue,
                };
                if !matches!(it.get("m_bContextEnabled"), Some(TVal::B(true))) || inhibit(&class) == 0.0 {
                    continue;
                }
                let cd = match it.get("m_ContextCooldown") {
                    Some(TVal::Struct(s)) => [get_f32(s, "m_fMinValue").unwrap_or(0.0), get_f32(s, "m_fMaxValue").unwrap_or(0.0)],
                    _ => [0.0, 0.0],
                };
                ctx.push(MeleeContextConfig { inhibition: inhibit(&class), name: class, cooldown: cd });
            }
            if !ctx.is_empty() {
                out.combat.melee_contexts = ctx;
                prov.insert("ai.combat.melee_contexts".into(), format!("{path}.m_ItemContextOverrides + DefaultAI.ini m_fInhibitionTime*"));
            }
        }
    }
    if let Some(v) = ai.sections.get("DishonoredGame.DisAINoiseManager").and_then(|s| s.get("m_LoudnessDistanceArray")) {
        let d: Vec<f32> = v.iter().filter_map(|x| x.parse().ok()).collect();
        if !d.is_empty() {
            out.loudness_distances = d;
            prov.insert("ai.loudness_distances".into(), "DefaultAI.ini [DishonoredGame.DisAINoiseManager] m_LoudnessDistanceArray".into());
        }
    }
    let range = |s: Option<&str>| -> Option<[f32; 2]> {
        let s = s?;
        let lo = s.split("m_fMinValue=").nth(1)?.split([',', ')']).next()?.parse().ok()?;
        let hi = s.split("m_fMaxValue=").nth(1)?.split([',', ')']).next()?.parse().ok()?;
        Some([lo, hi])
    };
    if let Some(r) = range(ai.get("DishonoredGame.DishonoredGlobalAIManager", "m_ReactionDelay")) {
        out.reaction_delay = r;
        prov.insert("ai.reaction_delay".into(), "DefaultAI.ini [DishonoredGame.DishonoredGlobalAIManager] m_ReactionDelay".into());
    }
    let crumbs = "DishonoredGame.DishonoredSearchCrumbsComponent";
    let s = &mut out.search;
    s.crumbs = ai.get(crumbs, "m_iNumSearchCrumbs").and_then(|v| v.parse().ok()).unwrap_or(s.crumbs);
    s.crumb_spacing = ai.get_f32(crumbs, "m_fDistanceBetweenCrumbs").unwrap_or(s.crumb_spacing);
    s.crumb_lifetime = ai.get_f32(crumbs, "m_fCrumbLifetime").unwrap_or(s.crumb_lifetime);
    s.heartbeat = ai.get_f32(crumbs, "m_fHeartbeatInterval").unwrap_or(s.heartbeat);
    s.wander_min = ai.get_f32(crumbs, "m_fMinWanderStep").unwrap_or(s.wander_min);
    s.wander_max = ai.get_f32(crumbs, "m_fMaxWanderStep").unwrap_or(s.wander_max);
    s.wander_angle_deg = ai.get_f32(crumbs, "m_fWanderAngularRange").unwrap_or(s.wander_angle_deg);
    prov.insert("ai.search.crumbs".into(), format!("DefaultAI.ini [{crumbs}]"));
    let cm = "DishonoredGame.DisGlobalCombatManager";
    if let Some(v) = ai.get(cm, "m_InnerSkirmishCaps[1]").and_then(|v| v.parse().ok()) {
        out.combat.skirmish_inner = v;
    }
    if let Some(v) = ai.get(cm, "m_TotalSkirmishCaps[1]").and_then(|v| v.parse().ok()) {
        out.combat.skirmish_total = v;
    }
    prov.insert("ai.combat.skirmish".into(), format!("DefaultAI.ini [{cm}] m_InnerSkirmishCaps[1] / m_TotalSkirmishCaps[1] (Normal)"));
    let npc = Ini::load_layered(&content.root, "NPC");
    let ctl = "DishonoredGame.DishonoredNPCController";
    out.auto_glance_radius = npc.get_f32(ctl, "m_fAutoGlanceRadius").unwrap_or(out.auto_glance_radius);
    out.auto_look_radius = npc.get_f32(ctl, "m_fAutoLookRadius").unwrap_or(out.auto_look_radius);
    out.glance_time = npc.get_f32(ctl, "m_fGlanceTime").unwrap_or(out.glance_time);
    out.combat.aim_time_before_shooting = npc.get_f32("DishonoredGame.StateNPCMasterAim", "m_fTimeAimingRequiresBeforeShooting").unwrap_or(out.combat.aim_time_before_shooting);
    prov.insert("ai.glance".into(), format!("DefaultNPC.ini [{ctl}]"));
    // Loudness enum values named in the tweaks (EAINoiseLoudness2 = index 9, Whisper = 2).
    if let Some(&d) = out.loudness_distances.get(9) {
        out.civilian_talking_loudness = d;
        prov.insert("ai.civilian_talking_loudness".into(), "BrainTweaks_Civilian.m_TalkingLoudness = EAINoiseLoudness2 -> m_LoudnessDistanceArray[9]".into());
    }
    if let Some(&d) = out.loudness_distances.get(2) {
        out.death_rattle_loudness = d;
        prov.insert("ai.death_rattle_loudness".into(), "Pwn_EliteGuard_*.m_DeathRattleLoudness = EAINoiseWhisper -> m_LoudnessDistanceArray[2]".into());
    }
    out.provenance = prov;
    out
}
