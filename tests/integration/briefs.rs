use std::collections::BTreeMap;

use crate::common;

use linewright::briefs::{BriefLayer, Briefs, OperatorBriefs, Unusable};
use linewright::git::BranchStart;
use linewright::layer::Layer;
use linewright::prompts::{self, Readers};
use linewright::reconcile::WorkKind;
use linewright::work_settings::RunKind;

fn layer(layer: Layer, files: &[(&str, &str)]) -> BriefLayer {
    BriefLayer {
        layer,
        files: files
            .iter()
            .map(|(path, text)| (path.to_string(), text.to_string()))
            .collect(),
    }
}

fn plugin_file<'a>(briefs: &'a Briefs, path: &str) -> &'a str {
    let (_, text) = briefs.plugin.iter().find(|(p, _)| *p == path).unwrap();
    text
}

#[test]
fn each_file_comes_from_the_first_layer_that_has_it_which_is_recorded() {
    let repository = layer(
        Layer::Repository,
        &[
            (
                "prompts/follow-ups/parse-error.md",
                "The repository's: {{error}}",
            ),
            ("plugin/skills/tdd/SKILL.md", "The repository's TDD."),
        ],
    );
    let operator = layer(
        Layer::Operator,
        &[
            (
                "prompts/follow-ups/parse-error.md",
                "The operator's: {{error}}",
            ),
            ("plugin/skills/tdd/SKILL.md", "The operator's TDD."),
            ("plugin/skills/pr/SKILL.md", "The operator's PR."),
        ],
    );

    let resolved = Briefs::resolve(&[&repository, &operator]);

    assert_eq!(resolved.unusable, []);
    let briefs = resolved.briefs;
    assert_eq!(
        prompts::parse_error(&briefs.prompts, "no JSON"),
        "The repository's: no JSON"
    );
    assert_eq!(
        plugin_file(&briefs, "skills/tdd/SKILL.md"),
        "The repository's TDD."
    );
    assert_eq!(
        plugin_file(&briefs, "skills/pr/SKILL.md"),
        "The operator's PR."
    );
    assert_eq!(
        plugin_file(&briefs, "LICENSE"),
        include_str!("../../briefs/plugin/LICENSE")
    );
    let layers: BTreeMap<&str, Layer> = briefs
        .layers()
        .iter()
        .map(|(path, layer)| (path.as_str(), *layer))
        .collect();
    assert_eq!(
        layers["prompts/follow-ups/parse-error.md"],
        Layer::Repository
    );
    assert_eq!(layers["plugin/skills/tdd/SKILL.md"], Layer::Repository);
    assert_eq!(layers["plugin/skills/pr/SKILL.md"], Layer::Operator);
    assert_eq!(layers["plugin/LICENSE"], Layer::Linewright);
    assert_eq!(layers["prompts/triage.md"], Layer::Linewright);
}

#[test]
fn a_prompt_that_cannot_be_used_is_reported_and_the_next_layers_stands_in() {
    let repository = layer(
        Layer::Repository,
        &[("prompts/follow-ups/parse-error.md", "No placeholder.")],
    );
    let operator = layer(
        Layer::Operator,
        &[(
            "prompts/follow-ups/parse-error.md",
            "The operator's: {{error}}",
        )],
    );

    let resolved = Briefs::resolve(&[&repository, &operator]);

    assert_eq!(
        resolved.unusable,
        [Unusable {
            path: "prompts/follow-ups/parse-error.md".into(),
            layer: Layer::Repository,
            error: "it lacks the placeholders `{{error}}`".into(),
        }]
    );
    assert_eq!(
        prompts::parse_error(&resolved.briefs.prompts, "no JSON"),
        "The operator's: no JSON"
    );
    assert_eq!(
        resolved.briefs.layers()["prompts/follow-ups/parse-error.md"],
        Layer::Operator
    );
}

