use std::collections::BTreeMap;
use std::path::Path;

use crate::common;

use linewright::briefs::{BriefLayer, Briefs, OperatorBriefs, Unusable};
use linewright::git::BranchStart;
use linewright::layer::Layer;
use linewright::prompts::{self, LooseEnd, Readers, Reads, SentBack, Templates};
use linewright::reconcile::WorkKind;
use linewright::results::ParseError;
use linewright::store::Blob;
use linewright::work_settings::RunKind;

/// The layer of `files`, each of which must be one a layer may hold.
fn layer(layer: Layer, files: &[(&str, &str)]) -> BriefLayer {
    let (layer, unusable) = BriefLayer::parse(layer, listing(files));
    assert_eq!(unusable, []);
    layer
}

fn listing(files: &[(&str, &str)]) -> Vec<(String, Blob)> {
    files
        .iter()
        .map(|(path, text)| (path.to_string(), Blob::Text(text.to_string())))
        .collect()
}

fn not_held(path: &str) -> Unusable {
    Unusable {
        path: path.into(),
        layer: Layer::Repository,
        error: "not a file a layer may hold".into(),
        holds: Readers::Every,
    }
}

fn no_json() -> SentBack<'static> {
    SentBack::Result(&ParseError::NoJson)
}

const NO_JSON: &str =
    "The final message holds no JSON object. End with the result as a single JSON object.";

const NO_COMMITS: &str = "you answered `done`, but the branch has no commits. Commit your work, \
                          or answer `gave-up` with the reason if the ticket needs no change.";

#[test_log::test]
fn each_file_comes_from_the_first_layer_that_has_it_which_is_recorded() {
    let repository = layer(
        Layer::Repository,
        &[
            (
                "prompts/follow-ups/result-error.md",
                "The repository's: {{error}}",
            ),
            ("parts/result-no-json.md", "The repository's own words."),
        ],
    );
    let operator = layer(
        Layer::Operator,
        &[
            (
                "prompts/follow-ups/result-error.md",
                "The operator's: {{error}}",
            ),
            (
                "prompts/follow-ups/loose-ends.md",
                "The operator's: {{problem}}",
            ),
            ("parts/result-no-json.md", "The operator's own words."),
            ("plugin/skills/tdd/SKILL.md", "The operator's TDD.\n"),
        ],
    );

    let resolved = Briefs::resolve(&[&repository, &operator]);

    assert_eq!(resolved.unusable, []);
    let briefs = resolved.briefs;
    assert_eq!(
        prompts::result_error(&briefs.templates, &no_json()),
        "The repository's: The repository's own words."
    );
    assert_eq!(
        prompts::loose_ends(&briefs.templates, &LooseEnd::NoCommits),
        format!("The operator's: {NO_COMMITS}")
    );
    let layers: BTreeMap<&str, Layer> = briefs
        .layers()
        .iter()
        .map(|(path, layer)| (path.as_str(), *layer))
        .collect();
    assert_eq!(
        layers["prompts/follow-ups/result-error.md"],
        Layer::Repository
    );
    assert_eq!(layers["parts/result-no-json.md"], Layer::Repository);
    assert_eq!(layers["prompts/follow-ups/loose-ends.md"], Layer::Operator);
    assert_eq!(layers["plugin/skills/tdd/SKILL.md"], Layer::Operator);
    assert_eq!(layers["prompts/triage.md"], Layer::Linewright);
    assert_eq!(layers["parts/facts.md"], Layer::Linewright);
    assert_eq!(
        layers["plugin/skills/implement/SKILL.md"],
        Layer::Linewright
    );
    assert!(!layers.contains_key("plugin/.claude-plugin/plugin.json"));
    assert!(!layers.contains_key("results/implement.schema.json"));
}

