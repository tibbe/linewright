//! What a GitHub Actions job log shows about why the job failed.

/// The most lines of a failed step's output a fix session is given
/// (`docs/specs/pull_requests.md`).
const MAX_LINES: usize = 50;

/// The end of the output of the step that failed in the job log `log`, or of
/// the whole log where it does not show which step failed.
pub fn failed_step_output(log: &str) -> String {
    // The log starts with a byte order mark.
    let log = log.trim_start_matches('\u{feff}');
    let lines: Vec<&str> = log.lines().map(untimestamped).collect();
    let shown = failed_step(&lines).unwrap_or(&lines);
    shown[shown.len().saturating_sub(MAX_LINES)..].join("\n")
}

/// The lines of the step that failed, found from the log's workflow command
/// markers
/// (<https://docs.github.com/en/actions/reference/workflows-and-actions/workflow-commands>):
/// from the last `##[group]Run …` line before the first `##[error]` line to
/// the step's last `##[error]` line, before the next step or the post-job
/// cleanup. In a composite action that is the failing sub-step.
fn failed_step<'a>(lines: &'a [&'a str]) -> Option<&'a [&'a str]> {
    let is_error = |l: &&str| l.starts_with("##[error]");
    let is_step = |l: &&str| l.starts_with("##[group]Run ");
    let first_error = lines.iter().position(is_error)?;
    let start = lines[..first_error].iter().rposition(is_step)?;
    let step_end = lines[first_error..]
        .iter()
        .position(|l| is_step(l) || l.starts_with("Post job cleanup"))
        .map_or(lines.len(), |i| first_error + i);
    let last_error = lines[..step_end].iter().rposition(is_error)?;
    Some(&lines[start..=last_error])
}

/// `line` without the timestamp GitHub starts each line of a job log with.
fn untimestamped(line: &str) -> &str {
    match line.split_once(' ') {
        Some((stamp, rest)) if chrono::DateTime::parse_from_rfc3339(stamp).is_ok() => rest,
        _ => line,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `lines` as a job log, each line timestamped as GitHub does.
    fn job_log(lines: &[&str]) -> String {
        lines
            .iter()
            .map(|l| format!("2026-10-14T09:12:03.1234567Z {l}\n"))
            .collect()
    }

    #[test]
    fn the_failed_step_runs_from_its_run_group_to_its_last_error() {
        let log = job_log(&[
            "Current runner version: '2.329.0'",
            "##[group]Operating System",
            "Ubuntu",
            "##[endgroup]",
            "##[group]Run actions/checkout@v5",
            "with:",
            "##[endgroup]",
            "Syncing repository: tibbe/linewright",
            "##[group]Run cargo test",
            "cargo test",
            "##[endgroup]",
            "   Compiling linewright v0.1.0",
            "error[E0425]: cannot find value `x` in this scope",
            "##[error]cannot find value `x` in this scope",
            "error: could not compile `linewright`",
            "##[error]Process completed with exit code 101.",
            "##[group]Run actions/upload-artifact@v4",
            "##[endgroup]",
            "##[error]No files were found with the provided path.",
            "Post job cleanup.",
            "[command]/usr/bin/git version",
            "Cleaning up orphan processes",
        ]);

        assert_eq!(
            failed_step_output(&log),
            "##[group]Run cargo test\n\
             cargo test\n\
             ##[endgroup]\n   \
             Compiling linewright v0.1.0\n\
             error[E0425]: cannot find value `x` in this scope\n\
             ##[error]cannot find value `x` in this scope\n\
             error: could not compile `linewright`\n\
             ##[error]Process completed with exit code 101."
        );
    }

    #[test]
    fn a_long_step_shows_only_its_last_50_lines() {
        let output: Vec<String> = (1..=60)
            .map(|n| format!("test case {n} ... FAILED"))
            .collect();
        let mut lines = vec!["##[group]Run cargo test", "##[endgroup]"];
        lines.extend(output.iter().map(String::as_str));
        lines.push("##[error]Process completed with exit code 101.");
        lines.push("Post job cleanup.");

        let shown = failed_step_output(&job_log(&lines));

        let shown: Vec<&str> = shown.lines().collect();
        assert_eq!(shown.len(), 50);
        assert_eq!(shown[0], "test case 12 ... FAILED");
        assert_eq!(shown[49], "##[error]Process completed with exit code 101.");
    }

    #[test]
    fn without_an_error_the_end_of_the_whole_log_stands_in() {
        let output: Vec<String> = (1..=60).map(|n| format!("line {n}")).collect();
        let mut lines = vec!["##[group]Run ./build.sh", "##[endgroup]"];
        lines.extend(output.iter().map(String::as_str));
        lines.push("Post job cleanup.");
        lines.push("Cleaning up orphan processes");

        let shown = failed_step_output(&job_log(&lines));

        let shown: Vec<&str> = shown.lines().collect();
        assert_eq!(shown.len(), 50);
        assert_eq!(shown[0], "line 13");
        assert_eq!(shown[49], "Cleaning up orphan processes");
    }

    #[test]
    fn an_empty_log_shows_nothing() {
        assert_eq!(failed_step_output(""), "");
    }

    #[test]
    fn a_failing_composite_action_shows_its_failing_sub_step() {
        let log = job_log(&[
            "##[group]Run actions/checkout@v5",
            "##[endgroup]",
            "##[group]Run ./.github/actions/build",
            "with:",
            "  target: x86_64",
            "##[endgroup]",
            "##[group]Run rustup toolchain install stable",
            "##[endgroup]",
            "info: component 'rustc' is up to date",
            "##[group]Run cargo build --release",
            "##[endgroup]",
            "error: linking with `cc` failed: exit status: 1",
            "##[error]Process completed with exit code 101.",
            "Post job cleanup.",
        ]);

        assert_eq!(
            failed_step_output(&log),
            "##[group]Run cargo build --release\n\
             ##[endgroup]\n\
             error: linking with `cc` failed: exit status: 1\n\
             ##[error]Process completed with exit code 101."
        );
    }

    #[test]
    fn the_logs_byte_order_mark_goes_with_its_first_timestamp() {
        let log = format!(
            "\u{feff}{}",
            job_log(&["Job is waiting for a runner", "Job was cancelled"])
        );

        assert_eq!(
            failed_step_output(&log),
            "Job is waiting for a runner\nJob was cancelled"
        );
    }
}
