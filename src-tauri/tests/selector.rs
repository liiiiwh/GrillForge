use grillforge_lib::configuration::{
    AgentRecord, AgentsDocument, ConfigDocument, ConfigurationFiles, MainRecord, ModelRecord,
    ModelsDocument, ProviderRecord,
};
use grillforge_lib::core::provider::{ApiKeyPlacement, EndpointMode, Protocol};
use grillforge_lib::selector;
use std::ffi::OsString;
use std::process::Command;

fn provider(id: &str, secret: &str) -> ProviderRecord {
    ProviderRecord {
        id: id.into(),
        name: id.into(),
        enabled: true,
        protocol: Protocol::AnthropicMessages,
        endpoint: format!("https://{id}.example.com/v1"),
        endpoint_mode: EndpointMode::BaseUrl,
        api_key_placement: ApiKeyPlacement::XApiKey,
        api_key: secret.into(),
        models_url: None,
    }
}

fn model(id: &str, provider_id: &str, capabilities: &[&str]) -> ModelRecord {
    ModelRecord {
        id: id.into(),
        provider_id: provider_id.into(),
        upstream_id: format!("upstream/{id}"),
        display_name: format!("Model {id}"),
        capabilities: capabilities.iter().map(|value| (*value).into()).collect(),
        protocol_capabilities: vec![],
    }
}

#[test]
fn selector_returns_stably_sorted_credential_free_effective_workers() {
    let root = tempfile::tempdir().unwrap();
    ConfigurationFiles::new(root.path())
        .save(
            &ConfigDocument {
                version: 1,
                providers: vec![
                    provider("second", "secret-second"),
                    provider("first", "secret-first"),
                ],
            },
            &ModelsDocument {
                version: 1,
                models: vec![
                    model("zeta", "second", &["review"]),
                    model("alpha", "first", &["coding", "reasoning"]),
                ],
            },
            &AgentsDocument {
                version: 1,
                agents: vec![AgentRecord {
                    id: "claude_code".into(),
                    adapter: "claude_code".into(),
                    enabled: true,
                    main: MainRecord::Native,
                    model_slots: Default::default(),
                    native_model_slots: Default::default(),
                    worker_mode: true,
                    enabled_workers: vec!["zeta".into(), "alpha".into()],
                    native_subagent_enabled: false,
                    subagents: vec![],
                }],
            },
        )
        .unwrap();
    std::fs::write(root.path().join("claude-code.snapshot.json"), "active").unwrap();

    let output = selector::select(root.path()).unwrap();
    let json = serde_json::to_value(output).unwrap();

    assert_eq!(
        json,
        serde_json::json!({
            "workers": [
                {
                    "modelId": "alpha",
                    "displayName": "Model alpha",
                    "capabilities": ["coding", "reasoning"],
                    "agentName": "grillforge-worker-alpha",
                    "routeAlias": "grillforge/alpha",
                    "providerId": "first",
                    "upstreamId": "upstream/alpha"
                },
                {
                    "modelId": "zeta",
                    "displayName": "Model zeta",
                    "capabilities": ["review"],
                    "agentName": "grillforge-worker-zeta",
                    "routeAlias": "grillforge/zeta",
                    "providerId": "second",
                    "upstreamId": "upstream/zeta"
                }
            ]
        })
    );
    let rendered = json.to_string();
    assert!(!rendered.contains("secret-first"));
    assert!(!rendered.contains("secret-second"));
}

#[test]
fn selector_returns_an_empty_pool_without_an_active_adapter_snapshot() {
    let root = tempfile::tempdir().unwrap();
    ConfigurationFiles::new(root.path())
        .save(
            &ConfigDocument {
                version: 1,
                providers: vec![provider("first", "secret")],
            },
            &ModelsDocument {
                version: 1,
                models: vec![model("alpha", "first", &["coding"])],
            },
            &AgentsDocument {
                version: 1,
                agents: vec![AgentRecord {
                    id: "claude_code".into(),
                    adapter: "claude_code".into(),
                    enabled: true,
                    main: MainRecord::Native,
                    model_slots: Default::default(),
                    native_model_slots: Default::default(),
                    worker_mode: true,
                    enabled_workers: vec!["alpha".into()],
                    native_subagent_enabled: false,
                    subagents: vec![],
                }],
            },
        )
        .unwrap();

    let output = selector::select(root.path()).unwrap();

    assert!(output.workers.is_empty());
}

#[test]
fn selector_cli_prints_exactly_one_json_document() {
    let root = tempfile::tempdir().unwrap();
    let documents = grillforge_lib::configuration::ConfigurationDocuments::default();
    ConfigurationFiles::new(root.path())
        .save(&documents.config, &documents.models, &documents.agents)
        .unwrap();
    let args = vec![
        OsString::from("selector"),
        OsString::from("--config-dir"),
        root.path().as_os_str().to_owned(),
    ];

    let output = selector::run_cli(args).unwrap();

    assert_eq!(output, Some("{\"workers\":[]}".to_string()));
    assert!(serde_json::from_str::<serde_json::Value>(output.as_deref().unwrap()).is_ok());
}

