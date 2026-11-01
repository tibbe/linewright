// -*- mode: Bluespec; -*-
/**
 * Model (a), the issue pipeline: one issue, the pull request Linewright opens
 * for it, a pull request of a person's that closes it, and an issue that may
 * block it.
 *
 * The issue starts labeled needs-triage, ready-for-agent or nothing, marked or
 * not, blocked or not; require-label and --auto-merge are each on or off.
 * People act twice a trace and Linewright stops once.
 */
module pipeline {
  import facts.* from "./facts"

  /// The issue, the issue that blocks it, Linewright's pull request for it and
  /// a person's pull request that closes it.
  pure val I = 1
  pure val B = 2
  pure val P = 11
  pure val Q = 12

  pure def start(status: str, required: bool, isBlocked: bool, qOpen: bool, rl: bool,
                 am: bool): Start = {
    items: Map(
      I -> { ...ABSENT, exists: true, status: status, required: required,
             blockedBy: if (isBlocked) Set(B) else Set() },
      B -> { ...ABSENT, exists: true },
      P -> { ...ABSENT, pr: true },
      Q -> { ...ABSENT, pr: true, closes: Set(I), exists: qOpen },
    ),
    requireLabel: rl, autoMerge: am,
  }

  import linewright(
    ISSUES = Set(1, 2), PRS = Set(11, 12), ORDER = [1, 2, 11, 12],
    STARTS = tuples(Set(NEEDS_TRIAGE, READY_FOR_AGENT, ""), Set(true, false), Set(true, false),
                    Set(true, false), Set(true, false))
      .map(t => start(t._1, t._2, t._3, false, t._4, t._5)),
    PEOPLE_ITEMS = Set(1, 11, 12), BLOCKERS = Set(2),
    PEOPLE_ACTS = 2, STOPS = 1,
    TICKET_SLOTS = [], GAP_SLOTS = [], SPEC_ISSUE_SLOT = Map(),
    PR_SLOT = Map(1 -> 11), PERSON_PRS = Set(12),
    SUB_ISSUE_PARENTS = Set(), SUB_ISSUE_CHILDREN = Set(), CUTTABLE = Set(),
    TRIAGE_OUTCOMES = Set("needs-info", "ready-for-agent", "ready-for-human"),
    DESIGN_OUTCOMES = Set("uncut", "handed back"),
    IMPLEMENT_OUTCOMES = Set("pr", "stall"),
    PR_OUTCOMES = Set("done", "stall"),
  ).* from "./linewright"

  /// A start for a scenario, with PEOPLE_ACTS or `people` actions for people.
  action setup(status: str, required: bool, isBlocked: bool, qOpen: bool, rl: bool, am: bool): bool =
    begin(start(status, required, isBlocked, qOpen, rl, am), PEOPLE_ACTS)
  action setupFor(status: str, required: bool, isBlocked: bool, qOpen: bool, rl: bool, am: bool,
                  people: int): bool =
    begin(start(status, required, isBlocked, qOpen, rl, am), people)

  def stateOfI: str = stateOf(world, I)

  // ===========================================================================
  // S1: a witness for each row of the tables
  // ===========================================================================

  run issueRow1Test = setup(READY_FOR_AGENT, true, false, false, false, false)
    .then(personClosesIssue(I)).expect(issueRow(world, I) == 1)
  run issueRow2Test = setup(READY_FOR_AGENT, true, false, true, false, false)
    .expect(issueRow(world, I) == 2)
  run issueRow4Test = setup(NEEDS_TRIAGE, true, false, false, false, false)
    .then(does("claim", I, "")).then(finishes).expect(issueRow(world, I) == 4)
  run issueRow5Test = setup(READY_FOR_AGENT, true, false, false, false, false)
    .then(does("claim", I, "")).expect(issueRow(world, I) == 5)
  run issueRow6Test = setup(READY_FOR_AGENT, true, false, false, false, false)
    .then(does("claim", I, "")).then(does("start run", I, "design"))
    .then(does("end run", I, "uncut")).then(finishes).expect(issueRow(world, I) == 6)
  run issueRow7Test = setup(NEEDS_TRIAGE, true, false, false, false, false)
    .expect(issueRow(world, I) == 7)
  /// "has a run in flight": a person removes the label while the run is in
  /// flight, before the cancel takes effect.
  run issueRow8Test = setup(NEEDS_TRIAGE, true, false, false, false, false)
    .then(does("claim", I, "")).then(does("start run", I, "triage"))
    .then(personRemovesStatusLabel(I))
    .expect(issueRow(world, I) == 8 and stateOfI == "being triaged")
  run issueRow9Test = setup("", true, false, false, false, false)
    .expect(issueRow(world, I) == 9 and stateOfI == "")
  run prRowsTest = setup(READY_FOR_AGENT, true, false, true, false, false)
    .expect(prRow(world, Q) == 1)
    .then(personMergesPr(Q)).expect(prRow(world, Q) == 2)
  run prRow3Test = setup(READY_FOR_AGENT, true, false, true, false, false)
    .then(personClosesPr(Q)).expect(prRow(world, Q) == 3)