#[test_log::test]
fn a_file_that_cannot_be_used_is_reported_and_the_next_layers_stands_in() {
    let repository = layer(
        Layer::Repository,
        &[
            ("prompts/follow-ups/result-error.md", "No placeholder."),
            ("plugin/skills/tdd/SKILL.md", "Mind the {{ticket}}.\n"),
        ],
    );
    let operator = layer(
        Layer::Operator,
        &[(
            "prompts/follow-ups/result-error.md",
            "The operator's: {{error}}",
        )],
    );

    let resolved = Briefs::resolve(&[&repository, &operator]);

    assert_eq!(
        resolved.unusable,
        [
            Unusable {
                path: "plugin/skills/tdd/SKILL.md".into(),
                layer: Layer::Repository,
                error: "it has unknown placeholders: `{{ticket}}`".into(),
                holds: Readers::Every,
            },
            Unusable {
                path: "prompts/follow-ups/result-error.md".into(),
                layer: Layer::Repository,
                error: "it lacks the placeholders `{{error}}`".into(),
                holds: Readers::Every,
            },
        ]
    );
    assert_eq!(
        prompts::result_error(&resolved.briefs.templates, &no_json()),
        format!("The operator's: {NO_JSON}")
    );
    assert_eq!(
        resolved.briefs.layers()["prompts/follow-ups/result-error.md"],
        Layer::Operator
    );
    assert_eq!(
        resolved.briefs.layers()["plugin/skills/tdd/SKILL.md"],
        Layer::Linewright
    );
}

#[test_log::test]
fn a_layers_files_that_it_may_not_hold_are_unusable_and_hidden_ones_skipped() {
    let directory_at_a_prompts_path = (
        "prompts/rework.md/x".to_string(),
        Blob::Text("Ours.".into()),
    );
    let (layer, unusable) = BriefLayer::parse(
        Layer::Repository,
        listing(&[
            ("prompts/implment.md", "A misspelling."),
            ("implement.md", "The layout of an earlier release."),
            ("results/implement.schema.json", "{}"),
            ("plugin/skills/mine/SKILL.md", "A skill of our own."),
            ("README.md", "Ours."),
            (".DS_Store", "Finder's."),
            (".git/HEAD", "ref: refs/heads/main"),
            ("parts/.facts.md.swp", "An editor's."),
            ("plugin/.claude-plugin/plugin.json", "{}"),
            ("prompts/follow-ups/result-error.md", "Ours: {{error}}"),
            (
                "parts/facts.md",
                "Ours: {{repo}} {{default_branch}} {{spec_glob}}",
            ),
            ("outputs/implement.md", "Ours."),
            ("plugin/skills/tdd/SKILL.md", "Our own TDD."),
        ])
        .into_iter()
        .chain([
            ("logo.png".to_string(), Blob::Binary),
            (".DS_Store".to_string(), Blob::Binary),
            ("parts/x/y.md".to_string(), Blob::Text("Too deep.".into())),
            ("parts/.cache/x".to_string(), Blob::Binary),
            directory_at_a_prompts_path,
        ]),
    );

    assert_eq!(
        layer.files.keys().collect::<Vec<_>>(),
        [
            "outputs/implement.md",
            "parts/facts.md",
            "plugin/skills/tdd/SKILL.md",
            "prompts/follow-ups/result-error.md",
        ]
    );
    assert_eq!(
        unusable,
        [
            not_held("README.md"),
            not_held("implement.md"),
            not_held("logo.png"),
            not_held("parts/x/y.md"),
            not_held("plugin/skills/mine/SKILL.md"),
            not_held("prompts/implment.md"),
            not_held("prompts/rework.md/x"),
            not_held("results/implement.schema.json"),
        ]
    );
}

