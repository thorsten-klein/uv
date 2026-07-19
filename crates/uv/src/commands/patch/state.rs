use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use uv_fs::Simplified;
use uv_normalize::PackageName;
use uv_workspace::Workspace;

/// The name of the file, stored in the project's virtual environment, that records the patches
/// applied by `uv patch apply`.
const STATE_FILE_NAME: &str = "uv-patches.json";

/// A record of the patches applied to a workspace, persisted in its virtual environment.
#[derive(Debug, Default, Serialize, Deserialize)]
pub(crate) struct AppliedManifest {
    #[serde(default)]
    pub(crate) applied: Vec<AppliedPatch>,
}

/// A single patch that was successfully applied.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) struct AppliedPatch {
    /// The patch's path, as it appeared in the manifest (for display purposes).
    pub(crate) path: String,
    /// The absolute path to the patch file on disk, so `uv patch reset` can revert it later
    /// regardless of which manifest originally referenced it.
    pub(crate) patch_file: PathBuf,
    /// The SHA-256 checksum of the patch file at the time it was applied.
    pub(crate) sha256sum: String,
    /// The workspace package the patch was applied to.
    pub(crate) package: PackageName,
    /// The command used to apply the patch; reused (with `-R` appended) to revert it.
    pub(crate) apply_command: String,
    pub(crate) applied_at: jiff::Timestamp,
}

/// Return the path to the virtual environment used to store applied-patch state for a workspace.
///
/// Respects `UV_PROJECT_ENVIRONMENT`/`--active`, like the rest of uv's project commands, and
/// otherwise defaults to `<workspace-root>/.venv`.
pub(crate) fn venv_root(workspace: &Workspace, active: Option<bool>) -> PathBuf {
    workspace
        .environment_selection(active)
        .explicit_path()
        .map_or_else(|| workspace.install_path().join(".venv"), Path::to_path_buf)
}

/// The path to the applied-patch state file within a virtual environment.
fn state_path(venv_root: &Path) -> PathBuf {
    venv_root.join(STATE_FILE_NAME)
}

/// Read the applied-patch state recorded in a workspace's virtual environment.
///
/// Returns an empty list if no patches have ever been applied, including when the virtual
/// environment does not exist yet.
pub(crate) fn read_applied(venv_root: &Path) -> Result<Vec<AppliedPatch>> {
    let state_path = state_path(venv_root);
    if !state_path.is_file() {
        return Ok(Vec::new());
    }

    let contents = fs_err::read_to_string(&state_path).with_context(|| {
        format!(
            "Failed to read applied-patch state `{}`",
            state_path.user_display()
        )
    })?;
    let manifest: AppliedManifest = serde_json::from_str(&contents).with_context(|| {
        format!(
            "Failed to parse applied-patch state `{}`",
            state_path.user_display()
        )
    })?;

    Ok(manifest.applied)
}

/// Persist the applied-patch state to a workspace's virtual environment, creating the directory
/// if it does not already exist.
pub(crate) fn write_applied(venv_root: &Path, applied: &[AppliedPatch]) -> Result<()> {
    fs_err::create_dir_all(venv_root).with_context(|| {
        format!(
            "Failed to create virtual environment directory `{}`",
            venv_root.user_display()
        )
    })?;

    let state_path = state_path(venv_root);
    let contents = serde_json::to_string_pretty(&AppliedManifest {
        applied: applied.to_vec(),
    })
    .context("Failed to serialize applied-patch state")?;
    fs_err::write(&state_path, contents).with_context(|| {
        format!(
            "Failed to write applied-patch state to `{}`",
            state_path.user_display()
        )
    })
}
