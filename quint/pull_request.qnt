// -*- mode: Bluespec; -*-
/**
 * Model (d), working a pull request: one open pull request Linewright opened,
 * worked by P ## Working a pull request, the first row that holds. P =
 * pull_requests.md, R = repository.md.
 *
 * Linewright makes one GitHub write a step and may stop between any two; a
 * inFlight's session is one step. CI and GitHub act on their own: checks appear,
 * finish or time out, and GitHub decides whether it allows the merge. People
 * act twice a trace: they comment, approve, push, move the default branch,
 * block or unblock the merge, unassign and assign Linewright, mark the pull
 * request a draft, and close or merge it. Checks may also appear once after
 * the grace period, as on a repository whose CI runs only once a pull request
 * is ready.
 */
module pull_request {
  /// ci-attempts, and the merges that fail before a hand-back.
  pure val CI_ATTEMPTS = 2
  pure val MERGE_TRIES = 2
  pure val PEOPLE_ACTS = 2
  pure val STOPS = 1
  pure val LATE_CHECKS = 1

  /// P ## Rework: a comment or review "is feedback until a rework run that was
  /// given it ends done". False reads it as the prose did before: posted after
  /// Linewright's last push, under which a rework run that answers without a
  /// push starts again with nothing new (noFlap fails).
  pure val FEEDBACK_UNTIL_REWORK_DONE = true

  /// What GitHub records about the pull request, and what Linewright counts.
  type Pr = {
    // "open", "merged" or "closed"
    status: str,
    // Linewright is assigned to it
    lw: bool,
    draft: bool,
    // "Its checks": "The check runs and commit statuses on its head.", summed
    // up: "none" appeared, one "running", one "failed", or all "green"
    checks: str,
    // its last push was ci-grace ago or more
    graceOver: bool,
    // ci-timeout has passed since its last push, or since Linewright was last
    // assigned to it where that is later
    timedOut: bool,
    // P ## Rework: feedback has come
    feedback: bool,
    // it conflicts with the default branch
    conflict: bool,
    // it is behind the default branch, and the repository requires it up to
    // date to merge
    behind: bool,
    // a person has approved it
    approved: bool,
    // it cannot be merged by a method the repository allows
    unmergeable: bool,
    // a run has ended on it since its last push
    ranSincePush: bool,
    // P ## Merging "Review": a person holds its merge
    held: bool,
    // whether GitHub allows the merge: "allows", "blocks" or "unknown"
    github: str,
    // this round: Linewright has commented on it naming what GitHub requires;
    // fix-ci runs ended; merges failed
    commented: bool,
    fixes: int,
    mergeFails: int,
    stalled: bool,
  }

  /// P ## Working a pull request: "Linewright works a pull request it opened
  /// as this table gives: at each poll, the first row that holds." "Its state
  /// is GitHub's: a draft, or ready for review."
  pure def rowState(x: Pr, am: bool): str = if (x.draft) "draft" else "ready for review"

