use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, Result, anyhow};

use crate::layer::Layer;
use crate::prompts::{self, Prompts, Readers};
use crate::store::{Blob, Entry};

/// The prompts a session is sent, each from the first layer that has it
/// (docs/specs/operation.md, "Prompts"). Its skills are always Linewright's
/// own, which [`Briefs::write_plugin`] writes.
#[derive(Clone, Debug)]
pub struct Briefs {
    pub prompts: Prompts,
    /// The layer of each prompt, by its path under the prompts' root.
    layers: BTreeMap<String, Layer>,
}

/// One layer of the prompts: each of Linewright's prompts it has, by its
/// path under the prompts' root, such as `parts/facts.md`. Built only by
/// [`PromptLayer::parse`], which looks at every file the layer holds.
#[derive(Clone, Debug)]
pub struct PromptLayer {
    pub layer: Layer,
    pub files: BTreeMap<String, String>,
}

/// A layer's file Linewright cannot use, and why.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Unusable {
    pub path: String,
    pub layer: Layer,
    pub error: String,
    /// The runs that would read it: every run for a file that is not one of
    /// Linewright's prompts, as no stage is known to read it.
    pub holds: Readers,
}

/// The briefs resolved from layers, and the prompts in them that could not
/// be used, for each of which the next layer's file stands in.
#[derive(Clone, Debug)]
pub struct Resolved {
    pub briefs: Briefs,
    pub unusable: Vec<Unusable>,
}

/// The operator's layer of the prompts, each of whose files can be used,
/// and the briefs it makes over Linewright's.
#[derive(Clone, Debug)]
pub struct OperatorPrompts {
    layer: PromptLayer,
    briefs: Arc<Briefs>,
}

/// The skills every session is given, as a Claude Code plugin.
const PLUGIN: [(&str, &str); 12] = [
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

impl PromptLayer {
    /// The layer holding `files`, each by its path under the layer's root,
    /// and those of them it cannot use: an entry that is not one of
    /// Linewright's prompts, or a prompt that cannot be read as text. An
    /// entry is skipped when its name, or that of a directory it is in,
    /// starts with `.` (docs/specs/operation.md, "Prompts").
    pub fn parse(
        layer: Layer,
        files: impl IntoIterator<Item = (String, Entry)>,
    ) -> (PromptLayer, Vec<Unusable>) {
        let mut parsed = PromptLayer {
            layer,
            files: BTreeMap::new(),
        };
        let mut unusable = Vec::new();
        for (path, entry) in files {
            if is_hidden(&path) {
                continue;
            }
            let (error, holds) = match (prompts::readers(&path), entry) {
                (Some(readers), Entry::File(blob)) => match blob.text() {
                    Ok(text) => {
                        parsed.files.insert(path, text);
                        continue;
                    }
                    Err(error) => (error, readers),
                },
                _ => (NOT_A_PROMPT, Readers::Every),
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

const NOT_A_PROMPT: &str = "not one of Linewright's prompts";

/// Whether any part of `path` names a hidden entry: one whose name starts
/// with `.`.
fn is_hidden(path: &str) -> bool {
    path.split('/').any(|name| name.starts_with('.'))
}

impl Briefs {
    pub fn shipped() -> Briefs {
        Briefs::resolve(&[]).briefs
    }

    /// Takes each prompt from the first of `layers` that has it and whose
    /// file can be used, or else from the prompts Linewright ships.
    pub fn resolve(layers: &[&PromptLayer]) -> Resolved {
        // Each path's layers that have it, the one in use first.
        let mut candidates: BTreeMap<&str, Vec<&PromptLayer>> = Prompts::paths()
            .into_iter()
            .map(|path| {
                let having = layers
                    .iter()
                    .filter(|layer| layer.files.contains_key(path))
                    .copied()
                    .collect();
                (path, having)
            })
            .collect();
        let mut unusable = Vec::new();
        // A file that cannot be used drops out, and the prompts are read
        // again with the next layer's in its place.
        let prompts = loop {
            let (prompts, failed) = Prompts::read(&mut |path| {
                let layer = candidates.get(path)?.first()?;
                layer.files.get(path).map(String::as_str)
            });
            if failed.is_empty() {
                break prompts;
            }
            for (path, error) in failed {
                let having = candidates
                    .get_mut(path)
                    .expect("only a path with a layer's file fails");
                let layer = having.remove(0);
                unusable.push(Unusable {
                    path: path.to_string(),
                    layer: layer.layer,
                    error: format!("{error:#}"),
                    holds: prompts::readers(path)
                        .expect("only one of Linewright's prompts is read"),
                });
            }
        };
        let layers_of = candidates
            .into_iter()
            .map(|(path, having)| {
                let layer = having
                    .first()
                    .map_or(Layer::Linewright, |layer| layer.layer);
                (path.to_string(), layer)
            })
            .collect();
        Resolved {
            briefs: Briefs {
                prompts,
                layers: layers_of,
            },
            unusable,
        }
    }

    /// The layer each prompt came from, by its path under the prompts' root.
    pub fn layers(&self) -> &BTreeMap<String, Layer> {
        &self.layers
    }

    /// Writes Linewright's skills, the same for every session, to `dir`.
    pub fn write_plugin(dir: &Path) -> std::io::Result<()> {
        for (path, text) in PLUGIN {
            let path = dir.join(path);
            std::fs::create_dir_all(path.parent().expect("plugin files sit in a directory"))?;
            std::fs::write(path, text)?;
        }
        Ok(())
    }
}

impl OperatorPrompts {
    /// The operator's layer, from the `prompts` directory, laid out as
    /// Linewright's own prompts are, if one is set. Every file it holds,
    /// hidden ones aside, must be one of Linewright's prompts and usable;
    /// the error names each that is not.
    pub fn read(dir: Option<&Path>) -> Result<OperatorPrompts> {
        let mut files = Vec::new();
        if let Some(dir) = dir {
            for path in files_under(dir)? {
                let file = dir.join(&path);
                let bytes = std::fs::read(&file)
                    .with_context(|| format!("reading the prompt {}", file.display()))?;
                files.push((path, Entry::File(Blob::from_bytes(bytes))));
            }
        }
        let (layer, mut unusable) = PromptLayer::parse(Layer::Operator, files);
        let resolved = Briefs::resolve(&[&layer]);
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
                "the `prompts` directory {} holds files Linewright cannot use:\n{}",
                dir.display(),
                named.join("\n")
            ));
        }
        Ok(OperatorPrompts {
            layer,
            briefs: Arc::new(resolved.briefs),
        })
    }

    pub fn layer(&self) -> &PromptLayer {
        &self.layer
    }

    /// The briefs of a run with no layer above the operator's.
    pub fn briefs(&self) -> Arc<Briefs> {
        self.briefs.clone()
    }
}

/// The path under `dir` of every file in it and its subdirectories, in
/// order, with `/` between the parts, leaving out hidden entries and what
/// is under them.
fn files_under(dir: &Path) -> Result<Vec<String>> {
    let mut files = Vec::new();
    let mut dirs = vec![(dir.to_path_buf(), String::new())];
    while let Some((at, prefix)) = dirs.pop() {
        let entries = std::fs::read_dir(&at)
            .with_context(|| format!("reading the prompts directory {}", at.display()))?;
        for entry in entries {
            let entry =
                entry.with_context(|| format!("reading the prompts directory {}", at.display()))?;
            let name = entry.file_name().to_string_lossy().into_owned();
            // Not descending into a hidden directory, such as `.git/`.
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
