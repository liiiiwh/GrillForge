use grillforge_lib::cli_discovery::{
    first_valid_candidate, first_valid_candidate_across_sources, login_shell_candidates_with,
};
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
            "#!/bin/sh\n[ \"$1\" = \"-lic\" ] || exit 9\nprintf '%s\\n' '{}' 'pi: aliased to something' '{}'\n",
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

#[test]
fn candidate_discovery_continues_with_shell_candidates_after_a_stale_primary_install() {
    let temp = tempdir().unwrap();
    let stale = temp.path().join("nvm/pi");
    let valid = temp.path().join("fnm/pi");
    fs::create_dir_all(stale.parent().unwrap()).unwrap();
    fs::create_dir_all(valid.parent().unwrap()).unwrap();
    fs::write(&stale, "stale").unwrap();
    fs::write(&valid, "valid").unwrap();

    let mut stale_inspections = 0;
    let found = first_valid_candidate_across_sources(
        [stale.clone()],
        || Ok::<_, String>(vec![stale.clone(), valid.clone()]),
        |path: &Path| {
            if path == valid {
                Ok(path.to_path_buf())
            } else {
                stale_inspections += 1;
                Err(format!("{} did not return a version", path.display()))
            }
        },
    )
    .unwrap();

    assert_eq!(found, Some(valid));
    assert_eq!(stale_inspections, 1);
}

#[test]
fn candidate_discovery_does_not_start_a_shell_when_a_primary_candidate_is_valid() {
    let temp = tempdir().unwrap();
    let valid = temp.path().join("path/pi");
    fs::create_dir_all(valid.parent().unwrap()).unwrap();
    fs::write(&valid, "valid").unwrap();

    let found = first_valid_candidate_across_sources(
        [valid.clone()],
        || -> Result<Vec<_>, String> { panic!("shell discovery must stay lazy") },
        |path: &Path| Ok::<_, String>(path.to_path_buf()),
    )
    .unwrap();

    assert_eq!(found, Some(valid));
}

#[test]
fn optional_shell_discovery_failure_means_the_cli_was_not_found() {
    let found = first_valid_candidate_across_sources(
        Vec::new(),
        || Err::<Vec<_>, _>("login shell CLI discovery timed out"),
        |_path: &Path| Ok::<_, &str>(()),
    )
    .expect("an unavailable optional discovery source is not a broken client");

    assert_eq!(found, None);
}
