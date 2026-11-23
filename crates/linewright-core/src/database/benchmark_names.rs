//! The migration that keeps the suites, cases and experiments of Linewright
//! 0.24.0 as benchmarks, tasks and benchmark runs (`docs/specs/benchmarks.md`),
//! in the database and in the work directory. Its names are those of the
//! releases on either side of it, whatever later releases call them.

use std::path::Path;

use anyhow::Context as _;

/// The tables, columns, indexes and trial states under their new names, and
/// the environments of trials keyed under `trials/`. `trials` is made again,
/// as its CHECK constraints hold the values that change.
/// <https://www.sqlite.org/lang_altertable.html#otheralter>
pub(super) const TABLES: &str = "ALTER TABLE suites RENAME TO benchmarks;
    ALTER TABLE cases RENAME TO tasks;
    ALTER TABLE tasks RENAME COLUMN suite TO benchmark;
    DROP INDEX experiments_of_suites;
    ALTER TABLE experiments RENAME TO benchmark_runs;
    ALTER TABLE benchmark_runs RENAME COLUMN suite TO benchmark;
    CREATE INDEX benchmark_runs_of_benchmarks ON benchmark_runs (benchmark);
    ALTER TABLE experiment_cases RENAME TO benchmark_run_tasks;
    ALTER TABLE benchmark_run_tasks RENAME COLUMN experiment TO benchmark_run;
    ALTER TABLE experiment_configurations RENAME TO benchmark_run_configurations;
    ALTER TABLE benchmark_run_configurations RENAME COLUMN experiment TO benchmark_run;
    CREATE TABLE trials_renamed (
        benchmark_run INTEGER NOT NULL REFERENCES benchmark_runs (id),
        repetition INTEGER NOT NULL CHECK (repetition > 0),
        task_seq INTEGER NOT NULL,
        configuration_seq INTEGER NOT NULL,
        state TEXT NOT NULL CHECK (state IN
            ('pending', 'running', 'ran', 'scoring', 'scored', 'unscored')),
        outcome TEXT CHECK (outcome IN ('done', 'handed-back', 'cancelled', 'usage-limit',
            'failed', 'in-flight', 'interrupted')),
        money REAL CHECK (money >= 0),
        seconds REAL CHECK (seconds >= 0),
        correctness TEXT CHECK (correctness IN ('pass', 'fail')),
        leak INTEGER CHECK (leak IN (0, 1)),
        scorer_money REAL CHECK (scorer_money >= 0),
        unscored TEXT CHECK (unscored IN ('outcome', 'scorer', 'cancelled')),
        reason TEXT,
        PRIMARY KEY (benchmark_run, repetition, task_seq, configuration_seq),
        FOREIGN KEY (benchmark_run, task_seq) REFERENCES benchmark_run_tasks (benchmark_run, seq),
        FOREIGN KEY (benchmark_run, configuration_seq)
            REFERENCES benchmark_run_configurations (benchmark_run, seq),
        CHECK ((outcome IS NULL) = (seconds IS NULL)),
        CHECK (money IS NULL OR outcome IS NOT NULL),
        CHECK (state NOT IN ('pending', 'running') OR outcome IS NULL),
        CHECK (state NOT IN ('ran', 'scoring', 'scored') OR outcome IS NOT NULL),
        CHECK ((correctness IS NOT NULL) = (state = 'scored')),
        CHECK ((leak IS NOT NULL) = (state = 'scored')),
        CHECK (scorer_money IS NULL OR state = 'scored' OR unscored IS 'scorer'),
        CHECK ((unscored IS NOT NULL) = (state = 'unscored')),
        CHECK (unscored IS NOT 'outcome' AND unscored IS NOT 'scorer' OR outcome IS NOT NULL),
        CHECK ((reason IS NOT NULL) = (unscored IS 'scorer'))
    ) STRICT;
    INSERT INTO trials_renamed (benchmark_run, repetition, task_seq, configuration_seq, state,
            outcome, money, seconds, correctness, leak, scorer_money, unscored, reason)
        SELECT experiment, repetition, case_seq, configuration_seq,
            CASE state
                WHEN 'replaying' THEN 'running'
                WHEN 'replayed' THEN 'ran'
                WHEN 'judging' THEN 'scoring'
                WHEN 'judged' THEN 'scored'
                WHEN 'unjudged' THEN 'unscored'
                ELSE state
            END,
            outcome, money, seconds, correctness, leak, judge_money,
            CASE unjudged WHEN 'judge' THEN 'scorer' ELSE unjudged END,
            reason
        FROM trials;
    DROP TABLE trials;
    ALTER TABLE trials_renamed RENAME TO trials;
    UPDATE environments
        SET dir = 'trials/environments/' || substr(dir, length('replays/environments/') + 1)
        WHERE substr(dir, 1, length('replays/environments/')) = 'replays/environments/';";

