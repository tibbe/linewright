/// A recorded transcript's first turn, as Claude Code streamed it.
pub const TURN_1: &str = include_str!("fixtures/transcript/turn-1.jsonl");
/// Its second turn.
pub const TURN_2: &str = include_str!("fixtures/transcript/turn-2.jsonl");

/// The recorded first turn of #1's triage, in the pieces a test writes it in.
pub struct Turn1 {
    /// The marker, the start event and the first Bash call, with its result.
    pub through_the_first_bash_result: String,
    /// The second Bash call, without its result.
    pub the_second_bash_call: String,
    /// The second Bash call's result.
    pub its_result: String,
    /// Everything after the second Bash call's result.
    pub the_rest: String,
}

impl Turn1 {
    /// Everything after the first Bash call's result.
    pub fn after_the_first_bash_result(&self) -> String {
        [&self.the_second_bash_call, &self.its_result, &self.the_rest]
            .into_iter()
            .cloned()
            .collect()
    }
}

pub fn turn_1() -> Turn1 {
    let lines: Vec<String> = TURN_1.lines().map(|line| format!("{line}\n")).collect();
    Turn1 {
        through_the_first_bash_result: lines[..4].concat(),
        the_second_bash_call: lines[4..6].concat(),
        its_result: lines[6].clone(),
        the_rest: lines[7..].concat(),
    }
}
