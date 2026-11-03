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
 * Encoded in full: R ## State, R ## Working an item, R ## Managed items, R ##
 * Writes, P ## Working a pull request, DS ## States. Their tables are read by
 * the spec-table directives (`scripts/fix model-tables` generates what each
 * encodes; scripts/model_tables.py describes them) and every other sentence of
 * theirs is quoted where the model encodes it.
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

  /// A run in flight: its stage, the state it started in, whether it is
  /// applying its result and, only while the Working an item rows are read for
  /// how it ends, the outcome it ends with.
  type Run = { stage: str, start: str, applying: bool, ending: str }
  pure val NO_RUN: Run = { stage: "", start: "", applying: false, ending: "" }

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
  /// a time (R ## Writes "One writer": "One of Linewright's runs or actions
  /// writes an item at a time.", A2).
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
  // State (R ## State)
  // ===========================================================================

  /// R ## State: "An item's state is the first row of its table below that
  /// holds, derived from what GitHub shows and, while a run is in flight on the
  /// item, from the run."
  // spec-table R "## State" state
  pure def issueState(w: World, i: int): str =
    // is closed, or has had a pull request that closes it merge into the default branch since it was last opened or given a status label | done
    if (when_is_closed_or_has_had_a_pull_request_that_closes_it_merge_into_the_default_branch_since_it_was_last_opened_or_given_a_status_label(w, i)) "done"
    // has an open pull request that closes it | in review
    else if (when_has_an_open_pull_request_that_closes_it(w, i)) "in review"
    // is a tracking issue | tracking
    else if (when_is_a_tracking_issue(w, i)) "tracking"
    // holds needs-triage and is assigned to Linewright | being triaged
    else if (when_holds_needs_triage_and_is_assigned_to_linewright(w, i)) "being triaged"
    // holds ready-for-agent, is assigned to Linewright and is without linewright:designed | being designed
    else if (when_holds_ready_for_agent_is_assigned_to_linewright_and_is_without_linewright_designed(w, i)) "being designed"
    // holds ready-for-agent and is assigned to Linewright | being implemented
    else if (when_holds_ready_for_agent_and_is_assigned_to_linewright(w, i)) "being implemented"
    // holds a status label | its status label's, such as needs-info
    else if (when_holds_a_status_label(w, i)) result_its_status_label_s_such_as_needs_info(w, i)
    // has a run in flight | the one it was in when the run started
    else if (when_has_a_run_in_flight(w, i)) result_the_one_it_was_in_when_the_run_started(w, i)
    // otherwise | no state
    else ""
  // spec-table end

  // spec-table R "## State" 2 state
  pure def prState(w: World, p: int): str =
    // is open | open
    if (when_is_open(w, p)) "open"
    // is merged | merged
    else if (when_is_merged(w, p)) "merged"
    // is closed without merging | closed
    else if (when_is_closed_without_merging(w, p)) "closed"
    else ""
  // spec-table end

  pure def stateOf(w: World, i: int): str =
    if (not(it(w, i).exists)) ""
    else if (PRS.contains(i)) prState(w, i)
    else issueState(w, i)

  pure def when_is_closed_or_has_had_a_pull_request_that_closes_it_merge_into_the_default_branch_since_it_was_last_opened_or_given_a_status_label(w: World, i: int): bool =
    not(it(w, i).open) or it(w, i).mergedSince
  pure def when_has_an_open_pull_request_that_closes_it(w: World, i: int): bool = openPrClosesIt(w, i)
  pure def when_is_a_tracking_issue(w: World, i: int): bool = isTrackingIssue(w, i)
  pure def when_holds_needs_triage_and_is_assigned_to_linewright(w: World, i: int): bool =
    it(w, i).status == NEEDS_TRIAGE and it(w, i).lw
  pure def when_holds_ready_for_agent_is_assigned_to_linewright_and_is_without_linewright_designed(w: World, i: int): bool =
    it(w, i).status == READY_FOR_AGENT and it(w, i).lw and not(it(w, i).designed)
  pure def when_holds_ready_for_agent_and_is_assigned_to_linewright(w: World, i: int): bool =
    it(w, i).status == READY_FOR_AGENT and it(w, i).lw
  pure def when_holds_a_status_label(w: World, i: int): bool = it(w, i).status != ""
  pure def result_its_status_label_s_such_as_needs_info(w: World, i: int): str = it(w, i).status
  pure def when_has_a_run_in_flight(w: World, i: int): bool = runInFlight(w, i)
  pure def result_the_one_it_was_in_when_the_run_started(w: World, i: int): str = w.runs.get(i).start
  pure def when_is_open(w: World, p: int): bool = it(w, p).open
  pure def when_is_merged(w: World, p: int): bool = it(w, p).merged
  pure def when_is_closed_without_merging(w: World, p: int): bool =
    not(it(w, p).open) and not(it(w, p).merged)

  /// Every state the tables name.
  pure val ISSUE_STATES = Set("done", "in review", "tracking", "being triaged",
    "being designed", "being implemented", NEEDS_TRIAGE, NEEDS_INFO,
    READY_FOR_AGENT, READY_FOR_HUMAN)
  pure val PR_STATES = Set("open", "merged", "closed")

  /// The row of each table that holds, for S1's witnesses.
  // spec-table R "## State" index
  pure def issueRow(w: World, i: int): int =
    // is closed, or has had a pull request that closes it merge into the default branch since it was last opened or given a status label | done
    if (when_is_closed_or_has_had_a_pull_request_that_closes_it_merge_into_the_default_branch_since_it_was_last_opened_or_given_a_status_label(w, i)) 1
    // has an open pull request that closes it | in review
    else if (when_has_an_open_pull_request_that_closes_it(w, i)) 2
    // is a tracking issue | tracking
    else if (when_is_a_tracking_issue(w, i)) 3
    // holds needs-triage and is assigned to Linewright | being triaged
    else if (when_holds_needs_triage_and_is_assigned_to_linewright(w, i)) 4
    // holds ready-for-agent, is assigned to Linewright and is without linewright:designed | being designed
    else if (when_holds_ready_for_agent_is_assigned_to_linewright_and_is_without_linewright_designed(w, i)) 5
    // holds ready-for-agent and is assigned to Linewright | being implemented
    else if (when_holds_ready_for_agent_and_is_assigned_to_linewright(w, i)) 6
    // holds a status label | its status label's, such as needs-info
    else if (when_holds_a_status_label(w, i)) 7
    // has a run in flight | the one it was in when the run started
    else if (when_has_a_run_in_flight(w, i)) 8
    // otherwise | no state
    else 9
  // spec-table end
  // spec-table R "## State" 2 index
  pure def prRow(w: World, p: int): int =
    // is open | open
    if (when_is_open(w, p)) 1
    // is merged | merged
    else if (when_is_merged(w, p)) 2
    // is closed without merging | closed
    else if (when_is_closed_without_merging(w, p)) 3
    else 0
  // spec-table end

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

  /// R ## Steering: a person "changes or removes the issue's status label" |
  /// "cancels a run the label no longer asks for"; "closes the issue",
  /// "merges or closes a pull request" or "unassigns Linewright" | "cancels a
  /// run on it". A3: ready-for-agent asks for both the design and the
  /// implement run.
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
  /// until its status label or the required label is applied again. A hold
  /// holds claims as under Holds." A7: a pull request it opened, on the same
  /// terms as an issue. Holds are not modelled: none is ever in force.
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

  /// R ## Working an item: "Linewright changes an item's labels and
  /// assignees, and starts work on it, only as this table gives: at each poll,
  /// the first row that holds for the item." What it does on an item no run
  /// is ending on: the first row that holds of those that do not end a run,
  /// then of R ## Steering's; `rowState` gives the State column's state.
  // spec-table R "## Working an item" rows keep=endsNoRun else=steering
  pure def working(w: World, i: int): Option =
    {
      val state = rowState(w, i)
      // any | the issue has tickets cut from it among its sub-issues and is without linewright:designed | discards the cut | as State derives
      if (when_the_issue_has_tickets_cut_from_it_among_its_sub_issues_and_is_without_linewright_designed(w, i) and endsNoRun(w, i, does_discards_the_cut(w, i))) does_discards_the_cut(w, i)
      // being designed, being implemented | a person has closed without merging a pull request Linewright opened that closes it, since the issue was last opened or given a status label | hands it back | ready-for-human
      else if (Set("being designed", "being implemented").contains(state) and when_a_person_has_closed_without_merging_a_pull_request_linewright_opened_that_closes_it_since_the_issue_was_last_opened_or_given_a_status_label(w, i) and endsNoRun(w, i, does_hands_it_back(w, i))) does_hands_it_back(w, i)
      // needs-info, ready-for-human, no state | Linewright is assigned to the issue | unassigns itself, with a comment saying what the issue needs first where a person assigned it | unchanged
      else if (Set("needs-info", "ready-for-human", "").contains(state) and when_linewright_is_assigned_to_the_issue(w, i) and endsNoRun(w, i, does_unassigns_itself_with_a_comment_saying_what_the_issue_needs_first_where_a_person_assigned_it(w, i))) does_unassigns_itself_with_a_comment_saying_what_the_issue_needs_first_where_a_person_assigned_it(w, i)
      // needs-triage, ready-for-agent | a person Linewright asked is assigned to it | unassigns them | unchanged
      else if (Set("needs-triage", "ready-for-agent").contains(state) and when_a_person_linewright_asked_is_assigned_to_it(w, i) and endsNoRun(w, i, does_unassigns_them(w, i))) does_unassigns_them(w, i)
      // tracking | it holds a status label or Linewright's assignment | removes the status label, then unassigns itself | tracking
      else if (Set("tracking").contains(state) and when_it_holds_a_status_label_or_linewright_s_assignment(w, i) and endsNoRun(w, i, does_removes_the_status_label_then_unassigns_itself(w, i))) does_removes_the_status_label_then_unassigns_itself(w, i)
      // tracking | it has sub-issues and every one is closed | closes it | done
      else if (Set("tracking").contains(state) and when_it_has_sub_issues_and_every_one_is_closed(w, i) and endsNoRun(w, i, does_closes_it(w, i))) does_closes_it(w, i)
      // needs-info | a reply has come | labels it needs-triage | needs-triage
      else if (Set("needs-info").contains(state) and when_a_reply_has_come(w, i) and endsNoRun(w, i, does_labels_it_needs_triage(w, i))) does_labels_it_needs_triage(w, i)
      // needs-triage, ready-for-agent | Linewright may claim it | assigns itself | being triaged, being designed, or being implemented where it carries linewright:designed
      else if (Set("needs-triage", "ready-for-agent").contains(state) and when_linewright_may_claim_it(w, i) and endsNoRun(w, i, does_assigns_itself(w, i))) does_assigns_itself(w, i)
      // any but done, merged and closed | it carries linewright:stalled and Linewright is assigned to it | removes linewright:stalled | unchanged
      else if (not(Set("done", "merged", "closed").contains(state)) and when_it_carries_linewright_stalled_and_linewright_is_assigned_to_it(w, i) and endsNoRun(w, i, does_removes_linewright_stalled(w, i))) does_removes_linewright_stalled(w, i)
      // being triaged | no run is in flight on it | starts a triage run | being triaged
      else if (Set("being triaged").contains(state) and when_no_run_is_in_flight_on_it(w, i) and endsNoRun(w, i, does_starts_a_triage_run(w, i))) does_starts_a_triage_run(w, i)
      // being triaged | the triage run ends with ready-for-agent | labels it ready-for-agent | being designed, or being implemented where it carries linewright:designed
      else if (Set("being triaged").contains(state) and when_the_triage_run_ends_with_ready_for_agent(w, i) and endsNoRun(w, i, does_labels_it_ready_for_agent(w, i))) does_labels_it_ready_for_agent(w, i)
      // being triaged | the triage run ends with needs-info | labels it needs-info, then assigns the people asked whom GitHub lets it assign | needs-info
      else if (Set("being triaged").contains(state) and when_the_triage_run_ends_with_needs_info(w, i) and endsNoRun(w, i, does_labels_it_needs_info_then_assigns_the_people_asked_whom_github_lets_it_assign(w, i))) does_labels_it_needs_info_then_assigns_the_people_asked_whom_github_lets_it_assign(w, i)
      // being triaged | the triage run ends with ready-for-human | labels it ready-for-human | ready-for-human
      else if (Set("being triaged").contains(state) and when_the_triage_run_ends_with_ready_for_human(w, i) and endsNoRun(w, i, does_labels_it_ready_for_human(w, i))) does_labels_it_ready_for_human(w, i)
      // being designed | no run is in flight on it | starts a design run | being designed
      else if (Set("being designed").contains(state) and when_no_run_is_in_flight_on_it(w, i) and endsNoRun(w, i, does_starts_a_design_run(w, i))) does_starts_a_design_run(w, i)
      // being designed | the design run builds it as it is | labels it linewright:designed | being implemented
      else if (Set("being designed").contains(state) and when_the_design_run_builds_it_as_it_is(w, i) and endsNoRun(w, i, does_labels_it_linewright_designed(w, i))) does_labels_it_linewright_designed(w, i)
      // being designed | the design run cuts it into tickets | creates the tickets, then labels it linewright:designed | tracking
      else if (Set("being designed").contains(state) and when_the_design_run_cuts_it_into_tickets(w, i) and endsNoRun(w, i, does_creates_the_tickets_then_labels_it_linewright_designed(w, i))) does_creates_the_tickets_then_labels_it_linewright_designed(w, i)
      // being designed | the design run finds nothing to build, in an issue Linewright created | closes it as completed | done
      else if (Set("being designed").contains(state) and when_the_design_run_finds_nothing_to_build_in_an_issue_linewright_created(w, i) and endsNoRun(w, i, does_closes_it_as_completed(w, i))) does_closes_it_as_completed(w, i)
      // being designed | the design run finds nothing to build in any other issue, or cannot make a design | hands it back | ready-for-human
      else if (Set("being designed").contains(state) and when_the_design_run_finds_nothing_to_build_in_any_other_issue_or_cannot_make_a_design(w, i) and endsNoRun(w, i, does_hands_it_back(w, i))) does_hands_it_back(w, i)
      // being implemented | no run is in flight on it | starts an implement run | being implemented
      else if (Set("being implemented").contains(state) and when_no_run_is_in_flight_on_it(w, i) and endsNoRun(w, i, does_starts_an_implement_run(w, i))) does_starts_an_implement_run(w, i)
      // being implemented | the implement run builds it | opens a pull request that closes it | in review
      else if (Set("being implemented").contains(state) and when_the_implement_run_builds_it(w, i) and endsNoRun(w, i, does_opens_a_pull_request_that_closes_it(w, i))) does_opens_a_pull_request_that_closes_it(w, i)
      // being triaged, being designed, being implemented | its run cannot finish | hands it back | ready-for-human
      else if (Set("being triaged", "being designed", "being implemented").contains(state) and when_its_run_cannot_finish(w, i) and endsNoRun(w, i, does_hands_it_back(w, i))) does_hands_it_back(w, i)
      // merged | a spec change Linewright may take up | opens its issue | merged
      else if (Set("merged").contains(state) and when_a_spec_change_linewright_may_take_up(w, i) and endsNoRun(w, i, does_opens_its_issue(w, i))) does_opens_its_issue(w, i)
      // open | a pull request Linewright opened, that it may claim | assigns itself | open
      else if (Set("open").contains(state) and when_a_pull_request_linewright_opened_that_it_may_claim(w, i) and endsNoRun(w, i, does_assigns_itself(w, i))) does_assigns_itself(w, i)
      // open | a pull request Linewright opened and is assigned to, whose first row of Working a pull request that holds does more than wait | does what that row gives | open, or merged
      else if (Set("open").contains(state) and when_a_pull_request_linewright_opened_and_is_assigned_to_whose_first_row_of_working_a_pull_request_that_holds_does_more_than_wait(w, i) and endsNoRun(w, i, does_does_what_that_row_gives(w, i))) does_does_what_that_row_gives(w, i)
      // open | a run on it cannot finish | hands it back | open
      else if (Set("open").contains(state) and when_a_run_on_it_cannot_finish(w, i) and endsNoRun(w, i, does_hands_it_back(w, i))) does_hands_it_back(w, i)
      // any but done | the issue carries more than one status label | removes all but the newest | unchanged
      else if (not(Set("done").contains(state)) and when_the_issue_carries_more_than_one_status_label(w, i) and endsNoRun(w, i, does_removes_all_but_the_newest(w, i))) does_removes_all_but_the_newest(w, i)
      else steering(w, i)
    }
  // spec-table end

  /// "A run in flight": "While a run is in flight on an item, only the rows
  /// for how that run ends hold for it."
  // spec-table R "## Working an item" rows keep=endsTheRun else=nothing
  pure def runEnding(w: World, i: int): Option =
    {
      val state = rowState(w, i)
      // any | the issue has tickets cut from it among its sub-issues and is without linewright:designed | discards the cut | as State derives
      if (when_the_issue_has_tickets_cut_from_it_among_its_sub_issues_and_is_without_linewright_designed(w, i) and endsTheRun(w, i, does_discards_the_cut(w, i))) does_discards_the_cut(w, i)
      // being designed, being implemented | a person has closed without merging a pull request Linewright opened that closes it, since the issue was last opened or given a status label | hands it back | ready-for-human
      else if (Set("being designed", "being implemented").contains(state) and when_a_person_has_closed_without_merging_a_pull_request_linewright_opened_that_closes_it_since_the_issue_was_last_opened_or_given_a_status_label(w, i) and endsTheRun(w, i, does_hands_it_back(w, i))) does_hands_it_back(w, i)
      // needs-info, ready-for-human, no state | Linewright is assigned to the issue | unassigns itself, with a comment saying what the issue needs first where a person assigned it | unchanged
      else if (Set("needs-info", "ready-for-human", "").contains(state) and when_linewright_is_assigned_to_the_issue(w, i) and endsTheRun(w, i, does_unassigns_itself_with_a_comment_saying_what_the_issue_needs_first_where_a_person_assigned_it(w, i))) does_unassigns_itself_with_a_comment_saying_what_the_issue_needs_first_where_a_person_assigned_it(w, i)
      // needs-triage, ready-for-agent | a person Linewright asked is assigned to it | unassigns them | unchanged
      else if (Set("needs-triage", "ready-for-agent").contains(state) and when_a_person_linewright_asked_is_assigned_to_it(w, i) and endsTheRun(w, i, does_unassigns_them(w, i))) does_unassigns_them(w, i)
      // tracking | it holds a status label or Linewright's assignment | removes the status label, then unassigns itself | tracking
      else if (Set("tracking").contains(state) and when_it_holds_a_status_label_or_linewright_s_assignment(w, i) and endsTheRun(w, i, does_removes_the_status_label_then_unassigns_itself(w, i))) does_removes_the_status_label_then_unassigns_itself(w, i)
      // tracking | it has sub-issues and every one is closed | closes it | done
      else if (Set("tracking").contains(state) and when_it_has_sub_issues_and_every_one_is_closed(w, i) and endsTheRun(w, i, does_closes_it(w, i))) does_closes_it(w, i)
      // needs-info | a reply has come | labels it needs-triage | needs-triage
      else if (Set("needs-info").contains(state) and when_a_reply_has_come(w, i) and endsTheRun(w, i, does_labels_it_needs_triage(w, i))) does_labels_it_needs_triage(w, i)
      // needs-triage, ready-for-agent | Linewright may claim it | assigns itself | being triaged, being designed, or being implemented where it carries linewright:designed
      else if (Set("needs-triage", "ready-for-agent").contains(state) and when_linewright_may_claim_it(w, i) and endsTheRun(w, i, does_assigns_itself(w, i))) does_assigns_itself(w, i)
      // any but done, merged and closed | it carries linewright:stalled and Linewright is assigned to it | removes linewright:stalled | unchanged
      else if (not(Set("done", "merged", "closed").contains(state)) and when_it_carries_linewright_stalled_and_linewright_is_assigned_to_it(w, i) and endsTheRun(w, i, does_removes_linewright_stalled(w, i))) does_removes_linewright_stalled(w, i)
      // being triaged | no run is in flight on it | starts a triage run | being triaged
      else if (Set("being triaged").contains(state) and when_no_run_is_in_flight_on_it(w, i) and endsTheRun(w, i, does_starts_a_triage_run(w, i))) does_starts_a_triage_run(w, i)
      // being triaged | the triage run ends with ready-for-agent | labels it ready-for-agent | being designed, or being implemented where it carries linewright:designed
      else if (Set("being triaged").contains(state) and when_the_triage_run_ends_with_ready_for_agent(w, i) and endsTheRun(w, i, does_labels_it_ready_for_agent(w, i))) does_labels_it_ready_for_agent(w, i)
      // being triaged | the triage run ends with needs-info | labels it needs-info, then assigns the people asked whom GitHub lets it assign | needs-info
      else if (Set("being triaged").contains(state) and when_the_triage_run_ends_with_needs_info(w, i) and endsTheRun(w, i, does_labels_it_needs_info_then_assigns_the_people_asked_whom_github_lets_it_assign(w, i))) does_labels_it_needs_info_then_assigns_the_people_asked_whom_github_lets_it_assign(w, i)
      // being triaged | the triage run ends with ready-for-human | labels it ready-for-human | ready-for-human
      else if (Set("being triaged").contains(state) and when_the_triage_run_ends_with_ready_for_human(w, i) and endsTheRun(w, i, does_labels_it_ready_for_human(w, i))) does_labels_it_ready_for_human(w, i)
      // being designed | no run is in flight on it | starts a design run | being designed
      else if (Set("being designed").contains(state) and when_no_run_is_in_flight_on_it(w, i) and endsTheRun(w, i, does_starts_a_design_run(w, i))) does_starts_a_design_run(w, i)
      // being designed | the design run builds it as it is | labels it linewright:designed | being implemented
      else if (Set("being designed").contains(state) and when_the_design_run_builds_it_as_it_is(w, i) and endsTheRun(w, i, does_labels_it_linewright_designed(w, i))) does_labels_it_linewright_designed(w, i)
      // being designed | the design run cuts it into tickets | creates the tickets, then labels it linewright:designed | tracking
      else if (Set("being designed").contains(state) and when_the_design_run_cuts_it_into_tickets(w, i) and endsTheRun(w, i, does_creates_the_tickets_then_labels_it_linewright_designed(w, i))) does_creates_the_tickets_then_labels_it_linewright_designed(w, i)
      // being designed | the design run finds nothing to build, in an issue Linewright created | closes it as completed | done
      else if (Set("being designed").contains(state) and when_the_design_run_finds_nothing_to_build_in_an_issue_linewright_created(w, i) and endsTheRun(w, i, does_closes_it_as_completed(w, i))) does_closes_it_as_completed(w, i)
      // being designed | the design run finds nothing to build in any other issue, or cannot make a design | hands it back | ready-for-human
      else if (Set("being designed").contains(state) and when_the_design_run_finds_nothing_to_build_in_any_other_issue_or_cannot_make_a_design(w, i) and endsTheRun(w, i, does_hands_it_back(w, i))) does_hands_it_back(w, i)
      // being implemented | no run is in flight on it | starts an implement run | being implemented
      else if (Set("being implemented").contains(state) and when_no_run_is_in_flight_on_it(w, i) and endsTheRun(w, i, does_starts_an_implement_run(w, i))) does_starts_an_implement_run(w, i)
      // being implemented | the implement run builds it | opens a pull request that closes it | in review
      else if (Set("being implemented").contains(state) and when_the_implement_run_builds_it(w, i) and endsTheRun(w, i, does_opens_a_pull_request_that_closes_it(w, i))) does_opens_a_pull_request_that_closes_it(w, i)
      // being triaged, being designed, being implemented | its run cannot finish | hands it back | ready-for-human
      else if (Set("being triaged", "being designed", "being implemented").contains(state) and when_its_run_cannot_finish(w, i) and endsTheRun(w, i, does_hands_it_back(w, i))) does_hands_it_back(w, i)
      // merged | a spec change Linewright may take up | opens its issue | merged
      else if (Set("merged").contains(state) and when_a_spec_change_linewright_may_take_up(w, i) and endsTheRun(w, i, does_opens_its_issue(w, i))) does_opens_its_issue(w, i)
      // open | a pull request Linewright opened, that it may claim | assigns itself | open
      else if (Set("open").contains(state) and when_a_pull_request_linewright_opened_that_it_may_claim(w, i) and endsTheRun(w, i, does_assigns_itself(w, i))) does_assigns_itself(w, i)
      // open | a pull request Linewright opened and is assigned to, whose first row of Working a pull request that holds does more than wait | does what that row gives | open, or merged
      else if (Set("open").contains(state) and when_a_pull_request_linewright_opened_and_is_assigned_to_whose_first_row_of_working_a_pull_request_that_holds_does_more_than_wait(w, i) and endsTheRun(w, i, does_does_what_that_row_gives(w, i))) does_does_what_that_row_gives(w, i)
      // open | a run on it cannot finish | hands it back | open
      else if (Set("open").contains(state) and when_a_run_on_it_cannot_finish(w, i) and endsTheRun(w, i, does_hands_it_back(w, i))) does_hands_it_back(w, i)
      // any but done | the issue carries more than one status label | removes all but the newest | unchanged
      else if (not(Set("done").contains(state)) and when_the_issue_carries_more_than_one_status_label(w, i) and endsTheRun(w, i, does_removes_all_but_the_newest(w, i))) does_removes_all_but_the_newest(w, i)
      else nothing(w, i)
    }
  // spec-table end
  pure def endsTheRun(w: World, i: int, o: Option): bool = o.act == "end run"
  pure def endsNoRun(w: World, i: int, o: Option): bool = o.act != "end run"
  pure def nothing(w: World, i: int): Option = NO_OPTION

  /// The state the rows' State column reads: while the rows are read for how
  /// a run ends, its stage's state (A5).
  pure def rowState(w: World, i: int): str = {
    val r = w.runs.get(i)
    if (r.ending != "") stateOfStage(r.stage) else stateOf(w, i)
  }
  pure def stateOfStage(stage: str): str =
    if (stage == "triage") "being triaged"
    else if (stage == "design") "being designed"
    else if (stage == "implement") "being implemented"
    else "open"

  /// The world as the rows read it for the run on `i` ending with `o`.
  pure def withEnding(w: World, i: int, o: str): World =
    { ...w, runs: w.runs.set(i, { ...w.runs.get(i), ending: o }) }
  pure def ending(w: World, i: int): str = w.runs.get(i).ending
  pure def endsWith(w: World, i: int, stage: str, o: str): bool =
    w.runs.get(i).stage == stage and ending(w, i) == o
  /// What Linewright does as a run ends: the writes of its outcome
  /// (resultWrites).
  pure def endRun(w: World, i: int): Option = opt("end run", i, ending(w, i))

  pure def when_the_issue_has_tickets_cut_from_it_among_its_sub_issues_and_is_without_linewright_designed(w: World, i: int): bool =
    owesDiscard(w, i)
  pure def when_a_person_has_closed_without_merging_a_pull_request_linewright_opened_that_closes_it_since_the_issue_was_last_opened_or_given_a_status_label(w: World, i: int): bool =
    it(w, i).noSince
  pure def when_linewright_is_assigned_to_the_issue(w: World, i: int): bool = it(w, i).lw
  pure def when_a_person_linewright_asked_is_assigned_to_it(w: World, i: int): bool = it(w, i).asked
  pure def when_it_holds_a_status_label_or_linewright_s_assignment(w: World, i: int): bool =
    it(w, i).status != "" or it(w, i).older != "" or it(w, i).lw
  pure def when_it_has_sub_issues_and_every_one_is_closed(w: World, i: int): bool =
    subIssues(w, i).size() > 0 and subIssues(w, i).forall(t => not(it(w, t).open))
  pure def when_a_reply_has_come(w: World, i: int): bool = it(w, i).reply
  pure def when_linewright_may_claim_it(w: World, i: int): bool = claimsOnItsOwn(w, i)
  /// "Blocked": "No row that starts a run holds for an issue while an open
  /// issue blocks it"; and only where the bounds give the run an outcome to
  /// end with.
  pure def when_no_run_is_in_flight_on_it(w: World, i: int): bool =
    not(runInFlight(w, i)) and not(blocked(w, i)) and outcomesOf(w, i, stageOf(stateOf(w, i))).size() > 0
  pure def when_the_triage_run_ends_with_ready_for_agent(w: World, i: int): bool =
    endsWith(w, i, "triage", READY_FOR_AGENT)
  pure def when_the_triage_run_ends_with_needs_info(w: World, i: int): bool =
    endsWith(w, i, "triage", NEEDS_INFO)
  pure def when_the_triage_run_ends_with_ready_for_human(w: World, i: int): bool =
    endsWith(w, i, "triage", READY_FOR_HUMAN)
  pure def when_the_design_run_builds_it_as_it_is(w: World, i: int): bool = endsWith(w, i, "design", "uncut")
  pure def when_the_design_run_cuts_it_into_tickets(w: World, i: int): bool = endsWith(w, i, "design", "cut")
  pure def when_the_design_run_finds_nothing_to_build_in_an_issue_linewright_created(w: World, i: int): bool =
    endsWith(w, i, "design", "nothing to build") and it(w, i).byLw
  pure def when_the_design_run_finds_nothing_to_build_in_any_other_issue_or_cannot_make_a_design(w: World, i: int): bool =
    (endsWith(w, i, "design", "nothing to build") and not(it(w, i).byLw)) or endsWith(w, i, "design", "handed back")
  pure def when_the_implement_run_builds_it(w: World, i: int): bool = endsWith(w, i, "implement", "pr")
  /// A stall (U ## Ending).
  pure def when_its_run_cannot_finish(w: World, i: int): bool = ending(w, i) == "stall"
  pure def when_a_run_on_it_cannot_finish(w: World, p: int): bool = ending(w, p) == "stall"
  /// S "Taking it up": "Linewright may take up a spec change that is not yet
  /// taken up, merged within the lookback window and marked for it", and that
  /// the bounds give an issue.
  pure def when_a_spec_change_linewright_may_take_up(w: World, p: int): bool =
    isSpecChange(w, p) and not(takenUp(w, p)) and SPEC_ISSUE_SLOT.keys().contains(p)
      and it(w, p).inLookback and markedForLinewright(w, p)
  /// "Claiming": "It claims a pull request it opened" (A7).
  pure def when_a_pull_request_linewright_opened_that_it_may_claim(w: World, p: int): bool =
    it(w, p).byLw and claimsOnItsOwn(w, p)
  /// Of Working a pull request, only its rework row, "feedback has come" |
  /// "starts a rework run", its merge row with auto-merge, and the rework run's
  /// end; pull_request.qnt models that table in full.
  pure def when_a_pull_request_linewright_opened_and_is_assigned_to_whose_first_row_of_working_a_pull_request_that_holds_does_more_than_wait(w: World, p: int): bool =
    it(w, p).byLw and it(w, p).lw
      and (if (ending(w, p) != "") ending(w, p) == "done" else it(w, p).feedback or w.autoMerge)
  pure def when_the_issue_carries_more_than_one_status_label(w: World, i: int): bool = it(w, i).older != ""
  pure def when_it_carries_linewright_stalled_and_linewright_is_assigned_to_it(w: World, i: int): bool =
    it(w, i).stalled and it(w, i).lw

  pure def does_discards_the_cut(w: World, i: int): Option = opt("discard", i, "")
  /// A run's end hands back by its outcome; otherwise the pull request closed
  /// unmerged.
  pure def does_hands_it_back(w: World, i: int): Option =
    if (ending(w, i) != "") endRun(w, i) else opt("hand back", i, "closed unmerged")
  pure def does_unassigns_itself_with_a_comment_saying_what_the_issue_needs_first_where_a_person_assigned_it(w: World, i: int): Option =
    opt("let go", i, "")
  pure def does_unassigns_them(w: World, i: int): Option = opt("unassign the people asked", i, "")
  pure def does_removes_the_status_label_then_unassigns_itself(w: World, i: int): Option =
    opt("tidy tracking issue", i, "")
  pure def does_closes_it(w: World, i: int): Option = opt("close tracking issue", i, "")
  pure def does_labels_it_needs_triage(w: World, i: int): Option = opt("take the reply", i, "")
  /// A16: one write.
  pure def does_assigns_itself(w: World, i: int): Option = opt("claim", i, "")
  pure def does_starts_a_triage_run(w: World, i: int): Option = opt("start run", i, "triage")
  pure def does_starts_a_design_run(w: World, i: int): Option = opt("start run", i, "design")
  pure def does_starts_an_implement_run(w: World, i: int): Option = opt("start run", i, "implement")
  pure def does_labels_it_ready_for_agent(w: World, i: int): Option = endRun(w, i)
  pure def does_labels_it_needs_info_then_assigns_the_people_asked_whom_github_lets_it_assign(w: World, i: int): Option =
    endRun(w, i)
  pure def does_labels_it_ready_for_human(w: World, i: int): Option = endRun(w, i)
  pure def does_labels_it_linewright_designed(w: World, i: int): Option = endRun(w, i)
  pure def does_creates_the_tickets_then_labels_it_linewright_designed(w: World, i: int): Option = endRun(w, i)
  pure def does_closes_it_as_completed(w: World, i: int): Option = endRun(w, i)
  pure def does_opens_a_pull_request_that_closes_it(w: World, i: int): Option = endRun(w, i)
  pure def does_opens_its_issue(w: World, p: int): Option = opt("take up", p, "")
  pure def does_does_what_that_row_gives(w: World, p: int): Option =
    if (ending(w, p) != "") endRun(w, p)
    else if (it(w, p).feedback) opt("start run", p, "pr")
    else opt("merge", p, "")
  pure def does_removes_all_but_the_newest(w: World, i: int): Option = opt("remove older label", i, it(w, i).older)
  /// R ## Working an item "Stalls": "Linewright removes linewright:stalled
  /// before it starts work on the item again." Its row precedes every row
  /// that starts a run (A16: a write of its own).
  pure def does_removes_linewright_stalled(w: World, i: int): Option = opt("clear stalled", i, "")

  /// R ## Steering, the rows Linewright follows at a poll: merged | "assigns
  /// Linewright to a spec change not yet taken up" | "opens its issue, then
  /// unassigns itself"; open | "assigns Linewright to any other pull request it
  /// did not open" | "unassigns itself with a comment".
  pure def steering(w: World, p: int): Option =
    if (isSpecChange(w, p) and not(takenUp(w, p)) and SPEC_ISSUE_SLOT.keys().contains(p) and it(w, p).lw)
      opt("take up", p, "")
    else if (PRS.contains(p) and not(it(w, p).byLw) and it(w, p).open and it(w, p).lw) opt("let go", p, "")
    else NO_OPTION

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
      // "A run in flight": "While a run is in flight on an item, only the rows
      // for how that run ends hold for it." It ends with each outcome within
      // the bounds; R ## Writes "A person's change during a run": "Linewright
      // acts on a person's change to it when the run ends".
      else outcomesOf(w, i, r.stage).map(o => runEnding(withEnding(w, i, o), i)).exclude(Set(NO_OPTION))
    }
    else Set(working(w, i)).exclude(Set(NO_OPTION))

  pure def optionsIn(w: World): Set[Option] = ITEMS.map(i => optionsOn(w, i)).flatten()

  val options: Set[Option] = optionsIn(world)

  // ===========================================================================
  // The writes of each series, R ## Writes "Order": "The writes that change
  // what an item asks for, such as linewright:designed, its status label, its
  // assignment or a pull request that closes it, come last."
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
  /// labels the item linewright:stalled as well". R ## Writes "Among them":
  /// "linewright:designed comes first, then the status label, then the
  /// assignment".
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

  /// The writes an option carries out: R ## Working an item: "Its writes are
  /// made in their order."
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
  /// it. A comment alone is not working it." R ## Writes "Its own writes":
  /// "Linewright tells its own writes from a person's by their author." Here
  /// they carry its name.
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
  /// asks for: linewright:designed and Linewright's assignment. A closed item
  /// asks for nothing: R ## Managed items "Asking for its work": "An open item
  /// Linewright is assigned to" (A11).
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
    val runs2 = runs.set(i, { stage: stage, start: stateOf(world, i), applying: false, ending: "" })
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
  /// the merge; R ## State has the issue done from the merge: "has had a pull
  /// request that closes it merge into the default branch".
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
  /// posted after its question" (A10); P ## Rework: "A comment or review from
  /// anyone other than Linewright is feedback until a rework run that was given
  /// it ends done": the run's done outcome is the push write, which clears it.
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
  /// R ## Steering, open | "merges or closes a pull request"; P ## The pull
  /// request "Closed without merging": "A PR Linewright opened that a person
  /// closes without merging is the maintainer's no".
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
  // spec-table DS "## States" map
  pure val WORDS: str -> str =
    Map(
      "needs-triage" -> "Waiting for triage",
      "being triaged" -> "Being triaged",
      "needs-info" -> "Waiting for an answer",
      "ready-for-human" -> "Waiting for a decision",
      "ready-for-agent" -> "Waiting for Linewright",
      "being designed" -> "Being designed",
      "being implemented" -> "Being implemented",
      "in review" -> "In review",
      "tracking" -> "Tracking",
      "done" -> "Done",
      "open" -> "Open",
      "merged" -> "Merged",
      "closed" -> "Closed",
    )
  // spec-table end

  /// M ## explain "State in JSON": "its name under State, in kebab-case".
  pure val JSON: str -> str = Map(
    NEEDS_TRIAGE -> "needs-triage", "being triaged" -> "being-triaged",
    NEEDS_INFO -> "needs-info", READY_FOR_HUMAN -> "ready-for-human",
    READY_FOR_AGENT -> "ready-for-agent", "being designed" -> "being-designed",
    "being implemented" -> "being-implemented", "in review" -> "in-review",
    "tracking" -> "tracking", "done" -> "done", "open" -> "open",
    "merged" -> "merged", "closed" -> "closed",
  )

  /// DS ### The board: "The board shows the managed items as cards in six
  /// columns, in this order, each card in the column of its item's state."
  /// (column, an issue's states, a pull request's states).
  // spec-table DS "### The board" board
  pure val COLUMNS: List[(str, Set[str], Set[str])] =
    [
      ("Triage", Set("needs-triage"), Set()),
      ("Needs a person", Set("needs-info", "ready-for-human"), Set()),
      ("Ready", Set("ready-for-agent"), Set()),
      ("Working", Set("being triaged", "being designed", "being implemented"), Set()),
      ("In review", Set("in review"), Set("open")),
      ("Done", Set("done"), Set("merged", "closed")),
    ]
  // spec-table end

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

  /// DS ### Cards: "A card is the item's number and title and, under them, at
  /// most one line that says what its column does not: what sets the item
  /// apart from the rest of its column, what a person acts on, or since when
  /// it has sat." The line of a card in column `col`: "Where more than one row
  /// of a column holds, the line is the first." Holds (U ## Holds), max-runs
  /// and checks are not modelled: their rows never hold.
  // spec-table DS "### Cards" lines column=col
  pure def cardLineIn(w: World, i: int, col: str): str =
    // Triage | "blocked by #12" | an open issue blocks it | 
    if (col == "Triage" and when_an_open_issue_blocks_it(w, i)) "blocked by #12"
    // Triage | "assigned to alice" | someone other than Linewright is assigned to it | 
    else if (col == "Triage" and when_someone_other_than_linewright_is_assigned_to_it(w, i)) "assigned to alice"
    // Triage | "unassigned" | Linewright was unassigned from it | 
    else if (col == "Triage" and when_linewright_was_unassigned_from_it(w, i)) "unassigned"
    // Triage | "not marked for Linewright" | it is not marked for Linewright | 
    else if (col == "Triage" and when_it_is_not_marked_for_linewright(w, i)) "not marked for Linewright"
    // Triage | "paused until" what ends the hold, naming a file: "paused until the environment is built", "paused until the default branch fixes .linewright/config.toml" | it would start, but for a hold | paused or queued
    else if (col == "Triage" and when_it_would_start_but_for_a_hold(w, i)) "paused until"
    // Triage | "queued" | it would start, but the runs in flight are at max-runs | paused or queued
    else if (col == "Triage" and when_it_would_start_but_the_runs_in_flight_are_at_max_runs(w, i)) "queued"
    // Triage | nothing but the time | otherwise: the next poll acts on it | 
    else if (col == "Triage") result_nothing_but_the_time(w, i)
    // Needs a person | "stalled" | it holds linewright:stalled | failed
    else if (col == "Needs a person" and when_it_holds_linewright_stalled(w, i)) "stalled"
    // Needs a person | "asked alice", "asked alice and bob", "asked alice, bob and carol": the issue's assignees, in GitHub's order; "people asked" with none | it holds needs-info | waiting for a person
    else if (col == "Needs a person" and when_it_holds_needs_info(w, i)) "asked alice"
    // Needs a person | "handed back" | Linewright's last act on it was letting go of it: a run handed it back, or its pull request closed unmerged | waiting for a person
    else if (col == "Needs a person" and when_linewright_s_last_act_on_it_was_letting_go_of_it_a_run_handed_it_back_or_its_pull_request_closed_unmerged(w, i)) "handed back"
    // Needs a person | "decision needed" | otherwise | waiting for a person
    else if (col == "Needs a person") "decision needed"
    // Ready | "blocked by #12" | an open issue blocks it | 
    else if (col == "Ready" and when_an_open_issue_blocks_it(w, i)) "blocked by #12"
    // Ready | "assigned to alice" | someone other than Linewright is assigned to it | 
    else if (col == "Ready" and when_someone_other_than_linewright_is_assigned_to_it(w, i)) "assigned to alice"
    // Ready | "unassigned" | Linewright was unassigned from it | 
    else if (col == "Ready" and when_linewright_was_unassigned_from_it(w, i)) "unassigned"
    // Ready | "not marked for Linewright" | it is not marked for Linewright | 
    else if (col == "Ready" and when_it_is_not_marked_for_linewright(w, i)) "not marked for Linewright"
    // Ready | "paused until" what ends the hold, naming a file: "paused until the environment is built", "paused until the default branch fixes .linewright/config.toml" | it would start, but for a hold | paused or queued
    else if (col == "Ready" and when_it_would_start_but_for_a_hold(w, i)) "paused until"
    // Ready | "queued" | it would start, but the runs in flight are at max-runs | paused or queued
    else if (col == "Ready" and when_it_would_start_but_the_runs_in_flight_are_at_max_runs(w, i)) "queued"
    // Ready | nothing but the time | otherwise: the next poll acts on it | 
    else if (col == "Ready") result_nothing_but_the_time(w, i)
    // Working | its stage and how long it has run, "implementation · 12m" | a run is in flight | in flight
    else if (col == "Working" and when_a_run_is_in_flight(w, i)) "implementation · 12m"
    // Working | "blocked by #12" | an open issue blocks it | 
    else if (col == "Working" and when_an_open_issue_blocks_it(w, i)) "blocked by #12"
    // Working | "paused until" what ends the hold, or "queued" | no run is in flight, as in Ready | paused or queued
    else if (col == "Working" and when_no_run_is_in_flight_as_in_ready(w, i)) "paused until"
    // In review | its pull request, where the card is not the pull request itself, then what stops it merging, the first of the table below that holds | always; a pull request opened by someone else is "#42 by alice" | that of what stops it merging
    else if (col == "In review" and when_always_a_pull_request_opened_by_someone_else_is_42_by_alice(w, i)) result_its_pull_request_where_the_card_is_not_the_pull_request_itself_then_what_stops_it_merging_the_first_of_the_table_below_that_holds(w, i)
    // Done | "closed in #42", the pull request whose merge made it done | an issue a merged pull request made done | 
    else if (col == "Done" and when_an_issue_a_merged_pull_request_made_done(w, i)) "closed in #42"
    // Done | "closed as completed", "closed as not planned" or "closed as a duplicate", GitHub's reason; "closed" where the reason is not known | an issue closed otherwise | 
    else if (col == "Done" and when_an_issue_closed_otherwise(w, i)) "closed as completed"
    // Done | "merged" | a pull request merged | 
    else if (col == "Done" and when_a_pull_request_merged(w, i)) "merged"
    // Done | "closed without merging" | a pull request closed unmerged | 
    else if (col == "Done" and when_a_pull_request_closed_unmerged(w, i)) "closed without merging"
    else ""
  // spec-table end
  pure def cardLine(w: World, i: int): str = cardLineIn(w, i, column(stateOf(w, i), PRS.contains(i)))

  /// DS ### Cards: "What stops a pull request merging, in the order the line
  /// picks from".
  // spec-table DS "### Cards" 2 lines
  pure def stopsItMerging(w: World, p: int): str =
    // "stalled" | it holds linewright:stalled | failed
    if (when_it_holds_linewright_stalled(w, p)) "stalled"
    // "handed back" | Linewright opened it, is not assigned to it, and its last act on it was letting go of it | waiting for a person
    else if (when_linewright_opened_it_is_not_assigned_to_it_and_its_last_act_on_it_was_letting_go_of_it(w, p)) "handed back"
    // "unassigned" | Linewright opened it and is not assigned to it otherwise | 
    else if (when_linewright_opened_it_and_is_not_assigned_to_it_otherwise(w, p)) "unassigned"
    // its stage and how long it has run, as in Working | a run is in flight on it | in flight
    else if (when_a_run_is_in_flight_on_it(w, p)) result_its_stage_and_how_long_it_has_run_as_in_working(w, p)
    // "checks failed" | a check failed | failed
    else if (when_a_check_failed(w, p)) "checks failed"
    // "checks running" | a check has not finished | in flight
    else if (when_a_check_has_not_finished(w, p)) "checks running"
    // "conflicts" | it cannot merge cleanly | failed
    else if (when_it_cannot_merge_cleanly(w, p)) "conflicts"
    // "behind main", naming the default branch | it is behind the default branch | 
    else if (when_it_is_behind_the_default_branch(w, p)) "behind main"
    // "draft" | it is a draft | 
    else if (when_it_is_a_draft(w, p)) "draft"
    // "changes requested" | GitHub's review decision is that changes are requested | waiting for a person
    else if (when_github_s_review_decision_is_that_changes_are_requested(w, p)) "changes requested"
    // "review required" | GitHub requires a review before it merges, such as a code owner's, that it has not had | waiting for a person
    else if (when_github_requires_a_review_before_it_merges_such_as_a_code_owner_s_that_it_has_not_had(w, p)) "review required"
    // "approved" | GitHub's review decision is approved | 
    else if (when_github_s_review_decision_is_approved(w, p)) "approved"
    // nothing | otherwise | 
    else result_nothing(w, p)
  // spec-table end

  pure def when_it_holds_linewright_stalled(w: World, i: int): bool = it(w, i).stalled
  pure def when_it_holds_needs_info(w: World, i: int): bool = it(w, i).status == NEEDS_INFO
  pure def when_linewright_s_last_act_on_it_was_letting_go_of_it_a_run_handed_it_back_or_its_pull_request_closed_unmerged(w: World, i: int): bool =
    it(w, i).letGo
  pure def when_an_open_issue_blocks_it(w: World, i: int): bool = ISSUES.contains(i) and blocked(w, i)
  pure def when_someone_other_than_linewright_is_assigned_to_it(w: World, i: int): bool =
    it(w, i).alice or it(w, i).asked
  /// A15.
  pure def when_linewright_was_unassigned_from_it(w: World, i: int): bool = it(w, i).unassignedSince
  pure def when_it_is_not_marked_for_linewright(w: World, i: int): bool = not(markedForLinewright(w, i))
  pure def when_it_would_start_but_for_a_hold(w: World, i: int): bool = false
  pure def when_it_would_start_but_the_runs_in_flight_are_at_max_runs(w: World, i: int): bool = false
  /// No line.
  pure def result_nothing_but_the_time(w: World, i: int): str = ""
  pure def when_a_run_is_in_flight(w: World, i: int): bool = runInFlight(w, i)
  pure def when_no_run_is_in_flight_as_in_ready(w: World, i: int): bool = false
  pure def when_always_a_pull_request_opened_by_someone_else_is_42_by_alice(w: World, i: int): bool = true
  pure def result_its_pull_request_where_the_card_is_not_the_pull_request_itself_then_what_stops_it_merging_the_first_of_the_table_below_that_holds(w: World, i: int): str =
    "its pull request"
  pure def when_an_issue_a_merged_pull_request_made_done(w: World, i: int): bool =
    ISSUES.contains(i) and it(w, i).mergedSince
  pure def when_an_issue_closed_otherwise(w: World, i: int): bool = ISSUES.contains(i) and not(it(w, i).open)
  pure def when_a_pull_request_merged(w: World, i: int): bool = it(w, i).merged
  pure def when_a_pull_request_closed_unmerged(w: World, i: int): bool =
    PRS.contains(i) and not(it(w, i).open) and not(it(w, i).merged)
  pure def when_linewright_opened_it_is_not_assigned_to_it_and_its_last_act_on_it_was_letting_go_of_it(w: World, p: int): bool =
    it(w, p).byLw and not(it(w, p).lw) and it(w, p).letGo
  pure def when_linewright_opened_it_and_is_not_assigned_to_it_otherwise(w: World, p: int): bool =
    it(w, p).byLw and not(it(w, p).lw)
  pure def when_a_run_is_in_flight_on_it(w: World, p: int): bool = runInFlight(w, p)
  pure def result_its_stage_and_how_long_it_has_run_as_in_working(w: World, p: int): str = "its stage"
  pure def when_a_check_failed(w: World, p: int): bool = false
  pure def when_a_check_has_not_finished(w: World, p: int): bool = false
  pure def when_it_cannot_merge_cleanly(w: World, p: int): bool = false
  pure def when_it_is_behind_the_default_branch(w: World, p: int): bool = false
  pure def when_it_is_a_draft(w: World, p: int): bool = false
  pure def when_github_s_review_decision_is_that_changes_are_requested(w: World, p: int): bool = false
  pure def when_github_requires_a_review_before_it_merges_such_as_a_code_owner_s_that_it_has_not_had(w: World, p: int): bool = false
  pure def when_github_s_review_decision_is_approved(w: World, p: int): bool = false
  /// No line.
  pure def result_nothing(w: World, p: int): str = ""

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
