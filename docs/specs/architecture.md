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
  git, the harness and the clock sit behind interfaces with a real and a fake
  implementation.

## State

State that people read or change (labels, assignees, pull requests, comments)
lives in the store and nowhere else.

- **The store is an interface; GitHub is the production store.** A local store
  of plain files serves replay and tests.
- **Reconciling.** Each poll takes a snapshot of the store and reconciles it
  with the runs in flight. Between polls the snapshot is kept, and a webhook
  delivery reads the items it names into it again and reconciles.
- **Machine state is in the store**: derived from its facts, what each item
  is now and how it got there, where they show it, and written as a
  `linewright:` label where they don't, such as that an item has been
  designed. Linewright may keep bookkeeping no person sets under the work
  directory.
- **Webhook deliveries are hints, not state.** A delivery says which item to
  read again; the payload never stands in for the store.
- **Spec changes are found by lookback** over the pull requests merged
  within the window ([Sources of work](sources_of_work.md)).

## Agent sessions

The harness is Claude Code. Each stage's agent works in one headless session,
and what it does depends only on its brief, the repository and what it reads
during the session.

- **One session per stage**, each in its own clone, without permission prompts,
  behind one harness interface. The model is configuration.
- **Isolation.** Sessions running at once cannot disturb each other, and a
  session never depends on what an earlier session left behind or on the host's
  user-level setup. Isolation is of what sessions can observe of each other, not
  of where they run.
- **Briefs are files read at startup**: the skills ([Skills](#skills)) and
  the prompts Linewright sends a session, each a template holding its task
  and result contract.
- **A session's skills and tools.** A session or sub-agent has Linewright's
  skills and the repository's own, and only the tools its work needs.
- **No person takes part in a session.** A decision an original skill puts to a
  person is made by an agent or handed back as an outcome, and judging an
  agent's own work, as review does, falls to agents in fresh contexts.
- **Every session's transcript is kept**: the harness's own record of the
  session, copied beside the session's brief and result under the work
  directory.

## Skills

A stage's skill is a fork of a published skill, kept as upstream's text but
for what Linewright's use of it requires; the rules that are Linewright's own
are the prompt's.

| skill | forked from |
|---|---|
| `triage`, `to-tickets`, `implement`, `code-review`, `pr`, `tdd` | `mattpocock/skills`, MIT, with its notice |
| `software-design` | `prior-art` in `tibbe/skills`, with what concerns products removed |

- **Invoking.** A session's prompt holds its task and the result contract
  as the steps the session takes, and the step a skill covers has the session
  call the skill; the prompts of rework, CI fixes and updates are their whole
  briefs. The skills a skill
  calls (`tdd`, `code-review`, `pr`) are given to the session as skills under
  a `linewright:` namespace and invoked as the originals invoke them; the
  design prompt has the session call `software-design`, then `to-tickets`.

## The manager–agent protocol

The brief and the result are the protocol between the manager and an agent, and
only the manager writes to the store.

1. The manager puts what the session needs from the store (issue, comments, spec
   diff, PR comments) into its brief.
2. The session works in its own clone under the work directory and commits
   there.
3. The session's final message is JSON: the outcome, tickets, comments, the PR's
   title and body.
4. The manager accepts the result and applies it: it pushes the branch and
   opens the PR through the store, and posts prose for people as comments.

- **4a. The manager can't accept the result.** It is sent back to the session
  with the reason, a bounded number of times.

An agent may read GitHub, the web or anything else its session can.

## Capabilities and credentials

Agents get the capabilities their work needs, and are limited where that costs
their work nothing.

- **What an agent has.** A shell, the web and GitHub, which it is trusted to
  use as its brief says.
- **What an agent lacks.** What its work does not need, such as Linewright's
  other credentials or a token that writes to GitHub.
- **What code enforces.** A rule is enforced in code only where it limits what a
  session is given, or where Linewright would malfunction without it. Anything
  else, such as an agent writing to GitHub outside its result, is the brief's to
  prevent.
- **Secrets are redacted.** What Linewright keeps, logs or posts has secrets
  replaced by `[redacted]`, except the commits it pushes: a commit holding a
  secret is refused ([Runs](runs.md)). Redaction is best effort. A secret of 1
  to 5 characters is neither redacted nor refused, which Linewright warns of at
  startup, naming its variable; an empty one is ignored.
- **A secret** is the value of a variable Linewright reads a credential from,
  or of one `secrets` names ([Configuration](operation.md#configuration)).
- **Identity is a user account**: a token for a GitHub user dedicated to
  Linewright, whose assignments are its claims.

## Automations

Automations sit beside the pipeline.

- **The pipeline's entry conditions are fixed** by the product spec.
- **An automation run is one-shot**, and reaches the pipeline only through the
  issues it files, which the pipeline works without knowing where they came
  from.

## The local API

`serve` answers questions about itself through a local API, its view of the
repository, its runs and their transcripts, and takes the operator's requests
of it, such as a drain.

- **Clients.** The monitoring commands and `drain` are its clients. The API
  must also be able to serve an MCP server, a `lw mcp` subcommand on stdio
  that is a client of the socket.

| aspect | decision |
|---|---|
| Transport | HTTP on a Unix socket |
| Paths | the API under `/api/` |
| Bodies | JSON; a monitoring command's `--json` prints the response as it is |
| A transcript being followed | server-sent events, which a reader resumes by event id |
| Access | file permissions: only `serve`'s user can open the socket |
| The socket's path | derived from the repository, in a short per-user directory such as `$XDG_RUNTIME_DIR` |
| Requests answered | only those whose `Host` is local: `localhost` or a loopback address, ignoring case, port and a trailing dot |

## The web UI

`serve` serves the web UI itself, on a port of its own, as HTML rendered on
the server.

- **Access**: whoever reaches the port; who reaches it is the operator's to
  restrict, such as with an authenticating proxy in front.
- **Rendered on the server**: askama templates, with htmx; a live update is a
  server-sent event carrying an HTML fragment, one event stream per page.
- **Styled with daisyUI**, on Tailwind.
- **The stylesheet is committed**, generated by Tailwind's standalone CLI;
  the checks regenerate it, and CI fails when it differs.
- **Assets are in the binary** (`rust-embed`).
- **Tested in-process**, driving the router under `cargo test`.

## Deployment and configuration

Linewright runs on a host or in a container the operator builds, and is
configured in layers.

- **The base image.** A published base image (`lw`, the harness, and the tools
  coding agents expect: `git`, `gh`, `python3`) is the starting point for
  the operator's image.
- **Configuration is layered**: flags, `LINEWRIGHT_*` environment variables, a
  TOML file, defaults, parsed once into one typed value.
- **Credentials come from the environment only.**
- **SIGQUIT drains and SIGTERM stops.**