  /// What Linewright does: the first row that holds.
  // spec-table P "## Working a pull request" rows else=noRow
  pure def todo(x: Pr, am: bool): str =
    {
      val state = rowState(x, am)
      // any | feedback has come | starts a rework run | unchanged
      if (when_feedback_has_come(x, am)) does_starts_a_rework_run(x, am)
      // any | it conflicts with the default branch | starts an update run | unchanged
      else if (when_it_conflicts_with_the_default_branch(x, am)) does_starts_an_update_run(x, am)
      // any | it cannot be merged by a method the repository allows, for a reason other than being behind, and no run has ended on it since its last push | starts an update run | unchanged
      else if (when_it_cannot_be_merged_by_a_method_the_repository_allows_for_a_reason_other_than_being_behind_and_no_run_has_ended_on_it_since_its_last_push(x, am)) does_starts_an_update_run(x, am)
      // any | none of its checks has appeared, and its last push was less than ci-grace ago | waits for its checks | unchanged
      else if (when_none_of_its_checks_has_appeared_and_its_last_push_was_less_than_ci_grace_ago(x, am)) does_waits_for_its_checks(x, am)
      // any | a check is still running ci-timeout after its last push, or after Linewright was last assigned to it where that is later | hands it back, naming the checks still running | unchanged
      else if (when_a_check_is_still_running_ci_timeout_after_its_last_push_or_after_linewright_was_last_assigned_to_it_where_that_is_later(x, am)) does_hands_it_back_naming_the_checks_still_running(x, am)
      // any | a check is still running | waits for its checks | unchanged
      else if (when_a_check_is_still_running(x, am)) does_waits_for_its_checks(x, am)
      // any | a check failed, and fewer than ci-attempts fix-ci runs have ended on it this round | starts a fix-ci run | unchanged
      else if (when_a_check_failed_and_fewer_than_ci_attempts_fix_ci_runs_have_ended_on_it_this_round(x, am)) does_starts_a_fix_ci_run(x, am)
      // any | a check failed | hands it back, naming the failing checks | unchanged
      else if (when_a_check_failed(x, am)) does_hands_it_back_naming_the_failing_checks(x, am)
      // any | it is behind the default branch, the repository requires it up to date to merge, and that alone keeps it from being ready to merge | starts an update run | unchanged
      else if (when_it_is_behind_the_default_branch_the_repository_requires_it_up_to_date_to_merge_and_that_alone_keeps_it_from_being_ready_to_merge(x, am)) does_starts_an_update_run(x, am)
      // draft | otherwise | marks it ready for review | ready for review
      else if (Set("draft").contains(state)) does_marks_it_ready_for_review(x, am)
      // ready for review | auto-merge is off, or a person holds its merge | waits for review | ready for review
      else if (Set("ready for review").contains(state) and when_auto_merge_is_off_or_a_person_holds_its_merge(x, am)) does_waits_for_review(x, am)
      // ready for review | two merges of it have failed this round | hands it back with GitHub's reason | ready for review
      else if (Set("ready for review").contains(state) and when_two_merges_of_it_have_failed_this_round(x, am)) does_hands_it_back_with_github_s_reason(x, am)
      // ready for review | GitHub blocks the merge, and Linewright has not commented on it this round naming what GitHub requires | comments naming what GitHub requires | ready for review
      else if (Set("ready for review").contains(state) and when_github_blocks_the_merge_and_linewright_has_not_commented_on_it_this_round_naming_what_github_requires(x, am)) does_comments_naming_what_github_requires(x, am)
      // ready for review | GitHub blocks the merge, or has not reported whether it allows it | waits for what GitHub requires | ready for review
      else if (Set("ready for review").contains(state) and when_github_blocks_the_merge_or_has_not_reported_whether_it_allows_it(x, am)) does_waits_for_what_github_requires(x, am)
      // ready for review | otherwise | merges it | merged
      else if (Set("ready for review").contains(state)) does_merges_it(x, am)
      else noRow(x, am)
    }
  // spec-table end
  /// A pull request is a draft or ready for review, and each ends on an
  /// "otherwise" row, so some row holds.
  pure def noRow(x: Pr, am: bool): str = ""
  /// The first row that holds, from 1.
  // spec-table P "## Working a pull request" row
  pure def firstRow(x: Pr, am: bool): int =
    {
      val state = rowState(x, am)
      // any | feedback has come | starts a rework run | unchanged
      if (when_feedback_has_come(x, am)) 1
      // any | it conflicts with the default branch | starts an update run | unchanged
      else if (when_it_conflicts_with_the_default_branch(x, am)) 2
      // any | it cannot be merged by a method the repository allows, for a reason other than being behind, and no run has ended on it since its last push | starts an update run | unchanged
      else if (when_it_cannot_be_merged_by_a_method_the_repository_allows_for_a_reason_other_than_being_behind_and_no_run_has_ended_on_it_since_its_last_push(x, am)) 3
      // any | none of its checks has appeared, and its last push was less than ci-grace ago | waits for its checks | unchanged
      else if (when_none_of_its_checks_has_appeared_and_its_last_push_was_less_than_ci_grace_ago(x, am)) 4
      // any | a check is still running ci-timeout after its last push, or after Linewright was last assigned to it where that is later | hands it back, naming the checks still running | unchanged
      else if (when_a_check_is_still_running_ci_timeout_after_its_last_push_or_after_linewright_was_last_assigned_to_it_where_that_is_later(x, am)) 5
      // any | a check is still running | waits for its checks | unchanged
      else if (when_a_check_is_still_running(x, am)) 6
      // any | a check failed, and fewer than ci-attempts fix-ci runs have ended on it this round | starts a fix-ci run | unchanged
      else if (when_a_check_failed_and_fewer_than_ci_attempts_fix_ci_runs_have_ended_on_it_this_round(x, am)) 7
      // any | a check failed | hands it back, naming the failing checks | unchanged
      else if (when_a_check_failed(x, am)) 8
      // any | it is behind the default branch, the repository requires it up to date to merge, and that alone keeps it from being ready to merge | starts an update run | unchanged
      else if (when_it_is_behind_the_default_branch_the_repository_requires_it_up_to_date_to_merge_and_that_alone_keeps_it_from_being_ready_to_merge(x, am)) 9
      // draft | otherwise | marks it ready for review | ready for review
      else if (Set("draft").contains(state)) 10
      // ready for review | auto-merge is off, or a person holds its merge | waits for review | ready for review
      else if (Set("ready for review").contains(state) and when_auto_merge_is_off_or_a_person_holds_its_merge(x, am)) 11
      // ready for review | two merges of it have failed this round | hands it back with GitHub's reason | ready for review
      else if (Set("ready for review").contains(state) and when_two_merges_of_it_have_failed_this_round(x, am)) 12
      // ready for review | GitHub blocks the merge, and Linewright has not commented on it this round naming what GitHub requires | comments naming what GitHub requires | ready for review
      else if (Set("ready for review").contains(state) and when_github_blocks_the_merge_and_linewright_has_not_commented_on_it_this_round_naming_what_github_requires(x, am)) 13
      // ready for review | GitHub blocks the merge, or has not reported whether it allows it | waits for what GitHub requires | ready for review
      else if (Set("ready for review").contains(state) and when_github_blocks_the_merge_or_has_not_reported_whether_it_allows_it(x, am)) 14
      // ready for review | otherwise | merges it | merged
      else if (Set("ready for review").contains(state)) 15
      else 0
    }
  // spec-table end

