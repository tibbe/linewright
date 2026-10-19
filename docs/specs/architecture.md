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
- **Machine state is derived** from the store's facts, what each item is now
  and how it got there, never written as labels. Linewright may keep
  bookkeeping no person sets under the work directory.
- **Webhook deliveries are hints, not state.** A delivery says which item to
  read again; the payload never stands in for the store.
- **Why.** GitHub neither orders deliveries nor redelivers a failed one, so
  state built from payloads would drift until a poll repaired it; reading the
  item again keeps every reconcile on the store's facts, as Kubernetes
  controllers, Argo CD and `rust-lang/bors` do. The cost is one read per changed
  item, against a full snapshot per poll.
- **Spec changes are found by lookback**: pull requests merged within the
  window, minus those labeled processed.

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
- **Briefs are files read at startup**: the skills, forked from
  `mattpocock/skills` (`triage`, `to-tickets`, `implement`, `code-review`, `pr`,
  `tdd`) with their MIT notice, and the prompts Linewright sends a session, each
  a template holding its task and result contract.
- **Invoking skills.** A session whose work a skill covers invokes it, as a
  person would invoke the original, with the task and the result contract; an
  update's prompt is its whole brief. The skills a skill calls (`tdd`,
  `code-review`, `pr`) are given to the session as skills under a `linewright:`
  namespace and invoked as the originals invoke them.
- **A session's skills and tools.** A session or sub-agent has Linewright's
  skills and the repository's own, and only the tools its work needs.
- **Why.** The harness's other skills and tools take up the session's context
  and compete with Linewright's skills.
- **No person takes part in a session.** A decision an original skill puts to a
  person is made by an agent or handed back as an outcome, and judging an
  agent's own work, as review does, falls to agents in fresh contexts.
- **Every session's transcript is kept**: the harness's own record of the
  session, copied beside the session's brief and result under the work
  directory.

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

The protocol is how the pipeline talks to an agent, not a limit on what the
agent reaches: an agent may read GitHub, the web or anything else its session
can.

## Capabilities and credentials

Agents get the capabilities their work needs, and are limited where that costs
their work nothing.

- **What an agent has.** An agent that debugs, researches and reviews needs a
  shell, the web and GitHub, so it has them, and is trusted to use them as its
  brief says.
- **What an agent lacks.** What its work does not need, such as Linewright's
  other credentials or a token that writes to GitHub.
- **What code enforces.** A rule is enforced in code only where it limits what a
  session is given, or where Linewright would malfunction without it. Anything
  else, such as an agent writing to GitHub outside its result, is the brief's to
  prevent, and a maintainer who wants stricter guards sets them in GitHub, such
  as required reviews on spec files.
- **Credentials are redacted.** What Linewright keeps, logs or posts has
  credentials replaced by `[redacted]`, except the commits it pushes: those are
  the agent's work, so a commit holding a credential is refused rather than
  rewritten ([Runs](runs.md)). Redaction is best effort.
- **A credential** is the value of a variable in Linewright's environment whose
  name ends in `_TOKEN`, `_KEY`, `_SECRET` or `_PASSWORD`.
- **Why.** Names rather than a declared list, so that the secrets an operator
  passes sessions are covered without a setting.
- **Identity is a user account**: a token for a GitHub user dedicated to
  Linewright, whose assignments are its claims.

## Workflows

Workflows sit beside the pipeline, not under one trigger model.

- **The pipeline's entry conditions are fixed** by the product spec.
- **A workflow run is one-shot**, and reaches the pipeline only through the
  issues it files, which the pipeline works without knowing where they came
  from.
- **Why.** The pipeline is Linewright's responsibility, so making its entry
  conditions configurable triggers would turn Linewright's own behavior into
  configuration while buying nothing the spec asks for. Prow keeps its core
  behavior in code beside configured jobs for the same reason; Zuul's single
  trigger model pays off for a CI system that routes many projects' events into
  pipelines, which Linewright does not do.

## The local API

`serve` answers questions about itself through a local API: its view of the
repository, its runs and their transcripts.

- **Clients.** The monitoring commands are its clients. The API must also be
  able to serve a web UI, reached from a browser through `ssh -L` to the socket,
  and an MCP server, a `lw mcp` subcommand on stdio that is a client of the
  socket.

| aspect | decision |
|---|---|
| Transport | HTTP on a Unix socket |
| Bodies | JSON; a monitoring command's `--json` prints the response as it is |
| A transcript being followed | server-sent events, which a reader resumes by event id |
| Access | file permissions: only `serve`'s user can open the socket |
| The socket's path | derived from the repository, in a short per-user directory such as `$XDG_RUNTIME_DIR` |
| Requests answered | only those whose `Host` is local |

- **Why.** A Unix socket rather than a loopback TCP port, so that file
  permissions are the access control and two instances cannot collide on a
  port.

## Deployment and configuration

Linewright runs on a host or in a container the operator builds, and is
configured in layers.

- **The base image.** A published base image (`lw`, the harness, and the tools
  coding agents expect: `git`, `gh`, `python3`) is the starting point for
  the operator's image.
- **Configuration is layered**: flags, `LINEWRIGHT_*` environment variables, a
  TOML file, defaults, parsed once into one typed value.
- **Credentials come from the environment only.**
