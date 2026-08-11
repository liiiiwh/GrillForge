use grillforge_lib::skills::SkillInstaller;
use std::fs;

const SKILL_MD: &str = include_str!("../../skills/grillforge-model-selector/SKILL.md");
const OPENAI_YAML: &str = include_str!("../../skills/grillforge-model-selector/agents/openai.yaml");
const SELECT_MODELS: &str =
    include_str!("../../skills/grillforge-model-selector/scripts/select_models.py");

#[test]
fn first_install_writes_the_complete_embedded_selector_skill() {
    let root = tempfile::tempdir().unwrap();

    SkillInstaller::install(root.path()).unwrap();

    let skill = root.path().join("grillforge-model-selector");
    assert_eq!(
        fs::read_to_string(skill.join("SKILL.md")).unwrap(),
        SKILL_MD
    );
    assert_eq!(
        fs::read_to_string(skill.join("agents/openai.yaml")).unwrap(),
        OPENAI_YAML
    );
    assert_eq!(
        fs::read_to_string(skill.join("scripts/select_models.py")).unwrap(),
        SELECT_MODELS
    );

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        assert_eq!(
            fs::metadata(skill.join("SKILL.md"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o644
        );
        assert_eq!(
            fs::metadata(skill.join("agents/openai.yaml"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o644
        );
        assert_eq!(
            fs::metadata(skill.join("scripts/select_models.py"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o755
        );
    }
}

#[cfg(unix)]
#[test]
fn repeated_install_is_a_noop_when_owned_files_are_current() {
    use std::os::unix::fs::PermissionsExt;

    let root = tempfile::tempdir().unwrap();
    SkillInstaller::install(root.path()).unwrap();
    let skill = root.path().join("grillforge-model-selector");
    let directories = [skill.join("agents"), skill.join("scripts"), skill.clone()];
    for directory in &directories {
        fs::set_permissions(directory, fs::Permissions::from_mode(0o555)).unwrap();
    }

    let result = SkillInstaller::install(root.path());

    for directory in directories.iter().rev() {
        fs::set_permissions(directory, fs::Permissions::from_mode(0o755)).unwrap();
    }
    result.unwrap();
}

#[test]
fn update_replaces_only_owned_files_and_preserves_unrelated_user_files() {
    let root = tempfile::tempdir().unwrap();
    let skill = root.path().join("grillforge-model-selector");
    fs::create_dir_all(skill.join("agents")).unwrap();
    fs::create_dir_all(skill.join("scripts")).unwrap();
    fs::write(skill.join("SKILL.md"), "old skill").unwrap();
    fs::write(skill.join("agents/openai.yaml"), "old metadata").unwrap();
    fs::write(skill.join("scripts/select_models.py"), "old wrapper").unwrap();
    fs::write(skill.join("notes.txt"), "user notes").unwrap();
    fs::write(skill.join("scripts/user-helper.py"), "user helper").unwrap();
    fs::create_dir_all(root.path().join("another-skill")).unwrap();
    fs::write(root.path().join("another-skill/SKILL.md"), "another skill").unwrap();

    SkillInstaller::install(root.path()).unwrap();

    assert_eq!(
        fs::read_to_string(skill.join("SKILL.md")).unwrap(),
        SKILL_MD
    );
    assert_eq!(
        fs::read_to_string(skill.join("agents/openai.yaml")).unwrap(),
        OPENAI_YAML
    );
    assert_eq!(
        fs::read_to_string(skill.join("scripts/select_models.py")).unwrap(),
        SELECT_MODELS
    );
    assert_eq!(
        fs::read_to_string(skill.join("notes.txt")).unwrap(),
        "user notes"
    );
    assert_eq!(
        fs::read_to_string(skill.join("scripts/user-helper.py")).unwrap(),
        "user helper"
    );
    assert_eq!(
        fs::read_to_string(root.path().join("another-skill/SKILL.md")).unwrap(),
        "another skill"
    );
}

#[test]
fn preflight_reports_an_owned_path_conflict_before_writing_any_file() {
    let root = tempfile::tempdir().unwrap();
    let skill = root.path().join("grillforge-model-selector");
    fs::create_dir_all(skill.join("agents")).unwrap();
    fs::create_dir_all(skill.join("scripts/select_models.py")).unwrap();
    fs::write(skill.join("SKILL.md"), "old skill").unwrap();
    fs::write(skill.join("agents/openai.yaml"), "old metadata").unwrap();

    let error = SkillInstaller::install(root.path()).unwrap_err();

    assert!(error.to_string().contains("scripts/select_models.py"));
    assert_eq!(
        fs::read_to_string(skill.join("SKILL.md")).unwrap(),
        "old skill"
    );
    assert_eq!(
        fs::read_to_string(skill.join("agents/openai.yaml")).unwrap(),
        "old metadata"
    );
}

#[cfg(unix)]
#[test]
fn first_write_failure_is_returned_without_attempting_later_files() {
    use std::os::unix::fs::PermissionsExt;

    let root = tempfile::tempdir().unwrap();
    let skill = root.path().join("grillforge-model-selector");
    fs::create_dir_all(skill.join("agents")).unwrap();
    fs::create_dir_all(skill.join("scripts")).unwrap();
    fs::set_permissions(&skill, fs::Permissions::from_mode(0o555)).unwrap();

    let result = SkillInstaller::install(root.path());

    fs::set_permissions(&skill, fs::Permissions::from_mode(0o755)).unwrap();
    let error = result.unwrap_err();
    assert!(error.to_string().contains("SKILL.md"));
    assert!(!skill.join("agents/openai.yaml").exists());
    assert!(!skill.join("scripts/select_models.py").exists());
}
