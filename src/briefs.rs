use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, Result, anyhow};

use crate::layer::Layer;
use crate::prompts::{self, OwnTemplates, Readers, Templates};
use crate::store::Blob;

/// The brief a session is sent: its templates and its plugin, each file
/// from the first layer that has it (docs/specs/operation.md, "Briefs").
#[derive(Clone, Debug)]
pub struct Briefs {
    pub templates: Templates,
    /// The plugin's files, by their paths under `briefs/plugin/`.
    plugin: BTreeMap<String, String>,
    /// The layer of each file a layer may hold, by its path under `briefs/`.
    layers: BTreeMap<String, Layer>,
}

/// One layer of the briefs: each file it has that a layer may hold, by its
/// path under `briefs/`, such as `prompts/implement.md` or
/// `plugin/skills/tdd/SKILL.md`.
#[derive(Clone, Debug)]
pub struct BriefLayer {
    pub layer: Layer,
    pub files: BTreeMap<String, String>,
}

/// A layer's file Linewright cannot use, and why.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Unusable {
    /// Its path under the layer's root.
    pub path: String,
    pub layer: Layer,
    pub error: String,
    /// The runs that would read it: every run for a file that a layer may
    /// not hold, as no run is known to read it.
    pub holds: Readers,
}

/// The briefs resolved from layers, and the files in them that could not be
/// used, for each of which the next layer's file stands in.
#[derive(Clone, Debug)]
pub struct Resolved {
    pub briefs: Briefs,
    pub unusable: Vec<Unusable>,
}

/// The operator's layer of the briefs, each of whose files can be used, and
/// the briefs it makes over Linewright's.
#[derive(Clone, Debug)]
pub struct OperatorBriefs {
    /// Linewright's own layer, under the operator's.
    own: OwnTemplates,
    layer: BriefLayer,
    briefs: Arc<Briefs>,
}

/// The skills every session is given, as a Claude Code plugin, by their
/// paths under `briefs/plugin/`: every file there, as a test checks.
const PLUGIN: [(&str, &str); 15] = [
    (
        ".claude-plugin/plugin.json",
        include_str!("../briefs/plugin/.claude-plugin/plugin.json"),
    ),
    ("LICENSE", include_str!("../briefs/plugin/LICENSE")),
    (
        "skills/triage/SKILL.md",
        include_str!("../briefs/plugin/skills/triage/SKILL.md"),
    ),
    (
        "skills/to-tickets/SKILL.md",
        include_str!("../briefs/plugin/skills/to-tickets/SKILL.md"),
    ),
    (
        "skills/software-design/SKILL.md",
        include_str!("../briefs/plugin/skills/software-design/SKILL.md"),
    ),
    (
        "skills/software-design/references/benchmark.md",
        include_str!("../briefs/plugin/skills/software-design/references/benchmark.md"),
    ),
    (
        "skills/software-design/references/external-behavior.md",
        include_str!("../briefs/plugin/skills/software-design/references/external-behavior.md"),
    ),
    (
        "skills/prototype/SKILL.md",
        include_str!("../briefs/plugin/skills/prototype/SKILL.md"),
    ),
    (
        "skills/implement/SKILL.md",
        include_str!("../briefs/plugin/skills/implement/SKILL.md"),
    ),
    (
        "skills/tdd/SKILL.md",
        include_str!("../briefs/plugin/skills/tdd/SKILL.md"),
    ),
    (
        "skills/tdd/tests.md",
        include_str!("../briefs/plugin/skills/tdd/tests.md"),
    ),
    (
        "skills/tdd/mocking.md",
        include_str!("../briefs/plugin/skills/tdd/mocking.md"),
    ),
    (
        "skills/code-review/SKILL.md",
        include_str!("../briefs/plugin/skills/code-review/SKILL.md"),
    ),
    (
        "skills/pr/SKILL.md",
        include_str!("../briefs/plugin/skills/pr/SKILL.md"),
    ),
    (
        "skills/pr/CREDITS.md",
        include_str!("../briefs/plugin/skills/pr/CREDITS.md"),
    ),
];

/// The plugin's files, by their paths under `briefs/plugin/`.
fn shipped_plugin() -> BTreeMap<String, String> {
    PLUGIN
        .iter()
        .map(|(path, text)| (path.to_string(), text.to_string()))
        .collect()
}