  /// S2's tables: every state has words and one column.
  run tablesTest = init.expect(S2_tables)

  // ===========================================================================
  // S8: past counterexamples
  // ===========================================================================

  /// F1: an issue a merged pull request made done, while GitHub has not
  /// closed it yet, ready-for-agent and assigned: no implement run.
  run f1MergedNotYetClosedTest = setup(READY_FOR_AGENT, true, false, false, false, false)
    .then(does("claim", I, "")).then(does("start run", I, "design"))
    .then(does("end run", I, "uncut")).then(finishes)
    .then(personOpensPr(Q)).then(personMergesPr(Q))
    .expect(and {
      stateOfI == "done", items.get(I).open, items.get(I).lw,
      not(mayNowStart(I, "implement")), not(mayNowStart(I, "design")),
    })

  /// F2: a hand-back stopped part way does not leave the issue
  /// ready-for-agent, unassigned and never claimed again; here stopped after
  /// linewright:designed is removed.
  run f2HandBackStoppedTest = setup(READY_FOR_AGENT, true, false, false, false, false)
    .then(does("claim", I, "")).then(does("start run", I, "design"))
    .then(does("end run", I, "uncut")).then(finishes)
    .then(does("start run", I, "implement")).then(does("end run", I, "stall"))
    .then(finishes).then(finishes)
    .then(lwStops)
    .then(40.reps(_ => settle))
    .expect(not(items.get(I).status == READY_FOR_AGENT and items.get(I).unassignedSince
                and not(items.get(I).lw)))

  /// F7: a pull request a person closed between polls is still the
  /// maintainer's no.
  run f7ClosedBetweenPollsTest = setup(READY_FOR_AGENT, true, false, false, false, false)
    .then(does("claim", I, "")).then(does("start run", I, "design"))
    .then(does("end run", I, "uncut")).then(finishes)
    .then(does("start run", I, "implement")).then(does("end run", I, "pr"))
    .then(personClosesPr(P))
    .expect(options.contains(opt("hand back", I, "closed unmerged")))

  /// F10: someone else's pull request assigned to Linewright by hand is let
  /// go of with a comment.
  run f10OthersPrTest = setup(READY_FOR_AGENT, true, false, true, false, false)
    .then(personAssignsLinewright(Q))
    .expect(options.contains(opt("let go", Q, "")))
    .then(does("let go", Q, "")).then(finishes)
    .expect(not(items.get(Q).lw))

  /// U2: after a person closes Linewright's pull request, the hand-back comes
  /// before any implement run.
  run u2HandBackFirstTest = setup(READY_FOR_AGENT, true, false, false, false, false)
    .then(does("claim", I, "")).then(does("start run", I, "design"))
    .then(does("end run", I, "uncut")).then(finishes)
    .then(does("start run", I, "implement")).then(does("end run", I, "pr"))
    .then(personClosesPr(P))
    .expect(stateOfI == "being implemented"
            and optionsOn(world, I) == Set(opt("hand back", I, "closed unmerged")))