/// A `briefs` directory holding only `files`, each at its path under it.
fn briefs_dir(files: &[(&str, &str)]) -> tempfile::TempDir {
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
    let dir = briefs_dir(&[("prompts/implement.md", IMPLEMENT)]);

    let operator = OperatorBriefs::read(Some(dir.path())).unwrap();

    assert_eq!(
        operator.layer().files.keys().collect::<Vec<_>>(),
        ["prompts/implement.md"]
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
    assert_eq!(briefs.layers()["prompts/implement.md"], Layer::Operator);
    assert_eq!(
        briefs.layers()["prompts/outputs/implement.md"],
        Layer::Linewright
    );
    assert_eq!(briefs.layers()["prompts/triage.md"], Layer::Linewright);
    assert_eq!(
        briefs.layers()["plugin/skills/tdd/SKILL.md"],
        Layer::Linewright
    );
    assert_eq!(
        plugin_file(&briefs, "skills/tdd/SKILL.md"),
        include_str!("../../briefs/plugin/skills/tdd/SKILL.md")
    );
}

#[test]
fn an_operators_files_outside_linewrights_layout_are_unused() {
    let dir = briefs_dir(&[
        ("README.md", "Ours."),
        ("prompts/tickets.md", "An earlier release's."),
        ("prompts/implement.md", IMPLEMENT),
    ]);

    let operator = OperatorBriefs::read(Some(dir.path())).unwrap();

    assert_eq!(operator.unused(), ["README.md", "prompts/tickets.md"]);
    assert_eq!(
        operator.layer().files.keys().collect::<Vec<_>>(),
        ["prompts/implement.md"]
    );
}

#[test]
fn an_operators_prompt_without_its_output_is_refused() {
    let dir = briefs_dir(&[(
        "prompts/implement.md",
        "{{facts}} {{branch}} {{holding}} {{default_branch}} {{ticket}}",
    )]);

    let error = OperatorBriefs::read(Some(dir.path())).unwrap_err();

    assert_eq!(
        format!("{error:#}"),
        "reading the prompt implement.md: it lacks the placeholders `{{output}}`"
    );
}

#[test]
fn an_operators_result_contract_is_filled_into_the_prompt() {
    let dir = briefs_dir(&[(
        "prompts/outputs/implement.md",
        "End with {\"outcome\": \"done\"}.\n",
    )]);

    let briefs = OperatorBriefs::read(Some(dir.path())).unwrap().briefs();

    assert!(
        implement_prompt(&briefs).ends_with("### 2. Output\n\nEnd with {\"outcome\": \"done\"}.\n"),
        "{}",
        implement_prompt(&briefs)
    );
}

#[test]
fn every_prompt_and_part_has_the_runs_it_is_sent_to_declared() {
    for path in prompts::Prompts::paths() {
        assert!(
            prompts::readers(path).is_some(),
            "{path} has no readers declared"
        );
    }
    assert_eq!(prompts::readers("tickets.md"), None);
}

#[test]
fn a_part_is_read_only_by_the_runs_it_is_sent_to_and_the_plugin_by_every_run() {
    let fix_ci = RunKind::Stage(WorkKind::FixCi);
    let implement = RunKind::Stage(WorkKind::Implement);
    let full_log = prompts::readers("parts/full-log.md").unwrap();
    let parse_error = prompts::readers("follow-ups/parse-error.md").unwrap();
    let automation = linewright::automation::parse(
        &"weekly".parse().unwrap(),
        "---\non:\n  schedule: \"0 9 * * 1\"\n---\nLook.\n",
    )
    .unwrap();

    assert!(full_log.read_by(&fix_ci));
    assert!(!full_log.read_by(&implement));
    assert!(!full_log.read_by(&RunKind::Automation(&automation)));
    assert!(parse_error.read_by(&RunKind::Automation(&automation)));
    assert_eq!(
        Briefs::readers("plugin/skills/tdd/SKILL.md"),
        Some(Readers::Every)
    );
    assert_eq!(Briefs::readers("prompts/parts/full-log.md"), Some(full_log));
    assert_eq!(Briefs::readers("README.md"), None);
}
