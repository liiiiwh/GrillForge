use grillforge_lib::cli_discovery::{first_valid_candidate, login_shell_candidates_with};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use tempfile::tempdir;

#[test]
fn login_shell_discovery_returns_unique_absolute_cli_paths() {
    let temp = tempdir().unwrap();
    let cli = temp.path().join("node/v22/bin/pi");
    fs::create_dir_all(cli.parent().unwrap()).unwrap();
    fs::write(&cli, "#!/bin/sh\nexit 0\n").unwrap();
    fs::set_permissions(&cli, fs::Permissions::from_mode(0o755)).unwrap();

    let shell = temp.path().join("shell");
    fs::write(
        &shell,
        format!(
            "#!/bin/sh\nprintf '%s\\n' '{}' 'pi: aliased to something' '{}'\n",
            cli.display(),
            cli.display()
        ),
    )
    .unwrap();
    fs::set_permissions(&shell, fs::Permissions::from_mode(0o755)).unwrap();

    assert_eq!(
        login_shell_candidates_with(&shell, "pi").unwrap(),
        vec![cli]
    );
}

#[test]
fn login_shell_discovery_rejects_untrusted_command_names() {
    let error = login_shell_candidates_with("/bin/sh", "pi; touch /tmp/nope")
        .expect_err("shell fragments must never enter a discovery command");

    assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
}

#[test]
fn candidate_discovery_skips_stale_entries_and_returns_the_first_valid_cli() {
    let temp = tempdir().unwrap();
    let stale = temp.path().join("stale");
    let valid = temp.path().join("valid");
    fs::write(&stale, "stale").unwrap();
    fs::write(&valid, "valid").unwrap();

    let found = first_valid_candidate([stale.clone(), valid.clone()], |path: &Path| {
        if path == valid {
            Ok(path.to_path_buf())
        } else {
            Err(format!("{} is stale", path.display()))
        }
    })
    .unwrap();

    assert_eq!(found, Some(valid));
}

#[test]
fn candidate_discovery_returns_the_last_actionable_error_when_all_entries_fail() {
    let temp = tempdir().unwrap();
    let first = temp.path().join("first");
    let last = temp.path().join("last");
    fs::write(&first, "first").unwrap();
    fs::write(&last, "last").unwrap();

    let error = first_valid_candidate([first, last.clone()], |path: &Path| {
        Err::<(), _>(format!("{} is invalid", path.display()))
    })
    .unwrap_err();

    assert_eq!(error, format!("{} is invalid", last.display()));
}
