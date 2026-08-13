use grillforge_lib::configuration::{
    AgentRecord, AgentsDocument, ConfigDocument, ConfigurationDocuments, ConfigurationFiles,
    MainRecord, ModelRecord, ModelsDocument, ProviderRecord,
};
use grillforge_lib::core::model::NativeProtocol;
use grillforge_lib::core::provider::{ApiKeyPlacement, EndpointMode, Protocol};
use std::fs;

fn valid_documents() -> (ConfigDocument, ModelsDocument, AgentsDocument) {
    (
        ConfigDocument {
            version: 2,
            providers: vec![ProviderRecord {
                id: "local".into(),
                name: "Local".into(),
                enabled: true,
                protocol: Protocol::OpenAiChatCompletions,
                endpoint: "http://127.0.0.1:8080/v1".into(),
                endpoint_mode: EndpointMode::BaseUrl,
                api_key_placement: ApiKeyPlacement::Bearer,
                api_key: "local-test-key".into(),
                models_url: None,
            }],
        },
        ModelsDocument {
            version: 2,
            models: vec![ModelRecord {
                id: "local-coder".into(),
                provider_id: "local".into(),
                upstream_id: "coder".into(),
                display_name: "Local Coder".into(),
                capabilities: vec!["coding".into()],
                protocol_capabilities: vec![],
                native_protocols: Some(vec![NativeProtocol::OpenAiChat]),
            }],
        },
        AgentsDocument {
            version: 2,
            extension_subagents: Vec::new(),
            mcp_mounted_client_ids: Vec::new(),
            agents: vec![AgentRecord {
                id: "test-agent".into(),
                adapter: "test".into(),
                enabled: true,
                main: MainRecord::Managed("local-coder".into()),
                model_slots: Default::default(),
                native_model_slots: Default::default(),
                model_pool: vec!["local-coder".into()],
                codex_agent_models: vec![],
                extension_subagent_ids: Vec::new(),
            }],
        },
    )
}

#[test]
fn valid_configuration_round_trips_across_three_yaml_files() {
    let directory = tempfile::tempdir().expect("temp directory");
    let files = ConfigurationFiles::new(directory.path());
    let (config, models, agents) = valid_documents();

    files.save(&config, &models, &agents).expect("valid save");
    let loaded = files.read().expect("valid read");

    assert_eq!(loaded.config, config);
    assert_eq!(loaded.models, models);
    assert_eq!(loaded.agents, agents);
}

#[test]
fn invalid_configuration_is_rejected_before_any_file_changes() {
    let directory = tempfile::tempdir().expect("temp directory");
    let files = ConfigurationFiles::new(directory.path());
    let (config, models, agents) = valid_documents();
    files.save(&config, &models, &agents).expect("initial save");
    let before = fs::read(directory.path().join("config.yaml")).expect("initial config");

    let mut invalid = config.clone();
    invalid.providers[0].endpoint = "http://api.example.com".into();
    let error = files
        .save(&invalid, &models, &agents)
        .expect_err("invalid endpoint must fail");

    assert_eq!(
        error.to_string(),
        "provider endpoint must use HTTPS unless it is loopback: http://api.example.com/"
    );
    assert_eq!(
        fs::read(directory.path().join("config.yaml")).expect("preserved config"),
        before
    );
}

#[test]
fn duplicate_verified_native_protocols_are_rejected() {
    let directory = tempfile::tempdir().expect("temp directory");
    let files = ConfigurationFiles::new(directory.path());
    let (config, mut models, agents) = valid_documents();
    models.models[0].native_protocols =
        Some(vec![NativeProtocol::OpenAiChat, NativeProtocol::OpenAiChat]);

    let error = files
        .save(&config, &models, &agents)
        .expect_err("duplicate protocol must fail");

    assert_eq!(
        error.to_string(),
        "duplicate native protocol for model: local-coder"
    );
}

#[test]
fn an_unwritable_later_target_does_not_partially_commit_earlier_documents() {
    let directory = tempfile::tempdir().expect("temp directory");
    let files = ConfigurationFiles::new(directory.path());
    let original = ConfigurationDocuments::default();
    files
        .save(&original.config, &original.models, &original.agents)
        .expect("initial configuration");
    let config_before = std::fs::read(directory.path().join("config.yaml")).expect("config");
    let models_before = std::fs::read(directory.path().join("models.yaml")).expect("models");
    std::fs::remove_file(directory.path().join("agents.yaml")).expect("remove agents file");
    std::fs::create_dir(directory.path().join("agents.yaml")).expect("blocking directory");

    let error = files
        .save(&original.config, &original.models, &original.agents)
        .expect_err("directory target must fail before commit");

    assert_eq!(error.to_string(), "could not access agents.yaml");
    assert_eq!(
        std::fs::read(directory.path().join("config.yaml")).expect("config unchanged"),
        config_before
    );
    assert_eq!(
        std::fs::read(directory.path().join("models.yaml")).expect("models unchanged"),
        models_before
    );
}

#[test]
fn first_open_creates_only_the_minimal_claude_code_configuration() {
    let directory = tempfile::tempdir().expect("temp directory");
    let files = ConfigurationFiles::new(directory.path());

    let documents = files.open_or_initialize().expect("first open");

    assert!(documents.config.providers.is_empty());
    assert!(documents.models.models.is_empty());
    assert_eq!(documents.agents.agents.len(), 1);
    assert_eq!(documents.agents.agents[0].id, "claude_code");
    for file in ["config.yaml", "models.yaml", "agents.yaml"] {
        assert!(directory.path().join(file).is_file());
    }
}

#[test]
fn partial_configuration_is_not_silently_repaired() {
    let directory = tempfile::tempdir().expect("temp directory");
    fs::write(
        directory.path().join("config.yaml"),
        "version: 2\nproviders: []\n",
    )
    .expect("partial state");
    let files = ConfigurationFiles::new(directory.path());

    let error = files
        .open_or_initialize()
        .expect_err("partial state must fail");

    assert_eq!(error.to_string(), "could not access models.yaml");
    assert!(!directory.path().join("models.yaml").exists());
    assert!(!directory.path().join("agents.yaml").exists());
}

#[cfg(unix)]
#[test]
fn configuration_files_are_user_only() {
    use std::os::unix::fs::PermissionsExt;

    let directory = tempfile::tempdir().expect("temp directory");
    ConfigurationFiles::new(directory.path())
        .open_or_initialize()
        .expect("initialize");

    for file in ["config.yaml", "models.yaml", "agents.yaml"] {
        let mode = fs::metadata(directory.path().join(file))
            .expect("metadata")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600, "{file}");
    }
}
