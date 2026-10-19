# Working with Linewright

Linewright works in the repository beside you, as a GitHub user of its own.
You work with it the way you work with a teammate: through issues, labels,
comments and pull requests. Everything it does shows on GitHub, and you never
need anything but GitHub to steer it.

This guide is for everyone who files, decides or reviews work: engineering,
sales, customer success. The [specs](specs/product.md) say exactly what
Linewright does; this guide says what you do.

## The labels

Labels are how you steer Linewright.

### Where an issue is

An issue carries one of these at a time. It says what the issue needs next.
Applying one is enough: if an issue carries more than one, Linewright keeps
the one applied last and removes the rest.

- `needs-triage`: not yet assessed. Linewright triages it. Apply it to have an
  issue triaged again, such as with your answer.
- `needs-info`: its assessment waits on a fact from the reporter. Linewright
  assigns the people it asks; a comment from anyone but Linewright replies.
- `ready-for-agent`: decided: an agent could build it, so Linewright builds
  it. Apply it to have an issue built as it stands; remove it to stop the
  work.
- `ready-for-human`: needs a person's decision or action. Answer by applying
  `ready-for-agent` or `needs-triage`.

### Other labels

- `bug`, `enhancement`: the category, in repositories without issue types.
  Change it if triage got it wrong.
- The required label (such as `linewright`): Linewright can be configured to
  start work only on issues and pull requests that carry a label of your
  choosing, so you can adopt it a little at a time. Where it is, apply that
  label to let Linewright start on an item. Removing it keeps Linewright from
  starting more on the item; work in flight goes on.
- `linewright:spec-processed`: Linewright's mark on a merged spec change it has
  cut into tickets. Leave it alone.

Don't label an issue `ready-for-agent` before it is clear what to build: an
issue labeled that way is built as it stands, without triage. A new report
gets `needs-triage`, and triage applies `ready-for-agent` once the issue is
clear.

Besides labels, assigning and pull requests say where an issue is. An issue
assigned to Linewright is one it is working on, and an issue with an open
pull request that closes it is in review. Linewright's pull requests carry no
such label: Linewright works one while it is assigned to it.

## Reporting an issue

For a bug a customer hit, or anything else that should change.

1. File the issue with the `needs-triage` label, which asks Linewright to
   triage it. An issue template can apply the label for you.
2. Linewright assigns itself and triages it: it reads the issue, the code and
   the spec, and tries to reproduce a bug. It comments with what it found and
   labels the issue with what happens next. It leaves alone an issue someone
   else is assigned to, one an open issue blocks and one an open pull request
   closes.
3. If it labels the issue `needs-info`, it has asked you something: it
   assigns the people it asks, usually you, names them all in its comment,
   and unassigns itself. Someone GitHub won't let it assign, such as a person
   without access to the repository, is named but not assigned. Reply in the
   thread: a comment from anyone but Linewright puts the issue back to
   `needs-triage`, and Linewright unassigns the people it assigned for the
   question, leaving anyone someone else assigned, and triages the issue
   again, claiming it as before.
4. If it labels the issue `ready-for-agent`, its comment is the brief an agent
   builds from. It stays assigned, and the work starts on its own.
5. If it labels the issue `ready-for-human`, the maintainer decides, and its
   comment is its recommendation. It unassigns itself. You don't need to do
   anything.

To stop a triage, unassign Linewright, remove `needs-triage` or close the
issue. Once unassigned, Linewright leaves the issue until `needs-triage`, or
the required label, is applied again, or until you assign it again.

Issues Linewright files itself, such as an automation's, are triaged the same
way when they carry `needs-triage`.

Linewright never closes an issue you filed. A duplicate, or something that
already works, goes to the maintainer.

## Deciding an issue

For the maintainer, on a `ready-for-human` issue. Linewright's comment says
what it needs: a decision from triage, or why it could not finish a ticket.

1. Answer in the thread, if there is something to say.
2. Apply a label: `ready-for-agent` to have it built as you decided, or
   `needs-triage` to have it triaged again with your answer. Answer first:
   the label starts the run, and the run reads the thread as it is then.

A comment alone starts nothing on a `ready-for-human` issue: the label is your
decision.

To give Linewright an issue directly, without triage, label it
`ready-for-agent`. Linewright assigns itself and builds it. It leaves alone an
issue someone else is assigned to, one an open issue blocks and one an open
pull request closes. To have it build one of those, assign it.

