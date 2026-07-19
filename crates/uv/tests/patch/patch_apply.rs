use anyhow::Result;
use assert_cmd::assert::OutputAssertExt;
use assert_fs::assert::PathAssert;
use assert_fs::fixture::{FileWriteStr, PathChild};
use predicates::prelude::predicate;

use uv_test::uv_snapshot;

const HELLO_TXT: &str = "line1\nline2\nline3\n";

const HELLO_PATCH: &str = "\
--- a/hello.txt
+++ b/hello.txt
@@ -1,3 +1,3 @@
 line1
-line2
+line2-changed
 line3
";

const HELLO_PATCH_SHA256: &str =
    "5dbcaac1600c4efda928296f53e803d0cd3d1e4aae205c486f0ac30bae3a1efa";

const WRONG_SHA256: &str = "0000000000000000000000000000000000000000000000000000000000000000";

fn write_manifest(workspace: &assert_fs::fixture::ChildPath, package: &str, sha256sum: &str) {
    workspace
        .child("patch.json")
        .write_str(&format!(
            r#"{{
  "patches": [
    {{
      "path": "hello.patch",
      "sha256sum": "{sha256sum}",
      "package": "{package}",
      "author": "Kermit D. Frog",
      "email": "itsnoteasy@being.gr",
      "date": "2020-04-20"
    }}
  ]
}}
"#
        ))
        .unwrap();
}

/// Apply a single patch to the workspace root package.
#[test]
fn patch_apply_simple() -> Result<()> {
    let context = uv_test::test_context!("3.12");
    context.init().arg("foo").assert().success();

    let workspace = context.temp_dir.child("foo");
    workspace.child("hello.txt").write_str(HELLO_TXT)?;
    workspace.child("hello.patch").write_str(HELLO_PATCH)?;
    write_manifest(&workspace, "foo", HELLO_PATCH_SHA256);

    uv_snapshot!(context.filters(), context.patch_apply().arg("-f").arg("patch.json").current_dir(&workspace), @"
    success: true
    exit_code: 0
    ----- stdout -----

    ----- stderr -----
    Applying `hello.patch` to `foo`
    Applied 1 patch in [TIME]
    "
    );

    workspace
        .child("hello.txt")
        .assert("line1\nline2-changed\nline3\n");
    workspace
        .child(".venv")
        .child("uv-patches.json")
        .assert(predicate::path::is_file());

    Ok(())
}

/// A manifest with no patches is a no-op.
#[test]
fn patch_apply_empty() -> Result<()> {
    let context = uv_test::test_context!("3.12");
    context.init().arg("foo").assert().success();

    let workspace = context.temp_dir.child("foo");
    workspace
        .child("patch.json")
        .write_str("{\"patches\": []}\n")?;

    uv_snapshot!(context.filters(), context.patch_apply().arg("-f").arg("patch.json").current_dir(&workspace), @"
    success: true
    exit_code: 0
    ----- stdout -----

    ----- stderr -----
    No patches to apply
    "
    );

    Ok(())
}

/// A patch whose contents don't match the recorded checksum is rejected before it is applied.
#[test]
fn patch_apply_sha256_mismatch() -> Result<()> {
    let context = uv_test::test_context!("3.12");
    context.init().arg("foo").assert().success();

    let workspace = context.temp_dir.child("foo");
    workspace.child("hello.txt").write_str(HELLO_TXT)?;
    workspace.child("hello.patch").write_str(HELLO_PATCH)?;
    write_manifest(&workspace, "foo", WRONG_SHA256);

    uv_snapshot!(context.filters(), context.patch_apply().arg("-f").arg("patch.json").current_dir(&workspace), @"
    success: false
    exit_code: 1
    ----- stdout -----

    ----- stderr -----
    error: SHA-256 mismatch for `hello.patch`:
      expected: 0000000000000000000000000000000000000000000000000000000000000000
      actual:   5dbcaac1600c4efda928296f53e803d0cd3d1e4aae205c486f0ac30bae3a1efa
    "
    );

    Ok(())
}

/// A patch that references a package that isn't a member of the workspace is rejected.
#[test]
fn patch_apply_missing_package() -> Result<()> {
    let context = uv_test::test_context!("3.12");
    context.init().arg("foo").assert().success();

    let workspace = context.temp_dir.child("foo");
    workspace.child("hello.txt").write_str(HELLO_TXT)?;
    workspace.child("hello.patch").write_str(HELLO_PATCH)?;
    write_manifest(&workspace, "bar", HELLO_PATCH_SHA256);

    uv_snapshot!(context.filters(), context.patch_apply().arg("-f").arg("patch.json").current_dir(&workspace), @"
    success: false
    exit_code: 1
    ----- stdout -----

    ----- stderr -----
    error: Package `bar` not found in workspace (from `hello.patch`)
    "
    );

    Ok(())
}