/// Where the plugin is under `briefs/`.
const PLUGIN_DIR: &str = "plugin/";

/// The path under `briefs/` of each of the plugin's files a layer may hold:
/// all but the hidden ones.
fn plugin_paths() -> impl Iterator<Item = String> {
    PLUGIN
        .iter()
        .filter(|(path, _)| !is_hidden(path))
        .map(|(path, _)| format!("{PLUGIN_DIR}{path}"))
}

/// The runs that read the file at `path` under `briefs/`, or `None` for a
/// file a layer may not hold: every run reads the plugin.
fn readers(path: &str) -> Option<Readers> {
    prompts::readers(path).or_else(|| plugin_paths().any(|p| p == path).then_some(Readers::Every))
}

impl BriefLayer {
    /// The layer holding `files`, each by its path under the layer's root,
    /// and those of them it cannot use: a file that a layer may not hold, or
    /// one that cannot be read as text. A file is skipped when it is hidden
    /// (docs/specs/operation.md, "Briefs").
    pub fn parse(
        layer: Layer,
        files: impl IntoIterator<Item = (String, Blob)>,
    ) -> (BriefLayer, Vec<Unusable>) {
        let mut parsed = BriefLayer {
            layer,
            files: BTreeMap::new(),
        };
        let mut unusable = Vec::new();
        for (path, blob) in files {
            if is_hidden(&path) {
                continue;
            }
            let (error, holds) = match readers(&path) {
                Some(readers) => match blob.text() {
                    Ok(text) => {
                        parsed.files.insert(path, text);
                        continue;
                    }
                    Err(error) => (error, readers),
                },
                None => (NOT_HELD, Readers::Every),
            };
            unusable.push(Unusable {
                path,
                layer,
                error: error.into(),
                holds,
            });
        }
        unusable.sort_by(|a, b| a.path.cmp(&b.path));
        (parsed, unusable)
    }
}

const NOT_HELD: &str = "not a file a layer may hold";

/// Whether any part of `path` names a hidden entry: one whose name starts
/// with `.`.
pub(crate) fn is_hidden(path: &str) -> bool {
    path.split('/').any(|name| name.starts_with('.'))
}

impl Briefs {
    pub fn shipped() -> Briefs {
        Briefs::resolve(&OwnTemplates::shipped(), &[]).briefs
    }

    /// Takes each file from the first of `layers` that has it and whose file
    /// can be used, or else from Linewright's own layer, `own`, whose files
    /// report [`Layer::Linewright`] whatever their text.
    pub fn resolve(own: &OwnTemplates, layers: &[&BriefLayer]) -> Resolved {
        let layers_having = |path: &str| -> Vec<&BriefLayer> {
            layers
                .iter()
                .filter(|layer| layer.files.contains_key(path))
                .copied()
                .collect()
        };
        let mut candidates: BTreeMap<&str, Vec<&BriefLayer>> = Templates::paths()
            .map(|path| (path, layers_having(path)))
            .collect();
        let mut unusable = Vec::new();
        let unusable_file = |layer: &BriefLayer, path: &str, error: String| Unusable {
            path: path.to_string(),
            layer: layer.layer,
            error,
            holds: readers(path).expect("only a file a layer may hold is resolved"),
        };
        let templates = loop {
            let (templates, failed) = Templates::read(own, &mut |path| {
                let layer = candidates.get(path)?.first()?;
                layer.files.get(path).map(String::as_str)
            });
            if failed.is_empty() {
                break templates;
            }
            for (path, error) in failed {
                let having = candidates
                    .get_mut(path)
                    .expect("only a path with a layer's file fails");
                let layer = having.remove(0);
                unusable.push(unusable_file(layer, path, format!("{error:#}")));
            }
        };
        let mut layers_of: BTreeMap<String, Layer> = candidates
            .into_iter()
            .map(|(path, having)| {
                let layer = having
                    .first()
                    .map_or(Layer::Linewright, |layer| layer.layer);
                (path.to_string(), layer)
            })
            .collect();
        let mut plugin = shipped_plugin();
        for path in plugin_paths() {
            let mut from = Layer::Linewright;
            for layer in layers_having(&path) {
                let text = &layer.files[&path];
                match prompts::without_placeholders(text) {
                    Ok(()) => {
                        let under = path.strip_prefix(PLUGIN_DIR).expect("a plugin path");
                        plugin.insert(under.to_string(), text.clone());
                        from = layer.layer;
                        break;
                    }
                    Err(error) => unusable.push(unusable_file(layer, &path, format!("{error:#}"))),
                }
            }
            layers_of.insert(path, from);
        }
        unusable.sort_by(|a, b| a.path.cmp(&b.path));
        Resolved {
            briefs: Briefs {
                templates,
                plugin,
                layers: layers_of,
            },
            unusable,
        }
    }