/// Moves what the work directory `work_dir` keeps under the names of
/// Linewright 0.24.0 to the new names: the tasks from `cases/` to `tasks/`,
/// the benchmark runs from `experiments/` to `benchmark-runs/`, each
/// output's `case` to `task` and `judgement.json` to `score.json`, and the
/// trials' directory from `replays/` to `trials/`. Each move is made only
/// where its source is, so a later run finishes what an earlier one left. A
/// scorer's record stays in `judge/`, where its score names it; an output
/// with no score has its unkept record moved to `scorer/`, where the scorer
/// that runs again removes it.
///
/// # Errors
///
/// When a directory cannot be read, a move fails, or a source and its
/// target both exist, which leaves the source for a person to settle.
pub(super) fn move_files(work_dir: &Path) -> anyhow::Result<()> {
    move_tasks(work_dir)?;
    let benchmark_runs = work_dir.join("benchmark-runs");
    move_if_present(&work_dir.join("experiments"), &benchmark_runs)?;
    for benchmark_run in subdirectories(&benchmark_runs)? {
        move_tasks(&benchmark_run.join(".kept"))?;
        for configuration in visible_subdirectories(&benchmark_run)? {
            for task in visible_subdirectories(&configuration)? {
                for output in visible_subdirectories(&task)? {
                    move_output(&output)?;
                }
            }
        }
    }
    move_if_present(&work_dir.join("replays"), &work_dir.join("trials"))
}

/// Moves the tasks in `dir` from `cases/` to `tasks/`, and each task's
/// `case.json` to `task.json`.
fn move_tasks(dir: &Path) -> anyhow::Result<()> {
    let tasks = dir.join("tasks");
    move_if_present(&dir.join("cases"), &tasks)?;
    for task in subdirectories(&tasks)? {
        move_if_present(&task.join("case.json"), &task.join("task.json"))?;
    }
    Ok(())
}

/// Moves the parts of the trial's output `output` to their new names.
fn move_output(output: &Path) -> anyhow::Result<()> {
    move_if_present(&output.join("case"), &output.join("task"))?;
    let score = output.join("score.json");
    move_if_present(&output.join("judgement.json"), &score)?;
    if !exists(&score)? {
        move_if_present(&output.join("judge"), &output.join("scorer"))?;
    }
    Ok(())
}

/// Moves `from` to `to` when `from` exists.
///
/// # Errors
///
/// When `from` and `to` both exist, or the move fails.
fn move_if_present(from: &Path, to: &Path) -> anyhow::Result<()> {
    if !exists(from)? {
        return Ok(());
    }
    anyhow::ensure!(
        !exists(to)?,
        "both {} and {} exist: remove the one not to keep",
        from.display(),
        to.display()
    );
    std::fs::rename(from, to)
        .with_context(|| format!("moving {} to {}", from.display(), to.display()))
}

/// Whether there is anything at `path`, a dangling symbolic link included.
fn exists(path: &Path) -> anyhow::Result<bool> {
    match std::fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e).with_context(|| format!("looking for {}", path.display())),
    }
}

/// The directories in `dir`, none when there is no `dir`.
fn subdirectories(dir: &Path) -> anyhow::Result<Vec<std::path::PathBuf>> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(vec![]),
        Err(e) => return Err(e).with_context(|| format!("reading {}", dir.display())),
    };
    let mut dirs = Vec::new();
    for entry in entries {
        let entry = entry.with_context(|| format!("reading {}", dir.display()))?;
        let file_type = entry
            .file_type()
            .with_context(|| format!("reading {}", entry.path().display()))?;
        if file_type.is_dir() {
            dirs.push(entry.path());
        }
    }
    Ok(dirs)
}

/// The directories in `dir` whose names do not start with a dot.
fn visible_subdirectories(dir: &Path) -> anyhow::Result<Vec<std::path::PathBuf>> {
    let mut dirs = subdirectories(dir)?;
    dirs.retain(|sub| {
        sub.file_name()
            .is_some_and(|name| !name.as_encoded_bytes().starts_with(b"."))
    });
    Ok(dirs)
}
