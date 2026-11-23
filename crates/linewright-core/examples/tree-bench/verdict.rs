//! Whether Linewright's way is within the margin of the fastest known way
//! (`docs/specs/runs.md`, Performance), from hyperfine's timings.

use anyhow::{Context as _, Result};
use serde::Deserialize;

/// How many times as long as the fastest known way Linewright's way may take
/// (`docs/specs/runs.md`, Performance, The margin).
pub const MARGIN: f64 = 1.2;

/// What hyperfine's `--export-json` holds, as far as the verdict reads it
/// (<https://github.com/sharkdp/hyperfine/blob/v1.19.0/src/export/json.rs>).
#[derive(Deserialize)]
struct Export {
    results: Vec<Timing>,
}

#[derive(Deserialize)]
struct Timing {
    command: String,
    mean: f64,
}

/// How Linewright's way compares with the fastest of the ways timed.
#[derive(Debug, PartialEq)]
pub struct Verdict {
    /// The command of the fastest way.
    pub fastest: String,
    /// Linewright's mean time over the fastest way's.
    pub ratio: f64,
}

impl Verdict {
    pub fn within_margin(&self) -> bool {
        self.ratio <= MARGIN
    }
}

/// The verdict on hyperfine's `--export-json` output, run with Linewright's
/// way as `--reference`, which hyperfine exports first.
pub fn verdict(export: &str) -> Result<Verdict> {
    let export: Export = serde_json::from_str(export).context("reading hyperfine's export")?;
    let linewright = export.results.first().context("no timings")?;
    let fastest = export
        .results
        .iter()
        .min_by(|a, b| a.mean.total_cmp(&b.mean))
        .context("no timings")?;
    Ok(Verdict {
        fastest: fastest.command.clone(),
        ratio: linewright.mean / fastest.mean,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// hyperfine 1.19.0's export of `hyperfine --reference 'sleep 0.02'
    /// 'sleep 0.01' 'sleep 0.03' --runs 3`, its fields past `mean` cut.
    const EXPORT: &str = r#"{
      "results": [
        { "command": "sleep 0.02", "mean": 0.020887585493333333, "stddev": 0.00026230690911474894 },
        { "command": "sleep 0.01", "mean": 0.010719155493333335, "stddev": 8.116924436221825e-05 },
        { "command": "sleep 0.03", "mean": 0.030789523160000002, "stddev": 0.0001027456570517675 }
      ]
    }"#;

    #[test_log::test]
    fn compares_the_reference_with_the_fastest_way() {
        let verdict = verdict(EXPORT).unwrap();
        assert_eq!(verdict.fastest, "sleep 0.01");
        assert!((verdict.ratio - 1.95).abs() < 0.01, "{verdict:?}");
        assert!(!verdict.within_margin());
    }

    #[test_log::test]
    fn is_within_the_margin_where_the_reference_is_the_fastest() {
        let export = EXPORT.replace("0.010719155493333335", "0.04");
        let verdict = verdict(&export).unwrap();
        assert_eq!(verdict.fastest, "sleep 0.02");
        assert!((verdict.ratio - 1.0).abs() < f64::EPSILON);
        assert!(verdict.within_margin());
    }

    #[test_log::test]
    fn is_within_the_margin_up_to_1_2_times_the_fastest() {
        let within = EXPORT.replace("0.010719155493333335", "0.0174064");
        assert!(verdict(&within).unwrap().within_margin());
        let over = EXPORT.replace("0.010719155493333335", "0.0174");
        assert!(!verdict(&over).unwrap().within_margin());
    }
}
