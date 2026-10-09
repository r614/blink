//! `ArkDynamicPylon`s placed in the retail maps (skipped without the install).
//! `cargo test -p dishonored_assets --test dynamic_pylons_retail -- --ignored --nocapture`

use dishonored_assets::Content;

#[test]
#[ignore]
fn dump() {
    let Ok(c) = Content::locate() else { return };
    let mut pkgs: Vec<String> = std::fs::read_dir(&c.cooked).unwrap().filter_map(|e| e.ok()?.file_name().to_str()?.strip_suffix(".upk").map(str::to_string)).filter(|n| n.starts_with("L_")).collect();
    pkgs.sort();
    for name in pkgs {
        let Ok(pkg) = c.package(&name) else { continue };
        for i in 0..pkg.exports.len() {
            if pkg.export_class(i) != "ArkDynamicPylon" {
                continue;
            }
            let props = pkg.read_object(i).map(|h| h.properties).unwrap_or_default();
            let list: Vec<String> = props.iter().filter(|p| !matches!(p.name.as_str(), "NavMeshPtr" | "ObstacleMesh" | "DynamicObstacleMesh" | "Components")).map(|p| format!("{}={:?}", p.name, p.value).chars().take(120).collect()).collect();
            println!("{name}:{} {}", pkg.obj_name(ue3_package::ObjRef::Export(i)), list.join(" "));
        }
    }
}
