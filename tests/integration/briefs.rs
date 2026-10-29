use std::collections::BTreeMap;

use crate::common;

use linewright::briefs::{Briefs, OperatorPrompts, PromptLayer, Unusable};
use linewright::git::BranchStart;
use linewright::layer::Layer;
use linewright::prompts::{self, Readers, Reads};
use linewright::reconcile::WorkKind;
use linewright::store::Blob;
use linewright::work_settings::RunKind;

/// The layer of `files`, each of which must be one of Linewright's prompts.
fn layer(layer: Layer, files: &[(&str, &str)]) -> PromptLayer {
    let (layer, unusable) = PromptLayer::parse(layer, listing(files));
    assert_eq!(unusable, []);
    layer
}

fn listing(files: &[(&str, &str)]) -> Vec<(String, Blob)> {
    files
        .iter()
        .map(|(path, text)| (path.to_string(), Blob::Text(text.to_string())))
        .collect()
}

fn not_a_prompt(path: &str) -> Unusable {
    Unusable {
        path: path.into(),
        layer: Layer::Repository,
        error: "not one of Linewright's prompts".into(),
        holds: Readers::Every,
    }
}

#[test]
fn each_prompt_comes_from_the_first_layer_that_has_it_which_is_recorded() {
    let repository = layer(
        Layer::Repository,
        &[("follow-ups/result-error.md", "The repository's: {{error}}")],
    );
    let operator = layer(
        Layer::Operator,
        &[
            ("follow-ups/result-error.md", "The operator's: {{error}}"),
            ("follow-ups/loose-ends.md", "The operator's: {{problem}}"),
        ],
    );

    let resolved = Briefs::resolve(&[&repository, &operator]);

    assert_eq!(resolved.unusable, []);
    let briefs = resolved.briefs;
    assert_eq!(
        prompts::result_error(&briefs.prompts, "no JSON"),
        "The repository's: no JSON"
    );
    assert_eq!(
        prompts::loose_ends(&briefs.prompts, "a stash"),
        "The operator's: a stash"
    );
    let layers: BTreeMap<&str, Layer> = briefs
        .layers()
        .iter()
        .map(|(path, layer)| (path.as_str(), *layer))
        .collect();
    assert_eq!(layers["follow-ups/result-error.md"], Layer::Repository);
    assert_eq!(layers["follow-ups/loose-ends.md"], Layer::Operator);
    assert_eq!(layers["triage.md"], Layer::Linewright);
    assert_eq!(layers.len(), 11);
    assert!(!layers.contains_key("parts/facts.md"));
    assert!(!layers.contains_key("outputs/implement.md"));
}

#[test]
fn a_prompt_that_cannot_be_used_is_reported_and_the_next_layers_stands_in() {
    let repository = layer(
        Layer::Repository,
        &[("follow-ups/result-error.md", "No placeholder.")],
    );
    let operator = layer(
        Layer::Operator,
        &[("follow-ups/result-error.md", "The operator's: {{error}}")],
    );

    let resolved = Briefs::resolve(&[&repository, &operator]);

    assert_eq!(
        resolved.unusable,
        [Unusable {
            path: "follow-ups/result-error.md".into(),
            layer: Layer::Repository,
            error: "it lacks the placeholders `{{error}}`".into(),
            holds: Readers::Every,
        }]
    );
    assert_eq!(
        prompts::result_error(&resolved.briefs.prompts, "no JSON"),
        "The operator's: no JSON"
    );
    assert_eq!(
        resolved.briefs.layers()["follow-ups/result-error.md"],
        Layer::Operator
    );
}

