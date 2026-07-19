use std::fmt::Write;
use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::{Context, Result, bail};
use owo_colors::OwoColorize;
use sha2::{Digest, Sha256};
use tokio::process::Command;

use uv_cache::Cache;
use uv_fs::Simplified;
use uv_warnings::warn_user;
use uv_workspace::{DiscoveryOptions, Workspace, WorkspaceCache};

use crate::commands::patch::schema::PatchManifest;
use crate::commands::patch::state::{AppliedPatch, read_applied, venv_root, write_applied};
use crate::commands::{ExitStatus, elapsed};
use crate::printer::Printer;

/// Apply the patches described in a JSON patch manifest to the current workspace.
pub(crate) async fn patch_apply(
    file: PathBuf,
    project_dir: &Path,
    cache: &Cache,
    workspace_cache: &WorkspaceCache,
    printer: Printer,
) -> Result<ExitStatus> {
    let start = Instant::now();

    let contents = fs_err::read_to_string(&file)
        .with_context(|| format!("Failed to read patch manifest `{}`", file.user_display()))?;
    let manifest: PatchManifest = serde_json::from_str(&contents)
        .with_context(|| format!("Failed to parse patch manifest `{}`", file.user_display()))?;

    if manifest.patches.is_empty() {
        writeln!(printer.stderr(), "No patches to apply")?;
        return Ok(ExitStatus::Success);
    }

    // Patch file paths are resolved relative to the directory containing the manifest, matching
    // `west patch`'s default `patch-base` of the manifest's own directory.
    let patch_base = file.parent().unwrap_or_else(|| Path::new(""));

    let workspace = Workspace::discover(
        project_dir,
        &DiscoveryOptions::default(),
        cache,
        workspace_cache,
    )
    .await?;

    let venv_root = venv_root(&workspace, None);

    // The full set of patches previously recorded as applied in this workspace's virtual
    // environment, across every manifest that has ever been applied to it. Patches for other
    // packages or manifests must be preserved as-is.
    let mut state = read_applied(&venv_root)?;

    let now = jiff::Timestamp::now();
    let mut applied_count = 0usize;
    let mut skipped_count = 0usize;

    for entry in &manifest.patches {
        let existing_index = state
            .iter()
            .position(|previous| previous.package == entry.package && previous.path == entry.path);

        if let Some(index) = existing_index
            && state[index].sha256sum == entry.sha256sum
        {
            warn_user!(
                "Patch `{}` was already applied to `{}`, skipping",
                entry.path,
                entry.package
            );
            skipped_count += 1;
            continue;
        }

        let Some(member) = workspace.packages().get(&entry.package) else {
            bail!(
                "Package `{}` not found in workspace (from `{}`)",
                entry.package,
                entry.path
            );
        };

        let patch_path = patch_base.join(&entry.path);
        let patch_contents = fs_err::read_to_string(&patch_path).with_context(|| {
            format!("Failed to read patch file `{}`", patch_path.user_display())
        })?;

        let actual_sha256 = sha256_normalized(&patch_contents);
        if actual_sha256 != entry.sha256sum {
            bail!(
                "SHA-256 mismatch for `{}`:\n  expected: {}\n  actual:   {}",
                patch_path.user_display(),
                entry.sha256sum,
                actual_sha256,
            );
        }

        // Resolve to an absolute path, since the apply command runs with the package root as its
        // working directory, which may differ from the manifest's directory. The absolute path is
        // also persisted, so `uv patch reset` can revert the patch without needing the manifest.
        let patch_path = fs_err::canonicalize(&patch_path).with_context(|| {
            format!(
                "Failed to resolve patch file `{}`",
                patch_path.user_display()
            )
        })?;

        let mut args = split_command(&entry.apply_command).map_err(|reason| {
            anyhow::anyhow!(
                "Failed to parse `apply-command` `{}`: {reason}",
                entry.apply_command
            )
        })?;
        if args.is_empty() {
            bail!("`apply-command` must not be empty");
        }
        let program = args.remove(0);

        writeln!(
            printer.stderr(),
            "{}",
            format!("Applying `{}` to `{}`", entry.path, entry.package).dimmed()
        )?;

        let output = Command::new(&program)
            .args(args)
            .arg(&patch_path)
            .current_dir(member.root())
            .output()
            .await
            .with_context(|| format!("Failed to run `{}`", entry.apply_command))?;

        if !output.status.success() {
            bail!(
                "Failed to apply `{}` to `{}`:\n{}",
                entry.path,
                entry.package,
                String::from_utf8_lossy(&output.stderr).trim_end(),
            );
        }

        let record = AppliedPatch {
            path: entry.path.clone(),
            patch_file: patch_path,
            sha256sum: entry.sha256sum.clone(),
            package: entry.package.clone(),
            apply_command: entry.apply_command.clone(),
            applied_at: now,
        };
        if let Some(index) = existing_index {
            state[index] = record;
        } else {
            state.push(record);
        }
        applied_count += 1;

        // Persist progress after every successful apply, so a later failure in this run doesn't
        // lose track of what has already been applied to disk.
        write_applied(&venv_root, &state)?;
    }

    if applied_count == 0 {
        writeln!(
            printer.stderr(),
            "{}",
            "All patches were already applied".dimmed()
        )?;
    } else {
        let s = if applied_count == 1 { "" } else { "es" };
        let skipped_suffix = if skipped_count == 0 {
            String::new()
        } else {
            let s = if skipped_count == 1 { "" } else { "es" };
            format!(", skipped {skipped_count} already-applied patch{s}")
        };
        writeln!(
            printer.stderr(),
            "{}",
            format!(
                "Applied {} {}{skipped_suffix}",
                format!("{applied_count} patch{s}").bold(),
                format!("in {}", elapsed(start.elapsed())).dimmed()
            )
            .dimmed()
        )?;
    }

    Ok(ExitStatus::Success)
}

