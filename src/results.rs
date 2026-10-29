use std::collections::BTreeMap;

use std::sync::LazyLock;

use jsonschema::error::ValidationErrorKind;
use jsonschema::{Registry, Resource, ValidationError, Validator};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Deserializer};
use serde_json::Value;

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
        // The schema requires a `comment` with tickets.
        Ok(IssueWork::Cut(Cut {
            tickets: raw.tickets.try_into()?,
            comment: raw.comment.unwrap_or_default().trim().to_string(),
        }))
    }
}

/// At least one ticket, as the schemas require, ordered so that every ticket
/// comes after those blocking it.
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
#[serde(tag = "outcome", rename_all = "kebab-case")]
pub enum ImplementResult {
    Done {
        #[serde(deserialize_with = "trimmed")]
        pr_title: String,
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
    let mut by_id = BTreeMap::new();
    for ticket in drafts {
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

/// A result whose fields Linewright posts as Markdown, each of which may
/// refer to attachments (docs/specs/runs.md, "Attachments"). Titles are not
/// Markdown, and what Linewright does not post is not listed.
pub trait Posts {
    fn markdown(&mut self) -> Vec<&mut String>;
}

fn gap_bodies(gaps: &mut [SpecGap]) -> impl Iterator<Item = &mut String> {
    gaps.iter_mut().map(|gap| &mut gap.body)
}

/// `text`, followed by the bodies of `gaps`.
fn with_gaps<'a>(text: &'a mut String, gaps: &'a mut [SpecGap]) -> Vec<&'a mut String> {
    [text].into_iter().chain(gap_bodies(gaps)).collect()
}

impl Posts for TriageResult {
    fn markdown(&mut self) -> Vec<&mut String> {
        vec![&mut self.comment]
    }
}

impl Posts for Cut {
    fn markdown(&mut self) -> Vec<&mut String> {
        self.tickets
            .0
            .iter_mut()
            .map(|ticket| &mut ticket.body)
            .chain([&mut self.comment])
            .collect()
    }
}

impl Posts for IssueWork {
    fn markdown(&mut self) -> Vec<&mut String> {
        match self {
            IssueWork::AsItIs => vec![],
            IssueWork::Cut(cut) => cut.markdown(),
        }
    }
}

impl<C: Posts> Posts for DesignResult<C> {
    fn markdown(&mut self) -> Vec<&mut String> {
        match self {
            DesignResult::Designed {
                design,
                cut,
                spec_gaps,
            } => [design]
                .into_iter()
                .chain(cut.markdown())
                .chain(gap_bodies(spec_gaps))
                .collect(),
            DesignResult::NothingToBuild { reason } | DesignResult::GaveUp { reason } => {
                vec![reason]
            }
            DesignResult::Blocked {
                question,
                spec_gaps,
            } => [question]
                .into_iter()
                .chain(gap_bodies(spec_gaps))
                .collect(),
        }
    }
}

impl Posts for ImplementResult {
    fn markdown(&mut self) -> Vec<&mut String> {
        match self {
            ImplementResult::Done {
                pr_body, spec_gaps, ..
            } => with_gaps(pr_body, spec_gaps),
            ImplementResult::Blocked {
                question,
                spec_gaps,
            } => [question]
                .into_iter()
                .chain(gap_bodies(spec_gaps))
                .collect(),
            ImplementResult::GaveUp { reason } => vec![reason],
        }
    }
}

impl Posts for PullWorkResult {
    fn markdown(&mut self) -> Vec<&mut String> {
        match self {
            PullWorkResult::Done {
                comment,
                pr_body,
                replies,
                spec_gaps,
            } => [comment]
                .into_iter()
                .chain(pr_body)
                .chain(replies.iter_mut().map(|reply| &mut reply.body))
                .chain(gap_bodies(spec_gaps))
                .collect(),
            PullWorkResult::Blocked {
                question,
                spec_gaps,
            } => [question]
                .into_iter()
                .chain(gap_bodies(spec_gaps))
                .collect(),
            PullWorkResult::GaveUp { reason } => vec![reason],
        }
    }
}

impl Posts for AutomationResult {
    fn markdown(&mut self) -> Vec<&mut String> {
        self.issues
            .iter_mut()
            .map(|issue| &mut issue.body)
            .collect()
    }
}

/// A result that may hand back spec gaps (docs/specs/runs.md, "Spec gaps").
pub trait SpecGaps {
    fn spec_gaps_mut(&mut self) -> Option<&mut Vec<SpecGap>>;
}

impl<C> SpecGaps for DesignResult<C> {
    fn spec_gaps_mut(&mut self) -> Option<&mut Vec<SpecGap>> {
        match self {
            DesignResult::Designed { spec_gaps, .. } | DesignResult::Blocked { spec_gaps, .. } => {
                Some(spec_gaps)
            }
            DesignResult::NothingToBuild { .. } | DesignResult::GaveUp { .. } => None,
        }
    }
}

impl SpecGaps for ImplementResult {
    fn spec_gaps_mut(&mut self) -> Option<&mut Vec<SpecGap>> {
        match self {
            ImplementResult::Done { spec_gaps, .. }
            | ImplementResult::Blocked { spec_gaps, .. } => Some(spec_gaps),
            ImplementResult::GaveUp { .. } => None,
        }
    }
}

impl SpecGaps for PullWorkResult {
    fn spec_gaps_mut(&mut self) -> Option<&mut Vec<SpecGap>> {
        match self {
            PullWorkResult::Done { spec_gaps, .. } | PullWorkResult::Blocked { spec_gaps, .. } => {
                Some(spec_gaps)
            }
            PullWorkResult::GaveUp { .. } => None,
        }
    }
}

/// A result type and the JSON Schema its session's result matches
/// (docs/specs/runs.md, "Contract").
pub trait Contract: DeserializeOwned {
    const SCHEMA: ResultSchema;
}

impl Contract for TriageResult {
    const SCHEMA: ResultSchema = ResultSchema::Triage;
}

impl Contract for DesignResult<Cut> {
    const SCHEMA: ResultSchema = ResultSchema::DesignSpecChange;
}

impl Contract for DesignResult<IssueWork> {
    const SCHEMA: ResultSchema = ResultSchema::DesignIssue;
}

impl Contract for ImplementResult {
    const SCHEMA: ResultSchema = ResultSchema::Implement;
}

impl Contract for PullWorkResult {
    const SCHEMA: ResultSchema = ResultSchema::PullRequest;
}

impl Contract for AutomationResult {
    const SCHEMA: ResultSchema = ResultSchema::Automation;
}

/// A stage's result schema in `docs/specs/results/`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum ResultSchema {
    Triage,
    DesignSpecChange,
    DesignIssue,
    Implement,
    PullRequest,
    Automation,
}

impl ResultSchema {
    const ALL: [ResultSchema; 6] = [
        ResultSchema::Triage,
        ResultSchema::DesignSpecChange,
        ResultSchema::DesignIssue,
        ResultSchema::Implement,
        ResultSchema::PullRequest,
        ResultSchema::Automation,
    ];

    /// Its file name and text, embedded from the spec so the binary checks
    /// the spec's file and nothing else.
    fn file(self) -> (&'static str, &'static str) {
        match self {
            ResultSchema::Triage => (
                "triage.schema.json",
                include_str!("../docs/specs/results/triage.schema.json"),
            ),
            ResultSchema::DesignSpecChange => (
                "design-spec-change.schema.json",
                include_str!("../docs/specs/results/design-spec-change.schema.json"),
            ),
            ResultSchema::DesignIssue => (
                "design-issue.schema.json",
                include_str!("../docs/specs/results/design-issue.schema.json"),
            ),
            ResultSchema::Implement => (
                "implement.schema.json",
                include_str!("../docs/specs/results/implement.schema.json"),
            ),
            ResultSchema::PullRequest => (
                "pull-request.schema.json",
                include_str!("../docs/specs/results/pull-request.schema.json"),
            ),
            ResultSchema::Automation => (
                "automation.schema.json",
                include_str!("../docs/specs/results/automation.schema.json"),
            ),
        }
    }
}

/// What every result schema shares, reached through their `$ref`s.
const COMMON: (&str, &str) = (
    "common.schema.json",
    include_str!("../docs/specs/results/common.schema.json"),
);

/// Where the schemas are registered, so that their relative `$ref`s resolve
/// among them. Nothing is ever fetched from it.
const SCHEMA_BASE: &str = "https://linewright.invalid/results/";

struct Schema {
    validator: Validator,
    /// The `outcome` of each of the top-level `oneOf`'s branches, in order.
    outcomes: Vec<String>,
}

static SCHEMAS: LazyLock<BTreeMap<ResultSchema, Schema>> =
    LazyLock::new(|| compile().expect("the embedded result schemas compile"));

fn compile() -> Result<BTreeMap<ResultSchema, Schema>, String> {
    let documents = ResultSchema::ALL
        .iter()
        .map(|schema| schema.file())
        .chain([COMMON])
        .map(|(name, text)| {
            serde_json::from_str::<Value>(text)
                .map(|value| (name, value))
                .map_err(|e| format!("{name}: {e}"))
        })
        .collect::<Result<BTreeMap<&str, Value>, String>>()?;
    let registry = Registry::new()
        .extend(documents.iter().map(|(name, value)| {
            (
                format!("{SCHEMA_BASE}{name}"),
                Resource::from_contents(value.clone()),
            )
        }))
        .and_then(jsonschema::RegistryBuilder::prepare)
        .map_err(|e| e.to_string())?;
    let compiled = |name: &str| -> Result<Schema, String> {
        let uri = format!("{SCHEMA_BASE}{name}");
        let value = &documents[name];
        let validator = jsonschema::options()
            .offline()
            .with_registry(&registry)
            .with_base_uri(&uri)
            .build(value)
            .map_err(|e| format!("{name}: {e}"))?;
        let resolver =
            registry.resolver(jsonschema::uri::from_str(&uri).map_err(|e| e.to_string())?);
        let outcomes = value
            .get("oneOf")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .map(|branch| {
                branch_outcome(
                    |reference| {
                        resolver
                            .lookup(reference)
                            .map(|resolved| resolved.contents())
                            .map_err(|e| e.to_string())
                    },
                    branch,
                )
            })
            .collect::<Result<_, _>>()
            .map_err(|e| format!("{name}: {e}"))?;
        Ok(Schema {
            validator,
            outcomes,
        })
    };
    compiled(COMMON.0)?;
    ResultSchema::ALL
        .iter()
        .map(|schema| Ok((*schema, compiled(schema.file().0)?)))
        .collect()
}

/// The `outcome` a `oneOf` branch requires, following its `$ref` with
/// `lookup`.
fn branch_outcome<'a>(
    lookup: impl Fn(&str) -> Result<&'a Value, String>,
    branch: &'a Value,
) -> Result<String, String> {
    let branch = match branch.get("$ref").and_then(Value::as_str) {
        Some(reference) => lookup(reference)?,
        None => branch,
    };
    branch
        .pointer("/properties/outcome/const")
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| "a `oneOf` branch has no `outcome` const".to_string())
}

