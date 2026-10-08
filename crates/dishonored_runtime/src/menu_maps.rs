//! The maps the launcher offers (free roam, the offline match and the server creator): the 42
//! base maps of the install (`research/dishonored/MAP_SWEEP.md`), in the campaign order of
//! `DefaultEngine.ini [DishonoredGame.DishonoredEngine] m_MapConfig`, with what the original
//! shows for them: the localised name (`DishonoredGame.int [MapNames]`, the loading screen's
//! map name) and the mission card picture (`m_ImageName` ->
//! `UI_Mission_Large.MissionsScreen_<ImageName>_Large`, the texture the Missions screen's
//! `ImgLoader` puts on a card; `Twk_GFxMoviePlayerMenuBase.m_MapLargeImagePackage`).
//!
//! Free roam starts the original Kismet with the map's entry event of the global sequence
//! (`ChangeLvl_*` in `DishonoredGameFull_P`, as the map sweep's smoke scenarios do), which
//! puts the player at the map's travel start. Ten maps are entered from a story-state switch
//! that sends a fresh game to a sibling map (the Pub by chapter, Kingsparrow / the lighthouse
//! by chaos, the epilogues): those start without the entry event, on the map's own Kismet and
//! default player start.

use dishonored_assets::ini::Ini;

/// (map, entry event of the global sequence, entered through a story-state redirect).
const BASE_MAPS: &[(&str, &str, bool)] = &[
    ("DishonoredGameFull_P", "", false),
    ("L_ArtDealer_P", "ChangeLvl_fromStreets1_toArtDealer", false),
    ("L_Boyle_Ext_P", "ChangeLvl_fromHub_toBoyle", false),
    ("L_Boyle_Int_P", "ChangeLvl_fromBoyleExterior_toBoyleInterior", false),
    ("L_Bridge_Part1a_P", "ChangeLvl_fromHub_toBridge", false),
    ("L_Bridge_Part1b_P", "ChangeLvl_fromBridge1a_toBridge1b", false),
    ("L_Bridge_Part1c_P", "ChangeLvl_fromBridge1b_toBridge1c", false),
    ("L_Bridge_Part2_P", "ChangeLvl_fromBridge1_toBridge2", false),
    ("L_Brothel_P", "ChangeLvl_fromStreets2_toBrothel", false),
    ("L_Distillery2_P", "ChangeLvl_fromStreets2_toDistillery", false),
    ("L_Distillery_P", "ChangeLvl_fromStreets1_toDistillery", false),
    ("L_Flooded_FAssassins_P", "ChangeLvl_fromFStreet_toFAssassins", false),
    ("L_Flooded_FGate_P", "ChangeLvl_fromStreetsSewer_toFgate", false),
    ("L_Flooded_FIntro_P", "ChangeLvl_fromHub_toFloodedDistrict", false),
    ("L_Flooded_FRefinery_P", "ChangeLvl_fromFStreet_toFRefineryAR", false),
    ("L_Flooded_FStreets_P", "ChangeLvl_fromFIntro_toFStreets1", false),
    ("L_Galvani1_P", "ChangeLvl_fromStreets1_toGalvani", false),
    ("L_Galvani2_P", "ChangeLvl_fromStreets2_toGalvani", false),
    ("L_Isl_HighChaos_P", "ChangeLvl_fromHub7_toLighthouseIsland", true),
    ("L_Isl_LowChaos_P", "ChangeLvl_fromHub7_toLighthouseIsland", false),
    ("L_LightH_HighChaos_P", "ChangeLvl_fromLighthouseIsland_toLighthouse", false),
    ("L_LightH_LowChaos_P", "ChangeLvl_fromLighthouseIsland_toLighthouse", true),
    ("L_Out_HighChaos_EmilyAlive_P", "ChangeLvl_toEpilogue", true),
    ("L_Out_HighChaos_emilyDead_P", "ChangeLvl_toEpilogue", true),
    ("L_Out_P", "ChangeLvl_toEpilogue", true),
    ("L_OutsiderDream_P", "ChangeLvl_fromHub01_toOutsiderDream", false),
    ("L_Ovrsr_Back_P", "ChangeLvl_fromOvrsrOffice_toOvrsrBack_Alley", false),
    ("L_Ovrsr_Kennel_P", "ChangeLvl_OvrsrSewerExit_to_Kennel", false),
    ("L_Ovrsr_P", "ChangeLvl_fromStreets1_toOverseerOffice", false),
    ("L_Prison_P", "ChangeLvl_fromTower_toPrison", false),
    ("L_PrsnSewer_P", "ChangeLvl_fromPrison_toSewer", false),
    ("L_Pub_Assault_P", "ChangeLvl_fromBoyle_toHub", true),
    ("L_Pub_Day_P", "ChangeLvl_fromBridge_toHub", true),
    ("L_Pub_Dusk_P", "ChangeLvl_fromStreets1_toHub", true),
    ("L_Pub_Morning_P", "ChangeLvl_fromStreets2_toHub", true),
    ("L_Pub_Night_P", "ChangeLvl_fromPrison_toHub", true),
    ("L_Streets1_P", "ChangeLvl_fromHub_toStreets1", false),
    ("L_Streets2_P", "ChangeLvl_fromHub_toStreets2", false),
    ("L_Streetsewer_P", "ChangeLvl_fromHub7_toStreetSewer", false),
    ("L_Tower_P", "ChangeLvl_fromPrison_toTower", false),
    ("L_TowerRtrn_Int_P", "ChangeLvl_fromTowerReturnYard_toTowerReturnInterior", false),
    ("L_TowerRtrn_Yard_P", "ChangeLvl_fromHub_toTowerReturnYard", false),
];

