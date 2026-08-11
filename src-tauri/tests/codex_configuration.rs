use grillforge_lib::application::ControlPlaneService;

#[test]
fn native_codex_models_are_saved_without_entering_the_provider_registry() {
    let temp = tempfile::tempdir().unwrap();
    let service = ControlPlaneService::new(temp.path());

    service
        .set_codex_native_main_model(Some("gpt-main".into()))
        .unwrap();
    service
        .set_codex_native_default_subagent_model(Some("gpt-worker".into()))
        .unwrap();
    let state = service
        .set_codex_native_custom_agent_model("reviewer".into(), Some("gpt-review".into()))
        .unwrap();

    assert!(state.codex_main_model_id.is_none());
    assert_eq!(
        state
            .codex_native_model_slots
            .get("main")
            .map(String::as_str),
        Some("gpt-main")
    );
    assert_eq!(
        state
            .codex_native_model_slots
            .get("default_subagent")
            .map(String::as_str),
        Some("gpt-worker")
    );
    assert_eq!(
        state
            .codex_native_model_slots
            .get("agent_reviewer")
            .map(String::as_str),
        Some("gpt-review")
    );
    assert!(state.providers.is_empty());
    assert!(state.models.is_empty());
}
