use grillforge_lib::presets::{
    ClientCompatibilityMode, ExclusionReason, PresetAuth, PresetClient, PresetEndpoint,
    PresetProtocol, catalog,
};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::PathBuf;
use std::process::Command;

const PINNED_COMMIT: &str = "413c09e0790c304506888ae24b9be72820aca126";
const SOURCES: [(&str, &str, usize); 3] = [
    (
        "src/config/claudeProviderPresets.ts",
        "9ad11d9e97993c84",
        72,
    ),
    ("src/config/codexProviderPresets.ts", "efc4ae7add747ef5", 67),
    (
        "src/config/geminiProviderPresets.ts",
        "306c8783d24feef9",
        22,
    ),
];
const RAW_CATALOG: &str = include_str!("../src/presets/catalog.json");

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("src-tauri has a workspace parent")
        .to_path_buf()
}

fn fnv1a64(bytes: &[u8]) -> String {
    let mut hash = 0xcbf29ce484222325_u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{hash:016x}")
}

fn is_slug(value: &str) -> bool {
    !value.is_empty()
        && !value.starts_with('-')
        && !value.ends_with('-')
        && value.split('-').all(|part| {
            !part.is_empty()
                && part
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
        })
}

#[test]
fn catalog_is_reproducible_from_the_pinned_source() {
    let catalog = catalog().expect("checked-in catalog must parse");
    let upstream = workspace_root().join("upstream/cc-switch");
    let actual_commit = Command::new("git")
        .args([
            "-C",
            upstream.to_str().expect("UTF-8 path"),
            "rev-parse",
            "HEAD",
        ])
        .output()
        .expect("git must read the pinned clone");

    assert!(actual_commit.status.success());
    assert_eq!(
        String::from_utf8(actual_commit.stdout).unwrap().trim(),
        PINNED_COMMIT
    );
    assert_eq!(catalog.source.commit, PINNED_COMMIT);
    assert_eq!(catalog.source.provider_count, 161);
    assert_eq!(catalog.source.files.len(), SOURCES.len());
    for (path, expected_hash, expected_count) in SOURCES {
        let source = catalog
            .source
            .files
            .iter()
            .find(|source| source.file == path)
            .unwrap_or_else(|| panic!("missing catalog source: {path}"));
        assert_eq!(source.fnv1a64, expected_hash);
        assert_eq!(source.provider_count, expected_count);
        assert_eq!(
            fnv1a64(&fs::read(upstream.join(path)).expect("pinned preset source must exist")),
            expected_hash
        );
    }

    let extraction = Command::new("node")
        .args(["scripts/extract_cc_switch_presets.mjs", "--check"])
        .current_dir(workspace_root())
        .output()
        .expect("Node must run the static extraction check");
    assert!(
        extraction.status.success(),
        "preset extraction is not reproducible: {}",
        String::from_utf8_lossy(&extraction.stderr)
    );
}

#[test]
fn catalog_has_only_stable_unique_mvp_presets() {
    let catalog = catalog().expect("checked-in catalog must parse");
    let mut ids = HashSet::new();
    let mut protocol_counts = HashMap::new();
    let mut auth_counts = HashMap::new();

    assert_eq!(catalog.schema_version, 2);
    assert_eq!(catalog.presets.len(), 151);
    for preset in &catalog.presets {
        assert!(is_slug(&preset.id), "invalid stable id: {}", preset.id);
        assert!(ids.insert(&preset.id), "duplicate preset id: {}", preset.id);
        if preset.auth == PresetAuth::XApiKey {
            assert!(matches!(
                preset.protocol,
                PresetProtocol::AnthropicMessages | PresetProtocol::GeminiNative
            ));
        }
        *protocol_counts.entry(preset.protocol).or_insert(0usize) += 1;
        *auth_counts.entry(preset.auth).or_insert(0usize) += 1;

        assert_eq!(preset.client_compatibility.len(), 3);
        for client in [
            PresetClient::ClaudeCode,
            PresetClient::Codex,
            PresetClient::Gemini,
        ] {
            let route = compatibility(preset, client);
            match route.mode {
                ClientCompatibilityMode::Unsupported => {
                    assert_eq!(route.protocol, None);
                    assert_eq!(route.auth, None);
                    assert_eq!(route.endpoint, None);
                    assert!(route.suggested_models.is_empty());
                }
                ClientCompatibilityMode::Direct | ClientCompatibilityMode::LocalRoute => {
                    assert!(route.protocol.is_some());
                    assert!(route.auth.is_some());
                    assert!(route.endpoint.is_some());
                }
            }
        }
        for (model, capabilities) in &preset.model_protocol_capabilities {
            assert!(
                preset
                    .client_compatibility
                    .values()
                    .any(|route| route.suggested_models.contains(model)),
                "{} has capability metadata for an unlisted model: {model}",
                preset.name
            );
            let unique: HashSet<_> = capabilities.iter().collect();
            assert_eq!(unique.len(), capabilities.len());
        }
    }

    assert_eq!(
        protocol_counts.get(&PresetProtocol::AnthropicMessages),
        Some(&63)
    );
    assert_eq!(
        protocol_counts.get(&PresetProtocol::OpenAiChatCompletions),
        Some(&18)
    );
    assert_eq!(
        protocol_counts.get(&PresetProtocol::OpenAiResponses),
        Some(&49)
    );
    assert_eq!(
        protocol_counts.get(&PresetProtocol::GeminiNative),
        Some(&21)
    );
    assert_eq!(auth_counts.get(&PresetAuth::Bearer), Some(&129));
    assert_eq!(auth_counts.get(&PresetAuth::XApiKey), Some(&22));
}

