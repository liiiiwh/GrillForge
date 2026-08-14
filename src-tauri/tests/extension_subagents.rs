use grillforge_lib::application::{
    ControlPlaneService, ExtensionSubAgentInput, ModelInput, ProviderInput,
};
use grillforge_lib::core::provider::{ApiKeyPlacement, EndpointMode, Protocol};

fn configured_service(root: &std::path::Path) -> ControlPlaneService {
    let service = ControlPlaneService::new(root);
    service
        .save_provider(ProviderInput {
            id: "local".into(),
            name: "Local".into(),
            protocol: Protocol::OpenAiResponses,
            endpoint: "http://127.0.0.1:18080/v1".into(),
            endpoint_mode: EndpointMode::BaseUrl,
            api_key_placement: ApiKeyPlacement::None,
            api_key: None,
            enabled: true,
            models_url: None,
        })
        .expect("provider");
    service
        .save_model(ModelInput {
            id: "coder".into(),
            name: "Coder".into(),
            upstream_id: "coder".into(),
            provider_id: "local".into(),
            capabilities: vec![],
            protocol_capabilities: vec![],
        })
        .expect("model");
    service
}

#[test]
fn extension_subagent_crud_is_exposed_in_public_state() {
    let root = tempfile::tempdir().expect("configuration root");
    let service = configured_service(root.path());

    let created = service
        .save_extension_subagent(ExtensionSubAgentInput {
            id: "claude-reviewer".into(),
            name: "Claude Reviewer".into(),
            source_client_id: "claude_code".into(),
            source_agent_id: "reviewer".into(),
            model_id: Some("coder".into()),
            capabilities: vec!["review".into()],
        })
        .expect("create extension SubAgent");

    assert_eq!(created.extension_subagents.len(), 1);
    assert_eq!(created.extension_subagents[0].id, "claude-reviewer");
    assert_eq!(
        created.extension_subagents[0].source_client_id,
        "claude_code"
    );
    assert_eq!(
        created.extension_subagents[0].model_id.as_deref(),
        Some("coder")
    );

    let updated = service
        .update_extension_subagent(ExtensionSubAgentInput {
            id: "claude-reviewer".into(),
            name: "Strict Reviewer".into(),
            source_client_id: "claude_code".into(),
            source_agent_id: "security-reviewer".into(),
            model_id: None,
            capabilities: vec!["review".into(), "security".into()],
        })
        .expect("update extension SubAgent");
    assert_eq!(updated.extension_subagents[0].name, "Strict Reviewer");
    assert_eq!(updated.extension_subagents[0].model_id, None);

    let deleted = service
        .delete_extension_subagent("claude-reviewer")
        .expect("delete extension SubAgent");
    assert!(deleted.extension_subagents.is_empty());
}

#[test]
fn save_generates_a_stable_id_when_extension_subagent_id_is_empty() {
    let first_root = tempfile::tempdir().expect("first configuration root");
    let second_root = tempfile::tempdir().expect("second configuration root");
    let first = configured_service(first_root.path());
    let second = configured_service(second_root.path());
    let input = || ExtensionSubAgentInput {
        id: String::new(),
        name: "代码审查".into(),
        source_client_id: "claude_code".into(),
        source_agent_id: "reviewer".into(),
        model_id: Some("coder".into()),
        capabilities: vec!["review".into()],
    };

    let first_id = first
        .save_extension_subagent(input())
        .expect("generated first ID")
        .extension_subagents[0]
        .id
        .clone();
    let second_id = second
        .save_extension_subagent(input())
        .expect("generated second ID")
        .extension_subagents[0]
        .id
        .clone();

    assert_eq!(first_id, second_id);
    assert!(first_id.starts_with("claude-code-reviewer-"));
    assert!(first_id.bytes().all(|character| {
        character.is_ascii_lowercase() || character.is_ascii_digit() || character == b'-'
    }));
}

