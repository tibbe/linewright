// -*- mode: Bluespec; -*-
/**
 * Linewright's spec as a state machine over what GitHub records.
 *
 * Each definition is named after the spec's own term and cites the clause it
 * encodes, quoting it: R = repository.md, D = design.md, P =
 * pull_requests.md, T = triage.md, S = sources_of_work.md, U = runs.md, DS =
 * design_system.md, M = monitoring.md. A reading the spec leaves open is an
 * assumption, A1 to A16, listed below.
 *
 * Three parties act. Linewright makes one GitHub write a step and may stop
 * (a crash or a kill) between any two. People act a bounded number of times
 * a trace. GitHub closes the issues a merged pull request closes, some time
 * after the merge. The model files (pipeline.qnt, cut.qnt, spec_change.qnt)
 * fix the items and the bounds.
 *
 * Assumptions, each cited where the model uses it:
 *
 * A1.  The comment that records an outcome is the first write of its series.
 * A2.  "One writer" is one series of writes at a time across all items; runs
 *      are in flight on several items at once.
 * A3.  `ready-for-agent` asks for both the design and the implement run, so a
 *      person changing `linewright:designed` during either cancels neither.
 * A4.  A cancel takes effect at a later step of Linewright's, not with the
 *      person's change, so a run can be in flight on an item that no longer
 *      asks for it (the State table's "has a run in flight" row). A run
 *      applying its result is not cancelled, and is in flight until its last
 *      write.
 * A5.  A run ends by the Working an item rows for its stage, whatever the
 *      item's state is by then.
 * A6.  A series of writes goes on to its end whatever people do to the item
 *      meanwhile; nothing in the spec says otherwise.
 * A7.  "It claims a pull request it opened" on an issue's terms: unassigned,
 *      marked, and not claimed again once Linewright was unassigned until the
 *      required label is applied again.
 * A8.  Of the Working an item rows, only the first (a cut left behind) holds
 *      for a closed issue: every other row's State is an open item's.
 * A9.  A ticket cut from a spec change's issue, and a spec gap issue its design
 *      files, reference the spec change.
 * A10. A person's comment on a `needs-info` issue Linewright has worked is a
 *      reply.
 * A11. In "Asking for its work", "open" applies to both clauses.
 * A12. A person who adds a status label may leave the old one, for Linewright
 *      to remove.
 * A13. A cut writes, in this order: the spec gap issue; each ticket, created
 *      with its labels as a sub-issue in one write; each blocked-by link; the
 *      comment; `linewright:designed`.
 * A14. A run whose result a stop interrupted decides the same when run again.
 * A15. The Ready line "unassigned" holds when Linewright was unassigned since
 *      the issue's status label or the required label was last applied.
 * A16. A claim is one write, the assignment; `linewright:stalled` is cleared by
 *      a write of its own once Linewright is assigned. Cleared first, in the
 *      same series, a person's relabel between the two writes has Linewright
 *      claim an item whose state no longer asks for a claim.
 */
module linewright {
  import facts.* from "./facts"

  // ===========================================================================
  // The universe each model file fixes
  // ===========================================================================

  const ISSUES: Set[int]
  const PRS: Set[int]
  /// Every item, in number order: the order Linewright writes them in.
  const ORDER: List[int]
  /// The items people label, assign, close, merge or comment on.
  const PEOPLE_ITEMS: Set[int]
  /// Issues people only close and reopen: the issues that block others.
  const BLOCKERS: Set[int]
  /// How many actions people take in a trace.
  const PEOPLE_ACTS: int
  /// How many times Linewright stops (a crash or a kill) in a trace.
  const STOPS: int
  /// The issues a cut creates as tickets, in the order it creates them.
  const TICKET_SLOTS: List[int]
  /// The spec gap issues a design files.
  const GAP_SLOTS: List[int]
  /// The issue Linewright opens for a spec change (S "Taking it up").
  const SPEC_ISSUE_SLOT: int -> int
  /// The pull request an implement run on an issue opens.
  const PR_SLOT: int -> int
  /// Pull requests of a person's that a person may open.
  const PERSON_PRS: Set[int]
  /// Issues people add sub-issues to, and the issues they add.
  const SUB_ISSUE_PARENTS: Set[int]
  const SUB_ISSUE_CHILDREN: Set[int]
  /// Issues a design may cut into tickets.
  const CUTTABLE: Set[int]
  /// What each stage's run may decide, within the bounds.
  const TRIAGE_OUTCOMES: Set[str]
  const DESIGN_OUTCOMES: Set[str]
  const IMPLEMENT_OUTCOMES: Set[str]
  const PR_OUTCOMES: Set[str]

  pure val ITEMS: Set[int] = ISSUES.union(PRS)

  /// Where traces start.
  const STARTS: Set[Start]

  /// A run in flight: its stage, the state it started in, and whether it is
  /// applying its result.
  type Run = { stage: str, start: str, applying: bool }
  pure val NO_RUN: Run = { stage: "", start: "", applying: false }

  /// One GitHub write.
  type Write = { op: str, item: int, arg: str, n: int }

  /// Something Linewright may do now: a claim, a run, or a series of writes.
  type Option = { act: str, item: int, arg: str }
  pure val NO_OPTION: Option = { act: "", item: 0, arg: "" }

  /// What a stop interrupted, for S5.
  type Sig = { state: str, designed: bool, lw: bool }
  type Stop = { active: bool, fresh: bool, item: int, end: Sig, personSince: bool }

  type World = { items: int -> Item, runs: int -> Run, requireLabel: bool, autoMerge: bool }

  // ===========================================================================
  // State variables
  // ===========================================================================

  var items: int -> Item
  var runs: int -> Run
  /// The writes left of the series Linewright is carrying out: one series at
  /// a time (R ## Writes "One writer", A2).
  var seq: List[Write]
  var seqOf: Option
  /// The outcome a run decides again after a stop interrupted applying it (A14).
  var redo: int -> { stage: str, outcome: str }
  var clock: int
  var peopleLeft: int
  var stopsLeft: int
  /// `require-label` set (R "Marked for Linewright"), and `--auto-merge`.
  var requireLabel: bool
  var autoMerge: bool
  /// (pull request, issue): GitHub has yet to close the issue the merge closes.
  var ghPending: Set[(int, int)]
  // Ghosts: what the properties need of the past.
  var stop: Stop
  var s4ok: bool
  var s7ok: bool
  var lwDid: Set[(int, str, str)]
  var flapped: bool
  /// Whether lwDid is kept: only for S6's flap check, whose run it makes
  /// four times as large.
  var trackFlaps: bool

  val world: World = { items: items, runs: runs, requireLabel: requireLabel, autoMerge: autoMerge }

  // ===========================================================================
  // Labels and assignment (R ## Labels, ## Assignment)
  // ===========================================================================

  /// R ## Labels "Status labels": "The first four are the status labels".
  pure val STATUS_LABELS = Set(NEEDS_TRIAGE, NEEDS_INFO, READY_FOR_AGENT, READY_FOR_HUMAN)
  /// R ## Labels, the table's "asks for work by" column: "Linewright".
  pure val ASKS_FOR_LINEWRIGHTS_WORK = Set(NEEDS_TRIAGE, READY_FOR_AGENT)

  pure def it(w: World, i: int): Item = w.items.get(i)
  pure def isIssue(w: World, i: int): bool = ISSUES.contains(i) and it(w, i).exists
  pure def isPr(w: World, i: int): bool = PRS.contains(i) and it(w, i).exists
  pure def runInFlight(w: World, i: int): bool = w.runs.get(i).stage != ""

  /// R ## Assignment "Marked for Linewright": "With require-label set, an item
  /// is marked for Linewright when it carries the required label ... Unset,
  /// every issue and pull request is marked for Linewright."
  pure def markedForLinewright(w: World, i: int): bool =
    not(w.requireLabel) or it(w, i).required

  /// R ## Working an item "Claiming": "that it can start: no open issue
  /// blocks it".
  pure def blockedBy(w: World, i: int): Set[int] =
    it(w, i).blockedBy.filter(b => isIssue(w, b) and it(w, b).open)
  pure def blocked(w: World, i: int): bool = blockedBy(w, i).size() > 0

  /// P ## The pull request "The issues it closes": "A PR is tied to issues
  /// only by GitHub's closing references".
  pure def prsClosing(w: World, i: int): Set[int] =
    PRS.filter(p => it(w, p).exists and it(w, p).closes.contains(i))
  pure def openPrClosesIt(w: World, i: int): bool =
    prsClosing(w, i).exists(p => it(w, p).open)

