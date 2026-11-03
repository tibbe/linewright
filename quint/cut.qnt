// -*- mode: Bluespec; -*-
/**
 * Model (b), cutting an issue: an issue a person filed, labeled
 * ready-for-agent; the two tickets its design may cut, the second blocked by
 * the first; the pull request Linewright opens for the first ticket; a pull
 * request of a person's that closes the first ticket; an issue of a person's
 * and a spec gap issue Linewright filed, which people may add to the issue's
 * sub-issues.
 *
 * require-label is on or off; --auto-merge is on, so that the first ticket
 * can finish without people. The second ticket has no pull request to be
 * built in: its implement run can only hand it back. A cut takes the ticket
 * slots still free, so a design after a discard cuts into at most what the
 * first cut left. People act twice a trace, on the issue, the first ticket
 * and the two pull requests, and add or remove sub-issues; Linewright stops
 * once.
 */
module cut {
  import facts.* from "./facts"

  /// The issue, its tickets, a person's issue, Linewright's spec gap issue;
  /// the first ticket's pull request and a person's.
  pure val X = 1
  pure val T1 = 3
  pure val T2 = 4
  pure val H = 6
  pure val G = 7
  pure val PT1 = 13
  pure val PP = 16

  pure def start(rl: bool): Start = {
    items: Map(
      X -> { ...ABSENT, exists: true, status: READY_FOR_AGENT, required: true },
      T1 -> ABSENT, T2 -> ABSENT,
      H -> { ...ABSENT, exists: true },
      G -> { ...ABSENT, exists: true, byLw: true, status: READY_FOR_HUMAN, required: true,
             worked: true },
      PT1 -> { ...ABSENT, pr: true },
      PP -> { ...ABSENT, pr: true, closes: Set(T1) },
    ),
    requireLabel: rl, autoMerge: true,
  }

  import linewright(
    ISSUES = Set(1, 3, 4, 6, 7), PRS = Set(13, 16), ORDER = [1, 3, 4, 6, 7, 13, 16],
    STARTS = Set(true, false).map(rl => start(rl)),
    PEOPLE_ITEMS = Set(1, 3, 13, 16), BLOCKERS = Set(),
    PEOPLE_ACTS = 2, STOPS = 1,
    TICKET_SLOTS = [3, 4], GAP_SLOTS = [], SPEC_ISSUE_SLOT = Map(),
    PR_SLOT = Map(3 -> 13), PERSON_PRS = Set(16),
    SUB_ISSUE_PARENTS = Set(1), SUB_ISSUE_CHILDREN = Set(6, 7), CUTTABLE = Set(1),
    TRIAGE_OUTCOMES = Set("ready-for-agent", "ready-for-human"),
    DESIGN_OUTCOMES = Set("cut", "uncut", "nothing to build", "handed back"),
    IMPLEMENT_OUTCOMES = Set("pr", "stall"),
    PR_OUTCOMES = Set("done", "stall"),
  ).* from "./linewright"

  /// A start for a scenario, with `people` actions for people.
  action setupFor(rl: bool, people: int): bool = begin(start(rl), people)

  def stateOfX: str = stateOf(world, X)

  /// Linewright claims the issue, designs it and cuts it into two tickets,
  /// then tidies the tracking issue.
  run cutDone = setupFor(false, 3)
    .then(does("claim", X, "")).then(does("start run", X, "design"))
    .then(does("end run", X, "cut")).then(4.reps(_ => finishes))
    .then(does("tidy tracking issue", X, "")).then(finishes)

  // ===========================================================================
  // S1: the tracking row
  // ===========================================================================

  run issueRow3Test = cutDone
    .expect(issueRow(world, X) == 3 and stateOfX == "tracking"
            and items.get(X).status == "" and not(items.get(X).lw)
            and ticketsCutFrom(world, X) == Set(T1, T2) and items.get(T2).blockedBy == Set(T1))

  // ===========================================================================
  // S8: past counterexamples
  // ===========================================================================

  /// F3: a person labels a tracking issue needs-triage or ready-for-agent: no
  /// triage and no claim; Linewright removes the label.
  run f3TrackingRelabeledTest = cutDone
    .then(personAddsStatusLabel(X, NEEDS_TRIAGE))
    .expect(stateOfX == "tracking" and optionsOn(world, X) == Set(opt("tidy tracking issue", X, "")))
    .then(does("tidy tracking issue", X, "")).expect(items.get(X).status == "")
  /// C3: a tracking issue labeled ready-for-agent is not claimed.
  run c3TrackingNotClaimedTest = cutDone
    .then(personAddsStatusLabel(X, READY_FOR_AGENT))
    .expect(not(options.contains(opt("claim", X, ""))))

  /// F5: an issue whose tickets a person took out of its sub-issues is not
  /// a tracking issue, and is not closed.
  run f5NoSubIssuesTest = cutDone
    .then(personRemovesSubIssue(T1)).then(personRemovesSubIssue(T2))
    .expect(stateOfX == "" and subIssues(world, X) == Set()
            and not(options.contains(opt("close tracking issue", X, ""))))