#[test]
fn compatibility_modes_match_cc_switch_proxy_boundaries() {
    let catalog = catalog().expect("checked-in catalog must parse");
    for preset in &catalog.presets {
        let claude = compatibility(preset, PresetClient::ClaudeCode);
        assert_eq!(
            claude.mode,
            if claude.protocol == Some(PresetProtocol::AnthropicMessages) {
                ClientCompatibilityMode::Direct
            } else {
                ClientCompatibilityMode::LocalRoute
            },
            "Claude route mismatch for {}",
            preset.name
        );

        let codex = compatibility(preset, PresetClient::Codex);
        assert_eq!(
            codex.mode,
            match preset.protocol {
                PresetProtocol::OpenAiResponses => ClientCompatibilityMode::Direct,
                PresetProtocol::AnthropicMessages | PresetProtocol::OpenAiChatCompletions =>
                    ClientCompatibilityMode::LocalRoute,
                PresetProtocol::GeminiNative => ClientCompatibilityMode::Unsupported,
            },
            "Codex route mismatch for {}",
            preset.name
        );

        let gemini = compatibility(preset, PresetClient::Gemini);
        assert!(matches!(
            (gemini.mode, gemini.protocol),
            (
                ClientCompatibilityMode::Direct,
                Some(PresetProtocol::GeminiNative)
            ) | (ClientCompatibilityMode::Unsupported, None)
        ));
    }
}

#[test]
fn catalog_is_the_supported_name_union_with_deterministic_canonical_protocols() {
    let catalog = catalog().expect("checked-in catalog must parse");
    for (id, protocol) in [
        ("tencent-hunyuan", PresetProtocol::OpenAiResponses),
        ("azure-openai", PresetProtocol::OpenAiResponses),
        ("gemini-native", PresetProtocol::GeminiNative),
        ("apinebula", PresetProtocol::OpenAiResponses),
        ("kimi", PresetProtocol::AnthropicMessages),
    ] {
        let preset = catalog
            .presets
            .iter()
            .find(|preset| preset.id == id)
            .unwrap_or_else(|| panic!("missing supported union preset: {id}"));
        assert_eq!(preset.protocol, protocol, "canonical protocol for {id}");
    }
}

fn compatibility(
    preset: &grillforge_lib::presets::ProviderPreset,
    client: PresetClient,
) -> &grillforge_lib::presets::PresetClientCompatibility {
    preset
        .client_compatibility
        .get(&client)
        .unwrap_or_else(|| panic!("{} must declare compatibility for {client:?}", preset.name))
}

#[test]
fn compatibility_uses_each_clients_explicit_cc_switch_route() {
    let catalog = catalog().expect("checked-in catalog must parse");
    let kimi = catalog
        .presets
        .iter()
        .find(|preset| preset.id == "kimi")
        .expect("Kimi preset");

    let claude = compatibility(kimi, PresetClient::ClaudeCode);
    assert_eq!(claude.mode, ClientCompatibilityMode::Direct);
    assert_eq!(claude.protocol, Some(PresetProtocol::AnthropicMessages));
    assert!(matches!(
        &claude.endpoint,
        Some(PresetEndpoint::Literal { url }) if url == "https://api.moonshot.cn/anthropic"
    ));

    let codex = compatibility(kimi, PresetClient::Codex);
    assert_eq!(codex.mode, ClientCompatibilityMode::LocalRoute);
    assert_eq!(codex.protocol, Some(PresetProtocol::AnthropicMessages));
    assert!(matches!(
        &codex.endpoint,
        Some(PresetEndpoint::Literal { url }) if url == "https://api.moonshot.cn/anthropic"
    ));

    let gemini = compatibility(kimi, PresetClient::Gemini);
    assert_eq!(gemini.mode, ClientCompatibilityMode::Unsupported);
    assert_eq!(gemini.protocol, None);
    assert_eq!(gemini.endpoint, None);

    let chat = catalog
        .presets
        .iter()
        .find(|preset| preset.id == "kimi-chat")
        .expect("Kimi Chat protocol variant");
    assert_eq!(chat.protocol, PresetProtocol::OpenAiChatCompletions);
    assert!(matches!(
        &chat.endpoint,
        PresetEndpoint::Literal { url } if url == "https://api.moonshot.cn/v1"
    ));
}