  // ===========================================================================
  // Spec changes and tracking issues (S ## Spec changes, D ## Cutting)
  // ===========================================================================

  /// S ## Spec changes "A spec change": "A pull request not Linewright's own,
  /// merged into the default branch, that touched a spec file."
  pure def isSpecChange(w: World, p: int): bool = isPr(w, p) and it(w, p).spec

  /// S "Taken up": "Its issue is the first such issue": one Linewright
  /// created, titled "Spec change: …", that links it. Only taking it up
  /// creates an issue so titled, in SPEC_ISSUE_SLOT; a spec gap or a ticket
  /// that references it is not one. 0 when there is none.
  pure def itsIssue(w: World, s: int): int =
    ISSUES.filter(t => it(w, t).exists and it(w, t).byLw and it(w, t).refs == s
                       and SPEC_ISSUE_SLOT.keys().exists(k => SPEC_ISSUE_SLOT.get(k) == t))
      .fold(0, (best, t) => if (best == 0 or it(w, t).born < it(w, best).born) t else best)

  /// S "Taken up": "A spec change is taken up once an issue Linewright
  /// created, titled "Spec change: …", links it, or once it carries
  /// linewright:designed."
  pure def takenUp(w: World, s: int): bool = itsIssue(w, s) != 0 or it(w, s).designed

  pure def subIssues(w: World, x: int): Set[int] =
    ISSUES.filter(t => it(w, t).exists and it(w, t).parent == x)

  /// D ## Cutting "The tracking issue": "tickets cut from it among its
  /// sub-issues: those Linewright created and added to it."
  pure def ticketsCutFrom(w: World, x: int): Set[int] =
    subIssues(w, x).filter(t => it(w, t).byLw and it(w, t).parentByLw)

  /// D ## Cutting "The tracking issue": "An issue cut into tickets is their
  /// tracking issue while it carries linewright:designed and has tickets cut
  /// from it among its sub-issues".
  pure def isTrackingIssue(w: World, x: int): bool =
    isIssue(w, x) and it(w, x).designed and ticketsCutFrom(w, x).size() > 0

  // ===========================================================================
  // State (R ## State): the first row of its table that holds
  // ===========================================================================

  /// R ## State, the issue table, rows in the spec's order.
  pure def issueState(w: World, i: int): str = {
    val x = it(w, i)
    // "is closed, or has had a pull request that closes it merge into the
    // default branch since it was last opened or given a status label"
    if (not(x.open) or x.mergedSince) "done"
    // "has an open pull request that closes it"
    else if (openPrClosesIt(w, i)) "in review"
    // "is a tracking issue"
    else if (isTrackingIssue(w, i)) "tracking"
    // "holds needs-triage and is assigned to Linewright"
    else if (x.status == NEEDS_TRIAGE and x.lw) "being triaged"
    // "holds ready-for-agent, is assigned to Linewright and is without
    // linewright:designed"
    else if (x.status == READY_FOR_AGENT and x.lw and not(x.designed)) "being designed"
    // "holds ready-for-agent and is assigned to Linewright"
    else if (x.status == READY_FOR_AGENT and x.lw) "being implemented"
    // "holds a status label": "its status label's"
    else if (x.status != "") x.status
    // "has a run in flight": "the one it was in when the run started"
    else if (runInFlight(w, i)) w.runs.get(i).start
    // "otherwise": "no state"
    else ""
  }

  /// R ## State, the pull request table.
  pure def prState(w: World, p: int): str = {
    val x = it(w, p)
    if (x.open) "open"
    else if (x.merged) "merged"
    else "closed"
  }

  pure def stateOf(w: World, i: int): str =
    if (not(it(w, i).exists)) ""
    else if (PRS.contains(i)) prState(w, i)
    else issueState(w, i)

  /// Every state the tables name.
  pure val ISSUE_STATES = Set("done", "in review", "tracking", "being triaged",
    "being designed", "being implemented", NEEDS_TRIAGE, NEEDS_INFO,
    READY_FOR_AGENT, READY_FOR_HUMAN)
  pure val PR_STATES = Set("open", "merged", "closed")

  /// The row of each table that holds, for S1's witnesses.
  pure def issueRow(w: World, i: int): int = {
    val x = it(w, i)
    if (not(x.open) or x.mergedSince) 1
    else if (openPrClosesIt(w, i)) 2
    else if (isTrackingIssue(w, i)) 3
    else if (x.status == NEEDS_TRIAGE and x.lw) 4
    else if (x.status == READY_FOR_AGENT and x.lw and not(x.designed)) 5
    else if (x.status == READY_FOR_AGENT and x.lw) 6
    else if (x.status != "") 7
    else if (runInFlight(w, i)) 8
    else 9
  }
  pure def prRow(w: World, p: int): int =
    if (it(w, p).open) 1 else if (it(w, p).merged) 2 else 3

  // ===========================================================================
  // Managed items (R ## Managed items)
  // ===========================================================================

  /// R ## Managed items "Asking for its work": "An open item Linewright is
  /// assigned to, or one marked for Linewright whose status label asks for its
  /// work." (A11: "open" for both.)
  pure def asksForItsWork(w: World, i: int): bool = {
    val x = it(w, i)
    x.open and (x.lw or (markedForLinewright(w, i) and ASKS_FOR_LINEWRIGHTS_WORK.contains(x.status)))
  }

  /// R ## Managed items: "Linewright manages an item it has worked or that
  /// asks for its work, while the item has a state."
  pure def managed(w: World, i: int): bool =
    it(w, i).exists and (it(w, i).worked or asksForItsWork(w, i)) and stateOf(w, i) != ""

  // ===========================================================================
  // Working an item (R ## Working an item, ## Steering)
  // ===========================================================================

  /// R ## Working an item: what Linewright starts in each state, by the
  /// table's "Linewright" column. A claim is "its assignment"; "pr" stands for
  /// the fix-ci, rework and update runs.
  pure def mayStart(state: str, itsOwnPr: bool): Set[str] =
    if (state == NEEDS_TRIAGE or state == READY_FOR_AGENT) Set("claim")
    else if (state == "being triaged") Set("triage")
    else if (state == "being designed") Set("design")
    else if (state == "being implemented") Set("implement")
    else if (state == "open" and itsOwnPr) Set("claim", "pr")
    else Set()

  /// R ## Steering: a run is cancelled when a person "changes an issue's
  /// status label" to one that does not ask for it, "closes an item, or merges
  /// a pull request", or "unassigns Linewright". A3: ready-for-agent asks for
  /// both the design and the implement run.
  pure def asksForRun(w: World, i: int, stage: str): bool = {
    val x = it(w, i)
    if (PRS.contains(i)) x.lw and x.open
    else and {
      x.lw, x.open,
      if (stage == "triage") x.status == NEEDS_TRIAGE else x.status == READY_FOR_AGENT,
    }
  }

  /// R ## Working an item, its first row: "the issue has tickets cut from it
  /// among its sub-issues and is without linewright:designed"; "A run in
  /// flight": "While a run is in flight on an item, only the rows for how
  /// that run ends hold for it."
  pure def owesDiscard(w: World, x: int): bool = and {
    isIssue(w, x), not(it(w, x).designed), not(runInFlight(w, x)),
    ticketsCutFrom(w, x).size() > 0,
  }

  /// R ## Working an item "Claiming": "On its own, Linewright claims an issue
  /// that is unassigned, marked for it and that it can start: no open issue
  /// blocks it. It claims a pull request it opened. An item Linewright was
  /// unassigned from, by itself or by anyone, is not claimed again on its own
  /// until its status label or the required label is applied again." A7: a
  /// pull request it opened, on the same terms as an issue.
  pure def claimsOnItsOwn(w: World, i: int): bool = {
    val x = it(w, i)
    and {
      not(x.lw), not(x.alice), not(x.asked),
      markedForLinewright(w, i),
      not(x.unassignedSince),
      mayStart(stateOf(w, i), PRS.contains(i) and x.byLw).contains("claim"),
      not(blocked(w, i)),
    }
  }

  pure def freeSlots(w: World, slots: List[int]): List[int] =
    slots.select(s => not(it(w, s).exists))

  /// Whether a run of `stage` on `i` may end with `o`, within the bounds.
  pure def feasible(w: World, i: int, stage: str, o: str): bool =
    if (o == "cut") CUTTABLE.contains(i) and freeSlots(w, TICKET_SLOTS).length() > 0
    else if (o == "pr") PR_SLOT.keys().contains(i) and not(it(w, PR_SLOT.get(i)).exists)
    // An issue is built as it is only where it has a pull request to be built in.
    else if (o == "uncut") PR_SLOT.keys().contains(i)
    else true