/// Why `value` does not match `schema`, each failing key named by its JSON
/// pointer, or `None` when it does.
fn mismatch(schema: &Schema, value: &Value) -> Option<String> {
    let mut lines = vec![];
    for error in schema.validator.iter_errors(value) {
        // The top-level `oneOf` picks a branch by `outcome`.
        let (ValidationErrorKind::OneOfNotValid { context }, true) =
            (error.kind(), error.instance_path().is_empty())
        else {
            lines.push(describe(&error));
            continue;
        };
        let outcome = value.get("outcome");
        match schema
            .outcomes
            .iter()
            .position(|o| outcome.and_then(Value::as_str) == Some(o))
        {
            Some(branch) => {
                lines.extend(context.get(branch).into_iter().flatten().map(describe));
            }
            None => lines.push(format!(
                "`/outcome` is {}; it must be one of {}",
                outcome.map_or_else(|| "missing".to_string(), Value::to_string),
                schema
                    .outcomes
                    .iter()
                    .map(|o| format!("{o:?}"))
                    .collect::<Vec<_>>()
                    .join(", ")
            )),
        }
    }
    (!lines.is_empty()).then(|| {
        format!(
            "The result does not match its schema:\n\n{}",
            lines
                .iter()
                .map(|line| format!("- {line}\n"))
                .collect::<String>()
        )
    })
}