  pure def when_feedback_has_come(x: Pr, am: bool): bool = x.feedback
  pure def when_it_conflicts_with_the_default_branch(x: Pr, am: bool): bool = x.conflict
  pure def when_it_cannot_be_merged_by_a_method_the_repository_allows_for_a_reason_other_than_being_behind_and_no_run_has_ended_on_it_since_its_last_push(x: Pr, am: bool): bool =
    x.unmergeable and not(x.ranSincePush)
  pure def when_none_of_its_checks_has_appeared_and_its_last_push_was_less_than_ci_grace_ago(x: Pr, am: bool): bool =
    x.checks == "none" and not(x.graceOver)
  pure def when_a_check_is_still_running_ci_timeout_after_its_last_push_or_after_linewright_was_last_assigned_to_it_where_that_is_later(x: Pr, am: bool): bool =
    x.checks == "running" and x.timedOut
  pure def when_a_check_is_still_running(x: Pr, am: bool): bool = x.checks == "running"
  pure def when_a_check_failed_and_fewer_than_ci_attempts_fix_ci_runs_have_ended_on_it_this_round(x: Pr, am: bool): bool =
    x.checks == "failed" and x.fixes < CI_ATTEMPTS
  pure def when_a_check_failed(x: Pr, am: bool): bool = x.checks == "failed"
  pure def when_it_is_behind_the_default_branch_the_repository_requires_it_up_to_date_to_merge_and_that_alone_keeps_it_from_being_ready_to_merge(x: Pr, am: bool): bool =
    x.behind and readyToMerge(x, am)
  pure def when_auto_merge_is_off_or_a_person_holds_its_merge(x: Pr, am: bool): bool = not(am) or x.held
  pure def when_two_merges_of_it_have_failed_this_round(x: Pr, am: bool): bool = x.mergeFails >= MERGE_TRIES
  pure def when_github_blocks_the_merge_and_linewright_has_not_commented_on_it_this_round_naming_what_github_requires(x: Pr, am: bool): bool =
    x.github == "blocks" and not(x.commented)
  pure def when_github_blocks_the_merge_or_has_not_reported_whether_it_allows_it(x: Pr, am: bool): bool =
    x.github != "allows"

