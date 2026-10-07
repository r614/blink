//! Retail test: the coverage scan of L_Boyle_Ext_P with pinned numbers. Skips without the
//! install. The totals are facts of the retail packages under the scanner's rules and are
//! pinned exactly; the handled counts are pinned as floors (coverage may grow, never drop
//! silently: raise the floor when it grows).

use dishonored_oracle::coverage::{self, scan, source::PortRegistry};
use std::path::PathBuf;

#[test]
fn boyle_ext_coverage_pinned() {
    let install = dishonored_oracle::install_dir();
    if !install.join("DishonoredGame").join("CookedPCConsole").is_dir() {
        eprintln!("no Dishonored install: skipped");
        return;
    }
    let t = std::time::Instant::now();
    let files = scan::list_packages(&install, false).unwrap();
    assert_eq!(files.len(), 470);
    let (scans, _, errors) = coverage::scan_all(&files, None);
    assert!(errors.is_empty(), "{errors:?}");
    assert_eq!(scans.iter().map(|s| s.parse_failures).sum::<u32>(), 0);
    let repo = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..").join("..");
    let reg = PortRegistry::scan(&repo).unwrap();
    let res = coverage::coverage(&scans, &reg, 100);
    eprintln!("scan + coverage of 470 packages in {:.2} s", t.elapsed().as_secs_f64());
    assert_eq!(res.maps.len(), 42, "base persistent maps");
    let m = res.maps.iter().find(|m| m.map == "L_Boyle_Ext_P").unwrap();
    // Totals (retail facts under the scanner rules).
    assert_eq!(m.packages.len(), PACKAGES);
    assert_eq!((m.actors.total, m.actor_classes.total), (ACTORS, ACTOR_CLASSES));
    assert_eq!((m.kismet_ops.total, m.kismet_op_classes.total, m.kismet_vars.total), (KISMET_OPS, KISMET_OP_CLASSES, KISMET_VARS));
    assert_eq!((m.props.total, m.prop_pairs.total), (PROPS, PROP_PAIRS));
    assert_eq!((m.mat_exprs.total, m.mat_expr_classes.total), (MAT_EXPRS, MAT_EXPR_CLASSES));
    assert_eq!((m.functions.native, m.functions.script, m.functions.script_bytes), (FN_NATIVE, FN_SCRIPT, FN_SCRIPT_BYTES));
    // Floors of what the port handles.
    assert!(m.actors.handled >= ACTORS_HANDLED, "placed actors handled {} < {ACTORS_HANDLED}", m.actors.handled);
    assert!(m.kismet_ops.handled >= KISMET_OPS_HANDLED, "Kismet ops handled {} < {KISMET_OPS_HANDLED}", m.kismet_ops.handled);
    assert!(m.props.handled >= PROPS_READ, "property settings read {} < {PROPS_READ}", m.props.handled);
}

// Measured 2026-10-07.
const PACKAGES: usize = 8;
const ACTORS: u64 = 1218;
const ACTOR_CLASSES: u64 = 70;
const KISMET_OPS: u64 = 1702;
const KISMET_OP_CLASSES: u64 = 116;
const KISMET_VARS: u64 = 1082;
// Settings of the classes the port handles (98141 / 1815 before the collision audit named the
// KActor family).
const PROPS: u64 = 98671;
const PROP_PAIRS: u64 = 1904;
const MAT_EXPRS: u64 = 604;
const MAT_EXPR_CLASSES: u64 = 7;
const FN_NATIVE: u32 = 823;
const FN_SCRIPT: u32 = 328;
const FN_SCRIPT_BYTES: u64 = 25532;
const ACTORS_HANDLED: u64 = 1080;
const KISMET_OPS_HANDLED: u64 = 1599;
const PROPS_READ: u64 = 66092;
