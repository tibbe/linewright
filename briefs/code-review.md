<!-- Forked from mattpocock/skills, skills/engineering/code-review (MIT, see
LICENSE). Made non-interactive for Linewright. -->

# Code Review

A two-axis review of your work: the diff between `HEAD` and the merge-base
with the default branch (the fixed point your task names).

- **Standards**: does the code conform to this repository's documented coding
  standards?
- **Spec**: does the code faithfully implement what the ticket and the spec
  ask for?

Both axes run as **parallel sub-agents** in fresh contexts, so they don't
pollute each other's context or yours. You then address their findings
yourself: there is nobody to hand the report to.

## Process

### 1. Pin the fixed point

Capture the diff command once: `git diff <fixed-point>...HEAD` (three-dot, so
the comparison is against the merge-base), and the commits with
`git log <fixed-point>..HEAD --oneline`. Confirm the diff is non-empty; commit
first if your work is uncommitted.

### 2. The spec source

The ticket in your task (its body and comments) and the spec files it relates
to. Paste the ticket into the Spec sub-agent's prompt.

### 3. The standards sources

Find every file in the repository that documents how code should be written.
When `CODING_STANDARDS.md` or `CONTRIBUTING.md` exists, it must be on the
list.

On top of whatever the repository documents, the Standards axis always carries
the **smell baseline** below, a fixed set of Fowler code smells (_Refactoring_,
ch. 3). Two rules bind it: **the repository overrides** (a documented standard
always wins; where it endorses something the baseline would flag, suppress the
smell), and each smell is **always a judgement call**, never a hard violation.
Skip anything tooling already enforces.

- **Mysterious Name**: a name that doesn't reveal what it does or holds. →
  rename it; if no honest name comes, the design's murky.
- **Duplicated Code**: the same logic shape in more than one hunk or file. →
  extract the shared shape, call it from both.
- **Feature Envy**: a method that reaches into another object's data more than
  its own. → move it onto the data it envies.
- **Data Clumps**: the same few fields or params travelling together. → bundle
  them into one type.
- **Primitive Obsession**: a primitive standing in for a domain concept. → give
  the concept its own small type.
- **Repeated Switches**: the same `switch`/`if`-cascade on the same type
  recurring. → polymorphism, or one map both sites share.
- **Shotgun Surgery**: one logical change forcing scattered edits. → gather
  what changes together into one module.
- **Divergent Change**: one module edited for several unrelated reasons. →
  split it.
- **Speculative Generality**: abstraction or hooks for needs the spec doesn't
  have. → delete it.
- **Message Chains**: long `a.b().c().d()` navigation. → hide the walk behind
  one method.
- **Middle Man**: a class or function that mostly delegates. → call the real
  target direct.
- **Refused Bequest**: an implementer that ignores most of what it inherits. →
  composition instead.

### 4. Spawn both sub-agents in parallel

Issue both sub-agent calls together. Each prompt is self-contained and tells
the sub-agent to **invoke no skills**: everything it needs is in the prompt.
Each also tells it that it may read GitHub but must not write to it.

**Standards sub-agent prompt:**

- The diff command and commit list.
- The standards files you found, plus the smell baseline above pasted in full.
- The brief: "Report, per file/hunk where relevant, (a) every place the diff
  violates a documented standard: cite the standard (file + the rule); and (b)
  any baseline smell you spot: name it and quote the hunk. Distinguish hard
  violations from judgement calls. Skip anything tooling enforces. Invoke no
  skills. Under 400 words."

**Spec sub-agent prompt:**

- The diff command and commit list.
- The ticket, pasted in full, and the paths of the spec files.
- The brief: "Report: (a) requirements the ticket or spec asked for that are
  missing or partial; (b) behavior in the diff that wasn't asked for (scope
  creep); (c) requirements that look implemented but where the implementation
  looks wrong. Quote the ticket or spec line for each finding. Invoke no
  skills. Under 400 words."

### 5. Address the findings

Keep the two reports apart while you weigh them: a change can pass one axis
and fail the other. Fix hard violations and spec findings; act on judgement
calls where you agree. A finding you decide against needs a reason you could
defend to the maintainer. A finding the spec doesn't settle is not a spec gap
unless it is a product decision: a reasonable maintainer could want either
answer, and the answer changes what a user sees. Otherwise decide it yourself
and fix or dismiss it like any other finding. A finding that is a spec gap is
not a fix.