#[test]
fn a_layers_files_that_are_not_linewrights_prompts_are_unusable_and_hidden_ones_skipped() {
    let (layer, unusable) = PromptLayer::parse(
        Layer::Repository,
        listing(&[
            ("implment.md", "A misspelling."),
            ("prompts/implement.md", "The layout of an earlier release."),
            ("plugin/skills/tdd/SKILL.md", "Our own TDD."),
            ("README.md", "Ours."),
            (".DS_Store", "Finder's."),
            (".git/HEAD", "ref: refs/heads/main"),
            ("parts/.facts.md.swp", "An editor's."),
            ("follow-ups/result-error.md", "Ours: {{error}}"),
        ])
        .into_iter()
        .chain([
            ("logo.png".to_string(), Blob::Binary),
            (".DS_Store".to_string(), Blob::Binary),
            ("parts/x/y.md".to_string(), Blob::Text("Too deep.".into())),
            ("parts/.cache/x".to_string(), Blob::Binary),
            // A directory at a prompt's path.
            ("rework.md/x".to_string(), Blob::Text("Ours.".into())),
        ]),
    );

    assert_eq!(
        layer.files.keys().collect::<Vec<_>>(),
        ["follow-ups/result-error.md"]
    );
    assert_eq!(
        unusable,
        [
            not_a_prompt("README.md"),
            not_a_prompt("implment.md"),
            not_a_prompt("logo.png"),
            not_a_prompt("parts/x/y.md"),
            not_a_prompt("plugin/skills/tdd/SKILL.md"),
            not_a_prompt("prompts/implement.md"),
            not_a_prompt("rework.md/x"),
        ]
    );
}

#[test]
fn a_prompt_that_cannot_be_read_as_text_is_unusable() {
    let (layer, unusable) = PromptLayer::parse(
        Layer::Operator,
        [
            ("triage.md".to_string(), Blob::Binary),
            ("design.md".to_string(), Blob::TooLarge),
        ],
    );

    assert!(layer.files.is_empty());
    assert_eq!(
        unusable,
        [
            Unusable {
                path: "design.md".into(),
                layer: Layer::Operator,
                error: "it is too large to read".into(),
                holds: Readers::Only(&[Reads::Stage(WorkKind::Design)]),
            },
            Unusable {
                path: "triage.md".into(),
                layer: Layer::Operator,
                error: "it is not UTF-8 text".into(),
                holds: Readers::Only(&[Reads::Stage(WorkKind::Triage)]),
            },
        ]
    );
}

/// A `prompts` directory holding only `files`, each at its path under it.
fn prompts_dir(files: &[(&str, &str)]) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    for (path, text) in files {
        let path = dir.path().join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }
    dir
}

const IMPLEMENT: &str = "Our own: {{facts}} {{branch}} {{holding}} {{default_branch}}\n\n{{ticket}}\n\n## Output\n\n{{output}}\n";

fn implement_prompt(briefs: &Briefs) -> String {
    let issue = common::ticket(7, "Count the widgets");
    let name = "owner/widgets".parse().unwrap();
    prompts::implement(
        &briefs.prompts,
        &prompts::Repo {
            name: &name,
            default_branch: "main",
            spec_glob: "docs/**",
        },
        &prompts::Ticket::new(&issue, None, vec![], None),
        "7-count-the-widgets",
        &BranchStart::New,
    )
}

#[test]
fn an_operators_directory_with_one_prompt_changes_that_prompt_and_the_rest_are_linewrights() {
    let dir = prompts_dir(&[("implement.md", IMPLEMENT)]);

    let operator = OperatorPrompts::read(Some(dir.path())).unwrap();

    assert_eq!(
        operator.layer().files.keys().collect::<Vec<_>>(),
        ["implement.md"]
    );
    let briefs = operator.briefs();
    let prompt = implement_prompt(&briefs);
    assert!(
        prompt.starts_with("Our own: - Repository: `owner/widgets`"),
        "{prompt}"
    );
    // The contract Linewright reads is filled in.
    assert!(
        prompt.contains(
            "\n\n## Output\n\nEnd your session with a final message that is a single JSON \
             object and nothing else, one of:\n\n- `done`, "
        ),
        "{prompt}"
    );
    assert!(
        prompt.ends_with(
            "`declined_findings` lists the review findings you did not apply (often none).\n"
        ),
        "{prompt}"
    );
    assert_eq!(briefs.layers()["implement.md"], Layer::Operator);
    assert!(!briefs.layers().contains_key("outputs/implement.md"));
    assert_eq!(briefs.layers()["triage.md"], Layer::Linewright);
}