  pure def does_starts_a_rework_run(x: Pr, am: bool): str = "rework"
  pure def does_starts_an_update_run(x: Pr, am: bool): str = "update"
  pure def does_waits_for_its_checks(x: Pr, am: bool): str = "wait"
  pure def does_hands_it_back_naming_the_checks_still_running(x: Pr, am: bool): str = "hand back"
  pure def does_starts_a_fix_ci_run(x: Pr, am: bool): str = "fix"
  pure def does_hands_it_back_naming_the_failing_checks(x: Pr, am: bool): str = "hand back"
  pure def does_marks_it_ready_for_review(x: Pr, am: bool): str = "ready"
  pure def does_waits_for_review(x: Pr, am: bool): str = "wait"
  pure def does_hands_it_back_with_github_s_reason(x: Pr, am: bool): str = "hand back"
  pure def does_comments_naming_what_github_requires(x: Pr, am: bool): str = "comment on block"
  pure def does_waits_for_what_github_requires(x: Pr, am: bool): str = "wait"
  pure def does_merges_it(x: Pr, am: bool): str = "merge"

  /// P ## Working a pull request "Ready to merge": "it is ready for review",
  /// "no person holds its merge", "it has everything the repository requires
  /// before a merge" and, with auto-merge off, "a person has approved it";
  /// being behind is its own fact. That "its checks have passed" and "it has
  /// no feedback" is the row's place in the table.
  pure def readyToMerge(x: Pr, am: bool): bool =
    not(x.draft) and not(x.held) and x.github != "blocks" and (am or x.approved)


  /// R ## Assignment "Handing back": a comment saying why, linewright:stalled
  /// for a stall, then the unassignment ("the assignment" comes last, R ##
  /// Writes).
  pure val HAND_BACK = ["comment", "stalled", "unassign"]

  /// A new head: its checks start again, and so do the facts about it.
  pure def pushed(x: Pr): Pr = { ...x, checks: "none", graceOver: false, timedOut: false,
                                 ranSincePush: false, github: "unknown" }

  /// P ## Working a pull request "A round": "A round starts when Linewright
  /// opens the pull request, when a rework run on it ends and when Linewright
  /// is assigned to it again."
  pure def newRound(x: Pr): Pr = { ...x, commented: false, fixes: 0, mergeFails: 0 }

  /// Where the pull request rests (S6): merged or closed; handed back or
  /// unassigned; or waiting, ready for review, on the maintainer, on a person
  /// who holds its merge, or on what GitHub requires.
  pure def rests(x: Pr, am: bool): bool = or {
    x.status != "open",
    not(x.lw),
    not(x.draft) and (not(am) or x.held or x.github == "blocks"),
  }

  // ===========================================================================
  // State
  // ===========================================================================

  var pr: Pr
  var am: bool
  /// The run in flight: "", "rework", "update" or "fix".
  var inFlight: str
  /// The writes of a series still to make.
  var series: List[str]
  var peopleLeft: int
  var stopsLeft: int
  var lateLeft: int
  /// S5: a stop in the middle of a series, and what the series would have
  /// left: (Linewright assigned, linewright:stalled, a draft, commented).
  var stop: { active: bool, personSince: bool, end: (bool, bool, bool, bool) }
  /// S6 no flapping: the runs Linewright has started since a person, CI or
  /// GitHub last acted, and whether it started one twice.
  var runsSince: Set[str]
  var flapped: bool

  pure val NO_STOP = { active: false, personSince: false, end: (false, false, false, false) }