#[test_log::test]
fn a_file_that_cannot_be_read_as_text_is_unusable() {
    let (layer, unusable) = BriefLayer::parse(
        Layer::Operator,
        [
            ("prompts/triage.md".to_string(), Blob::Binary),
            ("prompts/design.md".to_string(), Blob::TooLarge),
            ("plugin/LICENSE".to_string(), Blob::Binary),
        ],
    );

    assert!(layer.files.is_empty());
    assert_eq!(
        unusable,
        [
            Unusable {
                path: "plugin/LICENSE".into(),
                layer: Layer::Operator,
                error: "it is not text".into(),
                holds: Readers::Every,
            },
            Unusable {
                path: "prompts/design.md".into(),
                layer: Layer::Operator,
                error: "it is too large to read".into(),
                holds: Readers::Only(&[Reads::Stage(WorkKind::Design)]),
            },
            Unusable {
                path: "prompts/triage.md".into(),
                layer: Layer::Operator,
                error: "it is not text".into(),
                holds: Readers::Only(&[Reads::Stage(WorkKind::Triage)]),
            },
        ]
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
        &briefs.templates,
        &prompts::Repo {
            name: &name,
            default_branch: "main",
            spec_glob: "docs/**",
        },
        &common::ready(prompts::Ticket::read(&issue, None, vec![])),
        "7-count-the-widgets",
        &BranchStart::New,
        std::path::Path::new("/attachments"),
    )
}

#[test_log::test]
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
    assert!(
        prompt.contains(
            "\n\n## Output\n\nEnd your session with a final message that is a single JSON \
             object and nothing else, one of:\n\n- `done`, "
        ),
        "{prompt}"
    );
    assert!(
        prompt.contains(
            "`declined_findings` lists the review findings you did not apply.\n\n\
             To show a screenshot or a video in the Markdown your result posts, save the file \
             in your attachments directory, `/attachments`, "
        ) && prompt.ends_with("in a fenced code block or a `<details>` block when it is long.\n"),
        "{prompt}"
    );
    assert_eq!(briefs.layers()["prompts/implement.md"], Layer::Operator);
    assert_eq!(briefs.layers()["outputs/implement.md"], Layer::Linewright);
    assert_eq!(briefs.layers()["prompts/triage.md"], Layer::Linewright);
}

#[test_log::test]
fn an_operators_part_and_output_are_filled_into_the_prompt() {
    let dir = briefs_dir(&[
        (
            "parts/facts.md",
            "Ours: {{repo}} on {{default_branch}}, {{spec_glob}}.\n",
        ),
        ("outputs/implement.md", "Answer as you see fit.\n"),
    ]);

    let briefs = OperatorBriefs::read(Some(dir.path())).unwrap().briefs();
    let prompt = implement_prompt(&briefs);

    assert!(
        prompt.contains("Ours: owner/widgets on main, docs/**.\n"),
        "{prompt}"
    );
    assert!(
        prompt.contains("Answer as you see fit.\n\nTo show a screenshot"),
        "{prompt}"
    );
    assert_eq!(briefs.layers()["parts/facts.md"], Layer::Operator);
    assert_eq!(briefs.layers()["outputs/implement.md"], Layer::Operator);
}

#[test_log::test]
fn an_operators_automation_prompt_is_the_one_an_automation_is_sent() {
    let dir = briefs_dir(&[(
        "prompts/automation.md",
        "Mind the schedule of `{{name}}`, at most {{max_issues}}: {{instructions}}\n\
         {{facts}}\n\n{{output}}\n",
    )]);
    let automation = linewright::automation::parse(
        &"weekly".parse().unwrap(),
        "---\non:\n  schedule: \"0 6 * * 1\"\nmax-issues: 3\n---\nLook.\n",
    )
    .unwrap();
    let name = "owner/widgets".parse().unwrap();

    let briefs = OperatorBriefs::read(Some(dir.path())).unwrap().briefs();
    let prompt = prompts::automation(
        &briefs.templates,
        &prompts::Repo {
            name: &name,
            default_branch: "main",
            spec_glob: "docs/**",
        },
        &automation,
        std::path::Path::new("/attachments"),
    );

    assert!(
        prompt.starts_with(
            "Mind the schedule of `weekly`, at most 3: Look.\n- Repository: `owner/widgets`"
        ),
        "{prompt}"
    );
    assert_eq!(briefs.layers()["prompts/automation.md"], Layer::Operator);
}

