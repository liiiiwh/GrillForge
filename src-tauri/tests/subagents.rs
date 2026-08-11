use grillforge_lib::application::{ControlPlaneService, ModelInput, ProviderInput, SubAgentInput};
use grillforge_lib::core::provider::{ApiKeyPlacement, EndpointMode, Protocol};
use grillforge_lib::integration::IntegrationService;
use grillforge_lib::selector;

fn provider() -> ProviderInput {
    ProviderInput {
        id: "local".into(),
        name: "Local".into(),
        protocol: Protocol::OpenAiResponses,
        endpoint: "http://127.0.0.1:18080/v1".into(),
        endpoint_mode: EndpointMode::BaseUrl,
        api_key_placement: ApiKeyPlacement::None,
        api_key: None,
        enabled: true,
        models_url: None,
    }
}

fn model() -> ModelInput {
    ModelInput {
        id: "shared-model".into(),
        name: "Shared Model".into(),
        upstream_id: "shared-model".into(),
        provider_id: "local".into(),
        capabilities: vec![],
        protocol_capabilities: vec![],
    }
}

#[test]
fn user_can_create_unbounded_subagents_with_independent_capabilities() {
    let root = tempfile::tempdir().expect("config root");
    let service = ControlPlaneService::new(root.path());
    service.save_provider(provider()).expect("provider");
    service.save_model(model()).expect("model");

    service
        .save_subagent(SubAgentInput {
            id: "code-reviewer".into(),
            name: "代码审查".into(),
            model_id: "shared-model".into(),
            capabilities: vec!["review".into(), "security".into()],
            enabled: true,
        })
        .expect("reviewer");
    let state = service
        .save_subagent(SubAgentInput {
            id: "test-writer".into(),
            name: "测试编写".into(),
            model_id: "shared-model".into(),
            capabilities: vec!["testing".into()],
            enabled: true,
        })
        .expect("test writer");

    assert_eq!(state.subagents.len(), 2);
    assert_eq!(state.subagents[0].id, "code-reviewer");
    assert_eq!(state.subagents[0].capabilities, ["review", "security"]);
    assert_eq!(state.subagents[1].model_id, "shared-model");
}

#[test]
fn native_claude_subagent_is_enabled_by_default_and_toggles_independently() {
    let directory = tempfile::tempdir().expect("configuration directory");
    let service = ControlPlaneService::new(directory.path());

    assert!(
        service
            .state()
            .expect("initial state")
            .native_subagent_enabled
    );

    let disabled = service
        .set_native_subagent_enabled(false)
        .expect("disable native SubAgent");
    assert!(!disabled.native_subagent_enabled);
    assert!(disabled.subagents.is_empty());

    let enabled = service
        .set_native_subagent_enabled(true)
        .expect("enable native SubAgent");
    assert!(enabled.native_subagent_enabled);
    assert!(enabled.subagents.is_empty());
}

#[test]
fn selector_publishes_each_subagent_identity_and_its_own_capabilities() {
    let root = tempfile::tempdir().expect("config root");
    let service = ControlPlaneService::new(root.path());
    service.save_provider(provider()).expect("provider");
    service.save_model(model()).expect("model");
    for (id, name, capabilities) in [
        (
            "code-reviewer",
            "代码审查",
            vec!["review".into(), "security".into()],
        ),
        ("test-writer", "测试编写", vec!["testing".into()]),
    ] {
        service
            .save_subagent(SubAgentInput {
                id: id.into(),
                name: name.into(),
                model_id: "shared-model".into(),
                capabilities,
                enabled: true,
            })
            .expect("SubAgent");
    }
    std::fs::write(root.path().join("claude-code.snapshot.json"), "active")
        .expect("active adapter marker");

    let output = selector::select(root.path()).expect("selector output");

    assert_eq!(output.workers.len(), 3);
    assert_eq!(
        output.workers[0].agent_name,
        "grillforge-worker-claude-native"
    );
    assert_eq!(output.workers[0].route_alias, "inherit");
    assert_eq!(output.workers[0].provider_id, "anthropic-native");
    assert_eq!(
        output.workers[1].agent_name,
        "grillforge-worker-code-reviewer"
    );
    assert_eq!(output.workers[1].display_name, "代码审查");
    assert_eq!(output.workers[1].capabilities, ["review", "security"]);
    assert_eq!(
        output.workers[2].agent_name,
        "grillforge-worker-test-writer"
    );
    assert_eq!(output.workers[2].route_alias, "grillforge/shared-model");
}

#[test]
fn applying_subagents_generates_one_claude_agent_definition_per_identity() {
    let root = tempfile::tempdir().expect("root");
    let grillforge = root.path().join("grillforge");
    let claude = root.path().join("claude");
    let service = ControlPlaneService::new(&grillforge);
    service.save_provider(provider()).expect("provider");
    service.save_model(model()).expect("model");
    for (id, name, capabilities) in [
        (
            "code-reviewer",
            "代码审查",
            vec!["review".into(), "security".into()],
        ),
        ("test-writer", "测试编写", vec!["testing".into()]),
    ] {
        service
            .save_subagent(SubAgentInput {
                id: id.into(),
                name: name.into(),
                model_id: "shared-model".into(),
                capabilities,
                enabled: true,
            })
            .expect("SubAgent");
    }

    let status = IntegrationService::new(&claude, &grillforge)
        .apply(&service.state().expect("state"), "http://127.0.0.1:15721")
        .expect("apply");

    assert_eq!(status.generated_agent_names.len(), 3);
    assert!(
        status
            .generated_agent_names
            .contains(&"grillforge-worker-claude-native".into())
    );
    let native = std::fs::read_to_string(claude.join("agents/grillforge-worker-claude-native.md"))
        .expect("native Claude Agent");
    let reviewer =
        std::fs::read_to_string(claude.join("agents/grillforge-worker-code-reviewer.md"))
            .expect("reviewer Agent");
    let testing = std::fs::read_to_string(claude.join("agents/grillforge-worker-test-writer.md"))
        .expect("testing Agent");
    assert!(reviewer.contains("model: grillforge/shared-model"));
    assert!(native.contains("model: inherit"));
    assert!(reviewer.contains("review, security"));
    assert!(testing.contains("testing"));
}
