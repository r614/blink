//! Placed Kismet ops of one class across the retail maps (skipped without the install).
//! `DH_KISMET_CLASS=DisSeqAct_FireProjectile cargo test -p dishonored_assets --test kismet_ops_retail -- --ignored --nocapture`

use dishonored_assets::Content;

#[test]
#[ignore]
fn dump() {
    let Ok(c) = Content::locate() else { return };
    let class = std::env::var("DH_KISMET_CLASS").unwrap_or_else(|_| "DisSeqAct_FireProjectile".into());
    let mut pkgs: Vec<String> = std::fs::read_dir(&c.cooked).unwrap().filter_map(|e| e.ok()?.file_name().to_str()?.strip_suffix(".upk").map(str::to_string)).filter(|n| n.starts_with("L_")).collect();
    pkgs.sort();
    for name in pkgs {
        let Ok(pkg) = c.package(&name) else { continue };
        for i in 0..pkg.exports.len() {
            if pkg.export_class(i) != class {
                continue;
            }
            let props = pkg.read_object(i).map(|h| h.properties).unwrap_or_default();
            let list: Vec<String> = props
                .iter()
                .filter(|p| !matches!(p.name.as_str(), "InputLinks" | "OutputLinks" | "ObjPosX" | "ObjPosY" | "DrawWidth" | "DrawHeight" | "ParentSequence" | "ObjComment"))
                .map(|p| match &p.value {
                    ue3_package::properties::Value::Object(o) => format!("{}={}", p.name, pkg.obj_path(ue3_package::ObjRef::from_index(*o))),
                    v => format!("{}={:?}", p.name, v).chars().take(160).collect(),
                })
                .collect();
            println!("{name}:{} [export {i}] {}", pkg.obj_path(ue3_package::ObjRef::Export(i)), list.join(" "));
        }
    }
}