#[test]
fn selector_returns_an_empty_pool_when_worker_mode_is_off() {
    let root = tempfile::tempdir().unwrap();
    ConfigurationFiles::new(root.path())
        .save(
            &ConfigDocument {
                version: 1,
                providers: vec![provider("first", "secret")],
            },
            &ModelsDocument {
                version: 1,
                models: vec![model("alpha", "first", &["coding"])],
            },
            &AgentsDocument {
                version: 1,
                agents: vec![AgentRecord {
                    id: "claude_code".into(),
                    adapter: "claude_code".into(),
                    enabled: true,
                    main: MainRecord::Native,
                    model_slots: Default::default(),
                    native_model_slots: Default::default(),
                    worker_mode: false,
                    enabled_workers: vec!["alpha".into()],
                    native_subagent_enabled: false,
                    subagents: vec![],
                }],
            },
        )
        .unwrap();

    let output = selector::select(root.path()).unwrap();

    assert!(output.workers.is_empty());
}

#[test]
fn selector_returns_an_empty_pool_when_claude_code_is_disabled() {
    let root = tempfile::tempdir().unwrap();
    ConfigurationFiles::new(root.path())
        .save(
            &ConfigDocument {
                version: 1,
                providers: vec![provider("first", "secret")],
            },
            &ModelsDocument {
                version: 1,
                models: vec![model("alpha", "first", &["coding"])],
            },
            &AgentsDocument {
                version: 1,
                agents: vec![AgentRecord {
                    id: "claude_code".into(),
                    adapter: "claude_code".into(),
                    enabled: false,
                    main: MainRecord::Native,
                    model_slots: Default::default(),
                    native_model_slots: Default::default(),
                    worker_mode: true,
                    enabled_workers: vec!["alpha".into()],
                    native_subagent_enabled: false,
                    subagents: vec![],
                }],
            },
        )
        .unwrap();

    let output = selector::select(root.path()).unwrap();

    assert!(output.workers.is_empty());
}

#[test]
fn selector_surfaces_configuration_errors_instead_of_returning_native_fallback() {
    let root = tempfile::tempdir().unwrap();
    let documents = grillforge_lib::configuration::ConfigurationDocuments::default();
    ConfigurationFiles::new(root.path())
        .save(&documents.config, &documents.models, &documents.agents)
        .unwrap();
    std::fs::write(
        root.path().join("config.yaml"),
        "version: 2\nproviders: []\n",
    )
    .unwrap();

    let error = selector::select(root.path()).unwrap_err();

    assert_eq!(error.to_string(), "unsupported config.yaml version: 2");
}

#[test]
fn selector_rejects_configuration_without_claude_code() {
    let root = tempfile::tempdir().unwrap();
    ConfigurationFiles::new(root.path())
        .save(
            &ConfigDocument {
                version: 1,
                providers: vec![],
            },
            &ModelsDocument {
                version: 1,
                models: vec![],
            },
            &AgentsDocument {
                version: 1,
                agents: vec![],
            },
        )
        .unwrap();

    let error = selector::select(root.path()).unwrap_err();

    assert_eq!(error.to_string(), "agents.yaml is missing claude_code");
}

#[test]
fn selector_binary_writes_only_the_json_document_to_stdout() {
    let root = tempfile::tempdir().unwrap();
    let documents = grillforge_lib::configuration::ConfigurationDocuments::default();
    ConfigurationFiles::new(root.path())
        .save(&documents.config, &documents.models, &documents.agents)
        .unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_grillforge"))
        .args(["selector", "--config-dir"])
        .arg(root.path())
        .output()
        .unwrap();

    assert!(output.status.success());
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        "{\"workers\":[]}\n"
    );
    assert!(output.stderr.is_empty());
}

#[test]
fn selector_binary_exits_nonzero_on_invalid_configuration() {
    let root = tempfile::tempdir().unwrap();
    let documents = grillforge_lib::configuration::ConfigurationDocuments::default();
    ConfigurationFiles::new(root.path())
        .save(&documents.config, &documents.models, &documents.agents)
        .unwrap();
    std::fs::write(root.path().join("models.yaml"), "version: 9\nmodels: []\n").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_grillforge"))
        .args(["selector", "--config-dir"])
        .arg(root.path())
        .output()
        .unwrap();

    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert_eq!(
        String::from_utf8(output.stderr).unwrap(),
        "unsupported models.yaml version: 9\n"
    );
}
