//! `trace_summary <trace.json> [top N] [--skip-secs S]`: total and per-frame time per span
//! name from a Chrome trace (Bevy `trace_chrome`), per thread kind (main / render / other).
use anyhow::Result;
use std::collections::HashMap;

fn main() -> Result<()> {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let top: usize = a.get(1).and_then(|s| s.parse().ok()).unwrap_or(40);
    let skip_us: f64 = a.iter().position(|x| x == "--skip-secs").and_then(|i| a.get(i + 1)).and_then(|s| s.parse().ok()).unwrap_or(10.0) * 1e6;
    // One event per line (tracing-chrome); stream it, the file can be gigabytes.
    use std::io::BufRead;
    let reader = std::io::BufReader::new(std::fs::File::open(&a[0])?);
    let events = reader.lines().map_while(Result::ok).filter_map(|l| {
        let t = l.trim().trim_start_matches('[').trim_end_matches(']').trim_end_matches(',').to_string();
        if t.is_empty() { None } else { serde_json::from_str::<serde_json::Value>(&t).ok() }
    });
    let mut t0 = f64::NAN;
    let mut open: HashMap<(u64, String), Vec<f64>> = HashMap::new();
    let mut total: HashMap<String, (f64, u64)> = HashMap::new();
    let mut frames = 0u64;
    let (mut first, mut last) = (f64::MAX, 0f64);
    for e in events {
        let (Some(ph), Some(ts)) = (e["ph"].as_str(), e["ts"].as_f64()) else { continue };
        if t0.is_nan() {
            t0 = ts;
        }
        if ts - t0 < skip_us {
            continue;
        }
        let name = e["name"].as_str().unwrap_or("?").to_string();
        let tid = e["tid"].as_u64().unwrap_or(0);
        first = first.min(ts);
        last = last.max(ts);
        let mut add = |n: &str, d: f64| {
            let t = total.entry(n.to_string()).or_default();
            t.0 += d;
            t.1 += 1;
        };
        match ph {
            "B" => open.entry((tid, name)).or_default().push(ts),
            "E" => {
                if let Some(start) = open.get_mut(&(tid, name.clone())).and_then(|v| v.pop()) {
                    if name == "frame" || name == "update" && false {
                        frames += 1;
                    }
                    add(&name, ts - start);
                }
            }
            "X" => add(&name, e["dur"].as_f64().unwrap_or(0.0)),
            _ => {}
        }
    }
    let secs = (last - first) / 1e6;
    let frames = total.get("frame").map(|t| t.1).unwrap_or(frames).max(1);
    println!("{secs:.1} s analysed, {frames} frames ({:.1} fps)", frames as f64 / secs.max(1e-6));
    let mut v: Vec<_> = total.into_iter().collect();
    v.sort_by(|a, b| b.1.0.total_cmp(&a.1.0));
    for (n, (us, c)) in v.into_iter().take(top) {
        println!("{:>9.2} ms/frame {:>8} calls  {n}", us / 1000.0 / frames as f64, c);
    }
    Ok(())
}
