//! What a session hands back: its final message is a JSON object the manager
//! parses into one of these types (`docs/specs/architecture.md`, "Agents read
//! through the brief and write through the result"). A message that does not
//! parse yields the error to send back to the session.

use std::collections::BTreeMap;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Deserializer};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TriageOutcome {
    ReadyForAgent,
    NeedsInfo,
    ReadyForHuman,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct TriageResult {
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

#[derive(Deserialize)]
struct RawTickets {
    tickets: Vec<TicketDraft>,
    #[serde(deserialize_with = "trimmed")]
    comment: String,
}

/// Tickets ordered so that every ticket comes after those blocking it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tickets {
    pub tickets: Vec<Ticket>,
    pub comment: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ticket {
    pub title: String,
    pub body: String,
    /// The positions of the tickets blocking this one, all earlier in the
    /// order.
    pub blocked_by: Vec<usize>,
}

/// A spec change a session found the spec needs.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct SpecGap {
    pub title: String,
    pub body: String,
}

/// A pull request's title as the session wrote it: trimmed, and not empty.
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
    /// A spec gap blocks the ticket.
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

/// The result of work on an open pull request: rework, a CI fix or a
/// conflict resolved.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(tag = "outcome", rename_all = "kebab-case")]
pub enum PullWorkResult {
    Done {
        #[serde(deserialize_with = "trimmed")]
        comment: String,
        /// The pull request's body rewritten, when the change made it out of
        /// date.
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

/// The label an issue a workflow hands back is filed with.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum WorkflowIssueLabel {
    ReadyForAgent,
    ReadyForHuman,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct WorkflowIssue {
    pub title: String,
    #[serde(deserialize_with = "trimmed")]
    pub body: String,
    pub label: WorkflowIssueLabel,
}

/// What a workflow's run did and found, and the issues it hands back.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct WorkflowResult {
    pub summary: String,
    #[serde(default)]
    pub issues: Vec<WorkflowIssue>,
}

/// Text a session wrote for people, without the whitespace around it.
fn trimmed<'de, D: Deserializer<'de>>(deserializer: D) -> Result<String, D::Error> {
    Ok(String::deserialize(deserializer)?.trim().to_string())
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

pub fn parse_tickets(message: &str) -> Result<Tickets, String> {
    let raw: RawTickets = parse(message)?;
    let mut by_id = BTreeMap::new();
    for ticket in &raw.tickets {
        if ticket.title.trim().is_empty() {
            return Err(format!("Ticket {:?} has an empty title.", ticket.id));
        }
        if by_id.insert(ticket.id.clone(), ticket).is_some() {
            return Err(format!("Two tickets have the id {:?}.", ticket.id));
        }
    }
    for ticket in &raw.tickets {
        if let Some(unknown) = ticket.blocked_by.iter().find(|b| !by_id.contains_key(*b)) {
            return Err(format!(
                "Ticket {:?} is blocked by {unknown:?}, which is not a ticket id.",
                ticket.id
            ));
        }
    }
    // Kahn's algorithm, keeping the session's order among unblocked tickets.
    let mut placed: BTreeMap<&str, usize> = BTreeMap::new();
    let mut ordered = vec![];
    while ordered.len() < raw.tickets.len() {
        let next = raw.tickets.iter().find(|t| {
            !placed.contains_key(t.id.as_str())
                && t.blocked_by.iter().all(|b| placed.contains_key(b.as_str()))
        });
        let Some(next) = next else {
            return Err("The tickets' blocked_by edges form a cycle.".into());
        };
        placed.insert(&next.id, ordered.len());
        ordered.push(Ticket {
            title: next.title.clone(),
            body: next.body.clone(),
            blocked_by: next.blocked_by.iter().map(|b| placed[b.as_str()]).collect(),
        });
    }
    Ok(Tickets {
        tickets: ordered,
        comment: raw.comment,
    })
}

pub fn parse<T: DeserializeOwned>(message: &str) -> Result<T, String> {
    let json = extract_json(message).ok_or_else(|| {
        "The final message holds no JSON object. End with the result as a single JSON object."
            .to_string()
    })?;
    serde_json::from_str(json)
        .map_err(|e| format!("The result JSON does not match the contract: {e}."))
}

/// The JSON object in a final message, for the session's record.
pub fn json_value(message: &str) -> Option<serde_json::Value> {
    serde_json::from_str(extract_json(message)?).ok()
}

/// The JSON object in a final message: the whole message, the last fenced
/// block, or the span from the first `{` to the last `}`.
fn extract_json(message: &str) -> Option<&str> {
    let trimmed = message.trim();
    if trimmed.starts_with('{') && trimmed.ends_with('}') {
        return Some(trimmed);
    }
    if let Some(end) = trimmed.rfind("```") {
        let before = &trimmed[..end];
        if let Some(start) = before.rfind("```") {
            let block = &before[start + 3..];
            let block = block.strip_prefix("json").unwrap_or(block).trim();
            if block.starts_with('{') {
                return Some(block);
            }
        }
    }
    let start = trimmed.find('{')?;
    let end = trimmed.rfind('}')?;
    (start < end).then(|| &trimmed[start..=end])
}