/// Compute the SHA-256 checksum of a patch file's contents, normalizing line endings to `\n` to
/// match `west patch`'s checksum algorithm.
fn sha256_normalized(contents: &str) -> String {
    let normalized = contents.replace("\r\n", "\n").replace('\r', "\n");
    let mut hasher = Sha256::new();
    hasher.update(normalized.as_bytes());
    format!("{:x}", hasher.finalize())
}

/// Split a command string into arguments, similar to a shell's word-splitting.
///
/// Arguments are separated by whitespace. A substring wrapped in single or double quotes is
/// treated as a single argument, allowing embedded whitespace; within double quotes (but not
/// single quotes), a backslash escapes a following `"` or `\`. Outside of quotes, a backslash
/// escapes the following character.
pub(super) fn split_command(command: &str) -> std::result::Result<Vec<String>, &'static str> {
    let mut words = Vec::new();
    let mut word = String::new();
    let mut in_word = false;
    let mut chars = command.chars();

    while let Some(c) = chars.next() {
        match c {
            c if c.is_whitespace() => {
                if in_word {
                    words.push(std::mem::take(&mut word));
                    in_word = false;
                }
            }
            '\'' => {
                in_word = true;
                loop {
                    match chars.next() {
                        Some('\'') => break,
                        Some(c) => word.push(c),
                        None => return Err("unterminated `'` quote"),
                    }
                }
            }
            '"' => {
                in_word = true;
                loop {
                    match chars.next() {
                        Some('"') => break,
                        Some('\\') => match chars.next() {
                            Some(c @ ('"' | '\\')) => word.push(c),
                            Some(c) => {
                                word.push('\\');
                                word.push(c);
                            }
                            None => return Err("unterminated `\"` quote"),
                        },
                        Some(c) => word.push(c),
                        None => return Err("unterminated `\"` quote"),
                    }
                }
            }
            '\\' => {
                in_word = true;
                match chars.next() {
                    Some(c) => word.push(c),
                    None => return Err("trailing `\\`"),
                }
            }
            c => {
                in_word = true;
                word.push(c);
            }
        }
    }

    if in_word {
        words.push(word);
    }

    Ok(words)
}
