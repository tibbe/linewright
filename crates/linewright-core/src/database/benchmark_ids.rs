//! The migration that gives each benchmark and each task an ID, and keys
//! the rows and the files of benchmarks, tasks and benchmark runs by IDs
//! (`docs/specs/benchmarks.md`, Names and IDs). Its names are those of the
//! releases on either side of it, whatever later releases call them.

use super::Migration;

/// The table of the names of the entries of `tasks/` in the work directory,
/// which [`TASK_DIRS_ENTRIES`] fills and [`TABLES`] drops.
pub(super) const TASK_DIRS: &str = "CREATE TABLE task_dirs (name TEXT NOT NULL) STRICT";

pub(super) const TASK_DIRS_ENTRIES: Migration = Migration::Entries {
    table: "task_dirs",
    dir: "tasks",
};

/// The tables keyed by IDs. Benchmarks and tasks get their IDs in the order
/// they were made, from the counters of `ids`, which give the next. A task
/// that a benchmark run names, under the benchmark run's benchmark, and that
/// no longer is, was removed: it gets an ID after those that are, and is kept
/// with its name in `removed_tasks`. No task gets an ID that names a
/// directory in `tasks/` of no task, such as of a task captured into no
/// benchmark, so that no task moves onto it. Each table that changes is made
/// again.
/// <https://www.sqlite.org/lang_altertable.html#otheralter>
pub(super) const TABLES: &str = "CREATE TABLE ids (
        of TEXT PRIMARY KEY CHECK (of IN ('benchmarks', 'tasks')),
        last INTEGER NOT NULL CHECK (last >= 0)
    ) STRICT;
    CREATE TABLE benchmarks_by_id (
        id INTEGER PRIMARY KEY CHECK (id > 0),
        name TEXT NOT NULL UNIQUE,
        stage TEXT NOT NULL CHECK (stage IN ('triage', 'design', 'implement'))
    ) STRICT;
    INSERT INTO benchmarks_by_id (id, name, stage)
        SELECT row_number() OVER (ORDER BY rowid), name, stage FROM benchmarks;
    CREATE TABLE task_order (
        place INTEGER PRIMARY KEY,
        benchmark TEXT NOT NULL,
        name TEXT NOT NULL,
        UNIQUE (benchmark, name)
    ) STRICT;
    INSERT INTO task_order (benchmark, name) SELECT benchmark, name FROM tasks ORDER BY rowid;
    INSERT INTO task_order (benchmark, name)
        SELECT benchmark_runs.benchmark, benchmark_run_tasks.name
        FROM benchmark_run_tasks
        JOIN benchmark_runs ON benchmark_runs.id = benchmark_run_tasks.benchmark_run
        WHERE NOT EXISTS (SELECT 1 FROM tasks
            WHERE tasks.benchmark = benchmark_runs.benchmark
                AND tasks.name = benchmark_run_tasks.name)
        GROUP BY benchmark_runs.benchmark, benchmark_run_tasks.name
        ORDER BY MIN(benchmark_run_tasks.benchmark_run), benchmark_run_tasks.name;
    CREATE TABLE task_ids (
        id INTEGER PRIMARY KEY,
        benchmark TEXT NOT NULL,
        name TEXT NOT NULL,
        UNIQUE (benchmark, name)
    ) STRICT;
    INSERT INTO task_ids (id, benchmark, name)
        WITH RECURSIVE
            taken (id) AS (
                SELECT CAST(name AS INTEGER) FROM task_dirs
                WHERE CAST(CAST(name AS INTEGER) AS TEXT) = name
                    AND name NOT IN (SELECT name FROM tasks)),
            numbers (id) AS (
                SELECT 1
                UNION ALL
                SELECT id + 1 FROM numbers
                WHERE id < (SELECT COUNT(*) FROM task_order) + (SELECT COUNT(*) FROM taken)),
            free (id, place) AS (
                SELECT id, row_number() OVER (ORDER BY id) FROM numbers
                WHERE id NOT IN (SELECT id FROM taken))
        SELECT free.id, task_order.benchmark, task_order.name
        FROM task_order JOIN free ON free.place = task_order.place;
    CREATE TABLE tasks_by_id (
        id INTEGER PRIMARY KEY CHECK (id > 0),
        benchmark INTEGER NOT NULL REFERENCES benchmarks (id),
        name TEXT NOT NULL,
        run TEXT NOT NULL,
        seq INTEGER NOT NULL CHECK (seq > 0),
        UNIQUE (benchmark, seq),
        UNIQUE (benchmark, name)
    ) STRICT;
    INSERT INTO tasks_by_id (id, benchmark, name, run, seq)
        SELECT task_ids.id, benchmarks_by_id.id, tasks.name, tasks.run, tasks.seq
        FROM tasks
        JOIN task_ids ON task_ids.benchmark = tasks.benchmark AND task_ids.name = tasks.name
        JOIN benchmarks_by_id ON benchmarks_by_id.name = tasks.benchmark;
    CREATE TABLE removed_tasks (
        id INTEGER PRIMARY KEY CHECK (id > 0),
        benchmark INTEGER NOT NULL REFERENCES benchmarks (id),
        name TEXT NOT NULL
    ) STRICT;
    INSERT INTO removed_tasks (id, benchmark, name)
        SELECT task_ids.id, benchmarks_by_id.id, task_ids.name
        FROM task_ids
        JOIN benchmarks_by_id ON benchmarks_by_id.name = task_ids.benchmark
        WHERE task_ids.id NOT IN (SELECT id FROM tasks_by_id);
    CREATE TABLE benchmark_runs_by_id (
        id INTEGER PRIMARY KEY CHECK (id > 0),
        benchmark INTEGER NOT NULL REFERENCES benchmarks (id),
        launched_at TEXT NOT NULL,
        repetitions INTEGER NOT NULL CHECK (repetitions > 0),
        state TEXT NOT NULL CHECK (state IN ('open', 'ended', 'cancelled')),
        ended_at TEXT CHECK (state <> 'open' OR ended_at IS NULL)
    ) STRICT;
    INSERT INTO benchmark_runs_by_id (id, benchmark, launched_at, repetitions, state, ended_at)
        SELECT benchmark_runs.id, benchmarks_by_id.id, launched_at, repetitions, state, ended_at
        FROM benchmark_runs
        JOIN benchmarks_by_id ON benchmarks_by_id.name = benchmark_runs.benchmark;
    CREATE TABLE benchmark_run_tasks_by_id (
        benchmark_run INTEGER NOT NULL REFERENCES benchmark_runs (id),
        seq INTEGER NOT NULL CHECK (seq >= 0),
        task INTEGER NOT NULL CHECK (task > 0),
        PRIMARY KEY (benchmark_run, seq),
        UNIQUE (benchmark_run, task)
    ) STRICT;
    INSERT INTO benchmark_run_tasks_by_id (benchmark_run, seq, task)
        SELECT benchmark_run_tasks.benchmark_run, benchmark_run_tasks.seq, task_ids.id
        FROM benchmark_run_tasks
        JOIN benchmark_runs ON benchmark_runs.id = benchmark_run_tasks.benchmark_run
        JOIN task_ids ON task_ids.benchmark = benchmark_runs.benchmark
            AND task_ids.name = benchmark_run_tasks.name;
    INSERT INTO ids (of, last) VALUES
        ('benchmarks', (SELECT COALESCE(MAX(id), 0) FROM benchmarks_by_id)),
        ('tasks', (SELECT COALESCE(MAX(id), 0) FROM task_ids));
    DROP TABLE benchmark_run_tasks;
    DROP TABLE benchmark_runs;
    DROP TABLE tasks;
    DROP TABLE benchmarks;
    DROP TABLE task_ids;
    DROP TABLE task_order;
    DROP TABLE task_dirs;
    ALTER TABLE benchmarks_by_id RENAME TO benchmarks;
    ALTER TABLE tasks_by_id RENAME TO tasks;
    ALTER TABLE benchmark_runs_by_id RENAME TO benchmark_runs;
    ALTER TABLE benchmark_run_tasks_by_id RENAME TO benchmark_run_tasks;
    CREATE INDEX benchmark_runs_of_benchmarks ON benchmark_runs (benchmark);";

