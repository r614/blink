//! `facefx_dump <package> <export-substring> [-v] [--hex N]`: decode cooked FaceFX actors and
//! anim sets (research; output is derived from retail data, keep it out of the repository).
use anyhow::Result;
use dishonored_assets::Content;
use dishonored_assets::facefx::*;
use ue3_package::ObjRef;

fn main() -> Result<()> {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let c = Content::locate()?;
    let pkg = c.package(&a[0])?;
    let filt = a.get(1).cloned().unwrap_or_default().to_ascii_lowercase();
    let verbose = a.iter().any(|x| x == "-v");
    for i in 0..pkg.exports.len() {
        let class = pkg.export_class(i);
        if class != "FaceFXAsset" && class != "FaceFXAnimSet" {
            continue;
        }
        let path = pkg.obj_path(ObjRef::Export(i));
        if !path.to_ascii_lowercase().contains(&filt) {
            continue;
        }
        let raw = match raw_archive(&pkg, i) {
            Ok(r) => r,
            Err(e) => {
                println!("=== {path} ({class}) ERROR {e:#}");
                continue;
            }
        };
        println!("=== {path} ({class}) {} bytes", raw.len());
        if class == "FaceFXAnimSet" {
            match read_anim_set(raw) {
                Ok(s) => {
                    println!("  set {} actor {} group {} anims {}", s.name, s.actor, s.group, s.anims.len());
                    for an in &s.anims {
                        let len = an.curves.iter().flat_map(|c| c.keys.last()).map(|k| k.time).fold(0.0f32, f32::max);
                        println!("  anim {} curves {} len {len:.2} blend {}/{} {}", an.name, an.curves.len(), an.blend_in, an.blend_out, an.audio_path);
                        if verbose {
                            for cv in &an.curves {
                                println!("    {} interp {} keys {}", cv.name, cv.interp, cv.keys.len());
                            }
                        }
                    }
                }
                Err(e) => println!("  ERROR {e:#}"),
            }
        } else {
            match read_actor(raw) {
                Ok(act) => {
                    println!("  actor {} bones {} nodes {}", act.name, act.bones.len(), act.nodes.len());
                    for b in act.bones.iter().filter(|b| verbose || !b.poses.is_empty()) {
                        println!("   bone {} ref {:?} poses {:?}", b.name, b.reference, b.poses.iter().map(|p| (p.node, p.xf.pos, p.xf.rot)).collect::<Vec<_>>());
                    }
                    for (i, nd) in act.nodes.iter().enumerate() {
                        let ins: Vec<String> = nd.inputs.iter().map(|l| format!("{}:f{}{:?}", act.nodes[l.node as usize].name, l.func, l.params)).collect();
                        println!("   node {i} {} kind {} [{}, {}] op {} <- {}", nd.name, nd.kind, nd.min, nd.max, nd.op, ins.join(", "));
                    }
                }
                Err(e) => println!("  ERROR {e:#}"),
            }
        }
    }
    Ok(())
}