  action init = {
    nondet a = Set(true, false).oneOf()
    nondet u = Set(true, false).oneOf()
    all {
      pr' = { status: "open", lw: true, draft: true, checks: "none", graceOver: false,
              timedOut: false, feedback: false, conflict: false, behind: false,
              approved: false, unmergeable: u,
              ranSincePush: false, held: false, github: "unknown", commented: false,
              fixes: 0, mergeFails: 0, stalled: false },
      am' = a,
      inFlight' = "",
      series' = [],
      peopleLeft' = PEOPLE_ACTS,
      stopsLeft' = STOPS,
      lateLeft' = LATE_CHECKS,
      stop' = NO_STOP,
      runsSince' = Set(),
      flapped' = false,
    }
  }

  // ===========================================================================
  // Linewright
  // ===========================================================================

  pure def applyWrite(x: Pr, w: str): Pr =
    if (w == "stalled") { ...x, stalled: true }
    else if (w == "unassign") { ...x, lw: false }
    else if (w == "ready") { ...x, draft: false }
    else if (w == "comment on block") { ...x, commented: true }
    else x

  /// The next write of the series in progress.
  action writes: bool = {
    // GitHub may refuse a merge it allowed a moment before.
    nondet ok = Set(true, false).oneOf()
    all {
      series.length() > 0,
      val w = series.head()
      pr' = if (w != "merge") applyWrite(pr, w)
            else if (ok and pr.status == "open") { ...pr, status: "merged" }
            else { ...pr, mergeFails: pr.mergeFails + 1 },
      series' = series.tail(),
      stop' = stop,
      inFlight' = inFlight, am' = am, peopleLeft' = peopleLeft, stopsLeft' = stopsLeft,
      lateLeft' = lateLeft, runsSince' = runsSince, flapped' = flapped,
    }
  }

  /// The run in flight ends: R ## Working an item, "a run on it cannot
  /// finish" hands it back; otherwise it pushes, or finds nothing to push.
  /// "Feedback posted while a run works goes to its session before the run
  /// pushes". P ## Keeping up to date "Which runs": an update or a fix-ci run
  /// brings the branch up to date; a rework run does "where the branch
  /// conflicts with the default branch or cannot be merged by a method the
  /// repository allows", as it started, read as it may or may not have, and
  /// never for being behind.
  action runEnds: bool = {
    nondet end = Set("push", "nothing", "cannot finish").oneOf()
    // an update may leave it as unmergeable by a method as it was
    nondet repaired = Set(true, false).oneOf()
    nondet reworkUpdated = Set(true, false).oneOf()
    val updated = inFlight != "rework" or reworkUpdated
    val done =
      if (inFlight == "rework") { if (reworkUpdated) { ...pr, conflict: false } else pr }
      else { ...pr, behind: false, conflict: false }
    // a rework run that ends done answers the feedback it was given; read the
    // old way, any push did
    val answered = if (FEEDBACK_UNTIL_REWORK_DONE) inFlight == "rework" else end == "push"
    val onHead = if (end == "push") pushed(done) else done
    val after = if (answered) { ...onHead, feedback: false } else onHead
    val counted =
      if (inFlight == "rework") newRound(after)
      else if (inFlight == "fix") { ...after, fixes: after.fixes + 1 }
      else after
    // bringing the branch up to date may make it mergeable
    val brought = { ...counted, ranSincePush: true,
                    unmergeable: counted.unmergeable and not(updated and repaired) }
    all {
      inFlight != "",
      series.length() == 0,
      // An update or a fix finds nothing to push only where there is nothing
      // to bring up to date; a rework may answer without a push.
      end == "nothing" implies (inFlight == "rework"
                                or (inFlight == "update" and not(pr.behind or pr.conflict))),
      pr' = if (end == "cannot finish") pr else brought,
      series' = if (end == "cannot finish") HAND_BACK else [],
      inFlight' = "",
      am' = am, peopleLeft' = peopleLeft, stopsLeft' = stopsLeft, lateLeft' = lateLeft,
      stop' = stop, runsSince' = runsSince, flapped' = flapped,
    }
  }

  /// Linewright takes the first row that holds, where it does more than wait.
  action takesRow: bool = {
    val t = todo(pr, am)
    val isRun = Set("rework", "update", "fix").contains(t)
    all {
      inFlight == "",
      series.length() == 0,
      pr.status == "open",
      pr.lw,
      t != "wait",
      inFlight' = if (isRun) t else "",
      series' = if (isRun) [] else if (t == "hand back") HAND_BACK else [t],
      runsSince' = if (isRun) runsSince.union(Set(t)) else runsSince,
      flapped' = (flapped or (isRun and runsSince.contains(t))),
      pr' = pr, am' = am, peopleLeft' = peopleLeft, stopsLeft' = stopsLeft,
      lateLeft' = lateLeft, stop' = stop,
    }
  }

  action lwStep: bool = any { writes, runEnds, takesRow }

  /// Linewright stops: the rest of the series is not written, and the run in
  /// flight ends with the process.
  action lwStops: bool = {
    val rest = series.foldl(pr, (x, w) => applyWrite(x, w))
    all {
      stopsLeft > 0,
      series.length() > 0 or inFlight != "",
      stop' = if (series.length() > 0)
                { active: true, personSince: false,
                  end: (rest.lw, rest.stalled, rest.draft, rest.commented) }
              else NO_STOP,
      series' = [],
      inFlight' = "",
      stopsLeft' = stopsLeft - 1,
      runsSince' = Set(),
      pr' = pr, am' = am, peopleLeft' = peopleLeft, lateLeft' = lateLeft, flapped' = flapped,
    }
  }

  // ===========================================================================
  // CI and GitHub
  // ===========================================================================

  action ciMoves(x: Pr): bool = all {
    pr' = x,
    runsSince' = Set(),
    am' = am, inFlight' = inFlight, series' = series, peopleLeft' = peopleLeft,
    stopsLeft' = stopsLeft, lateLeft' = lateLeft, stop' = stop, flapped' = flapped,
  }

  /// Checks appear within the grace period or it passes; they finish or time
  /// out; GitHub decides whether it allows the merge.
  action ciStep: bool = any {
    all { pr.checks == "none", not(pr.graceOver), ciMoves({ ...pr, checks: "running" }) },
    all { pr.checks == "none", not(pr.graceOver), ciMoves({ ...pr, graceOver: true }) },
    all { pr.checks == "running", ciMoves({ ...pr, checks: "green" }) },
    all { pr.checks == "running", ciMoves({ ...pr, checks: "failed" }) },
    all { pr.checks == "running", not(pr.timedOut), ciMoves({ ...pr, timedOut: true }) },
    all { pr.github == "unknown", ciMoves({ ...pr, github: "allows" }) },
    all { pr.github == "unknown", ciMoves({ ...pr, github: "blocks" }) },
  }

  /// Checks that appear only once the pull request is ready, after the grace
  /// period.
  action lateChecks: bool = all {
    lateLeft > 0,
    pr.checks == "none",
    pr.graceOver,
    pr.status == "open",
    pr' = { ...pr, checks: "running" },
    lateLeft' = lateLeft - 1,
    runsSince' = Set(),
    am' = am, inFlight' = inFlight, series' = series, peopleLeft' = peopleLeft,
    stopsLeft' = stopsLeft, stop' = stop, flapped' = flapped,
  }

  // ===========================================================================
  // People
  // ===========================================================================

  /// A person acts: R ## Steering, open | "unassigns Linewright" | "cancels a
  /// run on the item"; "merges or closes a pull request" | "cancels a run on
  /// it".
  action personMoves(x: Pr): bool = all {
    peopleLeft > 0,
    pr.status == "open",
    pr' = x,
    inFlight' = if (x.lw and x.status == "open") inFlight else "",
    peopleLeft' = peopleLeft - 1,
    stop' = { ...stop, personSince: true },
    runsSince' = Set(),
    am' = am, series' = series, stopsLeft' = stopsLeft, lateLeft' = lateLeft,
    flapped' = flapped,
  }

  action peopleStep: bool = any {
    // a comment or a review asking for changes: feedback, and its author
    // holds the merge
    personMoves({ ...pr, feedback: true, held: true }),
    // a review that only approves
    personMoves({ ...pr, held: false, approved: true }),
    // a push of their own
    personMoves(pushed(pr)),
    // the default branch moves: the branch falls behind, where the repository
    // requires it up to date, or comes to conflict with it
    personMoves({ ...pr, behind: true, github: "unknown" }),
    personMoves({ ...pr, conflict: true, github: "unknown" }),
    // they change what GitHub requires, or meet it
    personMoves({ ...pr, github: "blocks" }),
    personMoves({ ...pr, github: "allows" }),
    // R ## Steering, open | "assigns Linewright to a pull request Linewright
    // opened" | "goes on with it"; a round starts (A round), and the timeout
    // counts from the assignment
    all { not(pr.lw), personMoves(newRound({ ...pr, lw: true, timedOut: false })) },
    all { pr.lw, personMoves({ ...pr, lw: false }) },
    personMoves({ ...pr, draft: true }),
    personMoves({ ...pr, status: "closed" }),
    personMoves({ ...pr, status: "merged" }),
  }

  action step: bool = any { lwStep, lwStops, ciStep, lateChecks, peopleStep }

  // ===========================================================================
  // Properties
  // ===========================================================================

  val vars = (pr, am, inFlight, series, peopleLeft, stopsLeft, lateLeft, stop, runsSince, flapped)

  /// Linewright has nothing to do: no run in flight, no write to make, and
  /// the first row that holds waits, where it works the pull request.
  val quiescent: bool = and {
    inFlight == "",
    series.length() == 0,
    (pr.status == "open" and pr.lw) implies todo(pr, am) == "wait",
  }
  /// CI and GitHub have nothing more to do on their own.
  val ciQuiet: bool = and {
    not(pr.checks == "none" and not(pr.graceOver)),
    pr.checks != "running",
    pr.github != "unknown",
  }

  /// S1: a witness that row k is the first that holds while Linewright works
  /// the pull request and is idle. Each is expected to be violated.
  def rowUnreached(k: int): bool =
    not(pr.status == "open" and pr.lw and inFlight == "" and series.length() == 0
        and firstRow(pr, am) == k + 1)
  val row0Unreached = rowUnreached(0)
  val row1Unreached = rowUnreached(1)
  val row2Unreached = rowUnreached(2)
  val row3Unreached = rowUnreached(3)
  val row4Unreached = rowUnreached(4)
  val row5Unreached = rowUnreached(5)
  val row6Unreached = rowUnreached(6)
  val row7Unreached = rowUnreached(7)
  val row8Unreached = rowUnreached(8)
  val row9Unreached = rowUnreached(9)
  val row10Unreached = rowUnreached(10)
  val row11Unreached = rowUnreached(11)
  val row12Unreached = rowUnreached(12)
  val row13Unreached = rowUnreached(13)
  val row14Unreached = rowUnreached(14)

  /// Fix and merge attempts are bounded: ci-attempts fix-ci runs and two
  /// failed merges a round.
  val attemptsBounded: bool = pr.fixes <= CI_ATTEMPTS and pr.mergeFails <= MERGE_TRIES

  /// S6, its safety half: wherever Linewright, CI and GitHub have nothing more
  /// to do, the pull request rests.
  val restsWhereQuiet: bool = (quiescent and ciQuiet) implies rests(pr, am)

  /// S6 no flapping: with people, CI and GitHub idle, Linewright never starts
  /// the same run twice.
  val noFlap: bool = not(flapped)

  val invariants: bool = and { attemptsBounded, restsWhereQuiet, noFlap }

  /// S6 progress: under weak fairness of Linewright, CI and GitHub,
  /// Linewright eventually has nothing more to do, for good.
  temporal progress: bool =
    (weakFair(lwStep, vars) and weakFair(ciStep, vars)) implies eventually(always(quiescent))

  /// S5 crash-safety: from a stop in the middle of a series, with people
  /// idle, Linewright reaches what the series would have left, or the pull
  /// request comes to rest.
  temporal crashSafety: bool =
    (weakFair(lwStep, vars) and weakFair(ciStep, vars)) implies
      always(stop.active implies eventually(or {
        stop.personSince,
        (pr.lw, pr.stalled, pr.draft, pr.commented) == stop.end,
        rests(pr, am),
      }))
}