## Assigning Linewright

Assigning Linewright to an issue asks it to work the issue now: whoever else
is assigned, whether or not the issue carries the required label, and also
after Linewright was unassigned from it. What it does depends on the label:

- `needs-triage`: it triages the issue.
- `ready-for-agent`: it builds it.
- Either, while an open issue blocks it or an open pull request closes it:
  it stays assigned and starts once it can. `lw status` shows what it waits
  for.
- `needs-info`, `ready-for-human` or no status label: there is nothing for it
  to do yet. It unassigns itself and comments saying what the issue needs
  first: `ready-for-agent` to have it built, or `needs-triage` to have it
  assessed. Apply one and assign it again.

## Changing the spec

For engineering. The spec in `docs/specs/` is what Linewright builds from, so
changing it is how you build.

1. Open a pull request that changes the spec, and merge it once it says what
   you want.
2. Linewright assigns itself to your merged pull request while it cuts the
   change into tickets, issues small enough for one agent to build well, and
   opens a tracking issue that lists them. Then it labels the pull request
   `linewright:spec-processed` and unassigns itself. The tickets are worked in
   order of what blocks what.
3. Each ticket becomes a pull request. The tracking issue closes when every
   ticket is closed.

Code that already matches the spec produces no tickets, so building something
by hand first is fine. Unassign Linewright from the pull request to stop the
cut. If Linewright cannot cut the change into tickets, it unassigns itself and
says why in a comment. Either way it leaves the change alone until you assign
Linewright to the pull request again, which has it try again, or, where
Linewright needs the required label, apply that label again. Assigning
Linewright to a merged spec change it has not cut has it cut the change, also
one without the required label.

Agents never change the spec. Where a ticket needs a spec change, Linewright
files a `ready-for-human` issue saying what is missing.

## Reviewing Linewright's pull requests

A pull request stays a draft until its CI passes, so you are only asked to
review green ones. Linewright fixes failing CI itself.

- **Ask for a change** by commenting or reviewing, as you would with anyone.
  Linewright makes the change, replies in your thread and asks you to review
  again. It leaves your threads for you to resolve.
- **Merge it** with whichever method you like; Linewright keeps the branch
  mergeable by each one your repository allows. With auto-merge on, Linewright
  merges green pull requests itself, but waits for your approval on any you
  commented on.
- **Reject it** by closing it. Linewright takes that as no, and labels the
  issue `ready-for-human`.
- **Pause it** by unassigning Linewright from the pull request. Linewright
  stops any run on it and leaves it as it is. Assign Linewright again to let
  it carry on: it addresses the comments, reviews, pushes and CI that came in
  meanwhile, with fresh attempts at fixing CI and merging.
- When Linewright cannot get CI green or cannot merge it, it unassigns itself
  and comments why. Assign it again to have it try again.

You can push commits to its branch; Linewright keeps them. When Linewright
stops or hands the pull request back part way through its work, it pushes the
work so far to the branch, with what the agent left uncommitted as one commit
titled "Unfinished work, kept when its run stopped". Its next run on the pull
request folds that commit into its own. A pull request
closes the issues it names with `Closes #N`. Edit those out and the issue is
no longer handled by that pull request, so Linewright works on it again.

## Stopping work on an issue

Change or remove its label, unassign Linewright or close the issue. Once the
label no longer asks for Linewright's work, such as when you relabel the issue
`ready-for-human`, Linewright stops any run on it and unassigns itself. Once
unassigned, by you or by itself, Linewright leaves the issue until
`ready-for-agent`, or the required label, is applied again, or until you
assign it again.

An issue reopened after its pull request merged is worked again while
Linewright is still assigned to it, without applying the label again.

Before a pull request exists, Linewright pushes the work so far to the
issue's branch when it stops or hands the issue back, and its comment names
the branch, so you can see what it tried. What the agent left uncommitted is
there as one commit titled "Unfinished work, kept when its run stopped". The
next run continues from the branch and folds that commit into its own; delete
the branch to have it start over. Closing the issue deletes it.

## Recurring work

For engineering. An automation is a Markdown file in `.linewright/automations/`
that tells an agent what to do on a schedule, such as filing an issue for each
bug fixed last week without a regression test. The issues it files say which
automation filed them, and are worked like any others. See
[Automations](specs/automations.md).