/// The moves of the files that were named for names to their IDs: each
/// task's directory, and in each benchmark run, its copy of each task and of
/// each configuration, and the outputs of each task under each
/// configuration. A configuration's ID is its place, from 1. The names are
/// those the tables held before [`TABLES`] and still hold.
pub(super) const MOVES: &str = "SELECT 'tasks/' || name, 'tasks/' || id FROM tasks
    UNION ALL
    SELECT 'benchmark-runs/' || benchmark_run_tasks.benchmark_run || '/.kept/tasks/'
            || COALESCE(tasks.name, removed_tasks.name),
        'benchmark-runs/' || benchmark_run_tasks.benchmark_run || '/.kept/tasks/'
            || benchmark_run_tasks.task
    FROM benchmark_run_tasks
    LEFT JOIN tasks ON tasks.id = benchmark_run_tasks.task
    LEFT JOIN removed_tasks ON removed_tasks.id = benchmark_run_tasks.task
    UNION ALL
    SELECT 'benchmark-runs/' || benchmark_run || '/.kept/configurations/' || name,
        'benchmark-runs/' || benchmark_run || '/.kept/configurations/' || (seq + 1)
    FROM benchmark_run_configurations
    UNION ALL
    SELECT 'benchmark-runs/' || configurations.benchmark_run || '/' || configurations.name || '/'
            || COALESCE(tasks.name, removed_tasks.name),
        'benchmark-runs/' || configurations.benchmark_run || '/' || (configurations.seq + 1) || '/'
            || benchmark_run_tasks.task
    FROM benchmark_run_configurations AS configurations
    JOIN benchmark_run_tasks ON benchmark_run_tasks.benchmark_run = configurations.benchmark_run
    LEFT JOIN tasks ON tasks.id = benchmark_run_tasks.task
    LEFT JOIN removed_tasks ON removed_tasks.id = benchmark_run_tasks.task
    ORDER BY 1, 2";