#[test_log::test]
fn an_operators_hidden_files_are_skipped() {
    let dir = briefs_dir(&[
        (".DS_Store", "Finder's."),
        (".git/HEAD", "ref: refs/heads/main"),
        ("plugin/.claude-plugin/plugin.json", "{}"),
        ("prompts/implement.md", IMPLEMENT),
    ]);

    let operator = OperatorBriefs::read(Some(dir.path())).unwrap();

    assert_eq!(
        operator.layer().files.keys().collect::<Vec<_>>(),
        ["prompts/implement.md"]
    );
}

#[test_log::test]
fn an_operators_files_that_cannot_be_used_are_refused_each_named() {
    let dir = briefs_dir(&[
        ("prompts/implment.md", "A misspelling."),
        ("results/implement.schema.json", "{}"),
        ("plugin/skills/tdd/SKILL.md", "Mind {{this}}."),
        (
            "prompts/implement.md",
            "{{facts}} {{branch}} {{holding}} {{default_branch}} {{ticket}}",
        ),
    ]);

    let error = OperatorBriefs::read(Some(dir.path())).unwrap_err();

    assert_eq!(
        format!("{error:#}"),
        format!(
            "the `briefs` directory {} holds files Linewright cannot use:\n\
             - plugin/skills/tdd/SKILL.md: it has unknown placeholders: `{{{{this}}}}`\n\
             - prompts/implement.md: it lacks the placeholders `{{{{output}}}}`\n\
             - prompts/implment.md: not a file a layer may hold\n\
             - results/implement.schema.json: not a file a layer may hold",
            dir.path().display()
        )
    );
}

#[test_log::test]
fn every_file_of_the_brief_but_the_results_and_the_plugins_manifest_is_layered() {
    let shipped = Path::new(env!("CARGO_MANIFEST_DIR")).join("briefs");
    let expected: Vec<String> = ["prompts", "parts", "outputs", "plugin"]
        .into_iter()
        .flat_map(|dir| {
            files_under(&shipped.join(dir))
                .into_keys()
                .filter(|path| !path.split('/').any(|name| name.starts_with('.')))
                .map(move |path| format!("{dir}/{path}"))
        })
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect();

    let layered: Vec<String> = Briefs::shipped().layers().keys().cloned().collect();

    assert_eq!(layered, expected);
}

#[test_log::test]
fn every_file_of_the_brief_but_the_plugin_has_its_readers_by_its_path_under_briefs() {
    let paths: Vec<&str> = Templates::paths().collect();
    let distinct: std::collections::BTreeSet<&str> = paths.iter().copied().collect();

    assert_eq!(distinct.len(), paths.len());
    for path in [
        "prompts/implement.md",
        "prompts/judge.md",
        "prompts/follow-ups/result-error.md",
        "parts/facts.md",
        "outputs/implement.md",
    ] {
        assert!(paths.contains(&path), "{path}");
        assert!(prompts::readers(path).is_some(), "{path}");
    }
    assert_eq!(prompts::readers("implement.md"), None);
    assert_eq!(prompts::readers("prompts/tickets.md"), None);
    assert_eq!(prompts::readers("results/implement.schema.json"), None);
}

#[test_log::test]
fn every_file_of_the_brief_has_its_values_placeholders() {
    let briefs = Path::new(env!("CARGO_MANIFEST_DIR")).join("briefs");
    let texts: BTreeMap<&str, String> = Templates::paths()
        .map(|path| (path, std::fs::read_to_string(briefs.join(path)).unwrap()))
        .collect();

    let (_, unusable) = Templates::read(&mut |path| texts.get(path).map(String::as_str));

    let unusable: Vec<String> = unusable
        .iter()
        .map(|(path, error)| format!("{path}: {error:#}"))
        .collect();
    assert_eq!(unusable, Vec::<String>::new());
}