#[test]
fn an_operators_hidden_files_are_skipped() {
    let dir = prompts_dir(&[
        (".DS_Store", "Finder's."),
        (".git/HEAD", "ref: refs/heads/main"),
        ("implement.md", IMPLEMENT),
    ]);

    let operator = OperatorPrompts::read(Some(dir.path())).unwrap();

    assert_eq!(
        operator.layer().files.keys().collect::<Vec<_>>(),
        ["implement.md"]
    );
}

#[test]
fn an_operators_files_that_cannot_be_used_are_refused_each_named() {
    let dir = prompts_dir(&[
        ("implment.md", "A misspelling."),
        ("plugin/skills/tdd/SKILL.md", "Our own TDD."),
        (
            "implement.md",
            "{{facts}} {{branch}} {{holding}} {{default_branch}} {{ticket}}",
        ),
    ]);

    let error = OperatorPrompts::read(Some(dir.path())).unwrap_err();

    assert_eq!(
        format!("{error:#}"),
        format!(
            "the `prompts` directory {} holds files Linewright cannot use:\n\
             - implement.md: it lacks the placeholders `{{{{output}}}}`\n\
             - implment.md: not one of Linewright's prompts\n\
             - plugin/skills/tdd/SKILL.md: not one of Linewright's prompts",
            dir.path().display()
        )
    );
}

#[test]
fn an_operators_part_or_result_contract_is_refused() {
    for path in ["parts/facts.md", "outputs/implement.md"] {
        let dir = prompts_dir(&[(path, "Ours.\n")]);

        let error = OperatorPrompts::read(Some(dir.path())).unwrap_err();

        assert_eq!(
            format!("{error:#}"),
            format!(
                "the `prompts` directory {} holds files Linewright cannot use:\n\
                 - {path}: not one of Linewright's prompts",
                dir.path().display()
            )
        );
    }
}

#[test]
fn the_prompts_a_layer_may_override_are_the_stage_prompts_and_the_follow_ups() {
    assert_eq!(
        prompts::Prompts::paths(),
        [
            "triage.md",
            "design.md",
            "implement.md",
            "rework.md",
            "fix-ci.md",
            "update.md",
            "automation.md",
            "follow-ups/result-error.md",
            "follow-ups/loose-ends.md",
            "follow-ups/usage-limit-reset.md",
            "follow-ups/late-feedback.md",
        ]
    );
    for path in prompts::Prompts::paths() {
        assert!(
            prompts::readers(path).is_some(),
            "{path} has no readers declared"
        );
    }
    assert_eq!(prompts::readers("tickets.md"), None);
    assert_eq!(prompts::readers("parts/facts.md"), None);
    assert_eq!(prompts::readers("outputs/implement.md"), None);
}

#[test]
fn a_prompt_is_read_only_by_the_runs_it_is_sent_to() {
    let fix_ci = RunKind::Stage(WorkKind::FixCi);
    let implement = RunKind::Stage(WorkKind::Implement);
    let fix_ci_prompt = prompts::readers("fix-ci.md").unwrap();
    let result_error = prompts::readers("follow-ups/result-error.md").unwrap();
    let automation = linewright::automation::parse(
        &"weekly".parse().unwrap(),
        "---\non:\n  schedule: \"0 9 * * 1\"\n---\nLook.\n",
    )
    .unwrap();

    assert!(fix_ci_prompt.read_by(&fix_ci));
    assert!(!fix_ci_prompt.read_by(&implement));
    assert!(!fix_ci_prompt.read_by(&RunKind::Automation(&automation)));
    assert!(result_error.read_by(&RunKind::Automation(&automation)));
}