    /// The layer each file a layer may hold came from, by its path under
    /// `briefs/`.
    pub fn layers(&self) -> &BTreeMap<String, Layer> {
        &self.layers
    }

    /// Writes the plugin's files to `dir`.
    pub fn write_plugin(&self, dir: &Path) -> std::io::Result<()> {
        for (path, text) in &self.plugin {
            let path = dir.join(path);
            std::fs::create_dir_all(path.parent().expect("plugin files sit in a directory"))?;
            std::fs::write(path, text)?;
        }
        Ok(())
    }
}

impl OperatorBriefs {
    /// The operator's layer, from the `briefs` directory, laid out as
    /// Linewright's own `briefs/`, if one is set. Every file it holds,
    /// hidden ones aside, must be one a layer may hold and usable; the error
    /// names each that is not.
    pub fn read(dir: Option<&Path>) -> Result<OperatorBriefs> {
        OperatorBriefs::read_over(OwnTemplates::shipped(), dir)
    }

    /// As [`OperatorBriefs::read`], over `own` in place of Linewright's
    /// shipped text.
    pub fn read_over(own: OwnTemplates, dir: Option<&Path>) -> Result<OperatorBriefs> {
        let mut files = Vec::new();
        if let Some(dir) = dir {
            for path in files_under(dir, "briefs directory")? {
                let file = dir.join(&path);
                let bytes = std::fs::read(&file)
                    .with_context(|| format!("reading the brief file {}", file.display()))?;
                files.push((path, Blob::from_bytes(bytes)));
            }
        }
        let (layer, mut unusable) = BriefLayer::parse(Layer::Operator, files);
        let resolved = Briefs::resolve(&own, &[&layer]);
        unusable.extend(resolved.unusable);
        if let Some(dir) = dir
            && !unusable.is_empty()
        {
            unusable.sort_by(|a, b| a.path.cmp(&b.path));
            let named: Vec<String> = unusable
                .iter()
                .map(|unusable| format!("- {}: {}", unusable.path, unusable.error))
                .collect();
            return Err(anyhow!(
                "the `briefs` directory {} holds files Linewright cannot use:\n{}",
                dir.display(),
                named.join("\n")
            ));
        }
        Ok(OperatorBriefs {
            own,
            layer,
            briefs: Arc::new(resolved.briefs),
        })
    }

    pub fn layer(&self) -> &BriefLayer {
        &self.layer
    }

    /// The briefs of `above`, highest first, over the operator's layer and
    /// Linewright's own.
    pub fn with_layers(&self, above: &[&BriefLayer]) -> Resolved {
        let mut layers = above.to_vec();
        layers.push(&self.layer);
        Briefs::resolve(&self.own, &layers)
    }

    /// The briefs of a run with no layer above the operator's.
    pub fn briefs(&self) -> Arc<Briefs> {
        self.briefs.clone()
    }
}

/// The path under `dir` of every file in it and its subdirectories, in
/// order, with `/` between the parts, leaving out hidden entries and what
/// is under them. An error names the directory it cannot read as the
/// `kind`, such as `briefs directory`.
pub(crate) fn files_under(dir: &Path, kind: &str) -> Result<Vec<String>> {
    let mut files = Vec::new();
    let mut dirs = vec![(dir.to_path_buf(), String::new())];
    while let Some((at, prefix)) = dirs.pop() {
        let entries = std::fs::read_dir(&at)
            .with_context(|| format!("reading the {kind} {}", at.display()))?;
        for entry in entries {
            let entry = entry.with_context(|| format!("reading the {kind} {}", at.display()))?;
            let name = entry.file_name().to_string_lossy().into_owned();
            if is_hidden(&name) {
                continue;
            }
            let path = format!("{prefix}{name}");
            if entry.path().is_dir() {
                dirs.push((entry.path(), format!("{path}/")));
            } else {
                files.push(path);
            }
        }
    }
    files.sort();
    Ok(files)
}
