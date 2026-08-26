use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use jiff::Timestamp;
use rustc_hash::FxHashSet;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use uv_distribution_types::{NameRequirementSpecification, Requirement, RequirementSource};
use uv_fs::{LockedFile, LockedFileError, LockedFileMode};
use uv_normalize::PackageName;
use uv_pep440::VersionSpecifiers;
use uv_pep508::MarkerTree;

/// File name for saved requirements, stored next to `pyvenv.cfg`. Used by `--amend`.
pub const REQUIREMENTS_HISTORY_FILENAME: &str = "uv-requirements-history.toml";

/// Lock file name for [`REQUIREMENTS_HISTORY_FILENAME`].
const LOCK_FILENAME: &str = ".uv-requirements-history.lock";

/// Requirements saved from past `uv pip install` runs for one virtual environment.
///
/// Saved on every install, with or without `--amend`. Only checked when `--amend` is used.
/// We only save version-range requirements (not URL/path/Git ones).
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct RequirementsHistory {
    /// One saved requirement per package name. A new install overwrites the old one.
    #[serde(default)]
    requirements: BTreeMap<PackageName, HistoryEntry>,
    /// Log of past installs. Not used for checks, just a history log.
    #[serde(default, rename = "invocation")]
    invocations: Vec<InvocationRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct HistoryEntry {
    /// The version range asked for, e.g. `">=1.0,<=2"`.
    specifier: String,
    added_at: Timestamp,
    added_by_invocation: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct InvocationRecord {
    id: u64,
    timestamp: Timestamp,
    args: Vec<String>,
}

#[derive(Debug, Error)]
pub enum RequirementsHistoryError {
    #[error(transparent)]
    Lock(#[from] LockedFileError),

    #[error("Failed to read requirements history at `{}`", path.display())]
    Io {
        path: PathBuf,
        #[source]
        err: std::io::Error,
    },

    #[error("Failed to parse requirements history at `{}`", path.display())]
    Toml {
        path: PathBuf,
        #[source]
        err: Box<toml::de::Error>,
    },

    #[error("Failed to serialize requirements history")]
    TomlWrite(#[from] toml::ser::Error),

    #[error("Failed to parse remembered requirement `{name}{specifier}` from requirements history")]
    Specifier {
        name: PackageName,
        specifier: String,
        #[source]
        err: uv_pep440::VersionSpecifiersParseError,
    },
}

impl RequirementsHistory {
    /// Path to the history file for a venv.
    pub fn path(venv_root: &Path) -> PathBuf {
        venv_root.join(REQUIREMENTS_HISTORY_FILENAME)
    }

    /// Lock the history file for a venv.
    ///
    /// Use [`LockedFileMode::Shared`] for a plain [`Self::read`], and
    /// [`LockedFileMode::Exclusive`] when you will read then write (from [`Self::read`] through
    /// [`Self::write`]).
    pub async fn acquire_lock(
        venv_root: &Path,
        mode: LockedFileMode,
    ) -> Result<LockedFile, RequirementsHistoryError> {
        Ok(LockedFile::acquire(
            venv_root.join(LOCK_FILENAME),
            mode,
            venv_root.display().to_string(),
        )
        .await?)
    }

    /// Read the history file for a venv.
    ///
    /// Returns an empty history if the file doesn't exist yet.
    pub fn read(venv_root: &Path) -> Result<Self, RequirementsHistoryError> {
        let path = Self::path(venv_root);
        let contents = match fs_err::read_to_string(&path) {
            Ok(contents) => contents,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(err) => return Err(RequirementsHistoryError::Io { path, err }),
        };
        toml::from_str(&contents).map_err(|err| RequirementsHistoryError::Toml {
            path,
            err: Box::new(err),
        })
    }

    /// Write the history file for a venv.
    pub fn write(&self, venv_root: &Path) -> Result<(), RequirementsHistoryError> {
        let path = Self::path(venv_root);
        let contents = toml::to_string_pretty(self)?;
        fs_err::write(&path, contents).map_err(|err| RequirementsHistoryError::Io { path, err })
    }

    /// Turn saved requirements into constraints, skipping any package in `skip` (packages the
    /// user chose to change via `--upgrade` or `--upgrade-package`).
    pub fn active_constraints(
        &self,
        skip: &FxHashSet<PackageName>,
    ) -> Result<Vec<NameRequirementSpecification>, RequirementsHistoryError> {
        self.requirements
            .iter()
            .filter(|(name, _)| !skip.contains(*name))
            .map(|(name, entry)| {
                let specifier: VersionSpecifiers =
                    entry
                        .specifier
                        .parse()
                        .map_err(|err| RequirementsHistoryError::Specifier {
                            name: name.clone(),
                            specifier: entry.specifier.clone(),
                            err,
                        })?;
                Ok(NameRequirementSpecification::from(Requirement {
                    name: name.clone(),
                    extras: Box::new([]),
                    groups: Box::new([]),
                    marker: MarkerTree::TRUE,
                    source: RequirementSource::Registry {
                        specifier,
                        index: None,
                        conflict: None,
                    },
                    origin: None,
                }))
            })
            .collect()
    }

    /// Save each package's requirement (replacing any old one for that package), and add a log
    /// row for this install. Packages not in `requirements` are left alone.
    pub fn record(&mut self, requirements: &[Requirement], args: Vec<String>) {
        let now = Timestamp::now();
        let invocation_id = self.invocations.len() as u64 + 1;

        for requirement in requirements {
            let RequirementSource::Registry { specifier, .. } = &requirement.source else {
                continue;
            };
            self.requirements.insert(
                requirement.name.clone(),
                HistoryEntry {
                    specifier: specifier.to_string(),
                    added_at: now,
                    added_by_invocation: invocation_id,
                },
            );
        }

        self.invocations.push(InvocationRecord {
            id: invocation_id,
            timestamp: now,
            args,
        });
    }
}