  pure def outcomesOf(w: World, i: int, stage: str): Set[str] = {
    val possible = if (stage == "triage") TRIAGE_OUTCOMES
      else if (stage == "design") DESIGN_OUTCOMES
      else if (stage == "implement") IMPLEMENT_OUTCOMES
      else PR_OUTCOMES
    possible.filter(o => feasible(w, i, stage, o))
  }

  pure val WORKING_STATES = Set("being triaged", "being designed", "being implemented")
  pure def stageOf(state: str): str =
    if (state == "being triaged") "triage"
    else if (state == "being designed") "design"
    else if (state == "being implemented") "implement"
    else ""

  pure def opt(act: str, i: int, arg: str): Option = { act: act, item: i, arg: arg }
  pure def when(c: bool, o: Option): Set[Option] = if (c) Set(o) else Set()

  /// The first row of a first-match table that holds.
  pure def firstRow(rows: List[(bool, Option)]): Set[Option] =
    rows.foldl(Set(), (acc, r) => if (acc.size() == 0 and r._1) Set(r._2) else acc)

  /// R ## Working an item: "Linewright changes an item's labels and
  /// assignees, and starts work on it, only as this table gives: at each poll,
  /// the first row that holds for the item." The rows for an issue, in the
  /// spec's order, as (when, what Linewright does). The last two are "any but
  /// done" in the table: they hold only while the issue is open (A8).
  pure def issueRows(w: World, i: int): List[(bool, Option)] = {
    val x = it(w, i)
    val st = issueState(w, i)
    [
      // any | "the issue has tickets cut from it among its sub-issues and is
      // without linewright:designed" | "discards the cut"
      (owesDiscard(w, i), opt("discard", i, "")),
      // being designed, being implemented | "a person has closed without
      // merging a pull request Linewright opened that closes it, since the
      // issue was last opened or given a status label" | "hands it back"
      (x.open and Set("being designed", "being implemented").contains(st) and x.noSince,
        opt("hand back", i, "closed unmerged")),
      // needs-info, ready-for-human, no state | "Linewright is assigned to the
      // issue" | "unassigns itself, with a comment saying what the issue needs
      // first where a person assigned it" (A8: the rows from here on are for
      // an open issue)
      (x.open and Set(NEEDS_INFO, READY_FOR_HUMAN, "").contains(st) and x.lw, opt("let go", i, "")),
      // needs-triage, ready-for-agent | "a person Linewright asked is assigned
      // to it" | "unassigns them"
      (x.open and ASKS_FOR_LINEWRIGHTS_WORK.contains(st) and x.asked,
        opt("unassign the people asked", i, "")),
      // tracking | "it holds a status label or Linewright's assignment" |
      // "removes the status label, then unassigns itself"
      (x.open and st == "tracking" and (x.status != "" or x.older != "" or x.lw),
        opt("tidy tracking issue", i, "")),
      // tracking | "it has sub-issues and every one is closed" | "closes it"
      (x.open and st == "tracking" and subIssues(w, i).size() > 0
         and subIssues(w, i).forall(t => not(it(w, t).open)),
        opt("close tracking issue", i, "")),
      // needs-info | "a reply has come" | "labels it needs-triage"
      (x.open and st == NEEDS_INFO and x.reply, opt("take the reply", i, "")),
      // needs-triage, ready-for-agent | "Linewright may claim it" | "assigns
      // itself"
      (x.open and claimsOnItsOwn(w, i), opt("claim", i, "")),
      // being triaged, being designed, being implemented | "no run is in
      // flight on it" | "starts a ... run"; "Blocked": "A run starts on an
      // issue only once no open issue blocks it."
      (x.open and WORKING_STATES.contains(st) and not(blocked(w, i))
         and outcomesOf(w, i, stageOf(st)).size() > 0,
        opt("start run", i, stageOf(st))),
      // any | "the issue carries more than one status label" | "removes all
      // but the newest"
      (x.open and x.older != "", opt("remove older label", i, x.older)),
      // any | "it carries linewright:stalled and Linewright is assigned to
      // it" | "removes linewright:stalled" (A16).
      (x.open and x.stalled and x.lw, opt("clear stalled", i, "")),
    ]
  }

  /// The table's rows for a pull request, in the spec's order, and the
  /// Steering rows for one a person assigned to Linewright.
  pure def prRows(w: World, p: int): List[(bool, Option)] = {
    val x = it(w, p)
    val spec = isSpecChange(w, p)
    [
      // merged | "a spec change Linewright may take up" | "opens its issue";
      // S "Taking it up": "Linewright may take up a spec change that is not
      // yet taken up, merged within the lookback window and marked for it";
      // R ## Steering, merged | "assigns Linewright to a spec change not yet
      // taken up" | "opens its issue, then unassigns itself".
      (spec and not(takenUp(w, p)) and SPEC_ISSUE_SLOT.keys().contains(p)
         and (x.lw or (x.inLookback and markedForLinewright(w, p))),
        opt("take up", p, "")),
      // open | "a pull request Linewright opened, that it may claim" |
      // "assigns itself"; "Claiming": "It claims a pull request it opened" (A7).
      (x.byLw and x.open and claimsOnItsOwn(w, p), opt("claim", p, "")),
      // open | "a pull request Linewright opened and is assigned to, for
      // which Pull requests asks for a fix-ci, rework or update run or a
      // merge" | "starts that run, or merges it"; P ## Rework: "A comment
      // or review from anyone other than Linewright, posted after Linewright's
      // last push, starts a rework run"; P ## Merging: "with it, Linewright
      // merges each green PR it opened."
      (x.byLw and x.open and x.lw and x.feedback, opt("start run", p, "pr")),
      (x.byLw and x.open and x.lw and w.autoMerge, opt("merge", p, "")),
      // R ## Steering, open | "assigns Linewright to any other pull request it
      // did not open" | "unassigns itself with a comment".
      (not(x.byLw) and x.open and x.lw, opt("let go", p, "")),
      // any | "it carries linewright:stalled and Linewright is assigned to
      // it" | "removes linewright:stalled".
      (x.byLw and x.open and x.stalled and x.lw, opt("clear stalled", p, "")),
    ]
  }

  /// Everything Linewright may do on `i` now, whether or not it is in the
  /// middle of a series of writes.
  pure def optionsOn(w: World, i: int): Set[Option] =
    if (not(it(w, i).exists)) Set()
    else if (runInFlight(w, i)) {
      val r = w.runs.get(i)
      if (r.applying) Set()
      // R ## Steering: "cancels a run"; U ## Ending "Cancelling" (A4: a step
      // of Linewright's; not while the run applies its result).
      else if (not(asksForRun(w, i, r.stage))) Set(opt("cancel", i, r.stage))
      // R ## Writes "A person's change during a run": "Linewright acts on a
      // person's change to it when the run ends"; the table's rows for how
      // the run ends (A5: by the run's stage).
      else outcomesOf(w, i, r.stage).map(o => opt("end run", i, o))
    }
    else if (PRS.contains(i)) firstRow(prRows(w, i))
    else firstRow(issueRows(w, i))

  pure def optionsIn(w: World): Set[Option] = ITEMS.map(i => optionsOn(w, i)).flatten()

  val options: Set[Option] = optionsIn(world)

  // ===========================================================================
  // The writes of each series (R ## Writes "Order", the table's "Linewright
  // writes" column)
  // ===========================================================================

  pure def wr(op: str, i: int, arg: str, n: int): Write = { op: op, item: i, arg: arg, n: n }
  pure def comment(i: int): Write = wr("comment", i, "", 0)
  pure def wif(c: bool, l: List[Write]): List[Write] = if (c) l else []

  /// A new status label, then the old ones removed: R ## Writes "Order": "a
  /// new status label is added before the old one is removed".
  pure def relabel(x: Item, i: int, l: str): List[Write] =
    List(wr("add label", i, l, 0))
      .concat(wif(x.status != "" and x.status != l, [wr("remove label", i, x.status, 0)]))
      .concat(wif(x.older != "" and x.older != l, [wr("remove label", i, x.older, 0)]))

  /// R ## Assignment "Handing back": "it labels an issue ready-for-human,
  /// unassigns itself from the item, removes linewright:designed where the
  /// design is the item's own, and says why in a comment." "Stalls": "A stall
  /// labels the item linewright:stalled as well". R ## Writes "Order":
  /// "linewright:designed before the status label and the status label before
  /// the assignment".
  pure def handBackWrites(w: World, i: int, stall: bool): List[Write] = {
    val x = it(w, i)
    List(comment(i))
      .concat(wif(stall, [wr("add stalled", i, "", 0)]))
      .concat(wif(x.designed and x.ownDesign, [wr("remove designed", i, "", 0)]))
      .concat(if (ISSUES.contains(i)) relabel(x, i, READY_FOR_HUMAN) else [])
      .concat([wr("unassign", i, "hand back", 0)])
  }

