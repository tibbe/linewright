//! Linewright's files in the repository: its settings for the work and its
//! prompts, read from the default branch as a run starts
//! (`docs/specs/repository.md`, Linewright's files).

use std::sync::Arc;

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::briefs::{Briefs, OperatorPrompts, PromptLayer};
use crate::layer::Layer;
use crate::prompts::{Prompts, Readers};
use crate::store::{Files, Store, Tree};
use crate::work_settings::{RepoSettings, RunKind};

/// The repository's file (`docs/specs/operation.md`, The repository's file).
pub const CONFIG: &str = ".linewright/config.toml";

/// The repository's layer of the prompts, laid out as `briefs/prompts/` is.
pub const PROMPTS: &str = ".linewright/prompts";

/// A file of the repository's Linewright cannot use, and why.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileError {
    /// Its path in the repository.
    pub path: String,
    pub error: String,
}

/// A file of the repository's Linewright cannot use, and the runs it holds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Unusable {
    pub file: FileError,
    pub holds: Readers,
}

/// The files of `unusable` that hold the runs of `run`.
pub fn holding<'a>(unusable: &'a [Unusable], run: &RunKind) -> Vec<&'a FileError> {
    unusable
        .iter()
        .filter(|unusable| unusable.holds.read_by(run))
        .map(|unusable| &unusable.file)
        .collect()
}

/// What the repository's files give the runs that start from one read of
/// them.
#[derive(Clone, Debug)]
pub struct RepoLayer {
    pub settings: RepoSettings,
    /// The briefs, each prompt from the repository's layer, the operator's
    /// or Linewright's own.
    pub briefs: Arc<Briefs>,
    pub unusable: Vec<Unusable>,
}

impl RepoLayer {
    /// The repository's layer as it is now on `branch`, over the operator's.
    pub async fn read(
        store: &dyn Store,
        branch: &str,
        operator: &OperatorPrompts,
    ) -> Result<RepoLayer> {
        let tree = Tree {
            dir: PROMPTS,
            depth: prompts_depth(),
        };
        let files = store
            .read_files(branch, &[CONFIG.to_string()], tree)
            .await?;
        Ok(RepoLayer::parse(files, operator))
    }

    /// The layer the repository's `files` make over the operator's prompts,
    /// its config among `files.files` and its prompts in `files.entries`. A
    /// file that cannot be used holds the runs that read it, and the next
    /// layer's stands in for every other run; one that is not one of
    /// Linewright's prompts holds every run.
    pub fn parse(mut files: Files, operator: &OperatorPrompts) -> RepoLayer {
        let mut unusable = Vec::new();
        let mut settings = RepoSettings::default();
        if let Some(config) = files.files.remove(CONFIG) {
            let parsed = config
                .text()
                .map_err(str::to_string)
                .and_then(|text| RepoSettings::parse(&text).map_err(|error| format!("{error:#}")));
            match parsed {
                Ok(parsed) => settings = parsed,
                Err(error) => unusable.push(Unusable {
                    file: FileError {
                        path: CONFIG.to_string(),
                        error,
                    },
                    holds: Readers::Every,
                }),
            }
        }
        let (layer, parsed) = PromptLayer::parse(Layer::Repository, files.entries);
        let briefs = if layer.files.is_empty() && parsed.is_empty() {
            operator.briefs()
        } else {
            let resolved = Briefs::resolve(&[&layer, operator.layer()]);
            unusable.extend(
                parsed
                    .into_iter()
                    .chain(resolved.unusable)
                    .map(|file| Unusable {
                        holds: file.holds,
                        file: FileError {
                            path: format!("{PROMPTS}/{}", file.path),
                            error: file.error,
                        },
                    }),
            );
            Arc::new(resolved.briefs)
        };
        unusable.sort_by(|a, b| a.file.path.cmp(&b.file.path));
        RepoLayer {
            settings,
            briefs,
            unusable,
        }
    }
}

/// How many levels down Linewright's prompts go: anything deeper is not one
/// of them by its path alone.
fn prompts_depth() -> usize {
    Prompts::paths()
        .into_iter()
        .map(|path| path.split('/').count())
        .max()
        .expect("Linewright ships prompts")
}
