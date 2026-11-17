# Architecture

Linewright is a controller, in the Kubernetes sense: the state of the work
lives in GitHub, and Linewright repeatedly compares it with what it has in
flight and acts to close the gap. The acting is done by agents, which take
their task in a brief and hand their result back to Linewright to apply.

## Language and structure

Linewright is one Rust program whose decisions are a pure function of what it
observes.

- **Rust, with GitHub's REST and GraphQL APIs in-process.**
- **Functional core, imperative shell.** The reconcile step is a pure function
  from a snapshot, the live runs and the clock to a list of actions. The store,
  the harness and the clock sit behind interfaces with a real and a fake
  implementation.

## State

State that people read or change (labels, assignees, pull requests, comments)
lives in the store and nowhere else.

- **The store is an interface; GitHub is the production store.**
- **Reconciling.** Each poll takes a snapshot of the store and reconciles it
  with the runs in flight. Between polls the snapshot is kept, and a webhook
  delivery reads the items it names, and those Linewright wrote to, into it
  again and reconciles.
- **Machine state is in the store**: an item's state is derived from it,
  and from Linewright's run while one is in flight on the item
  ([State](repository.md#state)).
- **Identity is a user account**: a token for a GitHub user dedicated to
  Linewright
  ([Requirements](repository.md#requirements)).

## Storage

Linewright keeps its own records in one SQLite database in the work
directory, through SQLx. A stream, or what a session or a person uses by its
path, is a file.

| Data                                                                                             | Kept as                                                                       |
| ------------------------------------------------------------------------------------------------ | ----------------------------------------------------------------------------- |
| each run outside a replay, with the input it recorded ([Recording a run's input](benchmarks.md#recording-a-runs-input)) | rows in the database                                                   |
| bookkeeping that no person sets, such as Linewright's pull requests and each automation's last scheduled run | rows in the database                                                  |
| the environment's commit, and the time of its build                                          | rows in the database                                                          |
| suites and experiments ([Benchmarks](benchmarks.md))                                             | rows in the database                                                          |
| a session's brief, follow-ups and transcript, and its maintenance command's output               | files of the session, with the transcript as one JSON Lines file that only grows |
| a session's attachments                                                                          | its attachments directory, each file by its own name ([Attachments](runs.md#attachments)) |
| the output of the environment's builds                                                           | files                                                                         |
| the clones and the environment                                                                   | directories                                                                   |
| cases and the outputs of replays                                                                 | directories of plain files ([Benchmarks](benchmarks.md))                      |

- **Many processes.** Each Linewright process on the host opens the database
  in WAL mode, so that it reads while another process writes.
- **The schema.** When a release first opens the database, it changes the
  database to the schema of the release, in one transaction.
- **While another process has it open.** A release changes the schema only
  while no other Linewright process has the database open. Otherwise it exits
  with an error that says so.
- **A newer schema.** A release refuses a database whose schema a newer
  release made. The error names that release, and says to restore a backup
  from before it.

## Agent sessions

The harness is Claude Code. Each run's agent works in headless sessions, and
what it does depends only on its brief, the repository and what it reads
during the session.

- **Each session is in its own clone** ([Stages](runs.md#stages)), behind
  one harness interface.
- **Sessions are processes beside Linewright**
  ([Isolation](sessions.md#isolation)).
- **In a replay, `gh` sends its requests to Linewright.** The configuration
  that Linewright gives a session's `gh` sets `http_unix_socket` to a Unix
  socket of Linewright
  ([The repository on GitHub](benchmarks.md#the-repository-on-github)).

## The protocol between Linewright and an agent

The brief and the result are the protocol between Linewright and an agent, and
only Linewright writes to the store.

1. Linewright puts what the session needs from the store into its brief.
2. The session works in its own clone under the work directory and commits
   there, and puts its attachments in its attachments directory
   ([Attachments](runs.md#attachments)).
3. The session's final message is JSON ([Contract](runs.md#contract)).
4. Linewright accepts the result and applies it through the store
   ([State](#state)).

## Enforcement

A rule is enforced in code only where it limits what a session is given, or
where Linewright would malfunction without it.

## Automations

Automations sit beside the pipeline.

- **An automation run is one-shot**, and reaches the pipeline only through the
  issues it files, which the pipeline works without knowing where they came
  from.

## The local API

`serve` answers questions about itself through a local API, its view of the
repository, its runs and their transcripts, and takes the operator's requests
of it, such as a drain.

- **Clients.** While `serve` runs, the monitoring commands, `drain` and
  `environment rebuild` ask it through the API
  ([Commands](monitoring.md#commands)).

| aspect | decision |
|---|---|
| Transport | HTTP on a Unix socket |
| Paths | the API under `/api/` |
| Bodies | JSON; a monitoring command's `--json` prints the response as it is |
| A transcript being followed | server-sent events, which a reader resumes by event id |

## The web UI

`serve` serves the web UI itself, on a port of its own, as HTML rendered on
the server.

- **Rendered on the server**: askama templates, with htmx; a live update is a
  server-sent event carrying an HTML fragment, one event stream per page.
- **Styled with daisyUI**, on Tailwind.
- **The stylesheet is committed**, generated by Tailwind's standalone CLI;
  the checks regenerate it, and CI fails when it differs.
- **Assets are in the binary** (`rust-embed`).

## Configuration

Configuration is layered ([Configuration](operation.md#configuration)). The
operator's layers are parsed once into one typed value.