  /// D ## Designing "Discarding a cut": "Discarding a cut closes the pull
  /// requests Linewright opened for its tickets and closes its open tickets as
  /// not planned, each with a comment, then removes its tickets from the
  /// issue's sub-issues."
  pure def discardWrites(w: World, x: int): List[Write] = {
    val tickets = TICKET_SLOTS.select(t => ticketsCutFrom(w, x).contains(t))
    val closing = tickets.foldl([], (acc, t) =>
      acc
        .concat(ORDER.select(p => prsClosing(w, t).contains(p) and it(w, p).open and it(w, p).byLw)
          .foldl([], (a, p) => a.concat([comment(p), wr("close pr", p, "", 0)])))
        .concat(wif(it(w, t).open, [comment(t), wr("close", t, "not planned", 0)])))
    closing.concat(tickets.foldl([], (a, t) => a.append(wr("detach", t, "", 0))))
  }

  /// The writes of what a run decided: R ## Working an item, the table's
  /// "Linewright writes" column, after the comment that records the outcome
  /// (R ## Comments; T ## Outcomes: "posts the agent's comment"; A1: first).
  pure def resultWrites(w: World, i: int, stage: str, o: str): List[Write] = {
    val x = it(w, i)
    if (stage == "triage") {
      // being triaged | "the triage run ends with ready-for-agent" | "labels
      // it ready-for-agent".
      if (o == READY_FOR_AGENT) List(comment(i)).concat(relabel(x, i, READY_FOR_AGENT))
      // ... "ends with needs-info" | "labels it needs-info, then assigns the
      // people asked whom GitHub lets it assign".
      else if (o == NEEDS_INFO)
        List(comment(i)).concat(relabel(x, i, NEEDS_INFO)).concat([wr("assign asked", i, "", 0)])
      // ... "ends with ready-for-human" | "labels it ready-for-human".
      else List(comment(i)).concat(relabel(x, i, READY_FOR_HUMAN))
    } else if (stage == "design") {
      // being designed | "the design run builds it as it is" | "labels it
      // linewright:designed".
      if (o == "uncut") List(comment(i), wr("add designed", i, "own", 0))
      // ... "cuts it into tickets" | "creates the tickets, then labels it
      // linewright:designed". D ## Cutting "Tickets": "Each ticket is a
      // sub-issue of its tracking issue, carries ready-for-agent and
      // linewright:designed, and is linked to the tickets it is blocked by".
      // D "Spec gaps"; U ## Ending "Spec gaps": a gap that does not block
      // "becomes a separate ready-for-human issue" (A13 for the order).
      else if (o == "cut") {
        val ts = freeSlots(w, TICKET_SLOTS)
        val gaps = freeSlots(w, GAP_SLOTS)
        val twoTickets = ts.length() > 1
        // (if, not wif: the list is out of bounds where the condition fails)
        (if (gaps.length() > 0) [wr("create spec gap", gaps[0], "", x.refs)] else [])
          .concat([wr("create ticket", ts[0], "", i)])
          .concat(if (twoTickets) [wr("create ticket", ts[1], "", i), wr("link", ts[1], "", ts[0])] else [])
          .concat([comment(i), wr("add designed", i, "own", 0)])
      }
      // ... "finds nothing to build, in an issue Linewright created" | "closes
      // it as completed"; "finds nothing to build in any other issue, or
      // cannot make a design" | "hands it back".
      else if (o == "nothing to build" and x.byLw) List(comment(i), wr("close", i, "completed", 0))
      else handBackWrites(w, i, false)
    } else if (stage == "implement") {
      // being implemented | "the implement run builds it" | "opens a pull
      // request that closes it"; P ## The pull request "Opening".
      if (o == "pr") List(wr("create pr", PR_SLOT.get(i), "", i))
      // being triaged, being designed, being implemented | "its run cannot
      // finish" | "hands it back" (a stall, here).
      else handBackWrites(w, i, true)
    } else {
      if (o == "done") List(wr("push", i, "", 0))
      else handBackWrites(w, i, true)
    }
  }

  /// The writes an option carries out.
  pure def optionWrites(w: World, o: Option): List[Write] = {
    val i = o.item
    val x = it(w, i)
    if (o.act == "hand back") handBackWrites(w, i, false)
    else if (o.act == "discard") discardWrites(w, i)
    else if (o.act == "remove older label") List(wr("remove label", i, x.older, 0))
    else if (o.act == "tidy tracking issue")
      wif(x.status != "", [wr("remove label", i, x.status, 0)])
        .concat(wif(x.older != "", [wr("remove label", i, x.older, 0)]))
        .concat(wif(x.lw, [wr("unassign", i, "", 0)]))
    else if (o.act == "close tracking issue") List(wr("close", i, "completed", 0))
    else if (o.act == "unassign the people asked") List(wr("unassign asked", i, "", 0))
    else if (o.act == "take the reply") relabel(x, i, NEEDS_TRIAGE)
    // "with a comment saying what the issue needs first where a person
    // assigned it"; a pull request: "unassigns itself with a comment".
    else if (o.act == "let go")
      wif(x.byHand or PRS.contains(i), [comment(i)]).concat([wr("unassign", i, "", 0)])
    // "assigns itself" (A16: one write).
    else if (o.act == "claim") List(wr("assign", i, "", 0))
    else if (o.act == "clear stalled") List(wr("remove stalled", i, "", 0))
    else if (o.act == "merge") List(wr("merge", i, "", 0))
    // S "Its issue": "Taking up a spec change opens its issue: "Spec change:
    // <PR title>", linking the pull request and labeled ready-for-agent"; R
    // ## Steering: "opens its issue, then unassigns itself".
    else if (o.act == "take up")
      List(wr("create spec change issue", SPEC_ISSUE_SLOT.get(i), "", i))
        .concat(wif(x.lw, [wr("unassign", i, "", 0)]))
    else if (o.act == "end run") resultWrites(w, i, w.runs.get(i).stage, o.arg)
    else []
  }

  // ===========================================================================
  // How one write changes what GitHub records
  // ===========================================================================

  pure def addStatus(x: Item, l: str): Item =
    if (x.status == l) x
    else { ...x, status: l, older: if (x.status != "") x.status else x.older,
           unassignedSince: false, mergedSince: false, noSince: false }

  pure def removeStatus(x: Item, l: str): Item =
    if (x.status == l) { ...x, status: x.older, older: "" }
    else if (x.older == l) { ...x, older: "" }
    else x

  /// R ## Managed items "Worked": "GitHub shows Linewright opening the item,
  /// claiming it, applying a label to it or opening a pull request that closes
  /// it"; R ## Writes "Its own writes": they carry its name.
  pure def applyWrite(w: World, wt: Write, born: int): int -> Item = {
    val i = wt.item
    val its = w.items
    val x = its.get(i)
    val made = { ...ABSENT, exists: true, byLw: true, born: born, required: w.requireLabel, worked: true }
    if (wt.op == "comment") its
    else if (wt.op == "add label")
      its.set(i, { ...addStatus(x, wt.arg), worked: true, letGo: false,
                   reply: if (wt.arg == NEEDS_INFO) false else x.reply })
    else if (wt.op == "remove label") its.set(i, { ...removeStatus(x, wt.arg), letGo: false })
    else if (wt.op == "add designed")
      its.set(i, { ...x, designed: true, ownDesign: wt.arg == "own", worked: true, letGo: false })
    else if (wt.op == "remove designed") its.set(i, { ...x, designed: false, ownDesign: false })
    else if (wt.op == "add stalled") its.set(i, { ...x, stalled: true, worked: true })
    else if (wt.op == "remove stalled") its.set(i, { ...x, stalled: false })
    else if (wt.op == "assign")
      its.set(i, { ...x, lw: true, byHand: false, worked: true, letGo: false })
    else if (wt.op == "unassign")
      its.set(i, { ...x, lw: false, byHand: false, unassignedSince: true,
                   letGo: wt.arg == "hand back" })
    else if (wt.op == "assign asked") its.set(i, { ...x, asked: true })
    else if (wt.op == "unassign asked") its.set(i, { ...x, asked: false })
    // A9: a ticket references the spec change its tracking issue does.
    else if (wt.op == "create ticket")
      its.set(i, { ...made, status: READY_FOR_AGENT, designed: true,
                   parent: wt.n, parentByLw: true, refs: its.get(wt.n).refs })
    else if (wt.op == "create spec gap")
      its.set(i, { ...made, status: READY_FOR_HUMAN, refs: wt.n })
    else if (wt.op == "create spec change issue")
      its.set(i, { ...made, status: READY_FOR_AGENT, refs: wt.n })
    else if (wt.op == "create pr")
      its.set(i, { ...made, pr: true, closes: Set(wt.n) })
        .setBy(wt.n, y => { ...y, worked: true })
    else if (wt.op == "link") its.set(i, { ...x, blockedBy: x.blockedBy.union(Set(wt.n)) })
    else if (wt.op == "close")
      its.set(i, { ...x, open: false, closedBy: "linewright", notPlanned: wt.arg == "not planned" })
    else if (wt.op == "close pr") its.set(i, { ...x, open: false, closedBy: "linewright" })
    else if (wt.op == "detach") its.set(i, { ...x, parent: 0, parentByLw: false })
    else if (wt.op == "merge") merged(its, i, "linewright")
    else if (wt.op == "push") its.set(i, { ...x, feedback: false })
    else its
  }