#[test]
fn save_input_accepts_an_omitted_or_null_id() {
    for (case, id) in [("omitted", None), ("null", Some(serde_json::Value::Null))] {
        let root = tempfile::tempdir().expect("configuration root");
        let service = configured_service(root.path());
        let mut value = serde_json::json!({
            "name": "Reviewer",
            "sourceClientId": "claude_code",
            "sourceAgentId": "reviewer",
            "modelId": null,
            "capabilities": []
        });
        if let Some(id) = id {
            value["id"] = id;
        }
        let input: ExtensionSubAgentInput =
            serde_json::from_value(value).unwrap_or_else(|error| panic!("{case} ID: {error}"));

        let state = service
            .save_extension_subagent(input)
            .expect("save generated ID");

        assert_eq!(state.extension_subagents.len(), 1);
        assert!(!state.extension_subagents[0].id.is_empty());
    }
}

#[test]
fn generated_id_collision_is_reported_explicitly() {
    let root = tempfile::tempdir().expect("configuration root");
    let service = configured_service(root.path());
    let input = || ExtensionSubAgentInput {
        id: String::new(),
        name: "Reviewer".into(),
        source_client_id: "claude_code".into(),
        source_agent_id: "reviewer".into(),
        model_id: None,
        capabilities: vec![],
    };
    let generated_id = service
        .save_extension_subagent(input())
        .expect("first definition")
        .extension_subagents[0]
        .id
        .clone();

    let error = service
        .save_extension_subagent(input())
        .expect_err("same generated identity must collide");

    assert_eq!(
        error,
        format!("duplicate extension SubAgent id: {generated_id}")
    );
}

#[test]
fn update_requires_the_existing_extension_subagent_id() {
    let root = tempfile::tempdir().expect("configuration root");
    let service = configured_service(root.path());
    service
        .save_extension_subagent(ExtensionSubAgentInput {
            id: "reviewer".into(),
            name: "Reviewer".into(),
            source_client_id: "claude_code".into(),
            source_agent_id: "reviewer".into(),
            model_id: None,
            capabilities: vec![],
        })
        .expect("existing extension SubAgent");

    let error = service
        .update_extension_subagent(ExtensionSubAgentInput {
            id: String::new(),
            name: "Renamed Reviewer".into(),
            source_client_id: "claude_code".into(),
            source_agent_id: "reviewer".into(),
            model_id: None,
            capabilities: vec![],
        })
        .expect_err("update without ID must fail");

    assert_eq!(error, "extension SubAgent update requires an id");
    assert_eq!(
        service
            .state()
            .expect("preserved state")
            .extension_subagents[0]
            .name,
        "Reviewer"
    );
}

#[test]
fn each_client_can_toggle_its_own_extension_subagent_bindings() {
    let root = tempfile::tempdir().expect("configuration root");
    let service = configured_service(root.path());
    service
        .save_extension_subagent(ExtensionSubAgentInput {
            id: "claude-reviewer".into(),
            name: "Claude Reviewer".into(),
            source_client_id: "claude_code".into(),
            source_agent_id: "reviewer".into(),
            model_id: Some("coder".into()),
            capabilities: vec!["review".into()],
        })
        .expect("extension SubAgent");

    service
        .set_client_extension_subagent_enabled("claude_code", "claude-reviewer", true)
        .expect("bind Claude Code");
    service
        .set_client_extension_subagent_enabled("claude_desktop", "claude-reviewer", true)
        .expect("bind Claude Client");
    let bound = service
        .set_client_extension_subagent_enabled("pi", "claude-reviewer", true)
        .expect("bind Pi");
    assert_eq!(
        bound.client_extension_subagent_ids["claude_code"],
        ["claude-reviewer"]
    );
    assert_eq!(
        bound.client_extension_subagent_ids["claude_desktop"],
        ["claude-reviewer"]
    );
    assert_eq!(
        bound.client_extension_subagent_ids["pi"],
        ["claude-reviewer"]
    );

    let unbound = service
        .set_client_extension_subagent_enabled("claude_code", "claude-reviewer", false)
        .expect("unbind Claude Code");
    assert!(unbound.client_extension_subagent_ids["claude_code"].is_empty());
    assert_eq!(
        unbound.client_extension_subagent_ids["pi"],
        ["claude-reviewer"]
    );
}