#[test]
fn provider_with_three_native_surfaces_keeps_stable_protocol_variants() {
    let catalog = catalog().expect("checked-in catalog must parse");
    let preset = catalog
        .presets
        .iter()
        .find(|preset| preset.id == "apinebula")
        .expect("APINebula preset");

    assert_eq!(preset.protocol, PresetProtocol::OpenAiResponses);
    assert_eq!(
        compatibility(preset, PresetClient::Codex).mode,
        ClientCompatibilityMode::Direct
    );

    let anthropic = catalog
        .presets
        .iter()
        .find(|preset| preset.id == "apinebula-anthropic")
        .expect("APINebula Anthropic variant");
    assert_eq!(anthropic.protocol, PresetProtocol::AnthropicMessages);
    let gemini = catalog
        .presets
        .iter()
        .find(|preset| preset.id == "apinebula-gemini")
        .expect("APINebula Gemini variant");
    assert_eq!(gemini.protocol, PresetProtocol::GeminiNative);
    assert_eq!(
        compatibility(gemini, PresetClient::Gemini).mode,
        ClientCompatibilityMode::Direct
    );
}

#[test]
fn codex_uses_the_explicit_local_anthropic_route_when_no_codex_preset_exists() {
    let catalog = catalog().expect("checked-in catalog must parse");
    let preset = catalog
        .presets
        .iter()
        .find(|preset| preset.id == "kat-coder")
        .expect("KAT-Coder preset");

    let codex = compatibility(preset, PresetClient::Codex);
    assert_eq!(codex.mode, ClientCompatibilityMode::LocalRoute);
    assert_eq!(codex.protocol, Some(PresetProtocol::AnthropicMessages));
    assert!(codex.endpoint.is_some());
}

#[test]
fn deepseek_keeps_both_native_protocol_routes_without_catalog_fallback() {
    let catalog = catalog().expect("checked-in catalog must parse");
    let deepseek = catalog
        .presets
        .iter()
        .find(|preset| preset.id == "deepseek")
        .expect("DeepSeek preset");

    assert_eq!(deepseek.protocol, PresetProtocol::OpenAiResponses);
    let codex = compatibility(deepseek, PresetClient::Codex);
    assert_eq!(codex.mode, ClientCompatibilityMode::Direct);
    assert_eq!(codex.protocol, Some(PresetProtocol::OpenAiResponses));
    assert!(matches!(
        &codex.endpoint,
        Some(PresetEndpoint::Literal { url }) if url == "https://api.deepseek.com"
    ));

    assert!(
        codex
            .suggested_models
            .iter()
            .any(|model| model == "deepseek-v4-pro")
    );
    assert!(
        codex
            .suggested_models
            .iter()
            .any(|model| model == "deepseek-v4-flash")
    );

    let anthropic = catalog
        .presets
        .iter()
        .find(|preset| preset.id == "deepseek-anthropic")
        .expect("DeepSeek Anthropic variant");
    let claude = compatibility(anthropic, PresetClient::ClaudeCode);
    assert_eq!(claude.mode, ClientCompatibilityMode::Direct);
    assert_eq!(claude.protocol, Some(PresetProtocol::AnthropicMessages));
    assert!(matches!(
        &claude.endpoint,
        Some(PresetEndpoint::Literal { url }) if url == "https://api.deepseek.com/anthropic"
    ));
}

#[test]
fn chat_reasoning_capabilities_come_only_from_explicit_cc_switch_metadata() {
    let catalog = catalog().expect("checked-in catalog must parse");
    let nvidia = catalog
        .presets
        .iter()
        .find(|preset| preset.id == "nvidia-chat")
        .expect("Nvidia Codex Chat variant");
    assert_eq!(
        nvidia.model_protocol_capabilities["moonshotai/kimi-k2.5"],
        vec![grillforge_lib::core::model::ProtocolCapability::ReasoningContent]
    );

    let opencode = catalog
        .presets
        .iter()
        .find(|preset| preset.id == "opencode-go")
        .expect("OpenCode Go preset");
    assert!(opencode.model_protocol_capabilities.is_empty());
}