fn describe(error: &ValidationError<'_>) -> String {
    let at = error.instance_path().as_str();
    if let ValidationErrorKind::Required { property } = error.kind() {
        let key = property
            .as_str()
            .map_or_else(|| property.to_string(), str::to_string);
        format!("`{at}/{key}` is missing")
    } else if at.is_empty() {
        format!("the result: {error}")
    } else {
        format!("`{at}`: {error}")
    }
}

pub fn parse<T: Contract>(message: &str) -> Result<T, String> {
    let json = extract_json(message).ok_or_else(|| {
        "The final message holds no JSON object. End with the result as a single JSON object."
            .to_string()
    })?;
    let value: Value =
        serde_json::from_str(json).map_err(|e| format!("The result is not valid JSON: {e}."))?;
    let schema = &SCHEMAS[&T::SCHEMA];
    if let Some(mismatch) = mismatch(schema, &value) {
        return Err(mismatch);
    }
    serde_json::from_value(value)
        .map_err(|e| format!("The result JSON does not match the contract: {e}."))
}

pub fn json_value(message: &str) -> Option<Value> {
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
    serde_json::from_str::<Value>(text).is_ok_and(|v| v.is_object())
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
    use serde_json::json;

    use super::*;

    fn rejected<T: Contract + std::fmt::Debug>(result: &Value) -> String {
        parse::<T>(&result.to_string()).expect_err("the result is sent back")
    }

    fn triage(result: &Value) -> Result<TriageResult, String> {
        parse_triage(&result.to_string(), &["bug".to_string()])
    }

    fn done(extra: &Value) -> Value {
        let mut result =
            json!({"outcome": "done", "pr_title": "Add gadgets", "pr_body": "Gadgets."});
        result
            .as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        result
    }

    fn ticket(id: &str, blocked_by: &[&str]) -> Value {
        json!({"id": id, "title": format!("Ticket {id}"), "body": "Build it.", "blocked_by": blocked_by})
    }

    #[test]
    fn every_schema_compiles_offline() {
        compile().unwrap();
    }

    #[test]
    fn a_declined_finding_without_evidence_is_sent_back() {
        let error = rejected::<ImplementResult>(&done(
            &json!({"declined_findings": [{"finding": "Rename it."}]}),
        ));
        assert!(
            error.contains("`/declined_findings/0/evidence` is missing"),
            "{error}"
        );
    }

    #[test]
    fn declined_findings_that_are_not_an_array_are_sent_back() {
        let error = rejected::<ImplementResult>(&done(&json!({"declined_findings": "none"})));
        assert!(error.contains("`/declined_findings`: "), "{error}");
        assert!(error.contains("\"array\""), "{error}");
    }

    #[test]
    fn a_done_result_with_declined_findings_is_accepted() {
        let result = parse::<ImplementResult>(
            &done(&json!({"declined_findings": [{"finding": "Rename it.", "evidence": "\"keep the name\""}]}))
                .to_string(),
        );
        assert!(
            matches!(result, Ok(ImplementResult::Done { .. })),
            "{result:?}"
        );
    }

    #[test]
    fn an_outcome_no_branch_has_lists_the_allowed_outcomes() {
        let error = rejected::<ImplementResult>(&json!({"outcome": "finished", "reason": "Done."}));
        assert!(
            error.contains(
                r#"`/outcome` is "finished"; it must be one of "done", "blocked", "gave-up""#
            ),
            "{error}"
        );
        let missing = rejected::<ImplementResult>(&json!({"reason": "Done."}));
        assert!(
            missing.contains("`/outcome` is missing; it must be one of"),
            "{missing}"
        );
    }

    #[test]
    fn only_the_errors_of_the_branch_the_outcome_names_are_given() {
        let error = rejected::<ImplementResult>(&json!({"outcome": "blocked"}));
        assert_eq!(
            error,
            "The result does not match its schema:\n\n- `/question` is missing\n"
        );
    }

    #[test]
    fn a_login_of_39_characters_with_a_leading_at_is_asked() {
        let login = format!("a-{}", "b".repeat(37));
        let result = triage(&json!({"outcome": "needs-info", "category": "bug", "comment": "Which?", "ask": [format!("@{login}")]}))
            .unwrap();
        assert_eq!(
            result.outcome,
            TriageOutcome::NeedsInfo {
                ask: vec![Login::new(&login)]
            }
        );
    }

    #[test]
    fn a_login_of_40_characters_is_sent_back() {
        let error = triage(&json!({"outcome": "needs-info", "category": "bug", "comment": "Which?", "ask": ["b".repeat(40)]}))
            .unwrap_err();
        assert!(error.contains("`/ask/0`: "), "{error}");
    }

    #[test]
    fn a_triage_outcome_outside_the_three_states_is_sent_back() {
        let error =
            triage(&json!({"outcome": "done", "category": "bug", "comment": "Fine."})).unwrap_err();
        assert!(
            error.contains(r#"`/outcome`: "done" is not one of "ready-for-agent", "needs-info" or "ready-for-human""#),
            "{error}"
        );
    }

    #[test]
    fn a_category_outside_the_categories_is_sent_back() {
        let error =
            triage(&json!({"outcome": "ready-for-agent", "category": "chore", "comment": "Fine."}))
                .unwrap_err();
        assert_eq!(error, r#"`category` is "chore"; it must be one of "bug"."#);
    }

    #[test]
    fn a_spec_change_designed_with_no_comment_is_sent_back() {
        let error = rejected::<DesignResult<Cut>>(
            &json!({"outcome": "designed", "design": "D.", "tickets": [ticket("a", &[])]}),
        );
        assert!(error.contains("`/comment` is missing"), "{error}");
    }

    #[test]
    fn an_issue_designed_with_no_tickets_and_no_comment_is_built_as_it_is() {
        let result: DesignResult<IssueWork> =
            parse(&json!({"outcome": "designed", "design": "D."}).to_string()).unwrap();
        assert!(
            matches!(
                result,
                DesignResult::Designed {
                    cut: IssueWork::AsItIs,
                    ..
                }
            ),
            "{result:?}"
        );
    }

    #[test]
    fn an_issue_designed_with_tickets_and_no_comment_is_sent_back() {
        let error = rejected::<DesignResult<IssueWork>>(
            &json!({"outcome": "designed", "design": "D.", "tickets": [ticket("a", &[])]}),
        );
        assert!(error.contains("`/comment` is missing"), "{error}");
    }

    fn cut(tickets: &[Value]) -> Result<DesignResult<Cut>, String> {
        parse(
            &json!({"outcome": "designed", "design": "D.", "tickets": tickets, "comment": "Cut."})
                .to_string(),
        )
    }

    #[test]
    fn two_tickets_with_one_id_are_sent_back() {
        assert_eq!(
            cut(&[ticket("a", &[]), ticket("a", &[])]).unwrap_err(),
            "The result JSON does not match the contract: two tickets have the id \"a\"."
        );
    }

    #[test]
    fn a_ticket_blocked_by_no_ticket_is_sent_back() {
        assert_eq!(
            cut(&[ticket("a", &["b"])]).unwrap_err(),
            "The result JSON does not match the contract: \
             ticket \"a\" is blocked by \"b\", which is not a ticket id."
        );
    }

    #[test]
    fn tickets_blocking_each_other_are_sent_back() {
        assert_eq!(
            cut(&[ticket("a", &["b"]), ticket("b", &["a"])]).unwrap_err(),
            "The result JSON does not match the contract: the tickets' blocked_by edges form a cycle."
        );
    }

    #[test]
    fn a_reply_with_a_resolve_that_is_not_a_boolean_is_sent_back() {
        let error = rejected::<PullWorkResult>(&json!({
            "outcome": "done",
            "comment": "Fixed.",
            "replies": [{"thread": "T1", "body": "Done.", "resolve": "yes"}],
        }));
        assert!(error.contains("`/replies/0/resolve`: "), "{error}");
        assert!(error.contains("\"boolean\""), "{error}");
    }

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
