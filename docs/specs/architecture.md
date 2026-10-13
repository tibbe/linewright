# Architecture

Linewright's architecturally significant decisions: the ones costly to change
later.

- **The store is an interface; GitHub is the production store.** State that
  people read or change (labels, assignees, pull requests, comments) lives in
  the store and nowhere else. Each poll takes a snapshot of the store and
  reconciles it with the runs in flight; between polls the snapshot is kept,
  and a webhook delivery reads the items it names into it again and
  reconciles. Machine state is derived from the store's facts (assignee,
  linked PR, review and check state), never written as labels. Linewright may
  keep bookkeeping no person reads or sets under the work directory. A local
  store of plain files serves replay and tests.
- **Functional core, imperative shell.** The reconcile step is a pure
  function from a snapshot, the live runs and the clock to a list of actions.
  The store, git, the agent runtime and the clock sit behind interfaces with
  a real and a fake implementation.
- **Agents are headless Claude Code sessions, one per stage**, each in its
  own clone, without permission prompts, behind one runtime interface. The
  model is configuration. Sessions are isolated: those running at once
  cannot disturb each other, and what a session does depends on its brief,
  the repository and what it reads during the session, never on what an
  earlier session left behind or on the host's user-level setup. Isolation
  is of what sessions can observe of each other, not of where they run.
- **The brief and the result are the protocol between the manager and an
  agent.** The manager puts what a session needs from the store (issue,
  comments, spec diff, PR comments) into its brief. The protocol is how the
  pipeline talks to an agent, not a limit on what the agent reaches: an agent
  may read GitHub, the web or anything else its session can. The session works
  in its own clone under the work directory and commits there; its final message
  is JSON the manager parses and applies: the outcome, tickets, comments, the
  PR's title and body. The manager pushes the branch and opens the PR through
  the store. Only the manager writes to the store; prose for people goes to
  comments. A result that does not parse is sent back to the session with the
  error, a bounded number of times.
- **Agents are trusted, and rules are kept by the brief.** Agents and
  maintainers are not adversaries; a rule is enforced in code only where
  Linewright would malfunction without it. Anything else, such as an agent
  writing to GitHub outside its result, is the brief's to prevent, and a
  maintainer who wants stricter guards sets them in GitHub, such as required
  reviews on spec files.
- **Briefs are files read at startup**, forked from `mattpocock/skills`
  (`triage`, `to-tickets`, `implement`, `code-review`, `pr`, `tdd`) with their
  MIT notice. Each stage's session invokes its stage's skill, as a person would
  invoke the original, with the task and the result contract. The skills that
  skill calls (`tdd`, `code-review`, `pr`) are given to the session as skills
  under a `linewright:` namespace and invoked as the originals invoke them, and
  the repository's own skills are available to every session and sub-agent. No
  person takes part in a session: a decision an original puts to a person is
  made by an agent or handed back as an outcome, and judging an agent's own
  work, as review does, falls to agents in fresh contexts.
- **Every session's trace is kept**: the runtime's own transcript, copied
  beside the session's brief and result under the work directory.
- **Rust, with GitHub's REST and GraphQL APIs in-process.**
- **Workflows sit beside the pipeline, not under one trigger model.** The
  pipeline is Linewright's responsibility and its entry conditions are fixed
  by the product spec, so making them configurable triggers would turn
  Linewright's own behavior into configuration while buying nothing the
  spec asks for. A workflow run is one-shot, and reaches the pipeline only
  through the issues it files, which the pipeline works without knowing
  where they came from. Prow keeps its core behavior in code beside
  configured jobs for the same reason; Zuul's single trigger model pays off
  for a CI system that routes many projects' events into pipelines, which
  Linewright does not do.
- **Webhook deliveries are hints, not state.** A delivery says which item to
  read again; the payload never stands in for the store. GitHub neither orders
  deliveries nor redelivers a failed one, so state built from payloads would
  drift until a poll repaired it; reading the item again keeps every reconcile
  on the store's facts, as Kubernetes controllers, Argo CD and rust-lang/bors
  do. The cost is one read per changed item, against a full snapshot per poll.
- **Spec changes are found by lookback**: pull requests merged within the
  window, minus those labeled processed.
- **Identity is a user account**: a token for a GitHub user dedicated to
  Linewright, whose assignments are its claims.
- **Runs on a host or in a container the operator builds.** A published base
  image (`linewright`, `git`, the agent runtime) is the starting point for
  the operator's image.
- **Configuration is layered**: flags, `LINEWRIGHT_*` environment variables,
  a TOML file, defaults, parsed once into one typed value. Credentials come
  from the environment only.