/// A manifest with an invalid `sha256sum` field fails to parse.
#[test]
fn patch_apply_malformed_manifest() -> Result<()> {
    let context = uv_test::test_context!("3.12");
    context.init().arg("foo").assert().success();

    let workspace = context.temp_dir.child("foo");
    write_manifest(&workspace, "foo", "not-a-valid-sha256");

    let mut filters = context.filters();
    filters.push((r" at line \d+ column \d+", ""));

    uv_snapshot!(filters, context.patch_apply().arg("-f").arg("patch.json").current_dir(&workspace), @"
    success: false
    exit_code: 1
    ----- stdout -----

    ----- stderr -----
    error: Failed to parse patch manifest `patch.json`

    Caused by: `sha256sum` must be a 64-character lowercase hex string, got `not-a-valid-sha256`
    "
    );

    Ok(())
}

/// `uv patch show` takes no arguments: it reports nothing when no patches have been applied yet.
#[test]
fn patch_show_none_applied() -> Result<()> {
    let context = uv_test::test_context!("3.12");
    context.init().arg("foo").assert().success();

    let workspace = context.temp_dir.child("foo");

    uv_snapshot!(context.filters(), context.patch_show().current_dir(&workspace), @"
    success: true
    exit_code: 0
    ----- stdout -----

    ----- stderr -----
    No patches have been applied
    "
    );

    Ok(())
}

/// `uv patch show` (no arguments) lists the patches recorded by a prior `uv patch apply` run, by
/// reading the state uv stores in the workspace's virtual environment.
#[test]
fn patch_show_after_apply() -> Result<()> {
    let context = uv_test::test_context!("3.12");
    context.init().arg("foo").assert().success();

    let workspace = context.temp_dir.child("foo");
    workspace.child("hello.txt").write_str(HELLO_TXT)?;
    workspace.child("hello.patch").write_str(HELLO_PATCH)?;
    write_manifest(&workspace, "foo", HELLO_PATCH_SHA256);

    context
        .patch_apply()
        .arg("-f")
        .arg("patch.json")
        .current_dir(&workspace)
        .assert()
        .success();

    let mut filters = context.filters();
    filters.push((r"\d{4}-\d{2}-\d{2}T[\d:.]+Z", "[TIMESTAMP]"));

    uv_snapshot!(filters, context.patch_show().current_dir(&workspace), @"
    success: true
    exit_code: 0
    ----- stdout -----
    foo hello.patch (applied [TIMESTAMP])

    ----- stderr -----
    "
    );

    Ok(())
}

/// Re-applying a manifest whose patches were already applied (with an unchanged checksum) is a
/// no-op: the patch is skipped with a warning instead of being re-applied.
#[test]
fn patch_apply_skip_already_applied() -> Result<()> {
    let context = uv_test::test_context!("3.12");
    context.init().arg("foo").assert().success();

    let workspace = context.temp_dir.child("foo");
    workspace.child("hello.txt").write_str(HELLO_TXT)?;
    workspace.child("hello.patch").write_str(HELLO_PATCH)?;
    write_manifest(&workspace, "foo", HELLO_PATCH_SHA256);

    // First run applies the patch normally.
    context
        .patch_apply()
        .arg("-f")
        .arg("patch.json")
        .current_dir(&workspace)
        .assert()
        .success();

    // Second run, without reverting the file, would fail if `git apply` were re-run; it must be
    // skipped instead.
    uv_snapshot!(context.filters(), context.patch_apply().arg("-f").arg("patch.json").current_dir(&workspace), @"
    success: true
    exit_code: 0
    ----- stdout -----

    ----- stderr -----
    warning: Patch `hello.patch` was already applied to `foo`, skipping
    All patches were already applied
    "
    );

    Ok(())
}

/// `uv patch reset` reports nothing to do when no patches have been applied.
#[test]
fn patch_reset_none_applied() -> Result<()> {
    let context = uv_test::test_context!("3.12");
    context.init().arg("foo").assert().success();

    let workspace = context.temp_dir.child("foo");

    uv_snapshot!(context.filters(), context.patch_reset().current_dir(&workspace), @"
    success: true
    exit_code: 0
    ----- stdout -----

    ----- stderr -----
    No patches have been applied
    "
    );

    Ok(())
}

/// `uv patch reset` reverts a previously applied patch and clears the recorded state.
#[test]
fn patch_reset_after_apply() -> Result<()> {
    let context = uv_test::test_context!("3.12");
    context.init().arg("foo").assert().success();

    let workspace = context.temp_dir.child("foo");
    workspace.child("hello.txt").write_str(HELLO_TXT)?;
    workspace.child("hello.patch").write_str(HELLO_PATCH)?;
    write_manifest(&workspace, "foo", HELLO_PATCH_SHA256);

    context
        .patch_apply()
        .arg("-f")
        .arg("patch.json")
        .current_dir(&workspace)
        .assert()
        .success();

    workspace
        .child("hello.txt")
        .assert("line1\nline2-changed\nline3\n");

    uv_snapshot!(context.filters(), context.patch_reset().current_dir(&workspace), @"
    success: true
    exit_code: 0
    ----- stdout -----

    ----- stderr -----
    Reverting `hello.patch` from `foo`
    Reverted 1 patch
    "
    );

    // The file is back to its original contents...
    workspace.child("hello.txt").assert(HELLO_TXT);

    // ...and the recorded state is cleared.
    uv_snapshot!(context.filters(), context.patch_show().current_dir(&workspace), @"
    success: true
    exit_code: 0
    ----- stdout -----

    ----- stderr -----
    No patches have been applied
    "
    );

    Ok(())
}
