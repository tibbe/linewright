use std::collections::BTreeMap;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Deserializer};

use crate::domain::Login;

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(tag = "outcome", rename_all = "kebab-case")]
pub enum TriageOutcome {
    ReadyForAgent,
    NeedsInfo {
        /// The people the question is for: none asks the reporter.
        #[serde(default, deserialize_with = "logins")]
        ask: Vec<Login>,
    },
    ReadyForHuman,
}

impl TriageOutcome {
    /// Only `ready-for-agent` still asks for Linewright's work.
    pub fn asks_for_linewright(&self) -> bool {
        *self == TriageOutcome::ReadyForAgent
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct TriageResult {
    #[serde(flatten)]
    pub outcome: TriageOutcome,
    pub category: String,
    #[serde(deserialize_with = "trimmed")]
    pub comment: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct TicketDraft {
    pub id: String,
    pub title: String,
    pub body: String,
    #[serde(default)]
    pub blocked_by: Vec<String>,
}

/// What a design session answers (docs/specs/design.md, "Designing"), with
/// `C` what a designed item is cut into.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(tag = "outcome", rename_all = "kebab-case")]
pub enum DesignResult<C> {
    Designed {
        #[serde(deserialize_with = "trimmed")]
        design: String,
        #[serde(flatten)]
        cut: C,
        #[serde(default)]
        spec_gaps: Vec<SpecGap>,
    },
    NothingToBuild {
        #[serde(deserialize_with = "trimmed")]
        reason: String,
    },
    Blocked {
        #[serde(deserialize_with = "trimmed")]
        question: String,
        #[serde(default)]
        spec_gaps: Vec<SpecGap>,
    },
    GaveUp {
        #[serde(deserialize_with = "trimmed")]
        reason: String,
    },
}

/// The tickets an item's work is cut into, as a spec change's always is
/// (docs/specs/design.md, "Cutting").
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct Cut {
    pub tickets: Tickets,
    /// Why the work is cut into these tickets.
    #[serde(deserialize_with = "trimmed")]
    pub comment: String,
}

/// What an issue's work is: built as it is, in one session, or cut into
/// tickets (docs/specs/design.md, "Cutting").
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(try_from = "RawIssueWork")]
pub enum IssueWork {
    AsItIs,
    Cut(Cut),
}

#[derive(Deserialize)]
struct RawIssueWork {
    #[serde(default)]
    tickets: Vec<TicketDraft>,
    #[serde(default)]
    comment: Option<String>,
}

impl TryFrom<RawIssueWork> for IssueWork {
    type Error = String;

    fn try_from(raw: RawIssueWork) -> Result<IssueWork, String> {
        if raw.tickets.is_empty() {
            return Ok(IssueWork::AsItIs);
        }
        let comment = raw
            .comment
            .map(|c| c.trim().to_string())
            .filter(|c| !c.is_empty())
            .ok_or("`comment` is missing: say why the work is cut into these tickets")?;
        Ok(IssueWork::Cut(Cut {
            tickets: raw.tickets.try_into()?,
            comment,
        }))
    }
}

/// At least one ticket, ordered so that every ticket comes after those
/// blocking it.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(try_from = "Vec<TicketDraft>")]
pub struct Tickets(Vec<Ticket>);

impl Tickets {
    pub fn in_order(&self) -> &[Ticket] {
        &self.0
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ticket {
    pub title: String,
    pub body: String,
    /// The positions of the tickets blocking this one, all earlier in the
    /// order.
    pub blocked_by: Vec<usize>,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct SpecGap {
    pub title: String,
    pub body: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(try_from = "String")]
pub struct PrTitle(String);

impl TryFrom<String> for PrTitle {
    type Error = &'static str;

    fn try_from(title: String) -> Result<PrTitle, Self::Error> {
        match title.trim() {
            "" => Err("`pr_title` is empty"),
            title => Ok(PrTitle(title.to_string())),
        }
    }
}

impl From<PrTitle> for String {
    fn from(title: PrTitle) -> String {
        title.0
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(tag = "outcome", rename_all = "kebab-case")]
pub enum ImplementResult {
    Done {
        pr_title: PrTitle,
        pr_body: String,
        #[serde(default)]
        spec_gaps: Vec<SpecGap>,
    },
    Blocked {
        question: String,
        #[serde(default)]
        spec_gaps: Vec<SpecGap>,
    },
    GaveUp {
        reason: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct ThreadReply {
    pub thread: String,
    #[serde(deserialize_with = "trimmed")]
    pub body: String,
    #[serde(default)]
    pub resolve: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(tag = "outcome", rename_all = "kebab-case")]
pub enum PullWorkResult {
    Done {
        #[serde(deserialize_with = "trimmed")]
        comment: String,
        #[serde(default)]
        pr_body: Option<String>,
        #[serde(default)]
        replies: Vec<ThreadReply>,
        #[serde(default)]
        spec_gaps: Vec<SpecGap>,
    },
    Blocked {
        question: String,
        #[serde(default)]
        spec_gaps: Vec<SpecGap>,
    },
    GaveUp {
        reason: String,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AutomationIssueLabel {
    ReadyForAgent,
    ReadyForHuman,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct AutomationIssue {
    pub title: String,
    #[serde(deserialize_with = "trimmed")]
    pub body: String,
    pub label: AutomationIssueLabel,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct AutomationResult {
    pub summary: String,
    #[serde(default)]
    pub issues: Vec<AutomationIssue>,
}

fn trimmed<'de, D: Deserializer<'de>>(deserializer: D) -> Result<String, D::Error> {
    Ok(String::deserialize(deserializer)?.trim().to_string())
}

/// GitHub logins, written with or without a leading `@`, each once.
fn logins<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Vec<Login>, D::Error> {
    let mut logins: Vec<Login> = vec![];
    for written in Vec::<String>::deserialize(deserializer)? {
        let login = Login::parse(written.trim().trim_start_matches('@')).ok_or_else(|| {
            serde::de::Error::custom(format!("{written:?} is not a GitHub login"))
        })?;
        if !logins.contains(&login) {
            logins.push(login);
        }
    }
    Ok(logins)
}

pub fn parse_triage(message: &str, categories: &[String]) -> Result<TriageResult, String> {
    let result: TriageResult = parse(message)?;
    if !categories.contains(&result.category) {
        return Err(format!(
            "`category` is {:?}; it must be one of {}.",
            result.category,
            categories
                .iter()
                .map(|c| format!("{c:?}"))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    Ok(result)
}

impl TryFrom<Vec<TicketDraft>> for Tickets {
    type Error = String;

    fn try_from(drafts: Vec<TicketDraft>) -> Result<Tickets, String> {
        ordered(&drafts).map(Tickets)
    }
}

fn ordered(drafts: &[TicketDraft]) -> Result<Vec<Ticket>, String> {
    if drafts.is_empty() {
        return Err(
            "`tickets` is empty: a spec change's work is always at least one ticket; \
             when the code already does what it asks, answer `nothing-to-build`"
                .into(),
        );
    }
    let mut by_id = BTreeMap::new();
    for ticket in drafts {
        if ticket.title.trim().is_empty() {
            return Err(format!("ticket {:?} has an empty title", ticket.id));
        }
        if by_id.insert(ticket.id.clone(), ticket).is_some() {
            return Err(format!("two tickets have the id {:?}", ticket.id));
        }
    }
    for ticket in drafts {
        if let Some(unknown) = ticket.blocked_by.iter().find(|b| !by_id.contains_key(*b)) {
            return Err(format!(
                "ticket {:?} is blocked by {unknown:?}, which is not a ticket id",
                ticket.id
            ));
        }
    }
    // Kahn's algorithm, keeping the session's order among unblocked tickets.
    let mut placed: BTreeMap<&str, usize> = BTreeMap::new();
    let mut ordered = vec![];
    while ordered.len() < drafts.len() {
        let next = drafts.iter().find(|t| {
            !placed.contains_key(t.id.as_str())
                && t.blocked_by.iter().all(|b| placed.contains_key(b.as_str()))
        });
        let Some(next) = next else {
            return Err("the tickets' blocked_by edges form a cycle".into());
        };
        placed.insert(&next.id, ordered.len());
        ordered.push(Ticket {
            title: next.title.clone(),
            body: next.body.clone(),
            blocked_by: next.blocked_by.iter().map(|b| placed[b.as_str()]).collect(),
        });
    }
    Ok(ordered)
}

pub fn parse<T: DeserializeOwned>(message: &str) -> Result<T, String> {
    let json = extract_json(message).ok_or_else(|| {
        "The final message holds no JSON object. End with the result as a single JSON object."
            .to_string()
    })?;
    serde_json::from_str(json)
        .map_err(|e| format!("The result JSON does not match the contract: {e}."))
}

pub fn json_value(message: &str) -> Option<serde_json::Value> {
    serde_json::from_str(extract_json(message)?).ok()
}

/// The result's JSON in `message`, found as docs/specs/runs.md ("Contract")
/// says: the whole message when it is one object, else its last fenced code
/// block when that holds one, else from its first `{` to its last `}`.
fn extract_json(message: &str) -> Option<&str> {
    let trimmed = message.trim();
    if is_object(trimmed) {
        return Some(trimmed);
    }
    if let Some(block) = last_fenced_block(trimmed).map(str::trim)
        && is_object(block)
    {
        return Some(block);
    }
    let start = trimmed.find('{')?;
    let end = trimmed.rfind('}')?;
    (start < end).then(|| &trimmed[start..=end])
}

fn is_object(text: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(text).is_ok_and(|v| v.is_object())
}

/// The content of the last fenced code block in `text`, whatever its info
/// string ([CommonMark](https://spec.commonmark.org/0.31.2/#fenced-code-blocks)).
fn last_fenced_block(text: &str) -> Option<&str> {
    let mut last = None;
    // The open fence's character and length, and where its content starts.
    let mut open: Option<(char, usize, usize)> = None;
    let mut offset = 0;
    for line in text.split_inclusive('\n') {
        let fence = line.trim_start();
        let fence_char = fence.chars().next().filter(|c| *c == '`' || *c == '~');
        let fence_len = fence_char.map_or(0, |c| fence.chars().take_while(|x| *x == c).count());
        match (open, fence_char) {
            (None, Some(c)) if fence_len >= 3 => open = Some((c, fence_len, offset + line.len())),
            (Some((c, len, start)), Some(d))
                if c == d && fence_len >= len && fence[fence_len..].trim().is_empty() =>
            {
                last = Some(&text[start..offset]);
                open = None;
            }
            _ => {}
        }
        offset += line.len();
    }
    last
}

#[cfg(test)]
mod tests {
    use super::*;

    const RESULT: &str = r#"{"summary": "Ran."}"#;

    #[test]
    fn a_message_braced_but_not_one_object_falls_through_to_its_last_fenced_block() {
        let message = format!(
            "{{ a draft }}\n\n```json\n{{\"summary\": \"Draft.\"}}\n```\n\n```\n{RESULT}\n```\n\nThat is {{ it }}"
        );
        assert_eq!(
            json_value(&message),
            Some(serde_json::from_str(RESULT).unwrap())
        );
        assert_eq!(parse::<AutomationResult>(&message).unwrap().summary, "Ran.");
    }

    #[test]
    fn a_last_fenced_block_with_any_info_string_is_found() {
        for fence in ["```JSON", "```", "~~~ json"] {
            let close = &fence[..3];
            let message = format!("Here it is:\n\n{fence}\n{RESULT}\n{close}\n");
            assert_eq!(
                parse::<AutomationResult>(&message).map(|r| r.summary),
                Ok("Ran.".to_string()),
                "{message}"
            );
        }
    }

    #[test]
    fn prose_around_a_bare_object_still_parses() {
        let message = format!("Done. The result:\n{RESULT}\nThanks.");
        assert_eq!(parse::<AutomationResult>(&message).unwrap().summary, "Ran.");
    }

    #[test]
    fn a_message_with_no_object_is_sent_back() {
        let error = parse::<AutomationResult>("I think it's a bug.").unwrap_err();
        assert!(error.contains("holds no JSON object"), "{error}");
    }
}
