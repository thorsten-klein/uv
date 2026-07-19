use std::sync::LazyLock;

use jiff::civil::Date;
use regex::Regex;
use serde::Deserialize;
use thiserror::Error;

use uv_normalize::PackageName;

static SHA256_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[0-9a-f]{64}$").unwrap());
static COMMIT_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[0-9a-f]{40}").unwrap());
static URL_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^https?://").unwrap());

/// A patch manifest, in JSON.
///
/// Mirrors the fields of the YAML schema used by `west patch` (see
/// `scripts/schemas/patch-schema.yml` in the Zephyr project), with `module` renamed to `package`
/// to match uv's terminology: each patch is applied against a member of the current
/// [`uv_workspace::Workspace`] rather than a west module.
#[derive(Debug, Default, Deserialize)]
pub(crate) struct PatchManifest {
    #[serde(default)]
    pub(crate) patches: Vec<PatchEntry>,
}

/// A single patch entry in a patch manifest.
///
/// Every field recognized by the west `patches.yml` schema is validated, matching the same
/// semantics as `pykwalify`, even though `uv patch apply` only acts on a subset of them.
#[derive(Debug, Clone, Deserialize)]
#[serde(try_from = "RawPatchEntry")]
pub(crate) struct PatchEntry {
    /// The path to the patch file, relative to the directory containing the manifest.
    pub(crate) path: String,
    /// The SHA-256 checksum of the patch file.
    pub(crate) sha256sum: String,
    /// The workspace package the patch is applied to.
    pub(crate) package: PackageName,
    /// The command used to apply the patch.
    pub(crate) apply_command: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "kebab-case")]
struct RawPatchEntry {
    path: String,
    sha256sum: String,
    package: PackageName,
    #[expect(dead_code)]
    author: String,
    email: String,
    date: String,
    #[serde(default = "default_true")]
    #[expect(dead_code)]
    upstreamable: bool,
    merge_pr: Option<String>,
    issue: Option<String>,
    #[expect(dead_code)]
    merge_status: Option<bool>,
    merge_commit: Option<String>,
    merge_date: Option<String>,
    #[serde(default = "default_apply_command")]
    apply_command: String,
    #[expect(dead_code)]
    comments: Option<String>,
    #[expect(dead_code)]
    custom: Option<serde_json::Value>,
}

fn default_true() -> bool {
    true
}

fn default_apply_command() -> String {
    "git apply".to_string()
}

/// An error that can occur when validating a [`RawPatchEntry`].
#[derive(Debug, Error)]
pub(crate) enum PatchEntryError {
    #[error("`sha256sum` must be a 64-character lowercase hex string, got `{0}`")]
    Sha256(String),
    #[error("`email` must contain an `@`, got `{0}`")]
    Email(String),
    #[error("`date` must be an ISO 8601 date (`YYYY-MM-DD`)")]
    Date(#[source] jiff::Error),
    #[error("`merge-date` must be an ISO 8601 date (`YYYY-MM-DD`)")]
    MergeDate(#[source] jiff::Error),
    #[error("`merge-pr` must be an `http://` or `https://` URL, got `{0}`")]
    MergePr(String),
    #[error("`issue` must be an `http://` or `https://` URL, got `{0}`")]
    Issue(String),
    #[error("`merge-commit` must start with a 40-character lowercase hex string, got `{0}`")]
    MergeCommit(String),
}

impl TryFrom<RawPatchEntry> for PatchEntry {
    type Error = PatchEntryError;

    fn try_from(raw: RawPatchEntry) -> Result<Self, Self::Error> {
        if !SHA256_RE.is_match(&raw.sha256sum) {
            return Err(PatchEntryError::Sha256(raw.sha256sum));
        }
        if let Some((prefix, suffix)) = raw.email.split_once('@') {
            if prefix.is_empty() || suffix.is_empty() {
                return Err(PatchEntryError::Email(raw.email));
            }
        } else {
            return Err(PatchEntryError::Email(raw.email));
        }
        raw.date.parse::<Date>().map_err(PatchEntryError::Date)?;
        if let Some(merge_date) = &raw.merge_date {
            merge_date
                .parse::<Date>()
                .map_err(PatchEntryError::MergeDate)?;
        }
        if let Some(merge_pr) = &raw.merge_pr
            && !URL_RE.is_match(merge_pr)
        {
            return Err(PatchEntryError::MergePr(merge_pr.clone()));
        }
        if let Some(issue) = &raw.issue
            && !URL_RE.is_match(issue)
        {
            return Err(PatchEntryError::Issue(issue.clone()));
        }
        if let Some(merge_commit) = &raw.merge_commit
            && !COMMIT_RE.is_match(merge_commit)
        {
            return Err(PatchEntryError::MergeCommit(merge_commit.clone()));
        }

        Ok(Self {
            path: raw.path,
            sha256sum: raw.sha256sum,
            package: raw.package,
            apply_command: raw.apply_command,
        })
    }
}
