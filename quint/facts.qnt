// -*- mode: Bluespec; -*-
/**
 * What GitHub records about an item, shared by the spec's rules
 * (linewright.qnt) and the model files, which build their items from it.
 */
module facts {

  /// The status labels (R ## Labels).
  pure val NEEDS_TRIAGE = "needs-triage"
  pure val NEEDS_INFO = "needs-info"
  pure val READY_FOR_AGENT = "ready-for-agent"
  pure val READY_FOR_HUMAN = "ready-for-human"

  type Item = {
    exists: bool,          // created yet (an item Linewright or a person may create later)
    pr: bool,              // a pull request; else an issue
    byLw: bool,            // Linewright created (opened) it
    born: int,             // when it was created, for "the first issue"
    open: bool,
    merged: bool,          // a pull request merged into the default branch
    closedBy: str,         // "", "person", "linewright" or "github"
    notPlanned: bool,      // closed as not planned
    status: str,           // the status label it holds: the newest, or ""
    older: str,            // an older status label it still carries, or ""
    designed: bool,        // linewright:designed
    ownDesign: bool,       // ... set by its own design (R "Handing back")
    stalled: bool,         // linewright:stalled
    required: bool,        // the required label
    lw: bool,              // assigned to Linewright
    byHand: bool,          // ... and a person assigned it
    alice: bool,           // a person assigned, by a person
    asked: bool,           // people Linewright assigned, asking them (T "Answers")
    unassignedSince: bool, // Linewright was unassigned from it since its status
                           // label or the required label was last applied
    worked: bool,          // R ## Managed items "Worked"
    letGo: bool,           // Linewright's last act on it was letting go of it
    mergedSince: bool,     // a pull request that closes it merged since it was
                           // last opened or given a status label
    noSince: bool,         // a pull request Linewright opened that closes it was
                           // closed unmerged by a person since then
    refs: int,             // the spec change it references, or 0
    parent: int,           // the issue whose sub-issue it is, or 0
    parentByLw: bool,      // Linewright added it to those sub-issues
    blockedBy: Set[int],   // the issues it is linked as blocked by
    reply: bool,           // a comment by a person after Linewright's needs-info question
    feedback: bool,        // a comment by a person after Linewright's last push
    spec: bool,            // a spec change: merged, not Linewright's, touched a spec file
    inLookback: bool,      // merged within the lookback window
    closes: Set[int],      // the issues a pull request closes
  }

  pure val ABSENT: Item = {
    exists: false, pr: false, byLw: false, born: 0, open: true, merged: false,
    closedBy: "", notPlanned: false, status: "", older: "", designed: false,
    ownDesign: false, stalled: false, required: false, lw: false, byHand: false,
    alice: false, asked: false, unassignedSince: false, worked: false,
    letGo: false, mergedSince: false, noSince: false, refs: 0, parent: 0,
    parentByLw: false, blockedBy: Set(), reply: false, feedback: false,
    spec: false, inLookback: false, closes: Set(),
  }

  /// Where a trace starts: the items, and whether require-label and
  /// --auto-merge are set.
  type Start = { items: int -> Item, requireLabel: bool, autoMerge: bool }
}