  /// A pull request merges: R ## State row 1, "merge into the default branch
  /// since".
  pure def merged(its: int -> Item, p: int, by: str): int -> Item = {
    val x = its.get(p)
    x.closes.fold(its.set(p, { ...x, open: false, merged: true, closedBy: by }), (acc, i) =>
      if (acc.get(i).open) acc.setBy(i, y => { ...y, mergedSince: true }) else acc)
  }
  pure def mergePending(its: int -> Item, p: int): Set[(int, int)] =
    its.get(p).closes.filter(i => its.get(i).open).map(i => (p, i))

  pure val CREATES = Set("create ticket", "create spec gap", "create spec change issue", "create pr")

  pure def applyAll(w: World, ws: List[Write]): World =
    ws.foldl(w, (acc, wt) => { ...acc, items: applyWrite(acc, wt, 1000) })

  /// What S5 compares of an item: its state and, while it is open, what it
  /// asks for: linewright:designed and Linewright's assignment. "A closed item
  /// keeps its labels and assignees" (R "While assigned"), and asks for
  /// nothing.
  pure def sig(w: World, i: int): Sig =
    if (it(w, i).open) { state: stateOf(w, i), designed: it(w, i).designed, lw: it(w, i).lw }
    else { state: stateOf(w, i), designed: false, lw: false }

  // ---------------------------------------------------------------------------
  // S4, S6 and S7, judged at each write
  // ---------------------------------------------------------------------------

  /// S4: Linewright claims an item or starts a run on it only where Starting
  /// work allows, and nothing starts on an item with a run in flight.
  pure def s4WriteOk(w: World, wt: Write): bool =
    wt.op != "assign" or and {
      mayStart(stateOf(w, wt.item), PRS.contains(wt.item) and it(w, wt.item).byLw).contains("claim"),
      not(runInFlight(w, wt.item)),
    }
  pure def s4StartOk(w: World, i: int, stage: str): bool =
    mayStart(stateOf(w, i), PRS.contains(i) and it(w, i).byLw).contains(stage)
      and not(runInFlight(w, i))

  /// S7: Linewright closes, or removes from sub-issues, an item it didn't
  /// create only where the spec says: a tracking issue whose sub-issues are
  /// all closed. It never closes or detaches a sub-issue a person added.
  pure def s7WriteOk(w: World, wt: Write): bool = {
    val x = it(w, wt.item)
    val personsSubIssue = x.parent != 0 and not(x.parentByLw)
    if (wt.op == "close")
      not(personsSubIssue) and (x.byLw or (isTrackingIssue(w, wt.item)
        and subIssues(w, wt.item).forall(t => not(it(w, t).open))))
    else if (wt.op == "close pr") x.byLw
    else if (wt.op == "detach") x.byLw and not(personsSubIssue)
    else true
  }

  /// S6, Linewright never repeats a write it has undone: each write's key,
  /// and the op that undoes it.
  pure def key(wt: Write): (int, str, str) =
    (wt.item, wt.op, if (wt.op == "add label" or wt.op == "remove label") wt.arg else "")
  pure def undoes(op: str): str =
    if (op == "add label") "remove label" else if (op == "remove label") "add label"
    else if (op == "add designed") "remove designed" else if (op == "remove designed") "add designed"
    else if (op == "add stalled") "remove stalled" else if (op == "remove stalled") "add stalled"
    else if (op == "assign") "unassign" else if (op == "unassign") "assign"
    else if (op == "assign asked") "unassign asked" else if (op == "unassign asked") "assign asked"
    else ""
  pure def flaps(did: Set[(int, str, str)], wt: Write): bool = {
    val k = key(wt)
    undoes(wt.op) != "" and did.contains(k) and did.contains((k._1, undoes(wt.op), k._3))
  }

  // ===========================================================================
  // Linewright's steps
  // ===========================================================================

  pure val NO_REDO = { stage: "", outcome: "" }

  /// A step of Linewright's: everything people and GitHub own stays.
  action lwMoves(its2: int -> Item, gh2: Set[(int, int)], seq2: List[Write], series2: Option,
                 runs2: int -> Run, redo2: int -> { stage: str, outcome: str }, clock2: int,
                 s4new: bool, s7new: bool, flapNew: bool, did2: Set[(int, str, str)]): bool = all {
    items' = its2, ghPending' = gh2, seq' = seq2, seqOf' = series2, runs' = runs2,
    redo' = redo2, clock' = clock2,
    s4ok' = s4new, s7ok' = s7new, flapped' = flapNew, trackFlaps' = trackFlaps, lwDid' = did2,
    stop' = { ...stop, fresh: false },
    peopleLeft' = peopleLeft, stopsLeft' = stopsLeft,
    requireLabel' = requireLabel, autoMerge' = autoMerge,
  }

  /// One GitHub write; the rest of its series waits for the next steps
  /// (R ## Writes: "stopped between any two of its writes"), whatever people
  /// do meanwhile (A6).
  action write(wt: Write, rest: List[Write], series: Option, runs1: int -> Run,
               redo1: int -> { stage: str, outcome: str }, did1: Set[(int, str, str)]): bool = {
    val w1 = { ...world, runs: runs1 }
    val done = rest.length() == 0
    val s4new = s4ok and s4WriteOk(w1, wt)
    val s7new = s7ok and s7WriteOk(w1, wt)
    val flapNew = flapped or flaps(did1, wt)
    lwMoves(
      applyWrite(w1, wt, clock + 1),
      if (wt.op == "merge") ghPending.union(mergePending(items, wt.item)) else ghPending,
      rest,
      if (done) NO_OPTION else series,
      // A run is in flight while it applies its result, until its last write.
      if (done and series.act == "end run") runs1.set(series.item, NO_RUN) else runs1,
      redo1,
      clock + (if (CREATES.contains(wt.op)) 1 else 0),
      s4new, s7new, flapNew, if (trackFlaps) did1.union(Set(key(wt))) else Set())
  }

  /// Linewright's next write of the series it is carrying out.
  action lwContinue: bool = all {
    seq.length() > 0,
    write(seq[0], seq.tail(), seqOf, runs, redo, lwDid),
  }

  /// A14: a run that a stop interrupted while it applied its result decides
  /// the same again.
  def redoAllows(o: Option): bool = {
    val r = redo.get(o.item)
    o.act != "end run" or r.stage != runs.get(o.item).stage
      or not(outcomesOf(world, o.item, r.stage).contains(r.outcome)) or o.arg == r.outcome
  }

  /// Linewright starts a run: no GitHub write.
  action startRun(i: int, stage: str): bool = {
    val runs2 = runs.set(i, { stage: stage, start: stateOf(world, i), applying: false })
    lwMoves(items, ghPending, seq, seqOf, runs2, redo, clock,
      s4ok and s4StartOk(world, i, stage), s7ok, flapped, lwDid)
  }

  /// Linewright takes up one of its options: a claim, a run or a series of
  /// writes, whose first write it makes now.
  action take(o: Option): bool = {
    val i = o.item
    if (o.act == "start run") startRun(i, o.arg)
    // U ## Ending "Cancelling": "A cancelled run posts nothing more".
    else if (o.act == "cancel")
      lwMoves(items, ghPending, seq, seqOf, runs.set(i, NO_RUN), redo, clock, s4ok, s7ok, flapped, lwDid)
    else {
      val ws = optionWrites(world, o)
      val runs2 = if (o.act == "end run") runs.set(i, { ...runs.get(i), applying: true }) else runs
      val redo2 = if (o.act == "end run") redo.set(i, NO_REDO) else redo
      // What a run decided is news, as a person's change is: S6's no repeat
      // of an undone write counts from the last of either.
      val did2 = if (o.act == "end run") Set() else lwDid
      all { ws.length() > 0, write(ws[0], ws.tail(), o, runs2, redo2, did2) }
    }
  }

