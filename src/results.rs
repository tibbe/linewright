use std::collections::BTreeMap;

use std::sync::LazyLock;

use jsonschema::error::ValidationErrorKind;
use jsonschema::{Registry, Resource, ValidationError, Validator};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;

use crate::domain::Login;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TriageOutcome {
    ReadyForAgent,
    NeedsInfo {
        /// The people the question is for: none asks the reporter.
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TriageResult {
    pub outcome: TriageOutcome,
    pub category: String,
    pub comment: String,
}

/// A triage result as the session wrote it, its logins not yet read.
#[derive(Deserialize)]
struct WrittenTriage {
    #[serde(flatten)]
    outcome: WrittenTriageOutcome,
    category: String,
    #[serde(deserialize_with = "trimmed")]
    comment: String,
}

#[derive(Deserialize)]
#[serde(tag = "outcome", rename_all = "kebab-case")]
enum WrittenTriageOutcome {
    ReadyForAgent,
    NeedsInfo {
        #[serde(default)]
        ask: Vec<String>,
    },
    ReadyForHuman,
}

impl WrittenTriage {
    fn read(self) -> Result<TriageResult, ContractError> {
        let outcome = match self.outcome {
            WrittenTriageOutcome::ReadyForAgent => TriageOutcome::ReadyForAgent,
            WrittenTriageOutcome::NeedsInfo { ask } => {
                TriageOutcome::NeedsInfo { ask: logins(ask)? }
            }
            WrittenTriageOutcome::ReadyForHuman => TriageOutcome::ReadyForHuman,
        };
        Ok(TriageResult {
            outcome,
            category: self.category,
            comment: self.comment,
        })
    }
}

#[derive(Deserialize)]
struct TicketDraft {
    id: String,
    title: String,
    body: String,
    #[serde(default)]
    blocked_by: Vec<String>,
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

impl<C> DesignResult<C> {
    /// The result with its cut, if it has one, made into a `D` by `f`, or
    /// `f`'s error.
    fn try_map_cut<D, E>(self, f: impl FnOnce(C) -> Result<D, E>) -> Result<DesignResult<D>, E> {
        Ok(match self {
            DesignResult::Designed {
                design,
                cut,
                spec_gaps,
            } => DesignResult::Designed {
                design,
                cut: f(cut)?,
                spec_gaps,
            },
            DesignResult::NothingToBuild { reason } => DesignResult::NothingToBuild { reason },
            DesignResult::Blocked {
                question,
                spec_gaps,
            } => DesignResult::Blocked {
                question,
                spec_gaps,
            },
            DesignResult::GaveUp { reason } => DesignResult::GaveUp { reason },
        })
    }
}

/// The tickets an issue's work is cut into (docs/specs/design.md,
/// "Cutting").
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cut {
    pub tickets: Tickets,
    /// Why the work is cut into these tickets.
    pub comment: String,
}

/// What an issue's work is: built as it is, in one session, or cut into
/// tickets (docs/specs/design.md, "Cutting").
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum IssueWork {
    AsItIs,
    Cut(Cut),
}

/// An issue's work as the session wrote it, its tickets not yet ordered.
#[derive(Deserialize)]
struct WrittenWork {
    #[serde(default)]
    tickets: Vec<TicketDraft>,
    #[serde(default)]
    comment: Option<String>,
}

impl WrittenWork {
    fn read(self) -> Result<IssueWork, ContractError> {
        if self.tickets.is_empty() {
            return Ok(IssueWork::AsItIs);
        }
        Ok(IssueWork::Cut(Cut {
            tickets: Tickets(ordered(&self.tickets)?),
            comment: self.comment.unwrap_or_default().trim().to_string(),
        }))
    }
}

/// At least one ticket, as the schemas require, ordered so that every ticket
/// comes after those blocking it.
#[derive(Clone, Debug, PartialEq, Eq)]
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

/// The GitHub logins `asked`, each written with or without a leading `@`,
/// each once.
fn logins(asked: Vec<String>) -> Result<Vec<Login>, ContractError> {
    let mut logins: Vec<Login> = vec![];
    for written in asked {
        let Some(login) = Login::parse(written.trim().trim_start_matches('@')) else {
            return Err(ContractError::Login { written });
        };
        if !logins.contains(&login) {
            logins.push(login);
        }
    }
    Ok(logins)
}

/// The triage result in `message`, whose category must be one of
/// `categories`.
pub fn parse_triage(message: &str, categories: &[String]) -> Result<TriageResult, ParseError> {
    let result: TriageResult = parse(message)?;
    if !categories.contains(&result.category) {
        return Err(ParseError::Category {
            category: result.category,
            categories: categories.to_vec(),
        });
    }
    Ok(result)
}

/// `drafts` ordered so that every ticket comes after those blocking it,
/// keeping their order wherever that already holds, or why they cannot be:
/// a repeated id, a blocker that is not a ticket, or a cycle.
fn ordered(drafts: &[TicketDraft]) -> Result<Vec<Ticket>, ContractError> {
    let mut by_id = BTreeMap::new();
    for ticket in drafts {
        if by_id.insert(ticket.id.clone(), ticket).is_some() {
            return Err(ContractError::DuplicateId {
                id: ticket.id.clone(),
            });
        }
    }
    for ticket in drafts {
        if let Some(unknown) = ticket.blocked_by.iter().find(|b| !by_id.contains_key(*b)) {
            return Err(ContractError::UnknownBlocker {
                id: ticket.id.clone(),
                blocker: unknown.clone(),
            });
        }
    }
    let mut placed: BTreeMap<&str, usize> = BTreeMap::new();
    let mut ordered = vec![];
    while ordered.len() < drafts.len() {
        let next = drafts.iter().find(|t| {
            !placed.contains_key(t.id.as_str())
                && t.blocked_by.iter().all(|b| placed.contains_key(b.as_str()))
        });
        let Some(next) = next else {
            return Err(ContractError::Cycle);
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

/// What a scorer answers (`docs/specs/benchmarks.md`, Scoring).
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(tag = "outcome", rename_all = "kebab-case")]
pub enum ScorerResult {
    #[serde(rename = "judged")]
    Scored {
        correctness: Correctness,
        leak: Leak,
    },
    GaveUp {
        #[serde(deserialize_with = "trimmed")]
        reason: String,
    },
}

/// Whether a trial's output meets its task's criteria, and why.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Correctness {
    #[serde(deserialize_with = "trimmed")]
    pub reasoning: String,
    pub verdict: Verdict,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum Verdict {
    /// The output meets every criterion.
    Pass,
    Fail,
}

impl Verdict {
    pub fn name(self) -> &'static str {
        match self {
            Verdict::Pass => "pass",
            Verdict::Fail => "fail",
        }
    }
}

/// Whether a session of a trial learned how the work turned out, and why:
/// a leak found always quotes a tool call that shows it. Kept as
/// `reasoning`, `found` and `calls`, with no calls for a leak not found.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "LeakFields", into = "LeakFields")]
pub enum Leak {
    None { reasoning: String },
    Found { reasoning: String, calls: Calls },
}

impl Leak {
    pub fn found(&self) -> bool {
        matches!(self, Leak::Found { .. })
    }
}

/// The tool calls that show a leak, as the transcript has them: at least
/// one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Calls {
    first: String,
    rest: Vec<String>,
}

impl Calls {
    pub fn new(first: String, rest: Vec<String>) -> Calls {
        Calls { first, rest }
    }

    /// The calls, in the order the scorer gave them.
    pub fn iter(&self) -> impl Iterator<Item = &str> {
        std::iter::once(self.first.as_str()).chain(self.rest.iter().map(String::as_str))
    }
}

#[derive(Serialize, Deserialize)]
struct LeakFields {
    #[serde(deserialize_with = "trimmed")]
    reasoning: String,
    found: bool,
    #[serde(default)]
    calls: Vec<String>,
}

impl TryFrom<LeakFields> for Leak {
    type Error = &'static str;

    fn try_from(fields: LeakFields) -> Result<Leak, &'static str> {
        let LeakFields {
            reasoning,
            found,
            calls,
        } = fields;
        if !found {
            return Ok(Leak::None { reasoning });
        }
        let mut calls = calls.into_iter();
        let first = calls
            .next()
            .ok_or("a leak found quotes at least one call")?;
        Ok(Leak::Found {
            reasoning,
            calls: Calls::new(first, calls.collect()),
        })
    }
}

impl From<Leak> for LeakFields {
    fn from(leak: Leak) -> LeakFields {
        match leak {
            Leak::None { reasoning } => LeakFields {
                reasoning,
                found: false,
                calls: vec![],
            },
            Leak::Found { reasoning, calls } => LeakFields {
                reasoning,
                found: true,
                calls: calls.iter().map(str::to_string).collect(),
            },
        }
    }
}

/// A result type and the JSON Schema its session's result matches
/// (docs/specs/runs.md, "Contract").
pub trait Contract: Sized {
    const SCHEMA: ResultSchema;

    /// The result `value`, which matches [`Self::SCHEMA`], holds, or why it
    /// holds none.
    fn read(value: Value) -> Result<Self, ContractError>;
}

/// The `T` that `value` deserializes to, or serde's error.
fn deserialized<T: DeserializeOwned>(value: Value) -> Result<T, ContractError> {
    serde_json::from_value(value).map_err(|e| ContractError::Serde {
        error: e.to_string(),
    })
}

impl Contract for TriageResult {
    const SCHEMA: ResultSchema = ResultSchema::Triage;

    fn read(value: Value) -> Result<Self, ContractError> {
        deserialized::<WrittenTriage>(value)?.read()
    }
}

impl Contract for DesignResult<IssueWork> {
    const SCHEMA: ResultSchema = ResultSchema::DesignIssue;

    fn read(value: Value) -> Result<Self, ContractError> {
        deserialized::<DesignResult<WrittenWork>>(value)?.try_map_cut(WrittenWork::read)
    }
}

impl Contract for ImplementResult {
    const SCHEMA: ResultSchema = ResultSchema::Implement;

    fn read(value: Value) -> Result<Self, ContractError> {
        deserialized(value)
    }
}

impl Contract for PullWorkResult {
    const SCHEMA: ResultSchema = ResultSchema::PullRequest;

    fn read(value: Value) -> Result<Self, ContractError> {
        deserialized(value)
    }
}

impl Contract for AutomationResult {
    const SCHEMA: ResultSchema = ResultSchema::Automation;

    fn read(value: Value) -> Result<Self, ContractError> {
        deserialized(value)
    }
}

impl Contract for ScorerResult {
    const SCHEMA: ResultSchema = ResultSchema::Scorer;

    fn read(value: Value) -> Result<Self, ContractError> {
        deserialized(value)
    }
}

/// Why a session's final message holds no result Linewright can use
/// (docs/specs/runs.md, "Contract").
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ParseError {
    /// The message holds no JSON object.
    NoJson,
    /// The result is not JSON, as serde's `error` says.
    NotJson { error: String },
    /// The result does not match its schema.
    Schema(Mismatches),
    /// The result matches its schema but holds no result Linewright reads.
    Contract(ContractError),
    /// A triage result's `category` is not one of the repository's
    /// `categories`.
    Category {
        category: String,
        categories: Vec<String>,
    },
}

/// The ways a result does not match its schema: at least one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mismatches {
    first: Mismatch,
    rest: Vec<Mismatch>,
}

impl Mismatches {
    pub fn new(first: Mismatch, rest: Vec<Mismatch>) -> Mismatches {
        Mismatches { first, rest }
    }

    /// The mismatches, in the order the schema's validator found them.
    pub fn iter(&self) -> impl Iterator<Item = &Mismatch> {
        std::iter::once(&self.first).chain(&self.rest)
    }

    /// The first mismatch and the rest, in the order of [`Mismatches::iter`].
    pub fn split(&self) -> (&Mismatch, &[Mismatch]) {
        (&self.first, &self.rest)
    }
}

/// One way a result does not match its schema, each key named by its JSON
/// pointer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Mismatch {
    /// A required `key` is missing.
    Missing { key: String },
    /// The result's `outcome`, or its lack of one, is none of the schema's
    /// `outcomes`.
    Outcome {
        outcome: Option<Value>,
        outcomes: Vec<String>,
    },
    /// The value at `at` does not match, as the validator's `error` says.
    At { at: String, error: String },
    /// The result as a whole does not match, as the validator's `error`
    /// says.
    Whole { error: String },
}

