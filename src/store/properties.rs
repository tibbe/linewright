//! The repository's properties, read from GitHub's answers as recorded
//! (`docs/specs/repository.md`, "The repository's properties").

use std::collections::BTreeMap;
use std::fmt;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{Categories, Identity, RepoSettings};
use crate::domain::{Login, MergeMethod, Repo};

/// GitHub's answers about the repository and Linewright's account, each
/// unchanged, by its [`Property::key`].
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RecordedProperties {
    pub resources: BTreeMap<String, Value>,
}

/// One of GitHub's answers a [`RecordedProperties`] holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Property {
    /// <https://docs.github.com/en/rest/repos/repos#get-a-repository>, as
    /// Linewright's account sees it.
    Repository,
    /// <https://docs.github.com/en/rest/users/users#get-the-authenticated-user>
    Account,
    /// The `data` of the answer to [`ISSUE_TYPES_AND_MERGE_METHODS_QUERY`].
    IssueTypesAndMergeMethods,
    /// <https://docs.github.com/en/rest/repos/rules#get-rules-for-a-branch>
    /// for the default branch, or `null` when GitHub did not answer with
    /// them, which reads as no rules.
    Rules,
    /// <https://docs.github.com/en/rest/branches/branch-protection#get-branch-protection>
    /// for the default branch, or `null` when GitHub did not answer with it,
    /// as it does not to an account that is not an admin, which reads as no
    /// protection.
    Protection,
}

impl Property {
    /// The key it is recorded under.
    pub fn key(self) -> &'static str {
        match self {
            Property::Repository => "repository",
            Property::Account => "account",
            Property::IssueTypesAndMergeMethods => "issue-types-and-merge-methods",
            Property::Rules => "rules",
            Property::Protection => "protection",
        }
    }
}

impl fmt::Display for Property {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Property::Repository => "the repository",
            Property::Account => "Linewright's account",
            Property::IssueTypesAndMergeMethods => "the issue types and merge methods",
            Property::Rules => "the default branch's rules",
            Property::Protection => "the default branch's protection",
        })
    }
}

/// The GraphQL query whose answer is [`Property::IssueTypesAndMergeMethods`],
/// with the variables `owner` and `name`.
pub const ISSUE_TYPES_AND_MERGE_METHODS_QUERY: &str = "query($owner: String!, $name: String!) {
  repository(owner: $owner, name: $name) {
    issueTypes(first: 25) { nodes { name isEnabled } }
    mergeCommitAllowed squashMergeAllowed rebaseMergeAllowed
  }
}";

impl RecordedProperties {
    /// The answer recorded for `property`, if any.
    pub fn resource(&self, property: Property) -> Option<&Value> {
        self.resources.get(property.key())
    }
}

impl FromIterator<(Property, Value)> for RecordedProperties {
    fn from_iter<I: IntoIterator<Item = (Property, Value)>>(answers: I) -> RecordedProperties {
        RecordedProperties {
            resources: answers
                .into_iter()
                .map(|(property, answer)| (property.key().to_string(), answer))
                .collect(),
        }
    }
}

/// What the recorded properties say.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Properties {
    /// With the default merge method, before the `merge-method` setting
    /// ([`RepoSettings::configured`]).
    pub settings: RepoSettings,
    pub identity: Identity,
    /// Linewright's account can push to the repository.
    pub can_push: bool,
}

impl Properties {
    /// Who Linewright acts as, refused when its account cannot push to the
    /// repository (`docs/specs/repository.md`, "Startup").
    pub fn pushing_identity(self) -> anyhow::Result<Identity> {
        if !self.can_push {
            anyhow::bail!(
                "{} cannot push to {}: Linewright's account needs write access",
                self.identity.login,
                self.settings.repo
            );
        }
        Ok(self.identity)
    }
}

/// The email the account `login`, of ID `id`, commits with: GitHub's
/// no-reply address for it
/// (<https://docs.github.com/en/account-and-profile/setting-up-and-managing-your-personal-account-on-github/managing-email-preferences/setting-your-commit-email-address#about-no-reply-email-addresses>).
pub fn noreply_email(id: u64, login: &str) -> String {
    format!("{id}+{login}@users.noreply.github.com")
}

/// Why the recorded properties do not read.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum PropertiesError {
    #[error("{0} is not recorded")]
    Missing(Property),
    #[error("{property} does not read: {why}")]
    Unreadable { property: Property, why: String },
}

#[derive(Deserialize)]
struct RestRepository {
    full_name: String,
    default_branch: String,
    permissions: Option<BTreeMap<String, bool>>,
}