  action lwStep: bool = any {
    lwContinue,
    all {
      seq.length() == 0,
      nondet choice = options.oneOf()
      all { redoAllows(choice), take(choice) },
    },
  }

  /// Linewright stops, by a crash or a kill, between two writes or with runs
  /// in flight. U ## Ending "Restarts": "A restart leaves the runs in flight
  /// interrupted."
  action lwStops: bool = {
    val interrupted = seq.length() > 0
    val i = seqOf.item
    val wEnd = applyAll({ ...world,
      runs: if (seqOf.act == "end run") runs.set(i, NO_RUN) else runs }, seq)
    all {
      stopsLeft > 0,
      interrupted or ITEMS.exists(j => runInFlight(world, j)),
      items' = items, ghPending' = ghPending, seq' = [], seqOf' = NO_OPTION,
      runs' = ITEMS.mapBy(_ => NO_RUN),
      redo' = if (interrupted and seqOf.act == "end run")
          redo.set(i, { stage: runs.get(i).stage, outcome: seqOf.arg })
        else redo,
      clock' = clock, stopsLeft' = stopsLeft - 1,
      stop' = if (interrupted)
          { active: true, fresh: true, item: i, end: sig(wEnd, i), personSince: false }
        else { ...stop, fresh: false },
      peopleLeft' = peopleLeft, requireLabel' = requireLabel, autoMerge' = autoMerge,
      s4ok' = s4ok, s7ok' = s7ok, flapped' = flapped, trackFlaps' = trackFlaps, lwDid' = lwDid,
    }
  }

  /// GitHub closes the issues a merged pull request closes, some time after
  /// the merge (R ## Working an item, row "in review": "nothing: GitHub closes
  /// the issue").
  action githubCloses: bool = all {
    ghPending.size() > 0,
    nondet pi = ghPending.oneOf()
    all {
      items' = if (items.get(pi._2).open)
          items.setBy(pi._2, y => { ...y, open: false, closedBy: "github", notPlanned: false })
        else items,
      ghPending' = ghPending.exclude(Set(pi)),
      seq' = seq, seqOf' = seqOf, runs' = runs, redo' = redo, clock' = clock,
      peopleLeft' = peopleLeft, stopsLeft' = stopsLeft,
      requireLabel' = requireLabel, autoMerge' = autoMerge,
      stop' = { ...stop, fresh: false },
      s4ok' = s4ok, s7ok' = s7ok, flapped' = flapped, trackFlaps' = trackFlaps, lwDid' = lwDid,
    },
  }

  /// The lookback window passes a spec change by (S ## Spec changes,
  /// `spec-lookback`).
  action lookbackEnds: bool = {
    val inWindow = PRS.filter(p => isSpecChange(world, p) and items.get(p).inLookback)
    all {
      inWindow.size() > 0,
      nondet s = inWindow.oneOf()
      all {
        items' = items.setBy(s, y => { ...y, inLookback: false }),
        ghPending' = ghPending, seq' = seq, seqOf' = seqOf, runs' = runs, redo' = redo,
        clock' = clock, peopleLeft' = peopleLeft, stopsLeft' = stopsLeft,
        requireLabel' = requireLabel, autoMerge' = autoMerge,
        stop' = { ...stop, fresh: false },
        s4ok' = s4ok, s7ok' = s7ok, flapped' = flapped, trackFlaps' = trackFlaps, lwDid' = lwDid,
      },
    }
  }

  // ===========================================================================
  // People's actions, each one GitHub write
  // ===========================================================================

  action personMoves(its2: int -> Item, gh2: Set[(int, int)]): bool = all {
    peopleLeft > 0,
    items' = its2, ghPending' = gh2, peopleLeft' = peopleLeft - 1,
    stop' = { ...stop, fresh: false, personSince: stop.personSince or stop.active },
    lwDid' = Set(),
    seq' = seq, seqOf' = seqOf, runs' = runs, redo' = redo, clock' = clock,
    stopsLeft' = stopsLeft, requireLabel' = requireLabel, autoMerge' = autoMerge,
    s4ok' = s4ok, s7ok' = s7ok, flapped' = flapped, trackFlaps' = trackFlaps,
  }

  def person(i: int): bool = PEOPLE_ITEMS.contains(i) and items.get(i).exists

  /// R ## Labels "Status labels": "Whoever sets one removes the others" (A12:
  /// a person may leave the old one for Linewright to remove).
  action personAddsStatusLabel(i: int, l: str): bool = all {
    person(i), ISSUES.contains(i), items.get(i).status != l,
    personMoves(items.setBy(i, x => addStatus(x, l)), ghPending),
  }
  action personRemovesStatusLabel(i: int): bool = all {
    person(i), ISSUES.contains(i), items.get(i).status != "",
    personMoves(items.setBy(i, x => removeStatus(x, x.status)), ghPending),
  }
  /// R ## Steering "removes linewright:designed"; R ## Labels "Linewright's
  /// labels": "a person may change it to steer".
  action personTogglesDesigned(i: int): bool = all {
    person(i),
    personMoves(items.setBy(i, x => { ...x, designed: not(x.designed), ownDesign: false }), ghPending),
  }
  /// R "Claiming": "until its status label or the required label is applied
  /// again".
  action personTogglesRequiredLabel(i: int): bool = all {
    person(i), requireLabel,
    personMoves(items.setBy(i, x => { ...x, required: not(x.required),
      unassignedSince: if (x.required) x.unassignedSince else false }), ghPending),
  }
  /// R ## Steering, "assigns Linewright to ...".
  action personAssignsLinewright(i: int): bool = all {
    person(i), not(items.get(i).lw),
    personMoves(items.setBy(i, x => { ...x, lw: true, byHand: true }), ghPending),
  }
  /// R ## Steering, "unassigns Linewright".
  action personUnassignsLinewright(i: int): bool = all {
    person(i), items.get(i).lw,
    personMoves(items.setBy(i, x => { ...x, lw: false, byHand: false, unassignedSince: true }), ghPending),
  }
  action personAssignsThemself(i: int): bool = all {
    person(i), not(items.get(i).alice),
    personMoves(items.setBy(i, x => { ...x, alice: true }), ghPending),
  }
  action personUnassignsThemself(i: int): bool = all {
    person(i), items.get(i).alice,
    personMoves(items.setBy(i, x => { ...x, alice: false }), ghPending),
  }
  action personClosesIssue(i: int): bool = all {
    person(i) or BLOCKERS.contains(i), ISSUES.contains(i), items.get(i).open,
    personMoves(items.setBy(i, x => { ...x, open: false, closedBy: "person", notPlanned: false }), ghPending),
  }
  /// R ## State row 1: "since it was last opened".
  action personReopensIssue(i: int): bool = all {
    person(i) or BLOCKERS.contains(i), ISSUES.contains(i), not(items.get(i).open),
    personMoves(items.setBy(i, x => { ...x, open: true, closedBy: "", notPlanned: false,
      mergedSince: false, noSince: false }), ghPending),
  }
  /// T ## Answers: "A reply is a comment from anyone other than Linewright
  /// posted after its question" (A10); P ## Rework: "A comment or review from anyone
  /// other than Linewright, posted after Linewright's last push".
  action personComments(i: int): bool = {
    val x = items.get(i)
    all {
      person(i),
      if (ISSUES.contains(i)) x.status == NEEDS_INFO and x.worked and not(x.reply)
      else x.byLw and x.open and not(x.feedback),
      personMoves(items.setBy(i, y => if (ISSUES.contains(i)) { ...y, reply: true }
        else { ...y, feedback: true }), ghPending),
    }
  }
  action personAddsSubIssue(c: int, p: int): bool = all {
    SUB_ISSUE_CHILDREN.contains(c), SUB_ISSUE_PARENTS.contains(p), c != p,
    items.get(c).exists, items.get(p).exists, items.get(c).parent == 0,
    personMoves(items.setBy(c, x => { ...x, parent: p, parentByLw: false }), ghPending),
  }
  action personRemovesSubIssue(c: int): bool = all {
    items.get(c).exists, items.get(c).parent != 0,
    SUB_ISSUE_PARENTS.contains(items.get(c).parent),
    personMoves(items.setBy(c, x => { ...x, parent: 0, parentByLw: false }), ghPending),
  }
  /// R ## Steering, "closes a pull request Linewright opened, without
  /// merging"; P "Closed without merging": "the maintainer's no".
  action personClosesPr(p: int): bool = {
    val x = items.get(p)
    all {
      person(p), PRS.contains(p), x.open,
      personMoves(x.closes.fold(items.set(p, { ...x, open: false, closedBy: "person" }), (acc, i) =>
        if (x.byLw) acc.setBy(i, y => { ...y, noSince: true }) else acc), ghPending),
    }
  }
  action personMergesPr(p: int): bool = all {
    person(p), PRS.contains(p), items.get(p).open,
    personMoves(merged(items, p, "person"), ghPending.union(mergePending(items, p))),
  }
  action personOpensPr(p: int): bool = all {
    PERSON_PRS.contains(p), not(items.get(p).exists),
    items.get(p).closes.forall(i => items.get(i).exists),
    personMoves(items.setBy(p, x => { ...x, exists: true, open: true }), ghPending),
  }