  /// N4: a needs-triage card assigned to alice, blocked, unassigned or not
  /// marked says why; Linewright's pull request, once unassigned, shows
  /// "unassigned" or "handed back".
  run n4AssignedToAliceTest = setup(NEEDS_TRIAGE, true, false, false, false, false)
    .then(personAssignsThemself(I)).expect(cardLine(world, I) == "assigned to alice")
  run n4BlockedTest = setup(NEEDS_TRIAGE, true, true, false, false, false)
    .expect(cardLine(world, I) == "blocked by")
  run n4UnassignedTest = setup(NEEDS_TRIAGE, true, false, false, false, false)
    .then(does("claim", I, "")).then(personUnassignsLinewright(I))
    .expect(cardLine(world, I) == "unassigned")
  run n4NotMarkedTest = setup(NEEDS_TRIAGE, false, false, false, true, false)
    .expect(managed(world, I) == false and cardLine(world, I) == "not marked for Linewright")
  run n4PrUnassignedTest = setup(READY_FOR_AGENT, true, false, false, false, false)
    .then(does("claim", I, "")).then(does("start run", I, "design"))
    .then(does("end run", I, "uncut")).then(finishes)
    .then(does("start run", I, "implement")).then(does("end run", I, "pr"))
    .then(does("claim", P, "")).then(personUnassignsLinewright(P))
    .expect(stopsItMerging(world, P) == "unassigned")

  /// N6: Linewright's superseded pull request closed after the issue is
  /// already done: no hand-back.
  run n6SupersededPrTest = setupFor(READY_FOR_AGENT, true, false, false, false, false, 3)
    .then(does("claim", I, "")).then(does("start run", I, "design"))
    .then(does("end run", I, "uncut")).then(finishes)
    .then(does("start run", I, "implement")).then(does("end run", I, "pr"))
    .then(personOpensPr(Q)).then(personMergesPr(Q)).then(personClosesPr(P))
    .expect(stateOfI == "done" and items.get(I).noSince
            and not(options.contains(opt("hand back", I, "closed unmerged"))))

  /// A3: a person closing their own pull request is not a no on Linewright's
  /// work on the issue it closes.
  run a3OwnPrClosedTest = setup(READY_FOR_AGENT, true, false, true, false, false)
    .then(personAssignsLinewright(I)).then(personClosesPr(Q))
    .expect(stateOfI == "being designed"
            and not(options.contains(opt("hand back", I, "closed unmerged"))))

  /// A4: a hand-back stopped after the unassign has already removed
  /// linewright:designed: the label comes before the assignment.
  run a4DesignedBeforeUnassignTest = setup(READY_FOR_AGENT, true, false, false, false, false)
    .then(does("claim", I, "")).then(does("start run", I, "design"))
    .then(does("end run", I, "uncut")).then(finishes)
    .then(does("start run", I, "implement")).then(does("end run", I, "stall"))
    .then(5.reps(_ => finishes))
    .expect(not(items.get(I).lw) and not(items.get(I).designed)
            and items.get(I).status == READY_FOR_HUMAN and items.get(I).stalled)

  /// Minor: a blocked issue Linewright is assigned to shows "blocked by #12"
  /// in Working, and no run starts.
  run blockedInWorkingTest = setup(READY_FOR_AGENT, true, true, false, false, false)
    .then(personAssignsLinewright(I))
    .expect(column(stateOfI, false) == "Working" and cardLine(world, I) == "blocked by"
            and not(mayNowStart(I, "design")))

  // ===========================================================================
  // S9: the motivating issues
  // ===========================================================================

  /// #813: a status label applied after the merge ends the merge's hold: the
  /// issue is worked again until GitHub closes it, which cancels the run.
  run issue813Test = setup("", true, false, true, false, false)
    .then(personMergesPr(Q)).expect(stateOfI == "done")
    .then(personAddsStatusLabel(I, READY_FOR_AGENT)).expect(stateOfI == READY_FOR_AGENT)
    .then(does("claim", I, "")).then(does("start run", I, "design"))
    .then(githubCloses).expect(stateOfI == "done" and options.contains(opt("cancel", I, "design")))

  /// #881: a merged pull request's state is merged, "merged" in JSON, whether
  /// Linewright manages it or not.
  run issue881Test = setup(READY_FOR_AGENT, true, false, false, false, false)
    .then(does("claim", I, "")).then(does("start run", I, "design"))
    .then(does("end run", I, "uncut")).then(finishes)
    .then(does("start run", I, "implement")).then(does("end run", I, "pr"))
    .then(personMergesPr(P))
    .expect(prState(world, P) == "merged" and JSON.get(prState(world, P)) == "merged"
            and managed(world, P))
  run issue881LeftAloneTest = setup(READY_FOR_AGENT, true, false, true, false, false)
    .then(personMergesPr(Q))
    .expect(prState(world, Q) == "merged" and not(managed(world, Q)))
}
