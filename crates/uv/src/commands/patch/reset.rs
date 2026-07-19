use std::fmt::Write;
use std::path::Path;

use anyhow::{Context, Result, bail};
use owo_colors::OwoColorize;
use tokio::process::Command;

use uv_cache::Cache;
use uv_workspace::{DiscoveryOptions, Workspace, WorkspaceCache};

use crate::commands::ExitStatus;
use crate::commands::patch::apply::split_command;
use crate::commands::patch::state::{read_applied, venv_root, write_applied};
use crate::printer::Printer;

/// Revert every patch recorded as applied in the workspace's virtual environment, in reverse
/// order of application.
pub(crate) async fn patch_reset(
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

    let venv_root = venv_root(&workspace, None);
    let mut state = read_applied(&venv_root)?;

    if state.is_empty() {
        writeln!(printer.stderr(), "No patches have been applied")?;
        return Ok(ExitStatus::Success);
    }

    // Revert most-recently-applied first. Iterate over an independent, sorted copy: `state` is
    // mutated (and progress persisted) after each successful revert, so indices into it would
    // shift out from under a positional iteration order.
    let mut to_revert = state.clone();
    to_revert.sort_by_key(|entry| std::cmp::Reverse(entry.applied_at));

    let mut reverted_count = 0usize;

    for entry in to_revert {
        let path = entry.path;
        let patch_file = entry.patch_file;
        let package = entry.package;
        let apply_command = entry.apply_command;
        let applied_at = entry.applied_at;

        let Some(member) = workspace.packages().get(&package) else {
            bail!("Package `{package}` not found in workspace (from `{path}`)");
        };

        let mut args = split_command(&apply_command).map_err(|reason| {
            anyhow::anyhow!("Failed to parse `apply-command` `{apply_command}`: {reason}")
        })?;
        if args.is_empty() {
            bail!("`apply-command` must not be empty");
        }
        let program = args.remove(0);
        args.push("-R".to_string());

        writeln!(
            printer.stderr(),
            "{}",
            format!("Reverting `{path}` from `{package}`").dimmed()
        )?;

        let output = Command::new(&program)
            .args(args)
            .arg(&patch_file)
            .current_dir(member.root())
            .output()
            .await
            .with_context(|| format!("Failed to run `{apply_command}`"))?;

        if !output.status.success() {
            bail!(
                "Failed to revert `{path}` from `{package}`:\n{}",
                String::from_utf8_lossy(&output.stderr).trim_end(),
            );
        }

        reverted_count += 1;

        // Persist progress after every successful revert, so a later failure doesn't lose track
        // of what has already been undone.
        state.retain(|entry| {
            !(entry.package == package && entry.path == path && entry.applied_at == applied_at)
        });
        write_applied(&venv_root, &state)?;
    }

    let s = if reverted_count == 1 { "" } else { "es" };
    writeln!(
        printer.stderr(),
        "{}",
        format!("Reverted {reverted_count} patch{s}").bold()
    )?;

    Ok(ExitStatus::Success)
}
