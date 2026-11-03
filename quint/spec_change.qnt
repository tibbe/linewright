// -*- mode: Bluespec; -*-
/**
 * Model (c), a spec change: a merged pull request that touched the spec; the
 * issue Linewright opens for it; the spec gap issue a design of that issue may
 * file, which references the spec change, or which another run filed before
 * the spec change was taken up; the one ticket its cut may create; and the
 * pull requests Linewright opens for the issue and the ticket.
 *
 * The spec change is merged within the lookback window or before it, and
 * carries the required label; the lookback window may pass it by during a
 * trace. require-label is on or off; --auto-merge is on. People act twice a
 * trace, on the spec change, its issue and the spec gap issue, and Linewright
 * stops once.
 */
module spec_change {
  import facts.* from "./facts"

  /// The spec change, its issue, a spec gap issue that references it, the
  /// ticket cut from its issue, and the pull requests for the issue and the
  /// ticket.
  pure val S = 21
  pure val SI = 1
  pure val G = 2
  pure val T = 3
  pure val PSI = 11
  pure val PT = 12

  pure def start(inLookback: bool, required: bool, gapFirst: bool, rl: bool): Start = {
    items: Map(
      S -> { ...ABSENT, exists: true, pr: true, open: false, merged: true, spec: true,
             inLookback: inLookback, required: required },
      SI -> ABSENT,
      // A spec gap issue another run filed before, which references the spec
      // change; or the slot for the one its issue's design files.
      G -> if (gapFirst) { ...ABSENT, exists: true, byLw: true, status: READY_FOR_HUMAN,
                           required: true, worked: true, refs: S }
           else ABSENT,
      T -> ABSENT,
      PSI -> { ...ABSENT, pr: true }, PT -> { ...ABSENT, pr: true },
    ),
    requireLabel: rl, autoMerge: true,
  }

  import linewright(
    ISSUES = Set(1, 2, 3), PRS = Set(11, 12, 21), ORDER = [1, 2, 3, 11, 12, 21],
    STARTS = tuples(Set(true, false), Set(true, false), Set(true, false))
      .map(t => start(t._1, true, t._2, t._3)),
    PEOPLE_ITEMS = Set(1, 2, 21), BLOCKERS = Set(),
    PEOPLE_ACTS = 2, STOPS = 1,
    TICKET_SLOTS = [3], GAP_SLOTS = [2], SPEC_ISSUE_SLOT = Map(21 -> 1),
    PR_SLOT = Map(1 -> 11, 3 -> 12), PERSON_PRS = Set(),
    SUB_ISSUE_PARENTS = Set(), SUB_ISSUE_CHILDREN = Set(), CUTTABLE = Set(1),
    TRIAGE_OUTCOMES = Set("ready-for-agent", "ready-for-human"),
    DESIGN_OUTCOMES = Set("cut", "uncut", "nothing to build", "handed back"),
    IMPLEMENT_OUTCOMES = Set("pr", "stall"),
    PR_OUTCOMES = Set("done", "stall"),
  ).* from "./linewright"

  /// A start for a scenario, with `people` actions for people.
  action setupFor(inLookback: bool, required: bool, gapFirst: bool, rl: bool, people: int): bool =
    begin(start(inLookback, required, gapFirst, rl), people)

  /// Linewright takes the spec change up, claims its issue and starts its
  /// design.
  run designing = setupFor(true, true, false, false, 2)
    .then(does("take up", S, "")).then(does("claim", SI, "")).then(does("start run", SI, "design"))

  /// The take-up: the spec change's issue, labeled ready-for-agent.
  run takeUpTest = setupFor(true, true, false, true, 2)
    .expect(options.contains(opt("take up", S, "")))
    .then(does("take up", S, ""))
    .expect(itsIssue(world, S) == SI and items.get(SI).status == READY_FOR_AGENT
            and items.get(SI).refs == S and items.get(SI).required and takenUp(world, S)
            and not(options.contains(opt("take up", S, ""))))

  // ===========================================================================
  // S8: past counterexamples, as the spec now reads
  // ===========================================================================

  /// C1 and N5: an old spec change assigned by hand is taken up and designed,
  /// and has a card, its issue's, while it is designed.
  run c1OldAssignedByHandTest = setupFor(false, false, false, true, 2)
    .expect(not(options.contains(opt("take up", S, ""))))
    .then(personAssignsLinewright(S))
    .then(does("take up", S, "")).then(finishes)
    .expect(not(items.get(S).lw) and itsIssue(world, S) == SI)
    .then(does("claim", SI, "")).then(does("start run", SI, "design"))
    .expect(stateOf(world, SI) == "being designed" and managed(world, SI)
            and column(stateOf(world, SI), false) == "Working")

  /// F9 (a): a spec change whose design Linewright handed back waits for a
  /// person, in Needs a person: its issue, labeled ready-for-human; and stays
  /// so past the lookback window.
  run f9aHandedBackTest = designing
    .then(does("end run", SI, "handed back")).then(3.reps(_ => finishes))
    .expect(stateOf(world, SI) == READY_FOR_HUMAN and column(stateOf(world, SI), false) == "Needs a person"
            and cardLine(world, SI) == "handed back")
    .then(lookbackEnds)
    .expect(stateOf(world, SI) == READY_FOR_HUMAN and takenUp(world, S)
            and not(options.contains(opt("take up", S, ""))))

  /// F9 (b): a spec change older than the lookback window that Linewright
  /// never worked is merged and not managed.
  run f9bOldNeverWorkedTest = setupFor(false, true, false, false, 2)
    .expect(prState(world, S) == "merged" and not(managed(world, S)) and options == Set())

  /// F4 and N2: a spec gap issue that references the spec change never
  /// becomes its issue, whoever relabels it.
  run f4GapFiledDuringTheDesignTest = designing
    .then(does("end run", SI, "cut")).then(5.reps(_ => finishes))
    .then(personRemovesStatusLabel(G))
    .expect(items.get(G).exists and itsIssue(world, S) == SI)
  run f4GapFiledBeforeTest = setupFor(true, true, true, false, 2)
    .expect(itsIssue(world, S) != G)

  /// C4 and F8: a spec change cut into tickets has no card of its own: it is
  /// not managed, so it shows on no issue's card either.
  run c4NoCardOfItsOwnTest = designing
    .then(does("end run", SI, "cut")).then(5.reps(_ => finishes))
    .expect(stateOf(world, SI) == "tracking" and not(managed(world, S)))

  /// Minor, as the spec now reads: a spec change is no longer designed
  /// itself; closing its issue cancels the issue's design run.
  run closingItsIssueCancelsTest = designing
    .then(personClosesIssue(SI))
    .expect(options.contains(opt("cancel", SI, "design")))

  // ===========================================================================
  // S9: the motivating issues
  // ===========================================================================

  /// #823: a spec change cut into tickets is merged; its issue is tracking.
  run issue823Test = designing
    .then(does("end run", SI, "cut")).then(5.reps(_ => finishes))
    .expect(prState(world, S) == "merged" and WORDS.get(prState(world, S)) == "Merged"
            and stateOf(world, SI) == "tracking")

  /// 1a in an issue Linewright created: closed as completed.
  run nothingToBuildTest = designing
    .then(does("end run", SI, "nothing to build")).then(finishes)
    .expect(stateOf(world, SI) == "done" and items.get(SI).closedBy == "linewright"
            and not(items.get(SI).notPlanned))
}