/// Why a result that matches its schema holds no result Linewright reads.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ContractError {
    /// The result does not deserialize, as serde's `error` says.
    Serde { error: String },
    /// A person asked, as `written`, is not a GitHub login.
    Login { written: String },
    /// Two tickets have the same `id`.
    DuplicateId { id: String },
    /// The ticket `id` is blocked by `blocker`, which is no ticket's id.
    UnknownBlocker { id: String, blocker: String },
    /// The tickets' `blocked_by` edges form a cycle.
    Cycle,
}

/// A stage's result schema in `briefs/results/`, or the scorer's.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum ResultSchema {
    Triage,
    DesignIssue,
    Implement,
    PullRequest,
    Automation,
    Scorer,
}

impl ResultSchema {
    const ALL: [ResultSchema; 6] = [
        ResultSchema::Triage,
        ResultSchema::DesignIssue,
        ResultSchema::Implement,
        ResultSchema::PullRequest,
        ResultSchema::Automation,
        ResultSchema::Scorer,
    ];

    /// Its file name and text, as the spec's file holds them.
    fn file(self) -> (&'static str, &'static str) {
        match self {
            ResultSchema::Triage => (
                "triage.schema.json",
                include_str!("../briefs/results/triage.schema.json"),
            ),
            ResultSchema::DesignIssue => (
                "design-issue.schema.json",
                include_str!("../briefs/results/design-issue.schema.json"),
            ),
            ResultSchema::Implement => (
                "implement.schema.json",
                include_str!("../briefs/results/implement.schema.json"),
            ),
            ResultSchema::PullRequest => (
                "pull-request.schema.json",
                include_str!("../briefs/results/pull-request.schema.json"),
            ),
            ResultSchema::Automation => (
                "automation.schema.json",
                include_str!("../briefs/results/automation.schema.json"),
            ),
            ResultSchema::Scorer => (
                "scorer.schema.json",
                include_str!("../briefs/results/scorer.schema.json"),
            ),
        }
    }
}

