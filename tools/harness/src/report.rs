//! Evidence report: every check with PASS/FAIL, measured values and artifact links.

use serde::Serialize;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize)]
pub struct Check {
    pub section: String,
    pub name: String,
    pub pass: bool,
    pub detail: String,
}

#[derive(Debug, Serialize)]
pub struct Report {
    pub out: PathBuf,
    pub checks: Vec<Check>,
    pub tables: Vec<(String, Vec<String>, Vec<Vec<String>>)>,
    pub artifacts: Vec<(String, String)>,
}

impl Report {
    pub fn new(out: &Path) -> Self {
        Self { out: out.to_path_buf(), checks: Vec::new(), tables: Vec::new(), artifacts: Vec::new() }
    }

    pub fn check(&mut self, section: &str, name: &str, pass: bool, detail: impl Into<String>) {
        let detail = detail.into();
        println!("[{}] {section} :: {name} — {detail}", if pass { "PASS" } else { "FAIL" });
        self.checks.push(Check { section: section.into(), name: name.into(), pass, detail });
    }

    pub fn table(&mut self, title: &str, header: &[&str], rows: Vec<Vec<String>>) {
        self.tables.push((title.into(), header.iter().map(|s| s.to_string()).collect(), rows));
    }

    pub fn artifact(&mut self, label: &str, path: &Path) {
        let rel = path.strip_prefix(&self.out).unwrap_or(path).display().to_string().replace('\\', "/");
        self.artifacts.push((label.into(), rel));
    }

    pub fn failed(&self) -> usize {
        self.checks.iter().filter(|c| !c.pass).count()
    }

    pub fn summary(&self) -> String {
        format!("{} checks, {} passed, {} failed", self.checks.len(), self.checks.len() - self.failed(), self.failed())
    }

    pub fn write(&self) -> anyhow::Result<PathBuf> {
        let mut md = String::from("# Harness report\n\n");
        md += &format!("**{}**\n\n", self.summary());
        let mut section = String::new();
        for c in &self.checks {
            if c.section != section {
                section = c.section.clone();
                md += &format!("\n## {section}\n\n| | Check | Detail |\n|---|---|---|\n");
            }
            md += &format!("| {} | {} | {} |\n", if c.pass { "PASS" } else { "**FAIL**" }, c.name, c.detail.replace('|', "/"));
        }
        for (title, header, rows) in &self.tables {
            md += &format!("\n### {title}\n\n| {} |\n|{}|\n", header.join(" | "), header.iter().map(|_| "---").collect::<Vec<_>>().join("|"));
            for r in rows {
                md += &format!("| {} |\n", r.join(" | "));
            }
        }
        if !self.artifacts.is_empty() {
            md += "\n## Artifacts\n\n";
            for (l, p) in &self.artifacts {
                md += &format!("- {l}: `{p}`\n");
            }
        }
        let path = self.out.join("REPORT.md");
        std::fs::write(&path, md)?;
        std::fs::write(self.out.join("report.json"), serde_json::to_string_pretty(self)?)?;
        Ok(path)
    }
}
