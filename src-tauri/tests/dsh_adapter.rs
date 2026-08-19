use grillforge_lib::adapters::dsh::{
    DshAdapter, DshModelSpec, DshPaths, DshRequest, DshTakeoverStatus,
};
use std::fs;
use tempfile::tempdir;

fn model(id: &str) -> DshModelSpec {
    DshModelSpec::new(id, "Coder", Some(128_000)).unwrap()
}

fn request() -> DshRequest {
    DshRequest::new(
        "http://127.0.0.1:15721/chat/dsh/v1",
        "gateway-secret",
        vec![model("grillforge/coder")],
        Some("grillforge/coder".into()),
    )
    .unwrap()
}

fn paths(root: &std::path::Path) -> DshPaths {
    DshPaths::new(
        root.join(".dsh/profiles/headless/cordis.patch.yml"),
        root.join(".dsh/.env"),
    )
}

#[test]
fn apply_owns_one_block_and_keeps_every_user_entry() {
    let temp = tempdir().unwrap();
    let paths = paths(temp.path());
    fs::create_dir_all(paths.patch_path.parent().unwrap()).unwrap();
    fs::write(
        &paths.patch_path,
        "# my own layer\n- id: timer\n  disabled: true\n",
    )
    .unwrap();
    let adapter = DshAdapter::new(paths.clone(), temp.path().join("grillforge"));
    adapter.apply(request()).unwrap();

    let patch = fs::read_to_string(&paths.patch_path).unwrap();
    assert!(patch.contains("- id: timer"), "{patch}");
    assert!(patch.contains("# my own layer"), "{patch}");
    assert!(patch.contains("@deepseek-ai/dsh-llm-pi-ai"), "{patch}");
    assert!(patch.contains("api: openai-completions"), "{patch}");
    assert!(
        patch.contains("apiKeyEnv: GRILLFORGE_DSH_API_KEY"),
        "{patch}"
    );
    assert!(patch.contains("contextWindow: 128000"), "{patch}");

    // The credential is a reference here and a value only in the env file.
    assert!(!patch.contains("gateway-secret"), "{patch}");
    let env = fs::read_to_string(&paths.credentials_path).unwrap();
    assert!(
        env.contains("GRILLFORGE_DSH_API_KEY=gateway-secret"),
        "{env}"
    );

    // Applying twice leaves one managed block, not two.
    adapter.apply(request()).unwrap();
    let patch = fs::read_to_string(&paths.patch_path).unwrap();
    assert_eq!(patch.matches("# >>> grillforge").count(), 1, "{patch}");
    assert_eq!(patch.matches("- id: timer").count(), 1, "{patch}");
    assert_eq!(
        adapter.status().unwrap().takeover,
        DshTakeoverStatus::Active
    );
}

#[test]
fn disable_restores_the_user_layer_exactly() {
    let temp = tempdir().unwrap();
    let paths = paths(temp.path());
    fs::create_dir_all(paths.patch_path.parent().unwrap()).unwrap();
    let original = "# my own layer\n- id: timer\n  disabled: true\n";
    fs::write(&paths.patch_path, original).unwrap();
    fs::create_dir_all(paths.credentials_path.parent().unwrap()).unwrap();
    fs::write(&paths.credentials_path, "OTHER_KEY=keep-me\n").unwrap();

    let adapter = DshAdapter::new(paths.clone(), temp.path().join("grillforge"));
    adapter.apply(request()).unwrap();
    adapter.disable().unwrap();

    assert_eq!(fs::read_to_string(&paths.patch_path).unwrap(), original);
    assert_eq!(
        fs::read_to_string(&paths.credentials_path).unwrap(),
        "OTHER_KEY=keep-me\n"
    );
    assert_eq!(
        adapter.status().unwrap().takeover,
        DshTakeoverStatus::Inactive
    );
}

#[test]
fn an_edit_outside_grillforge_is_reported_as_drift() {
    let temp = tempdir().unwrap();
    let paths = paths(temp.path());
    let adapter = DshAdapter::new(paths.clone(), temp.path().join("grillforge"));
    adapter.apply(request()).unwrap();
    fs::write(&paths.patch_path, "- id: timer\n").unwrap();
    assert_eq!(
        adapter.status().unwrap().takeover,
        DshTakeoverStatus::Drifted
    );
    assert!(adapter.disable().is_err());
}

#[test]
fn a_request_must_be_loopback_and_name_a_configured_default() {
    assert!(
        DshRequest::new(
            "https://example.com/v1",
            "secret",
            vec![model("grillforge/coder")],
            None,
        )
        .is_err()
    );
    assert!(
        DshRequest::new(
            "http://127.0.0.1:15721/chat/dsh/v1",
            "secret",
            vec![model("grillforge/coder")],
            Some("grillforge/other".into()),
        )
        .is_err()
    );
    assert!(DshModelSpec::new("coder", "Coder", None).is_err());
}