#[test_log::test]
fn the_judges_files_are_read_by_the_judge_only() {
    for path in [
        "prompts/judge.md",
        "outputs/judge.md",
        "parts/replay-output.md",
    ] {
        let readers = prompts::readers(path).unwrap();

        assert!(readers.read_by(&RunKind::Judge), "{path}");
        for kind in WorkKind::ALL {
            assert!(!readers.read_by(&RunKind::Stage(kind)), "{path}");
        }
    }
}

#[test_log::test]
fn a_prompt_is_read_only_by_the_runs_it_is_sent_to() {
    let fix_ci = RunKind::Stage(WorkKind::FixCi);
    let implement = RunKind::Stage(WorkKind::Implement);
    let fix_ci_prompt = prompts::readers("prompts/fix-ci.md").unwrap();
    let result_error = prompts::readers("prompts/follow-ups/result-error.md").unwrap();
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

#[test_log::test]
fn a_file_with_an_error_holds_the_judge_only_where_it_holds_every_run() {
    let holds_the_judge = |path| prompts::readers(path).unwrap().read_by(&RunKind::Judge);

    assert!(holds_the_judge("prompts/follow-ups/result-error.md"));
    assert!(holds_the_judge("prompts/follow-ups/usage-limit-reset.md"));
    for path in [
        "prompts/triage.md",
        "prompts/implement.md",
        "prompts/fix-ci.md",
        "prompts/follow-ups/late-feedback.md",
    ] {
        assert!(!holds_the_judge(path), "{path}");
    }
}

#[test_log::test]
fn the_plugin_written_is_every_file_of_briefs_plugin_as_it_is() {
    let dir = tempfile::tempdir().unwrap();

    Briefs::shipped().write_plugin(dir.path()).unwrap();

    let shipped = Path::new(env!("CARGO_MANIFEST_DIR")).join("briefs/plugin");
    let written = files_under(dir.path());
    assert!(written.contains_key(".claude-plugin/plugin.json"));
    assert_eq!(written, files_under(&shipped));
}

#[test_log::test]
fn the_plugin_written_is_the_briefs_own_with_each_skill_from_its_layer() {
    let dir = tempfile::tempdir().unwrap();
    let operator = layer(
        Layer::Operator,
        &[("plugin/skills/tdd/SKILL.md", "The operator's TDD.\n")],
    );

    Briefs::resolve(&[&operator])
        .briefs
        .write_plugin(dir.path())
        .unwrap();

    let shipped = Path::new(env!("CARGO_MANIFEST_DIR")).join("briefs/plugin");
    let mut expected = files_under(&shipped);
    expected.insert("skills/tdd/SKILL.md".into(), "The operator's TDD.\n".into());
    assert_eq!(files_under(dir.path()), expected);
}

/// Every file under `root`, hidden ones too, by its path under it, with its
/// text.
#[track_caller]
fn files_under(root: &Path) -> BTreeMap<String, String> {
    let mut files = BTreeMap::new();
    let mut dirs = vec![root.to_path_buf()];
    while let Some(dir) = dirs.pop() {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                dirs.push(path);
            } else {
                let under = path.strip_prefix(root).unwrap().to_str().unwrap();
                files.insert(under.to_string(), std::fs::read_to_string(&path).unwrap());
            }
        }
    }
    files
}

#[test_log::test]
fn the_loose_ends_follow_up_is_read_by_every_run_whose_result_may_attach_a_secret() {
    let readers = prompts::readers("prompts/follow-ups/loose-ends.md").unwrap();
    let automation = linewright::automation::parse(
        &"weekly".parse().unwrap(),
        "---\non:\n  schedule: \"0 9 * * 1\"\n---\nLook.\n",
    )
    .unwrap();

    for kind in WorkKind::ALL {
        assert!(readers.read_by(&RunKind::Stage(kind)), "{kind:?}");
    }
    assert!(readers.read_by(&RunKind::Automation(&automation)));
}
