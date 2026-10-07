//! Scorecard and ranked work list, written as markdown plus JSON, with one evidence dossier
//! per work item (`dossiers/<n>.md`), so an agent can pick an item and fix it without
//! re-running the investigation.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

/// One thing to fix, with its evidence.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct WorkItem {
    /// Stable identifier: `<probe>:<kind>:<key>`.
    pub id: String,
    pub title: String,
    /// The probe that found it (`coverage`, `collision`, `trace`...).
    pub probe: String,
    /// The game system it belongs to.
    pub system: String,
    /// Instances, disagreeing queries or diverging ticks.
    pub count: u64,
    /// Ranking score: count times the probe's weight (ORACLE.md "Ranking").
    pub impact: f64,
    pub maps: Vec<String>,
    /// Dossier lines (markdown).
    pub evidence: Vec<String>,
    /// Commands that reproduce or re-check it.
    pub repro: Vec<String>,
    /// Where in the port the fix most likely goes.
    pub code: Vec<String>,
}

/// A titled table.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Table {
    pub title: String,
    pub headers: Vec<String>,
    pub rows: Vec<Vec<String>>,
}

impl Table {
    pub fn new(title: &str, headers: &[&str]) -> Self {
        Self { title: title.into(), headers: headers.iter().map(|s| s.to_string()).collect(), rows: Vec::new() }
    }
    pub fn row(&mut self, cells: Vec<String>) {
        self.rows.push(cells);
    }
    pub fn markdown(&self) -> String {
        let mut s = String::new();
        if !self.title.is_empty() {
            let _ = writeln!(s, "### {}\n", self.title);
        }
        let _ = writeln!(s, "| {} |", self.headers.join(" | "));
        let _ = writeln!(s, "|{}", "---|".repeat(self.headers.len()));
        for r in &self.rows {
            let cells: Vec<String> = r.iter().map(|c| c.replace('|', "\\|")).collect();
            let _ = writeln!(s, "| {} |", cells.join(" | "));
        }
        s
    }
}

/// The scorecard of a run.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Scorecard {
    pub title: String,
    /// `key: value` facts (exe hash, timings, totals).
    pub summary: Vec<(String, String)>,
    pub tables: Vec<Table>,
    pub notes: Vec<String>,
}

impl Scorecard {
    pub fn markdown(&self) -> String {
        let mut s = format!("# {}\n\n", self.title);
        for (k, v) in &self.summary {
            let _ = writeln!(s, "- **{k}**: {v}");
        }
        s.push('\n');
        for t in &self.tables {
            s.push_str(&t.markdown());
            s.push('\n');
        }
        for n in &self.notes {
            let _ = writeln!(s, "{n}\n");
        }
        s
    }
}

pub fn worklist_markdown(items: &[WorkItem], limit: usize) -> String {
    let mut s = String::from("# Work list\n\nRanked by impact (count x probe weight). Each item has a dossier in `dossiers/`.\n\n");
    let _ = writeln!(s, "| # | impact | probe | system | item | count | maps | dossier |\n|---|---|---|---|---|---|---|---|");
    for (i, w) in items.iter().take(limit).enumerate() {
        let maps = if w.maps.len() > 3 { format!("{} (+{})", w.maps[..3].join(", "), w.maps.len() - 3) } else { w.maps.join(", ") };
        let _ = writeln!(s, "| {} | {:.1} | {} | {} | {} | {} | {} | [{}](dossiers/{}.md) |", i + 1, w.impact, w.probe, w.system, w.title.replace('|', "\\|"), w.count, maps, i + 1, i + 1);
    }
    if items.len() > limit {
        let _ = writeln!(s, "\n{} more items in worklist.json.", items.len() - limit);
    }
    s
}

pub fn dossier_markdown(rank: usize, w: &WorkItem) -> String {
    let mut s = format!("# {rank}. {}\n\n- id: `{}`\n- probe: {}\n- system: {}\n- count: {}\n- impact: {:.2}\n", w.title, w.id, w.probe, w.system, w.count, w.impact);
    if !w.maps.is_empty() {
        let _ = writeln!(s, "- maps: {}", w.maps.join(", "));
    }
    s.push_str("\n## Evidence\n\n");
    for e in &w.evidence {
        let _ = writeln!(s, "{e}\n");
    }
    if !w.repro.is_empty() {
        s.push_str("## Reproduce / re-check\n\n```text\n");
        for r in &w.repro {
            let _ = writeln!(s, "{r}");
        }
        s.push_str("```\n\n");
    }
    if !w.code.is_empty() {
        s.push_str("## Where to look\n\n");
        for c in &w.code {
            let _ = writeln!(s, "- {c}");
        }
    }
    s
}

/// Write `SCORECARD.md`, `scorecard.json`, `WORKLIST.md`, `worklist.json` and the dossiers of
/// the first `dossier_limit` items into `dir`.
pub fn write_all(dir: &Path, card: &Scorecard, items: &[WorkItem], dossier_limit: usize) -> Result<Vec<PathBuf>> {
    std::fs::create_dir_all(dir.join("dossiers"))?;
    let mut out = Vec::new();
    let mut put = |name: &str, text: String| -> Result<()> {
        let p = dir.join(name);
        std::fs::write(&p, text)?;
        out.push(p);
        Ok(())
    };
    put("SCORECARD.md", card.markdown())?;
    put("scorecard.json", serde_json::to_string_pretty(card)?)?;
    put("WORKLIST.md", worklist_markdown(items, dossier_limit))?;
    put("worklist.json", serde_json::to_string_pretty(items)?)?;
    for (i, w) in items.iter().take(dossier_limit).enumerate() {
        std::fs::write(dir.join("dossiers").join(format!("{}.md", i + 1)), dossier_markdown(i + 1, w))?;
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markdown_escapes_pipes_and_links_dossiers() {
        let mut t = Table::new("T", &["a", "b"]);
        t.row(vec!["x|y".into(), "1".into()]);
        assert!(t.markdown().contains("| x\\|y | 1 |"));
        let w = WorkItem { id: "c:k:x".into(), title: "Fix x".into(), probe: "collision".into(), count: 3, impact: 3.0, maps: vec!["A".into()], evidence: vec!["e".into()], ..Default::default() };
        let md = worklist_markdown(std::slice::from_ref(&w), 10);
        assert!(md.contains("[1](dossiers/1.md)"));
        let d = dossier_markdown(1, &w);
        assert!(d.contains("# 1. Fix x") && d.contains("## Evidence"));
        let dir = std::env::temp_dir().join(format!("oracle_report_{}", std::process::id()));
        let files = write_all(&dir, &Scorecard { title: "S".into(), ..Default::default() }, &[w], 5).unwrap();
        assert_eq!(files.len(), 4);
        assert!(dir.join("dossiers/1.md").is_file());
        std::fs::remove_dir_all(&dir).ok();
    }
}