  /// F6: a ticket whose linewright:designed a person removes is designed
  /// again.
  run f6TicketDesignedAgainTest = cutDone
    .then(does("claim", T1, "")).then(personTogglesDesigned(T1))
    .expect(stateOf(world, T1) == "being designed" and mayNowStart(T1, "design"))

  /// F9 (c): a managed ticket that is not marked shows "not marked for
  /// Linewright" in Ready.
  run f9cTicketNotMarkedTest = setupFor(true, 3)
    .then(does("claim", X, "")).then(does("start run", X, "design"))
    .then(does("end run", X, "cut")).then(4.reps(_ => finishes))
    .then(personTogglesRequiredLabel(T1))
    .expect(managed(world, T1) and column(stateOf(world, T1), false) == "Ready"
            and cardLine(world, T1) == "not marked for Linewright")

  /// C2: removing linewright:designed from a cut issue has it designed again
  /// only once it asks for it, labeled ready-for-agent.
  run c2RedesignOnlyWhenAskedTest = cutDone
    .then(personTogglesDesigned(X))
    .expect(optionsOn(world, X) == Set(opt("discard", X, "")))
    .then(does("discard", X, "")).then(5.reps(_ => finishes))
    .expect(stateOfX == "" and optionsOn(world, X) == Set())
    .then(personAddsStatusLabel(X, READY_FOR_AGENT)).then(does("claim", X, ""))
    .expect(mayNowStart(X, "design"))

  /// O1: a discarded cut does not set off the discard again.
  run o1DiscardOnceTest = cutDone
    .then(personTogglesDesigned(X)).then(does("discard", X, "")).then(5.reps(_ => finishes))
    .expect(not(owesDiscard(world, X)) and not(items.get(T1).open) and items.get(T1).notPlanned)

  /// U1: a sub-issue a person added is not a ticket.
  run u1PersonsSubIssueTest = cutDone
    .then(personAddsSubIssue(H, X))
    .expect(ticketsCutFrom(world, X) == Set(T1, T2) and subIssues(world, X).contains(H))

  /// N1: an interrupted cut is discarded before the redesign run starts.
  run n1InterruptedCutTest = setupFor(false, 2)
    .then(does("claim", X, "")).then(does("start run", X, "design"))
    .then(does("end run", X, "cut")).then(finishes).then(lwStops)
    .expect(stateOfX == "being designed" and optionsOn(world, X) == Set(opt("discard", X, "")))

  /// N3: Linewright's spec gap issue, which a person adds as a sub-issue of a
  /// designed issue: the issue is not a tracking issue for it, and a discard
  /// never closes or detaches it.
  run n3GapAsSubIssueTest = setupFor(false, 3)
    .then(personTogglesDesigned(X)).then(personAddsSubIssue(G, X))
    .expect(not(isTrackingIssue(world, X)))
  run n3GapNotDiscardedTest = cutDone
    .then(personAddsSubIssue(G, X)).then(personTogglesDesigned(X))
    .then(does("discard", X, "")).then(5.reps(_ => finishes))
    .expect(items.get(G).open and items.get(G).parent == X and s7ok)

  /// Minor: discarding closes the pull requests Linewright opened for its
  /// tickets, not "as not planned".
  run discardClosesPrsTest = cutDone
    .then(does("claim", T1, "")).then(does("start run", T1, "implement"))
    .then(does("end run", T1, "pr"))
    .then(personTogglesDesigned(X)).then(does("discard", X, "")).then(7.reps(_ => finishes))
    .expect(not(items.get(PT1).open) and not(items.get(PT1).merged)
            and items.get(PT1).closedBy == "linewright" and not(items.get(PT1).notPlanned)
            and items.get(T1).notPlanned)

  /// Minor: a cut that ends with linewright:designed (where a stop right
  /// after it leaves the issue) ends with the status label and the
  /// assignment removed.
  run cutStoppedAfterDesignedTest = setupFor(false, 2)
    .then(does("claim", X, "")).then(does("start run", X, "design"))
    .then(does("end run", X, "cut")).then(4.reps(_ => finishes))
    .expect(stateOfX == "tracking" and items.get(X).lw and seq.length() == 0
            and optionsOn(world, X) == Set(opt("tidy tracking issue", X, "")))
    .then(does("tidy tracking issue", X, "")).then(finishes)
    .expect(items.get(X).status == "" and not(items.get(X).lw))

  // ===========================================================================
  // S9: the motivating issues
  // ===========================================================================

  /// #828 and #830: an open tracking issue whose tickets are all closed is
  /// managed and says Tracking until Linewright closes it.
  run issue828and830Test = cutDone
    .then(personClosesIssue(T1)).then(personRemovesSubIssue(T2))
    .expect(managed(world, X) and WORDS.get(stateOfX) == "Tracking"
            and options.contains(opt("close tracking issue", X, "")))
}
