//! Classified residuals: the definition of done.
//!
//! A probe (or a map, or a system) is done when every remaining difference is classified by
//! cause, not when its agreement reads 100 %. Some residuals are port bugs to fix; others are
//! understood and kept (the oracle cannot see something, the original is itself inconsistent,
//! a sampling artefact at a triangle edge). Classification never hides a residual: classified
//! items stay in the work list with their cause, and the scorecard counts them per cause kind.
//!
//! Causes come from a cause book (`fixtures/oracle/causes.ron`, authored): rules that match
//! work-item ids with `*` wildcards and name the cause with its evidence.

use crate::report::{Table, WorkItem};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum CauseKind {
    /// The port does it differently: fix the port.
    PortBug,
    /// The port does not implement it yet.
    PortMissing,
    /// Explained and kept (with the evidence that it cannot or should not change).
    KnownDifference,
    /// The measurement cannot tell (a hook limitation, a sampling artefact): improve the probe.
    OracleLimitation,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Cause {
    /// The rule that classified the item.
    pub rule: String,
    pub kind: CauseKind,
    pub text: String,
    /// Where the explanation is established (a doc, an address, an unknown's id).
    #[serde(default)]
    pub evidence: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CauseRule {
    pub id: String,
    /// Work-item id pattern, `*` matches any run of characters (`collision:*:systematic: line 0 V*`).
    pub matches: String,
    pub kind: CauseKind,
    pub cause: String,
    #[serde(default)]
    pub evidence: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct CauseBook {
    pub rules: Vec<CauseRule>,
}

/// `*` wildcard match, case-sensitive.
pub fn glob(pattern: &str, s: &str) -> bool {
    let parts: Vec<&str> = pattern.split('*').collect();
    if parts.len() == 1 {
        return pattern == s;
    }
    let mut rest = s;
    for (i, p) in parts.iter().enumerate() {
        if i == 0 {
            let Some(r) = rest.strip_prefix(p) else { return false };
            rest = r;
        } else if i == parts.len() - 1 {
            return rest.ends_with(p);
        } else {
            let Some(k) = rest.find(p) else { return false };
            rest = &rest[k + p.len()..];
        }
    }
    true
}

impl CauseBook {
    pub fn load(path: &Path) -> Result<Self> {
        if !path.is_file() {
            return Ok(Self::default());
        }
        ron::from_str(&std::fs::read_to_string(path)?).with_context(|| format!("{}", path.display()))
    }

    /// Classify every item (first matching rule).
    pub fn classify(&self, items: &mut [WorkItem]) {
        for w in items.iter_mut() {
            w.cause = self.rules.iter().find(|r| glob(&r.matches, &w.id)).map(|r| Cause { rule: r.id.clone(), kind: r.kind, text: r.cause.clone(), evidence: r.evidence.clone() });
        }
    }
}

/// Residuals per probe: (items, count) per cause kind and unclassified.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ResidualSummary {
    /// probe -> kind (`unclassified` or a [`CauseKind`] name) -> (items, summed count).
    pub by_probe: BTreeMap<String, BTreeMap<String, (u64, u64)>>,
}

impl ResidualSummary {
    pub fn of(items: &[WorkItem]) -> Self {
        let mut s = Self::default();
        for w in items {
            let k = w.cause.as_ref().map(|c| format!("{:?}", c.kind)).unwrap_or_else(|| "unclassified".into());
            let e = s.by_probe.entry(w.probe.clone()).or_default().entry(k).or_default();
            e.0 += 1;
            e.1 += w.count;
        }
        s
    }
    /// Done: no unclassified residual left for the probe.
    pub fn done(&self, probe: &str) -> bool {
        self.by_probe.get(probe).is_none_or(|m| !m.contains_key("unclassified"))
    }
    pub fn table(&self) -> Table {
        let mut t = Table::new("Residuals by cause (done = nothing unclassified)", &["probe", "unclassified", "PortBug", "PortMissing", "KnownDifference", "OracleLimitation", "done"]);
        for (p, m) in &self.by_probe {
            let c = |k: &str| m.get(k).map(|(n, c)| format!("{n} items / {c}")).unwrap_or_else(|| "-".into());
            t.row(vec![p.clone(), c("unclassified"), c("PortBug"), c("PortMissing"), c("KnownDifference"), c("OracleLimitation"), if self.done(p) { "yes".into() } else { "no".into() }]);
        }
        t
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn globbing_classification_and_done() {
        assert!(glob("collision:*:systematic: line 0 V*", "collision:L_A_P:systematic: line 0 V checks hit 4.00 uu earlier"));
        assert!(glob("a*", "a") && glob("*b", "ab") && glob("a*c*e", "abcde") && !glob("a*c", "abd") && glob("x", "x") && !glob("x", "xy"));
        let book: CauseBook = ron::from_str(
            r#"(rules: [(id: "backoff", matches: "collision:*:systematic: line *", kind: PortMissing, cause: "line back-off not ported", evidence: "collision branch hit_kind::engine_time")])"#,
        )
        .unwrap();
        let mut items = vec![
            WorkItem { id: "collision:M:systematic: line 0 V checks".into(), probe: "collision".into(), count: 10, ..Default::default() },
            WorkItem { id: "collision:M:L:Wall.C0".into(), probe: "collision".into(), count: 3, ..Default::default() },
        ];
        book.classify(&mut items);
        assert_eq!(items[0].cause.as_ref().unwrap().kind, CauseKind::PortMissing);
        assert!(items[1].cause.is_none());
        let s = ResidualSummary::of(&items);
        assert!(!s.done("collision"));
        assert_eq!(s.by_probe["collision"]["unclassified"], (1, 3));
        items.pop();
        assert!(ResidualSummary::of(&items).done("collision"));
    }
}