  action peopleStep: bool = {
    nondet i = ITEMS.oneOf()
    any {
      nondet l = STATUS_LABELS.oneOf()
      personAddsStatusLabel(i, l),
      personRemovesStatusLabel(i),
      personTogglesDesigned(i),
      personTogglesRequiredLabel(i),
      personAssignsLinewright(i),
      personUnassignsLinewright(i),
      personAssignsThemself(i),
      personUnassignsThemself(i),
      personClosesIssue(i),
      personReopensIssue(i),
      personComments(i),
      personRemovesSubIssue(i),
      personClosesPr(i),
      personMergesPr(i),
      personOpensPr(i),
      nondet p = ITEMS.oneOf()
      personAddsSubIssue(i, p),
    }
  }

  // ===========================================================================
  // Start and step
  // ===========================================================================

  /// Every start in STARTS. init leaves the flap ghost off; initFlaps keeps
  /// it, for S6_noFlap, whose run it makes four times as large. (Each
  /// assigns every variable itself: TLC cannot take an init that shares an
  /// action with another.)
  action init: bool = {
    nondet s = STARTS.oneOf()
    all {
      items' = s.items, requireLabel' = s.requireLabel, autoMerge' = s.autoMerge,
      runs' = ITEMS.mapBy(_ => NO_RUN), seq' = [], seqOf' = NO_OPTION,
      redo' = ITEMS.mapBy(_ => NO_REDO), clock' = 100,
      peopleLeft' = PEOPLE_ACTS, stopsLeft' = STOPS, ghPending' = Set(),
      stop' = NO_STOP, s4ok' = true, s7ok' = true, lwDid' = Set(), flapped' = false,
      trackFlaps' = false,
    }
  }
  action initFlaps: bool = {
    nondet s = STARTS.oneOf()
    all {
      items' = s.items, requireLabel' = s.requireLabel, autoMerge' = s.autoMerge,
      runs' = ITEMS.mapBy(_ => NO_RUN), seq' = [], seqOf' = NO_OPTION,
      redo' = ITEMS.mapBy(_ => NO_REDO), clock' = 100,
      peopleLeft' = PEOPLE_ACTS, stopsLeft' = STOPS, ghPending' = Set(),
      stop' = NO_STOP, s4ok' = true, s7ok' = true, lwDid' = Set(), flapped' = false,
      trackFlaps' = true,
    }
  }

  /// A given start, with `people` actions for people: for scenarios.
  action begin(s: Start, people: int): bool = all {
    items' = s.items, requireLabel' = s.requireLabel, autoMerge' = s.autoMerge,
    runs' = ITEMS.mapBy(_ => NO_RUN), seq' = [], seqOf' = NO_OPTION,
    redo' = ITEMS.mapBy(_ => NO_REDO), clock' = 100,
    peopleLeft' = people, stopsLeft' = STOPS, ghPending' = Set(),
    stop' = NO_STOP, s4ok' = true, s7ok' = true, lwDid' = Set(), flapped' = false,
    trackFlaps' = false,
  }

  pure val NO_STOP: Stop = { active: false, fresh: false, item: 0,
    end: { state: "", designed: false, lw: false }, personSince: false }

  action step: bool = any { lwStep, githubCloses, lookbackEnds, lwStops, peopleStep }

  /// Linewright and GitHub alone, people idle; stutters once Linewright has
  /// nothing to do. For scenarios.
  action settle: bool = any {
    lwStep, githubCloses,
    all {
      seq.length() == 0, options.size() == 0, ghPending.size() == 0,
      items' = items, ghPending' = ghPending, seq' = seq, seqOf' = seqOf, runs' = runs,
      redo' = redo, clock' = clock, peopleLeft' = peopleLeft, stopsLeft' = stopsLeft,
      requireLabel' = requireLabel, autoMerge' = autoMerge, stop' = stop,
      s4ok' = s4ok, s7ok' = s7ok, flapped' = flapped, trackFlaps' = trackFlaps, lwDid' = lwDid,
    },
  }

  // ---------------------------------------------------------------------------
  // For scenarios: Linewright takes a named option, or finishes its series
  // ---------------------------------------------------------------------------

  action keepAll: bool = all {
    items' = items, ghPending' = ghPending, seq' = seq, seqOf' = seqOf, runs' = runs,
    redo' = redo, clock' = clock, peopleLeft' = peopleLeft, stopsLeft' = stopsLeft,
    requireLabel' = requireLabel, autoMerge' = autoMerge, stop' = stop,
    s4ok' = s4ok, s7ok' = s7ok, flapped' = flapped, trackFlaps' = trackFlaps, lwDid' = lwDid,
  }

  /// Linewright takes the option `act` on `i`, which it must have.
  action does(act: str, i: int, arg: str): bool = all {
    seq.length() == 0,
    options.contains(opt(act, i, arg)),
    redoAllows(opt(act, i, arg)),
    take(opt(act, i, arg)),
  }

  /// Linewright makes its next write, if it is carrying out a series.
  action finishes: bool = any { lwContinue, all { seq.length() == 0, keepAll } }

  /// Whether Linewright may now start `stage` on `i`.
  def mayNowStart(i: int, stage: str): bool = options.contains(opt("start run", i, stage))

  // ===========================================================================
  // The board (DS ## States, ### The board, ### Cards)
  // ===========================================================================

  /// DS ## States: "An item's state is shown in these words."
  pure val WORDS: str -> str = Map(
    NEEDS_TRIAGE -> "Waiting for triage",
    "being triaged" -> "Being triaged",
    NEEDS_INFO -> "Waiting for an answer",
    READY_FOR_HUMAN -> "Waiting for a decision",
    READY_FOR_AGENT -> "Waiting for Linewright",
    "being designed" -> "Being designed",
    "being implemented" -> "Being implemented",
    "in review" -> "In review",
    "tracking" -> "Tracking",
    "done" -> "Done",
    "open" -> "Open",
    "merged" -> "Merged",
    "closed" -> "Closed",
  )

  /// M ## explain "State in JSON": "its name under State, in kebab-case".
  pure val JSON: str -> str = Map(
    NEEDS_TRIAGE -> "needs-triage", "being triaged" -> "being-triaged",
    NEEDS_INFO -> "needs-info", READY_FOR_HUMAN -> "ready-for-human",
    READY_FOR_AGENT -> "ready-for-agent", "being designed" -> "being-designed",
    "being implemented" -> "being-implemented", "in review" -> "in-review",
    "tracking" -> "tracking", "done" -> "done", "open" -> "open",
    "merged" -> "merged", "closed" -> "closed",
  )

  /// DS ### The board, the column table: (column, an issue's states, a pull
  /// request's states), in the board's order.
  pure val COLUMNS: List[(str, Set[str], Set[str])] = [
    ("Triage", Set(NEEDS_TRIAGE), Set()),
    ("Needs a person", Set(NEEDS_INFO, READY_FOR_HUMAN), Set()),
    ("Ready", Set(READY_FOR_AGENT), Set()),
    ("Working", Set("being triaged", "being designed", "being implemented"), Set()),
    ("In review", Set("in review"), Set("open")),
    ("Done", Set("done"), Set("merged", "closed")),
  ]

  pure def columnsOf(state: str, pr: bool): Set[str] =
    COLUMNS.foldl(Set(), (acc, c) =>
      if ((if (pr) c._3 else c._2).contains(state)) acc.union(Set(c._1)) else acc)
  pure def column(state: str, pr: bool): str =
    columnsOf(state, pr).fold("", (acc, c) => c)

