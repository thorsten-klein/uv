use std::fmt::Write;
use std::path::Path;

use anyhow::Result;
use owo_colors::OwoColorize;

use uv_cache::Cache;
use uv_workspace::{DiscoveryOptions, Workspace, WorkspaceCache};

use crate::commands::ExitStatus;
use crate::commands::patch::state::{read_applied, venv_root};
use crate::printer::Printer;

/// Show the patches that were applied the last time `uv patch apply` was run in the workspace.
pub(crate) async fn patch_show(
    project_dir: &Path,
    cache: &Cache,
    workspace_cache: &WorkspaceCache,
    printer: Printer,
) -> Result<ExitStatus> {
    let workspace = Workspace::discover(
        project_dir,
        &DiscoveryOptions::default(),
        cache,
        workspace_cache,
    )
    .await?;

    let mut applied = read_applied(&venv_root(&workspace, None))?;

    if applied.is_empty() {
        writeln!(printer.stderr(), "No patches have been applied")?;
        return Ok(ExitStatus::Success);
    }

    applied.sort_by(|a, b| (&a.package, &a.path).cmp(&(&b.package, &b.path)));

    for entry in &applied {
        writeln!(
            printer.stdout(),
            "{} {} {}",
            entry.package.cyan(),
            entry.path,
            format!("(applied {})", entry.applied_at).dimmed()
        )?;
    }

    Ok(ExitStatus::Success)
}