#[test]
fn deleting_a_bound_extension_subagent_is_rejected() {
    let root = tempfile::tempdir().expect("configuration root");
    let service = configured_service(root.path());
    service
        .save_extension_subagent(ExtensionSubAgentInput {
            id: "claude-reviewer".into(),
            name: "Claude Reviewer".into(),
            source_client_id: "claude_code".into(),
            source_agent_id: "reviewer".into(),
            model_id: None,
            capabilities: vec![],
        })
        .expect("extension SubAgent");
    service
        .set_client_extension_subagent_enabled("codex", "claude-reviewer", true)
        .expect("bind Codex");

    let error = service
        .delete_extension_subagent("claude-reviewer")
        .expect_err("bound extension SubAgent must be preserved");

    assert_eq!(
        error,
        "extension SubAgent claude-reviewer is still bound to clients: codex"
    );
    assert_eq!(
        service
            .state()
            .expect("preserved state")
            .extension_subagents[0]
            .id,
        "claude-reviewer"
    );
}

#[test]
fn invalid_extension_subagent_input_is_rejected_without_changing_state() {
    let root = tempfile::tempdir().expect("configuration root");
    let service = configured_service(root.path());

    let unknown_client = service
        .save_extension_subagent(ExtensionSubAgentInput {
            id: "reviewer".into(),
            name: "Reviewer".into(),
            source_client_id: "not_installed_adapter".into(),
            source_agent_id: "reviewer".into(),
            model_id: None,
            capabilities: vec![],
        })
        .expect_err("unknown source client must fail");
    assert_eq!(
        unknown_client,
        "unsupported client adapter: not_installed_adapter"
    );

    let duplicate_capability = service
        .save_extension_subagent(ExtensionSubAgentInput {
            id: "reviewer".into(),
            name: "Reviewer".into(),
            source_client_id: "claude_code".into(),
            source_agent_id: "reviewer".into(),
            model_id: Some("coder".into()),
            capabilities: vec!["review".into(), "review".into()],
        })
        .expect_err("duplicate capability must fail");
    assert_eq!(
        duplicate_capability,
        "invalid or duplicate extension SubAgent capability: review"
    );
    assert!(
        service
            .state()
            .expect("unchanged state")
            .extension_subagents
            .is_empty()
    );
}

#[test]
fn extension_subagent_capabilities_preserve_case_and_deduplicate_case_insensitively() {
    let root = tempfile::tempdir().expect("configuration root");
    let service = configured_service(root.path());

    let state = service
        .save_extension_subagent(ExtensionSubAgentInput {
            id: "reviewer".into(),
            name: "Reviewer".into(),
            source_client_id: "claude_code".into(),
            source_agent_id: "reviewer".into(),
            model_id: Some("coder".into()),
            capabilities: vec!["Gener".into(), "CodeReview".into()],
        })
        .expect("mixed-case capability labels are valid");
    assert_eq!(
        state.extension_subagents[0].capabilities,
        vec!["Gener", "CodeReview"]
    );

    let error = service
        .update_extension_subagent(ExtensionSubAgentInput {
            id: "reviewer".into(),
            name: "Reviewer".into(),
            source_client_id: "claude_code".into(),
            source_agent_id: "reviewer".into(),
            model_id: Some("coder".into()),
            capabilities: vec!["Review".into(), "review".into()],
        })
        .expect_err("capability labels differing only by case are duplicates");
    assert_eq!(
        error,
        "invalid or duplicate extension SubAgent capability: review"
    );
}

#[test]
fn extension_subagent_rejects_a_model_from_a_disabled_provider() {
    let root = tempfile::tempdir().expect("configuration root");
    let service = configured_service(root.path());
    service
        .update_provider(ProviderInput {
            id: "local".into(),
            name: "Local".into(),
            protocol: Protocol::OpenAiResponses,
            endpoint: "http://127.0.0.1:18080/v1".into(),
            endpoint_mode: EndpointMode::BaseUrl,
            api_key_placement: ApiKeyPlacement::None,
            api_key: None,
            enabled: false,
            models_url: None,
        })
        .expect("disable provider");

    let error = service
        .save_extension_subagent(ExtensionSubAgentInput {
            id: "reviewer".into(),
            name: "Reviewer".into(),
            source_client_id: "claude_code".into(),
            source_agent_id: "reviewer".into(),
            model_id: Some("coder".into()),
            capabilities: vec![],
        })
        .expect_err("disabled provider must fail");

    assert_eq!(
        error,
        "extension SubAgent reviewer model uses disabled provider: local"
    );
}
