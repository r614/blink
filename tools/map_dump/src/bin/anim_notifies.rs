//! `anim_notifies <package> <animset path> [filter]`: sequence lengths, rates and notify
//! events (time, duration, class, key properties) — gameplay timing evidence.
use anyhow::{Context, Result};
use dishonored_assets::Content;
use ue3_package::properties::{Value, find, format_value};
use ue3_package::ObjRef;

fn main() -> Result<()> {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let c = Content::locate()?;
    let pkg = c.package(&a[0])?;
    let set = (0..pkg.exports.len()).find(|&i| pkg.obj_path(ObjRef::Export(i)).eq_ignore_ascii_case(&a[1])).context("animset")?;
    let filt = a.get(2).map(|s| s.to_ascii_lowercase());
    let sh = pkg.read_object(set)?;
    for s in find(&sh.properties, "Sequences").context("seqs")?.array_objects()? {
        let ObjRef::Export(e) = ObjRef::from_index(s) else { continue };
        let h = pkg.read_object(e)?;
        let name = match find(&h.properties, "SequenceName").map(|p| &p.value) { Some(Value::Name(n)) => n.clone(), _ => "?".into() };
        if filt.as_ref().is_some_and(|f| !name.to_ascii_lowercase().contains(f)) { continue; }
        let len = find(&h.properties, "SequenceLength").and_then(|p| p.as_float()).unwrap_or(0.0);
        let rate = find(&h.properties, "RateScale").and_then(|p| p.as_float()).unwrap_or(1.0);
        println!("{name}: length {len:.3}s rate {rate}");
        if let Some(n) = find(&h.properties, "Notifies") {
            for ev in n.array_structs(&pkg).unwrap_or_default() {
                let t = find(&ev, "Time").and_then(|p| p.as_float()).unwrap_or(0.0);
                let d = find(&ev, "Duration").and_then(|p| p.as_float()).unwrap_or(0.0);
                let obj = find(&ev, "Notify").and_then(|p| p.as_object()).map(ObjRef::from_index);
                let (class, extra) = match obj {
                    Some(ObjRef::Export(ni)) => {
                        let props = pkg.read_object(ni).map(|h| h.properties).unwrap_or_default();
                        let ex: Vec<String> = props.iter().filter(|p| !matches!(p.value, Value::Object(_))).take(6).map(|p| format!("{}={}", p.name, format_value(&pkg, &p.value))).collect();
                        (pkg.export_class(ni), ex.join(" "))
                    }
                    _ => ("None".into(), String::new()),
                };
                println!("   t={t:.3} dur={d:.3} {class} {extra}");
            }
        }
    }
    Ok(())
}