  /// DS ### The board: "A tracking issue has no card." "One card per piece of
  /// work": "a pull request, and a run on it, show on the card of each issue
  /// it closes that has one, and a pull request that closes no issue with a
  /// card has its own."
  pure def placement(w: World, i: int): str =
    if (ISSUES.contains(i))
      if (issueState(w, i) == "tracking") "heads a lane" else "card"
    else if (it(w, i).closes.exists(j => managed(w, j) and issueState(w, j) != "tracking"))
      "on its issue's card"
    else "card"
  pure def hasOwnCard(w: World, i: int): bool = managed(w, i) and placement(w, i) == "card"

  /// DS ### Cards, the line in Ready (and Triage: "as in Ready"), the first
  /// that holds. Holds (the usage limit, the drain, max-runs) are not modelled.
  pure def readyLine(w: World, i: int): str = {
    val x = it(w, i)
    // "blocked by #12": "an open issue blocks it"
    if (ISSUES.contains(i) and blocked(w, i)) "blocked by"
    // "assigned to alice": "someone other than Linewright is assigned to it"
    else if (x.alice or x.asked) "assigned to alice"
    // "unassigned": "Linewright was unassigned from it" (A15)
    else if (x.unassignedSince) "unassigned"
    // "not marked for Linewright": "it is not marked for Linewright"
    else if (not(markedForLinewright(w, i))) "not marked for Linewright"
    // "nothing but the time": "otherwise: the next poll acts on it"
    else ""
  }

  /// DS ### Cards, the line in Needs a person: "stalled", "asked alice",
  /// "handed back", or "decision needed" otherwise.
  pure def needsAPersonLine(w: World, i: int): str = {
    val x = it(w, i)
    if (x.stalled) "stalled"
    else if (x.status == NEEDS_INFO) "asked"
    else if (x.letGo) "handed back"
    else "decision needed"
  }

  /// DS ### Cards, the line in Working.
  pure def workingLine(w: World, i: int): str =
    if (runInFlight(w, i)) "its stage"
    else if (ISSUES.contains(i) and blocked(w, i)) "blocked by"
    // "paused ...", "queued": "no run is in flight, as in Ready" (holds only)
    else ""

  pure def cardLine(w: World, i: int): str = {
    val col = column(stateOf(w, i), PRS.contains(i))
    if (col == "Triage" or col == "Ready") readyLine(w, i)
    else if (col == "Needs a person") needsAPersonLine(w, i)
    else if (col == "Working") workingLine(w, i)
    else ""
  }

  /// DS ### Cards, "What stops a pull request merging", its first rows.
  pure def stopsItMerging(w: World, p: int): str = {
    val x = it(w, p)
    if (x.stalled) "stalled"
    else if (x.byLw and not(x.lw) and x.letGo) "handed back"
    else if (x.byLw and not(x.lw)) "unassigned"
    else if (runInFlight(w, p)) "its stage"
    else ""
  }

  // ===========================================================================
  // Properties
  // ===========================================================================

  /// S1 Derivation: every item that exists has exactly one state, one its
  /// table names. The tables are first-match functions, so one row is its
  /// row; the witness runs in each model file show which rows are reachable.
  val S1_derivation: bool = ITEMS.forall(i => {
    val st = stateOf(world, i)
    not(items.get(i).exists)
      or (if (PRS.contains(i)) PR_STATES.contains(st) else st == "" or ISSUE_STATES.contains(st))
  })

  /// S2 Words and columns, the tables: every state has words and a name in
  /// JSON, and every state of each kind of item has exactly one column,
  /// tracking aside, which has none.
  pure val S2_tables: bool = and {
    ISSUE_STATES.union(PR_STATES).forall(s => WORDS.keys().contains(s) and JSON.keys().contains(s)),
    ISSUE_STATES.forall(s => s == "tracking" or columnsOf(s, false).size() == 1),
    PR_STATES.forall(s => columnsOf(s, true).size() == 1),
    columnsOf("tracking", false).size() == 0,
  }

  /// S2 Words and columns: every managed item with a state has exactly one
  /// column, or heads a lane (a tracking issue), as the board says.
  val S2_wordsAndColumns: bool = ITEMS.forall(i => managed(world, i) implies {
    val st = stateOf(world, i)
    val p = placement(world, i)
    and {
      WORDS.keys().contains(st),
      p == "card" implies columnsOf(st, PRS.contains(i)).size() == 1,
      p == "heads a lane" implies st == "tracking",
    }
  })

  /// Linewright has something to do on `i` now: an option on it, or a write
  /// of the series it is carrying out.
  def lwActsOn(i: int): bool =
    optionsOn(world, i).size() > 0 or seq.select(wt => wt.item == i).length() > 0

  /// S3 No card waits on nobody: for every managed item in Triage, Needs a
  /// person, Ready or Working, its card line either says why it waits, or
  /// Linewright has an action enabled on it now.
  val S3_noCardWaitsOnNobody: bool = ITEMS.forall(i => {
    val col = column(stateOf(world, i), PRS.contains(i))
    (hasOwnCard(world, i) and Set("Triage", "Needs a person", "Ready", "Working").contains(col))
      implies (cardLine(world, i) != "" or lwActsOn(i))
  })

  /// S4 Work follows state, judged at each claim and run start.
  val S4_workFollowsState: bool = s4ok

  /// S7 People's items are theirs, judged at each close and detach.
  val S7_peoplesItemsAreTheirs: bool = s7ok

  /// Linewright has nothing more to do.
  val quiescent: bool = and {
    seq.length() == 0, options.size() == 0, ghPending.size() == 0,
    ITEMS.forall(i => not(runInFlight(world, i))),
  }

  /// S6's resting places: left alone, every item comes to rest: done;
  /// waiting on a person (an answer, a decision, a review or merge, or a label
  /// or assignment after they stopped Linewright); or outside Linewright's
  /// work.
  pure def rests(w: World, i: int): bool = {
    val st = stateOf(w, i)
    val x = it(w, i)
    or {
      // done
      Set("done", "merged", "closed").contains(st),
      // waiting on a person: an answer, a decision, a review or merge
      Set(NEEDS_INFO, READY_FOR_HUMAN, "in review", "open").contains(st),
      // a label or assignment after they stopped Linewright: unassigned it,
      // or took the status label away
      ASKS_FOR_LINEWRIGHTS_WORK.contains(st) and not(x.lw) and x.unassignedSince,
      st == "",
      // outside Linewright's work: not marked for it, assigned to someone
      // else or to a person it asked, or blocked
      ISSUES.contains(i) and (not(markedForLinewright(w, i)) or x.alice or x.asked or blocked(w, i)),
    }
  }

  /// A tracking issue comes to rest with its tickets.
  pure def restsWithTickets(w: World, i: int): bool = or {
    rests(w, i),
    stateOf(w, i) == "tracking" and ticketsCutFrom(w, i).forall(t => rests(w, t)),
  }

  /// S6 Progress, the safety half: wherever Linewright has nothing more to
  /// do, every managed item rests in one of S6's places.
  val S6_restsWhereQuiet: bool =
    quiescent implies ITEMS.forall(i => managed(world, i) implies restsWithTickets(world, i))

  /// S6, nothing flaps: with people idle, Linewright never repeats a write it
  /// has undone.
  val S6_noFlap: bool = not(flapped)

  val vars = (items, runs, seq, seqOf, redo, clock, peopleLeft, stopsLeft, requireLabel,
              autoMerge, ghPending, stop, s4ok, s7ok, lwDid, flapped, trackFlaps)

  /// S6 Progress, the temporal half: under weak fairness of Linewright's
  /// steps (and GitHub's), Linewright eventually has nothing more to do, for
  /// good. People's actions are bounded, so they end.
  temporal S6_progress: bool =
    (weakFair(lwStep, vars) and weakFair(githubCloses, vars)) implies eventually(always(quiescent))

  /// S5 Crash-safety: if Linewright stops between two writes and people
  /// leave the item alone, it finishes as the whole series would have, or the
  /// item comes to rest as S6 says. The end the series would have reached is
  /// the item's state and, while it is open, its linewright:designed and
  /// Linewright's assignment.
  temporal S5_crashSafety: bool =
    (weakFair(lwStep, vars) and weakFair(githubCloses, vars)) implies
      always(stop.fresh implies eventually(or {
        // (read first, for TLC, which reads the rest only for an active stop)
        not(stop.active),
        stop.personSince,
        sig(world, stop.item) == stop.end,
        restsWithTickets(world, stop.item),
      }))

  /// The invariants for one TLC run: all but S6's flaps, checked from
  /// initFlaps.
  val invariants: bool = and {
    S1_derivation, S2_wordsAndColumns, S3_noCardWaitsOnNobody,
    S4_workFollowsState, S6_restsWhereQuiet, S7_peoplesItemsAreTheirs,
  }
}
