//! Which original Wwise events stand for which simulation events.
//!
//! Event names are the `AkEvent` object names used by the game packages (player
//! AnimSets, weapon/power archetypes, DishonoredGame defaults); see
//! research/dishonored/AUDIO_FORMAT.md for where each one is referenced.

/// Banks holding the player/combat events below.
pub const PLAYER_BANKS: &[&str] = &[
    "Bank_Player",
    "Bank_Weapon",
    "Bank_Weapon_Sword_Corvo",
    "Bank_Power_Player",
    "Bank_Impact",
    "Bank_Footsteps",
    "Bank_VO_Bark",
    "Bank_Phys",
    "Bank_AI_Breath_Pain",
];

/// Presentation cue -> original event. Keys are referenced by the runtime mapping.
pub const SIM_CUES: &[(&str, &str)] = &[
    ("sword_swing", "Snd_Weapon_Sword_Whooshe_ak"),
    ("sword_parry_whoosh", "Snd_Weapon_Sword_whooshe_Parry_cue_ak"),
    ("sword_clash", "Snd_Imp_Sword_vs_Sword_ak"),
    ("sword_hit_body", "Snd_Imp_Sword_on_Body_cue_ak"),
    ("sword_hit_head", "Snd_Imp_Sword_on_Head_cue_ak"),
    ("assassinate", "Snd_Sword_P_ass_front_master_Cue_ak"),
    ("assassinate_drop", "Snd_Sword_P_ass_dropback_master_cue_ak"),
    ("pistol_fire", "Snd_Weapon_Gun_Corvo_Fire"),
    ("pistol_reload", "Snd_Weapon_Gun_Corvo_Reload"),
    ("crossbow_fire", "Snd_Crossbow_P_Fire"),
    ("bullet_body", "Imp_Bullet_on_Body"),
    ("bullet_head", "Imp_Bullet_on_Head"),
    ("bolt_body", "Imp_Arrow_on_Body"),
    ("bolt_head", "Imp_Arrow_on_Head"),
    ("blink_start", "Snd_Power_P_Blink_Start"),
    ("blink_end", "Snd_Power_P_Blink_Stop"),
    ("power_empty", "Snd_Power_Empty"),
    ("windblast", "Snd_Pwr_P_WindBlast_lvl_1"),
    ("dark_vision_on", "Darkvision_Start"),
    ("dark_vision_off", "Darkvision_Stop"),
    ("jump", "Snd_P_Jump"),
    ("mantle_low", "Snd_P_Mantle_Low_cue_ak"),
    ("mantle_medium", "Snd_P_Mantle_Medium_cue_ak"),
    ("mantle_high", "Snd_P_Mantle_High_cue_ak"),
    ("pain_low", "Snd_VO_Bark_Player_Pain_low_cue_ak"),
    ("pain_big", "Snd_VO_Bark_Player_Pain_Big"),
    ("npc_pain", "Guard_Breath_Pain"),
    ("death_vo", "Snd_VO_Bark_P_Death_cue_ak"),
    ("body_fall", "Snd_Phys_Body_Hard_Impact"),
];

pub fn cue_event(key: &str) -> Option<&'static str> {
    SIM_CUES.iter().find(|c| c.0 == key).map(|c| c.1)
}

/// Footstep surfaces with `FS_P_<Surface>_*` events in Bank_Footsteps.
pub const SURFACES: &[&str] = &[
    "Carpet", "Dirt", "Foliage", "Garbage", "Glass", "Grass", "Gravel", "Metal", "Mud", "Rooftile", "Stone",
    "Stone_Wet", "Water", "Wood",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Gait {
    /// `_Sn`: sneaking (crouched).
    Sneak,
    /// `_R`: walking/running.
    Run,
    /// `_Sp`: sprinting.
    Sprint,
}

/// Footstep event for a surface. NPCs use the guard set (no sneak variants).
pub fn footstep_event(surface: &str, gait: Gait, npc: bool) -> String {
    let who = if npc { "Guard" } else { "P" };
    let g = match (gait, npc) {
        (Gait::Sneak, false) => "Sn",
        (Gait::Sprint, _) => "Sp",
        _ => "R",
    };
    format!("FS_{who}_{surface}_{g}")
}

pub fn landing_event(surface: &str, high: bool) -> String {
    format!("FS_P_{surface}_Fall_{}", if high { "High" } else { "Small" })
}

/// Surface guess from the colliding mesh/actor name. The game picks footsteps from
/// the material's physical material, which is not imported yet (APPROXIMATION).
pub fn surface_from_name(name: &str) -> &'static str {
    let l = name.to_ascii_lowercase();
    let has = |k: &[&str]| k.iter().any(|w| l.contains(w));
    if has(&["water", "puddle"]) {
        "Water"
    } else if has(&["grass", "lawn", "garden"]) {
        "Grass"
    } else if has(&["gravel"]) {
        "Gravel"
    } else if has(&["mud"]) {
        "Mud"
    } else if has(&["hedge", "bush", "foliage", "leaves", "ivy", "plant"]) {
        "Foliage"
    } else if has(&["dirt", "soil", "ground", "terrain"]) {
        "Dirt"
    } else if has(&["carpet", "rug"]) {
        "Carpet"
    } else if has(&["roof", "tile", "shingle"]) {
        "Rooftile"
    } else if has(&["metal", "iron", "steel", "pipe", "grate", "rail", "gate", "chain"]) {
        "Metal"
    } else if has(&["wood", "plank", "boat", "dock", "crate", "barrel", "stair_w", "beam", "table"]) {
        "Wood"
    } else if has(&["glass", "window"]) {
        "Glass"
    } else if has(&["trash", "garbage", "debris"]) {
        "Garbage"
    } else {
        "Stone"
    }
}

/// Level ambience: state events applied first, then the bed and the score.
#[derive(Debug, Clone)]
pub struct MapAmbience {
    pub banks: &'static [&'static str],
    pub state_events: &'static [&'static str],
    pub bed: &'static str,
    pub music: Option<&'static str>,
}

pub fn map_ambience(map: &str) -> Option<MapAmbience> {
    match map.to_ascii_lowercase().as_str() {
        "l_boyle_ext_p" => Some(MapAmbience {
            banks: &["Bank_Amb_LVL_Boyle_Estate", "Bank_Music_Boyle"],
            state_events: &["Set_State_Amb_LVL_Boyle_Ext_Streets", "Music_State_Boyle_Explor"],
            bed: "Play_Amb_LVL_Boyle_Estate",
            music: Some("Play_Music_Boyle"),
        }),
        "l_boyle_int_p" => Some(MapAmbience {
            banks: &["Bank_Amb_LVL_Boyle_Int", "Bank_Music_Boyle"],
            state_events: &["Set_State_Amb_LVL_Boyle_Int_Hall_01", "Music_State_Boyle_Explor"],
            bed: "Play_Amb_LVL_Boyle_Int",
            music: Some("Play_Music_Boyle"),
        }),
        _ => None,
    }
}

/// Every event the runtime may post for the player/combat layer (for pre-decoding
/// and the harness coverage check).
pub fn all_player_events() -> Vec<String> {
    let mut v: Vec<String> = SIM_CUES.iter().map(|c| c.1.to_string()).collect();
    for s in SURFACES {
        for g in [Gait::Sneak, Gait::Run, Gait::Sprint] {
            v.push(footstep_event(s, g, false));
        }
        for g in [Gait::Run, Gait::Sprint] {
            v.push(footstep_event(s, g, true));
        }
        v.push(landing_event(s, false));
        v.push(landing_event(s, true));
    }
    v.sort();
    v.dedup();
    v
}