#[derive(Deserialize)]
struct RestUser {
    login: String,
    id: u64,
}

#[derive(Deserialize)]
struct QueryData {
    repository: QueriedRepository,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct QueriedRepository {
    issue_types: Option<IssueTypes>,
    merge_commit_allowed: bool,
    squash_merge_allowed: bool,
    rebase_merge_allowed: bool,
}

#[derive(Deserialize)]
struct IssueTypes {
    nodes: Vec<IssueType>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct IssueType {
    name: String,
    is_enabled: bool,
}

#[derive(Deserialize)]
struct Rule {
    #[serde(rename = "type")]
    kind: String,
}

#[derive(Deserialize)]
struct Protection {
    required_linear_history: Option<Enabled>,
}

#[derive(Deserialize)]
struct Enabled {
    enabled: bool,
}

/// Reads `recorded` as Linewright reads the repository's properties: the
/// enabled issue types as the categories, or the labels when there are
/// none; linear history required by a rule or by the protection; and the
/// account's commit email as its [`noreply_email`].
/// Fails, naming the property, when one is missing or does not read.
pub fn read(recorded: &RecordedProperties) -> Result<Properties, PropertiesError> {
    let repository: RestRepository = recorded_answer(recorded, Property::Repository)?;
    let repo: Repo =
        repository
            .full_name
            .parse()
            .map_err(|e: anyhow::Error| PropertiesError::Unreadable {
                property: Property::Repository,
                why: format!("{e:#}"),
            })?;
    let account: RestUser = recorded_answer(recorded, Property::Account)?;
    let QueryData {
        repository: queried,
    } = recorded_answer(recorded, Property::IssueTypesAndMergeMethods)?;
    let rules: Option<Vec<Rule>> = recorded_answer(recorded, Property::Rules)?;
    let protection: Option<Protection> = recorded_answer(recorded, Property::Protection)?;

    let allowed_merge_methods = [
        (MergeMethod::Merge, queried.merge_commit_allowed),
        (MergeMethod::Squash, queried.squash_merge_allowed),
        (MergeMethod::Rebase, queried.rebase_merge_allowed),
    ]
    .into_iter()
    .filter_map(|(method, allowed)| allowed.then_some(method))
    .collect();
    let types: Vec<String> = queried
        .issue_types
        .map(|types| types.nodes)
        .unwrap_or_default()
        .into_iter()
        .filter(|t| t.is_enabled)
        .map(|t| t.name)
        .collect();
    let categories = if types.is_empty() {
        Categories::Labels
    } else {
        Categories::IssueTypes(types)
    };
    let linear_history = rules
        .unwrap_or_default()
        .iter()
        .any(|rule| rule.kind == "required_linear_history")
        || protection
            .and_then(|p| p.required_linear_history)
            .is_some_and(|required| required.enabled);
    let can_push = repository
        .permissions
        .and_then(|p| p.get("push").copied())
        .unwrap_or(false);
    Ok(Properties {
        settings: RepoSettings::new(
            repo,
            repository.default_branch,
            linear_history,
            allowed_merge_methods,
            categories,
        ),
        identity: Identity {
            login: Login::new(&account.login),
            name: account.login.clone(),
            email: noreply_email(account.id, &account.login),
        },
        can_push,
    })
}

fn recorded_answer<T: DeserializeOwned>(
    recorded: &RecordedProperties,
    property: Property,
) -> Result<T, PropertiesError> {
    let value = recorded
        .resource(property)
        .ok_or(PropertiesError::Missing(property))?;
    T::deserialize(value).map_err(|e| PropertiesError::Unreadable {
        property,
        why: e.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::domain::MergeMethods;

    const EVERY_PROPERTY: [Property; 5] = [
        Property::Repository,
        Property::Account,
        Property::IssueTypesAndMergeMethods,
        Property::Rules,
        Property::Protection,
    ];

    #[track_caller]
    fn fixture(text: &str) -> Value {
        serde_json::from_str(text).unwrap()
    }

    /// What GitHub answered for tibbe/linewright on 2026-11-15, to
    /// tibbe-bot: its default branch's rules require linear history, and its
    /// protection is not given.
    #[track_caller]
    fn tibbe_linewright() -> RecordedProperties {
        [
            (
                Property::Repository,
                fixture(include_str!("fixtures/rest-repository.json")),
            ),
            (
                Property::Account,
                fixture(include_str!("fixtures/rest-user.json")),
            ),
            (
                Property::IssueTypesAndMergeMethods,
                fixture(include_str!(
                    "fixtures/graphql-issue-types-and-merge-methods.json"
                ))["data"]
                    .clone(),
            ),
            (
                Property::Rules,
                fixture(include_str!("fixtures/rest-rules-main.json")),
            ),
            (Property::Protection, Value::Null),
        ]
        .into_iter()
        .collect()
    }

    /// [`tibbe_linewright`], with `answer` for `property`.
    #[track_caller]
    fn recorded_with(property: Property, answer: Value) -> RecordedProperties {
        let mut recorded = tibbe_linewright();
        recorded
            .resources
            .insert(property.key().to_string(), answer);
        recorded
    }

    #[test_log::test]
    fn githubs_answers_read_into_the_settings_and_the_identity() {
        let properties = read(&tibbe_linewright()).unwrap();

        assert_eq!(
            properties,
            Properties {
                settings: RepoSettings {
                    repo: "tibbe/linewright".parse().unwrap(),
                    default_branch: "main".into(),
                    merge_method: MergeMethod::Rebase,
                    allowed_merge_methods: MergeMethods::from_iter([MergeMethod::Rebase]),
                    linear_history: true,
                    categories: Categories::IssueTypes(vec![
                        "Task".into(),
                        "Bug".into(),
                        "Feature".into()
                    ]),
                },
                identity: Identity {
                    login: Login::new("tibbe-bot"),
                    name: "tibbe-bot".into(),
                    email: "339618913+tibbe-bot@users.noreply.github.com".into(),
                },
                can_push: true,
            }
        );
    }

    #[test_log::test]
    fn the_protection_alone_can_require_linear_history() {
        let mut recorded = recorded_with(Property::Rules, json!([]));
        recorded.resources.insert(
            Property::Protection.key().to_string(),
            fixture(include_str!("fixtures/rest-protection-documented.json")),
        );

        assert!(read(&recorded).unwrap().settings.linear_history);
    }

    #[test_log::test]
    fn without_rules_or_protection_linear_history_is_not_required() {
        let recorded = recorded_with(Property::Rules, Value::Null);

        assert!(!read(&recorded).unwrap().settings.linear_history);
    }

    #[test_log::test]
    fn without_enabled_issue_types_the_categories_are_labels() {
        let mut answer = fixture(include_str!(
            "fixtures/graphql-issue-types-and-merge-methods.json"
        ))["data"]
            .clone();
        answer["repository"]["issueTypes"] = Value::Null;

        let recorded = recorded_with(Property::IssueTypesAndMergeMethods, answer);

        assert_eq!(
            read(&recorded).unwrap().settings.categories,
            Categories::Labels
        );
    }

    #[test_log::test]
    fn an_account_that_cannot_push_is_refused_its_identity() {
        let mut answer = fixture(include_str!("fixtures/rest-repository.json"));
        answer["permissions"]["push"] = json!(false);
        let properties = read(&recorded_with(Property::Repository, answer)).unwrap();

        let refused = properties.pushing_identity().unwrap_err();

        assert_eq!(
            refused.to_string(),
            "tibbe-bot cannot push to tibbe/linewright: Linewright's account needs write access"
        );
    }

    #[test_log::test]
    fn a_missing_property_is_named() {
        for property in EVERY_PROPERTY {
            let mut recorded = tibbe_linewright();
            recorded.resources.remove(property.key());

            let error = read(&recorded).unwrap_err();

            assert_eq!(error, PropertiesError::Missing(property));
            assert!(
                error.to_string().starts_with(&property.to_string()),
                "{error}"
            );
        }
    }

    #[test_log::test]
    fn a_property_that_does_not_read_is_named() {
        let malformed = [
            (
                Property::Repository,
                json!({"full_name": "no-slash", "default_branch": "main"}),
            ),
            (Property::Repository, json!([])),
            (Property::Account, json!({"login": "tibbe-bot"})),
            (
                Property::IssueTypesAndMergeMethods,
                json!({"repository": {}}),
            ),
            (Property::Rules, json!({"type": "required_linear_history"})),
            (Property::Protection, json!([])),
        ];
        for (property, answer) in malformed {
            let error = read(&recorded_with(property, answer.clone())).unwrap_err();

            assert!(
                matches!(&error, PropertiesError::Unreadable { property: named, .. } if *named == property),
                "{answer}: {error}"
            );
            assert!(
                error.to_string().starts_with(&property.to_string()),
                "{error}"
            );
        }
    }
}