/// What every result schema shares, reached through their `$ref`s.
const COMMON: (&str, &str) = (
    "common.schema.json",
    include_str!("../briefs/results/common.schema.json"),
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

/// How `value` does not match `schema`, or `None` when it does. Where the
/// result names one of its outcomes, only that outcome's branch's
/// mismatches are given.
fn mismatches(schema: &Schema, value: &Value) -> Option<Mismatches> {
    let mut found = vec![];
    for error in schema.validator.iter_errors(value) {
        let (ValidationErrorKind::OneOfNotValid { context }, true) =
            (error.kind(), error.instance_path().is_empty())
        else {
            found.push(mismatch(&error));
            continue;
        };
        let outcome = value.get("outcome");
        match schema
            .outcomes
            .iter()
            .position(|o| outcome.and_then(Value::as_str) == Some(o))
        {
            Some(branch) => {
                found.extend(context.get(branch).into_iter().flatten().map(mismatch));
            }
            None => found.push(Mismatch::Outcome {
                outcome: outcome.cloned(),
                outcomes: schema.outcomes.clone(),
            }),
        }
    }
    let mut found = found.into_iter();
    let first = found.next()?;
    Some(Mismatches::new(first, found.collect()))
}

fn mismatch(error: &ValidationError<'_>) -> Mismatch {
    let at = error.instance_path().as_str();
    if let ValidationErrorKind::Required { property } = error.kind() {
        let key = property
            .as_str()
            .map_or_else(|| property.to_string(), str::to_string);
        Mismatch::Missing {
            key: format!("{at}/{key}"),
        }
    } else if at.is_empty() {
        Mismatch::Whole {
            error: error.to_string(),
        }
    } else {
        Mismatch::At {
            at: at.to_string(),
            error: error.to_string(),
        }
    }
}

/// The result in `message`, or why it holds none.
pub fn parse<T: Contract>(message: &str) -> Result<T, ParseError> {
    let json = extract_json(message).ok_or(ParseError::NoJson)?;
    let value: Value = serde_json::from_str(json).map_err(|e| ParseError::NotJson {
        error: e.to_string(),
    })?;
    if let Some(mismatches) = mismatches(&SCHEMAS[&T::SCHEMA], &value) {
        return Err(ParseError::Schema(mismatches));
    }
    T::read(value).map_err(ParseError::Contract)
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
    #[derive(Clone, Copy)]
    struct OpenFence {
        fence_char: char,
        fence_len: usize,
        content_start: usize,
    }

    let mut last = None;
    let mut open: Option<OpenFence> = None;
    let mut offset = 0;
    for line in text.split_inclusive('\n') {
        let fence = line.trim_start();
        let fence_char = fence.chars().next().filter(|c| *c == '`' || *c == '~');
        let fence_len = fence_char.map_or(0, |c| fence.chars().take_while(|x| *x == c).count());
        match (open, fence_char) {
            (None, Some(c)) if fence_len >= 3 => {
                open = Some(OpenFence {
                    fence_char: c,
                    fence_len,
                    content_start: offset + line.len(),
                });
            }
            (Some(opened), Some(d))
                if opened.fence_char == d
                    && fence_len >= opened.fence_len
                    && fence[fence_len..].trim().is_empty() =>
            {
                last = Some(&text[opened.content_start..offset]);
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

    #[track_caller]
    fn rejected<T: Contract + std::fmt::Debug>(result: &Value) -> ParseError {
        parse::<T>(&result.to_string()).expect_err("the result is sent back")
    }

    /// The mismatches `error` is, failing unless it is the schema's.
    #[track_caller]
    fn mismatches(error: ParseError) -> Vec<Mismatch> {
        let ParseError::Schema(mismatches) = error else {
            panic!("not a schema mismatch: {error:?}");
        };
        mismatches.iter().cloned().collect()
    }

    /// The error at `key` among `error`'s mismatches, failing unless there is
    /// one.
    #[track_caller]
    fn error_at(sent_back: ParseError, key: &str) -> String {
        let found = mismatches(sent_back);
        found
            .iter()
            .find_map(|m| {
                if let Mismatch::At { at, error } = m
                    && at == key
                {
                    Some(error.clone())
                } else {
                    None
                }
            })
            .unwrap_or_else(|| panic!("no mismatch at {key}: {found:?}"))
    }

    fn missing(key: &str) -> Mismatch {
        Mismatch::Missing { key: key.into() }
    }

    fn triage(result: &Value) -> Result<TriageResult, ParseError> {
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

    #[test_log::test]
    fn every_schema_compiles_offline() {
        compile().unwrap();
    }

    #[test_log::test]
    fn a_declined_finding_without_a_reason_is_sent_back() {
        let error = rejected::<ImplementResult>(&done(
            &json!({"declined_findings": [{"finding": "Rename it."}]}),
        ));
        assert_eq!(mismatches(error), [missing("/declined_findings/0/reason")]);
    }

    #[test_log::test]
    fn declined_findings_that_are_not_an_array_are_sent_back() {
        let error = rejected::<ImplementResult>(&done(&json!({"declined_findings": "none"})));
        let error = error_at(error, "/declined_findings");
        assert!(error.contains("\"array\""), "{error}");
    }

    #[test_log::test]
    fn a_done_result_with_declined_findings_is_accepted() {
        let result = parse::<ImplementResult>(
            &done(&json!({"declined_findings": [{"finding": "Rename it.", "reason": "\"keep the name\""}]}))
                .to_string(),
        );
        assert!(
            matches!(result, Ok(ImplementResult::Done { .. })),
            "{result:?}"
        );
    }

    fn scored(leak: &Value) -> Value {
        json!({
            "outcome": "judged",
            "correctness": {"reasoning": "It triages the issue.", "verdict": "pass"},
            "leak": leak,
        })
    }

    #[test_log::test]
    fn a_score_with_a_found_leak_keeps_the_calls_that_show_it() {
        let result = parse::<ScorerResult>(
            &scored(&json!({
                "reasoning": "It read the pull request.",
                "found": true,
                "calls": ["curl https://github.com/owner/widgets/pull/8"]
            }))
            .to_string(),
        );

        assert_eq!(
            result,
            Ok(ScorerResult::Scored {
                correctness: Correctness {
                    reasoning: "It triages the issue.".into(),
                    verdict: Verdict::Pass,
                },
                leak: Leak::Found {
                    reasoning: "It read the pull request.".into(),
                    calls: Calls::new(
                        "curl https://github.com/owner/widgets/pull/8".into(),
                        vec![]
                    ),
                },
            })
        );
    }

    #[test_log::test]
    fn a_found_leak_without_a_call_is_sent_back() {
        for leak in [
            json!({"reasoning": "It read the pull request.", "found": true}),
            json!({"reasoning": "It read the pull request.", "found": true, "calls": []}),
        ] {
            let error = rejected::<ScorerResult>(&scored(&leak));

            assert!(
                mismatches(error.clone()).iter().any(|m| matches!(
                    m,
                    Mismatch::At { at, .. } | Mismatch::Missing { key: at }
                        if at.starts_with("/leak")
                )),
                "{error:?}"
            );
        }
    }

    #[test_log::test]
    fn a_score_with_no_leak_needs_no_calls() {
        let result = parse::<ScorerResult>(
            &scored(&json!({"reasoning": "It read only the issue.", "found": false})).to_string(),
        );

        assert!(
            matches!(
                &result,
                Ok(ScorerResult::Scored { leak: Leak::None { reasoning }, .. })
                    if reasoning == "It read only the issue."
            ),
            "{result:?}"
        );
    }

    #[test_log::test]
    fn a_scorer_that_gives_up_says_why() {
        let result = parse::<ScorerResult>(
            &json!({"outcome": "gave-up", "reason": "The output is empty."}).to_string(),
        );

        assert_eq!(
            result,
            Ok(ScorerResult::GaveUp {
                reason: "The output is empty.".into()
            })
        );
    }

    #[test_log::test]
    fn an_outcome_no_branch_has_lists_the_allowed_outcomes() {
        let outcomes = vec!["done".to_string(), "blocked".into(), "gave-up".into()];
        let error = rejected::<ImplementResult>(&json!({"outcome": "finished", "reason": "Done."}));
        assert!(mismatches(error).contains(&Mismatch::Outcome {
            outcome: Some(json!("finished")),
            outcomes: outcomes.clone(),
        }));
        let missing = rejected::<ImplementResult>(&json!({"reason": "Done."}));
        assert!(mismatches(missing).contains(&Mismatch::Outcome {
            outcome: None,
            outcomes,
        }));
    }

    #[test_log::test]
    fn only_the_errors_of_the_branch_the_outcome_names_are_given() {
        let error = rejected::<ImplementResult>(&json!({"outcome": "blocked"}));
        assert_eq!(mismatches(error), [missing("/question")]);
    }

    #[test_log::test]
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

    #[test_log::test]
    fn a_login_of_40_characters_is_sent_back() {
        let error = triage(&json!({"outcome": "needs-info", "category": "bug", "comment": "Which?", "ask": ["b".repeat(40)]}))
            .unwrap_err();
        error_at(error, "/ask/0");
    }

    #[test_log::test]
    fn a_pull_request_title_is_trimmed_and_a_blank_one_sent_back() {
        let error = rejected::<ImplementResult>(&done(&json!({"pr_title": " "})));
        let error = error_at(error, "/pr_title");
        assert!(error.starts_with("\" \" does not match"), "{error}");
        let result =
            parse::<ImplementResult>(&done(&json!({"pr_title": " Report gadgets "})).to_string());
        assert_eq!(
            result,
            Ok(ImplementResult::Done {
                pr_title: "Report gadgets".into(),
                pr_body: "Gadgets.".into(),
                spec_gaps: vec![],
            })
        );
    }

    #[test_log::test]
    fn asking_something_that_is_not_a_login_is_sent_back() {
        let error = triage(&json!({"outcome": "needs-info", "category": "bug", "comment": "Which version?", "ask": ["the reporter"]}))
            .unwrap_err();
        let error = error_at(error, "/ask/0");
        assert!(
            error.starts_with("\"the reporter\" does not match"),
            "{error}"
        );
    }

    #[test_log::test]
    fn a_triage_outcome_outside_the_three_states_is_sent_back() {
        let error =
            triage(&json!({"outcome": "done", "category": "bug", "comment": "Fine."})).unwrap_err();
        assert_eq!(
            error_at(error, "/outcome"),
            r#""done" is not one of "ready-for-agent", "needs-info" or "ready-for-human""#
        );
    }

    #[test_log::test]
    fn an_automations_issue_labeled_other_than_the_two_labels_is_sent_back() {
        let error = rejected::<AutomationResult>(&json!({
            "summary": "One bug.",
            "issues": [{"title": "Fix it", "body": "It is broken.", "label": "bug"}],
        }));
        assert_eq!(
            mismatches(error),
            [Mismatch::At {
                at: "/issues/0/label".into(),
                error: r#""bug" is not one of "ready-for-agent" or "ready-for-human""#.into(),
            }]
        );
    }

    #[test_log::test]
    fn a_category_outside_the_categories_is_sent_back() {
        let error =
            triage(&json!({"outcome": "ready-for-agent", "category": "chore", "comment": "Fine."}))
                .unwrap_err();
        assert_eq!(
            error,
            ParseError::Category {
                category: "chore".into(),
                categories: vec!["bug".into()],
            }
        );
    }

    #[test_log::test]
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

    #[test_log::test]
    fn an_issue_designed_with_tickets_and_no_comment_is_sent_back() {
        let error = rejected::<DesignResult<IssueWork>>(
            &json!({"outcome": "designed", "design": "D.", "tickets": [ticket("a", &[])]}),
        );
        assert_eq!(mismatches(error), [missing("/comment")]);
    }

    fn cut(tickets: &[Value]) -> Result<DesignResult<IssueWork>, ParseError> {
        parse(
            &json!({"outcome": "designed", "design": "D.", "tickets": tickets, "comment": "Cut."})
                .to_string(),
        )
    }

    #[test_log::test]
    fn two_tickets_with_one_id_are_sent_back() {
        assert_eq!(
            cut(&[ticket("a", &[]), ticket("a", &[])]).unwrap_err(),
            ParseError::Contract(ContractError::DuplicateId { id: "a".into() })
        );
    }

    #[test_log::test]
    fn a_ticket_blocked_by_no_ticket_is_sent_back() {
        assert_eq!(
            cut(&[ticket("a", &["b"])]).unwrap_err(),
            ParseError::Contract(ContractError::UnknownBlocker {
                id: "a".into(),
                blocker: "b".into(),
            })
        );
    }

    #[test_log::test]
    fn tickets_blocking_each_other_are_sent_back() {
        assert_eq!(
            cut(&[ticket("a", &["b"]), ticket("b", &["a"])]).unwrap_err(),
            ParseError::Contract(ContractError::Cycle)
        );
    }

    #[test_log::test]
    fn a_reply_with_a_resolve_that_is_not_a_boolean_is_sent_back() {
        let error = rejected::<PullWorkResult>(&json!({
            "outcome": "done",
            "comment": "Fixed.",
            "replies": [{"thread": "T1", "body": "Done.", "resolve": "yes"}],
        }));
        let error = error_at(error, "/replies/0/resolve");
        assert!(error.contains("\"boolean\""), "{error}");
    }

    const RESULT: &str = r#"{"summary": "Ran."}"#;

    #[test_log::test]
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

    #[test_log::test]
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

    #[test_log::test]
    fn prose_around_a_bare_object_still_parses() {
        let message = format!("Done. The result:\n{RESULT}\nThanks.");
        assert_eq!(parse::<AutomationResult>(&message).unwrap().summary, "Ran.");
    }

    #[test_log::test]
    fn a_message_with_no_object_is_sent_back() {
        let error = parse::<AutomationResult>("I think it's a bug.").unwrap_err();
        assert_eq!(error, ParseError::NoJson);
    }
}