#[test]
fn deepseek_uses_the_codex_responses_endpoint_from_cc_switch() {
    let catalog = catalog().expect("checked-in catalog must parse");
    let deepseek = catalog
        .presets
        .iter()
        .find(|preset| preset.id == "deepseek")
        .expect("DeepSeek preset");

    assert_eq!(deepseek.protocol, PresetProtocol::OpenAiResponses);
    assert_eq!(deepseek.auth, PresetAuth::Bearer);
    assert!(matches!(
        &deepseek.endpoint,
        PresetEndpoint::Literal { url } if url == "https://api.deepseek.com"
    ));
    assert!(
        deepseek
            .suggested_models
            .iter()
            .any(|model| model == "deepseek-v4-flash")
    );
    assert_eq!(
        deepseek.model_protocol_capabilities["deepseek-v4-flash"],
        vec![grillforge_lib::core::model::ProtocolCapability::ReasoningItems]
    );
}

#[test]
fn exclusions_are_explicit_and_complete() {
    let catalog = catalog().expect("checked-in catalog must parse");
    let mut reasons = HashMap::new();
    for excluded in &catalog.exclusions {
        *reasons.entry(excluded.reason).or_insert(0usize) += 1;
    }

    assert_eq!(catalog.exclusions.len(), 10);
    assert_eq!(reasons.get(&ExclusionReason::NativeDefault), Some(&3));
    assert_eq!(
        reasons.get(&ExclusionReason::BedrockRequiresAgentSpecificAuth),
        Some(&2)
    );
    assert_eq!(reasons.get(&ExclusionReason::ManagedOauth), Some(&4));
    assert_eq!(reasons.get(&ExclusionReason::CustomTemplate), Some(&1));

    let names: HashSet<_> = catalog
        .exclusions
        .iter()
        .map(|item| item.name.as_str())
        .collect();
    for expected in [
        "Claude Official",
        "OpenAI Official",
        "Google Official",
        "GitHub Copilot",
        "Codex",
        "xAI (Grok)",
        "xAI (Grok) OAuth",
        "AWS Bedrock (AKSK)",
        "AWS Bedrock (API Key)",
        "自定义",
    ] {
        assert!(names.contains(expected), "missing exclusion: {expected}");
    }

    assert!(
        catalog.presets.iter().any(|preset| preset.id == "xai-grok"),
        "the API-key xAI route remains available even though Claude's managed OAuth route is excluded"
    );
}

#[test]
fn catalog_contains_no_agent_env_or_commercial_metadata() {
    for forbidden in [
        "ANTHROPIC_",
        "CLAUDE_CODE_",
        "settingsConfig",
        "websiteUrl",
        "apiKeyUrl",
        "isPartner",
        "primePartner",
        "partnerPromotionKey",
        "endpointCandidates",
        "inFailoverQueue",
        "failover",
        "iconColor",
        "\"icon\"",
        "\"category\"",
        "aff=",
        "${",
    ] {
        assert!(
            !RAW_CATALOG.contains(forbidden),
            "catalog leaked forbidden field or value: {forbidden}"
        );
    }
}

#[test]
fn every_endpoint_is_literal_or_has_fully_declared_parameters() {
    let catalog = catalog().expect("checked-in catalog must parse");
    let parameterized: Vec<_> = catalog
        .presets
        .iter()
        .filter(|preset| matches!(preset.endpoint, PresetEndpoint::Parameterized { .. }))
        .collect();

    assert_eq!(parameterized.len(), 1);
    assert_eq!(parameterized[0].name, "KAT-Coder");

    for preset in &catalog.presets {
        if let Some(models_url) = &preset.models_url {
            url::Url::parse(models_url).expect("models URL must be absolute");
        }
        match &preset.endpoint {
            PresetEndpoint::Literal { url } => {
                assert!(!url.contains('{') && !url.contains('}'));
                url::Url::parse(url).expect("literal endpoint must be an absolute URL");
            }
            PresetEndpoint::Parameterized {
                template,
                parameters,
            } => {
                assert!(!parameters.is_empty());
                for parameter in parameters {
                    assert!(parameter.required);
                    assert!(is_slug(&parameter.id.replace('_', "-")));
                    assert!(template.contains(&format!("{{{}}}", parameter.id)));
                }

                let mut remainder = template.clone();
                for parameter in parameters {
                    remainder = remainder.replace(&format!("{{{}}}", parameter.id), "value");
                }
                assert!(
                    !remainder.contains(['{', '}']),
                    "undeclared endpoint template token: {template}"
                );
                url::Url::parse(&remainder).expect("resolved endpoint template must be a URL");
            }
        }
    }
}