#[derive(Clone, Debug, PartialEq)]
pub struct MapEntry {
    /// Persistent level name (`L_Boyle_Ext_P`).
    pub map: String,
    /// Localised name (`[MapNames]`), else `m_FriendlyName`, else the level name.
    pub name: String,
    /// `m_ImageName` (the card picture `MissionsScreen_<image>_Large`).
    pub image: Option<String>,
    /// Entry event for free roam (None: the level's own Kismet only).
    pub start_event: Option<String>,
    /// Entered through a story-state redirect in the original.
    pub redirected: bool,
}

fn field<'a>(s: &'a str, key: &str) -> Option<&'a str> {
    let i = s.find(&format!("{key}="))? + key.len() + 1;
    let rest = &s[i..];
    let rest = rest.strip_prefix('"').unwrap_or(rest);
    let end = rest.find(['"', ',', ')']).unwrap_or(rest.len());
    Some(&rest[..end])
}

/// The catalogue, in `m_MapConfig` order (maps it does not list last, by name).
pub fn catalog() -> Vec<MapEntry> {
    let mut cfg: Vec<(String, String, String)> = Vec::new();
    let mut loc = Ini::default();
    if let Ok(content) = dishonored_assets::Content::locate() {
        if let Some(root) = content.cooked.parent().and_then(|p| p.parent()) {
            let engine = Ini::load_layered(root, "Engine");
            for v in engine.sections.get("DishonoredGame.DishonoredEngine").and_then(|s| s.get("m_MapConfig")).into_iter().flatten() {
                if let Some(n) = field(v, "m_Name") {
                    cfg.push((n.to_string(), field(v, "m_FriendlyName").unwrap_or("").to_string(), field(v, "m_ImageName").unwrap_or("").to_string()));
                }
            }
            if let Ok(b) = std::fs::read(root.join("DishonoredGame/Localization/INT/DishonoredGame.int")) {
                loc.parse_into(&dishonored_assets::loc::decode(&b));
            }
        }
    }
    let mut v: Vec<(usize, MapEntry)> = BASE_MAPS
        .iter()
        .map(|(m, ev, red)| {
            let c = cfg.iter().position(|c| c.0.eq_ignore_ascii_case(m));
            let (friendly, image) = c.map(|i| (cfg[i].1.clone(), cfg[i].2.clone())).unwrap_or_default();
            let name = loc.get("MapNames", m).map(|s| s.trim().trim_matches('"').to_string()).filter(|s| !s.is_empty()).or((!friendly.is_empty()).then_some(friendly)).unwrap_or_else(|| m.to_string());
            let entry = MapEntry {
                map: m.to_string(),
                name,
                image: (!image.is_empty()).then_some(image),
                start_event: (!ev.is_empty() && !red).then(|| ev.to_string()),
                redirected: *red,
            };
            (c.unwrap_or(usize::MAX), entry)
        })
        .collect();
    v.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.map.cmp(&b.1.map)));
    v.into_iter().map(|(_, e)| e).collect()
}

/// The entry for `map` (or a plain one for maps outside the catalogue).
pub fn entry(maps: &[MapEntry], map: &str) -> MapEntry {
    maps.iter().find(|m| m.map.eq_ignore_ascii_case(map)).cloned().unwrap_or(MapEntry { map: map.to_string(), name: map.to_string(), image: None, start_event: None, redirected: false })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn forty_two_base_maps() {
        assert_eq!(BASE_MAPS.len(), 42);
        let redirected = BASE_MAPS.iter().filter(|m| m.2).count();
        assert_eq!(redirected, 10);
    }

    #[test]
    fn config_fields() {
        let v = r#"(m_Name="L_Tower_P",m_FriendlyName="Dunwall Tower",m_ImageName="EmpressTower",m_LoadingMovieName="LoadingEmpressTower")"#;
        assert_eq!(field(v, "m_Name"), Some("L_Tower_P"));
        assert_eq!(field(v, "m_ImageName"), Some("EmpressTower"));
        assert_eq!(field(v, "m_FriendlyName"), Some("Dunwall Tower"));
    }
}
