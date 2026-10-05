//! End-to-end harness: plays the port and records evidence that it works.
//!
//! ```text
//! harness sim                 headless mechanics scenarios on the real map
//! harness net  [--players 8] [--seconds 60]
//!                             real master + dedicated server processes, bot clients at
//!                             0/30/60/100/150 ms (+loss); FFA played to completion;
//!                             prediction/correction/RTT/snapshot metrics; replay verify
//! harness visual [--seconds 20]
//!                             server + bots + one rendered client recording MP4 and
//!                             a scripted offline smoke run with screenshots
//! harness fidelity [--configs a,b] [--reference DIR]
//!                             fixed Boyle Ext viewpoints with each visual feature on/off;
//!                             before/after pairs + optional SSIM vs original captures
//! harness audio               Wwise media decode sanity per codec + every simulation-event
//!                             mapping resolves to a decodable original sound
//! harness replay FILE         re-simulate a server replay and verify every state hash
//! harness mission [--mission FILE]  plays a custom mission end to end (headless + dedicated server)
//! harness travel              Ext -> Int -> Ext through the original doors; level state kept
//! harness conversation        original Boyle conversations headless (subtitles, VO, choices,
//!                             outputs, flags) + the conversation mission end to end
//! harness all                 sim, net, visual, contracts, audio; writes harness_out/<run>/REPORT.md
//! ```

mod audio;
mod contracts;
mod conversation;
mod mission;
mod travel;
mod fidelity;
mod netsoak;
mod report;
mod scenarios;
mod visual;

use anyhow::Result;
use std::path::PathBuf;

pub struct Ctx {
    pub out: PathBuf,
    pub bin_dir: PathBuf,
    pub args: Vec<String>,
}

impl Ctx {
    pub fn opt(&self, f: &str) -> Option<String> {
        self.args.iter().position(|a| a == f).and_then(|i| self.args.get(i + 1)).cloned()
    }
    pub fn bin(&self, name: &str) -> PathBuf {
        let exe = if cfg!(windows) { format!("{name}.exe") } else { name.to_string() };
        self.bin_dir.join(exe)
    }
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let cmd = args.first().cloned().unwrap_or_else(|| "all".into());
    let run_id = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_secs();
    let out = PathBuf::from(args.iter().position(|a| a == "--out").and_then(|i| args.get(i + 1)).cloned().unwrap_or_else(|| format!("harness_out/{run_id}")));
    std::fs::create_dir_all(&out)?;
    let bin_dir = std::env::current_exe()?.parent().unwrap().to_path_buf();
    let ctx = Ctx { out: out.clone(), bin_dir, args: args.clone() };
    let mut rep = report::Report::new(&out);
    match cmd.as_str() {
        "sim" => scenarios::run(&ctx, &mut rep)?,
        "net" => netsoak::run(&ctx, &mut rep)?,
        "visual" => visual::run(&ctx, &mut rep)?,
        "contracts" => contracts::run(&ctx, &mut rep)?,
        "fidelity" => fidelity::run(&ctx, &mut rep)?,
        "audio" => audio::run(&ctx, &mut rep)?,
        "mission" => mission::run(&ctx, &mut rep)?,
        "travel" => travel::run(&ctx, &mut rep)?,
        "conversation" => conversation::run(&ctx, &mut rep)?,
        "replay" => {
            let file = args.get(1).expect("replay file");
            netsoak::verify_replay_file(&PathBuf::from(file), &mut rep)?;
        }
        "all" => {
            scenarios::run(&ctx, &mut rep)?;
            netsoak::run(&ctx, &mut rep)?;
            visual::run(&ctx, &mut rep)?;
            contracts::run(&ctx, &mut rep)?;
            mission::run(&ctx, &mut rep)?;
            travel::run(&ctx, &mut rep)?;
            audio::run(&ctx, &mut rep)?;
        }
        other => anyhow::bail!("unknown command {other}"),
    }
    let path = rep.write()?;
    println!("\n{}\nreport: {}", rep.summary(), path.display());
    if rep.failed() > 0 {
        std::process::exit(1);
    }
    Ok(())
}
