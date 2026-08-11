use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

use crate::core::model::ProtocolCapability;

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct ProviderPresetCatalog {
    pub schema_version: u32,
    pub source: CatalogSource,
    pub exclusions: Vec<ExcludedPreset>,
    pub presets: Vec<ProviderPreset>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct CatalogSource {
    pub repository: String,
    pub commit: String,
    pub provider_count: usize,
    pub files: Vec<CatalogSourceFile>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct CatalogSourceFile {
    pub file: String,
    pub fnv1a64: String,
    pub provider_count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct ExcludedPreset {
    pub client: PresetClient,
    pub name: String,
    pub reason: ExclusionReason,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ExclusionReason {
    NativeDefault,
    BedrockRequiresAgentSpecificAuth,
    ManagedOauth,
    CustomTemplate,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct ProviderPreset {
    pub id: String,
    pub name: String,
    pub protocol: PresetProtocol,
    pub auth: PresetAuth,
    pub endpoint: PresetEndpoint,
    pub suggested_models: Vec<String>,
    #[serde(default)]
    pub model_protocol_capabilities: BTreeMap<String, Vec<ProtocolCapability>>,
    pub models_url: Option<String>,
    pub client_compatibility: BTreeMap<PresetClient, PresetClientCompatibility>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PresetClient {
    ClaudeCode,
    Codex,
    Gemini,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct PresetClientCompatibility {
    pub mode: ClientCompatibilityMode,
    pub protocol: Option<PresetProtocol>,
    pub auth: Option<PresetAuth>,
    pub endpoint: Option<PresetEndpoint>,
    pub suggested_models: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ClientCompatibilityMode {
    Direct,
    LocalRoute,
    Unsupported,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PresetProtocol {
    AnthropicMessages,
    #[serde(rename = "openai_responses")]
    OpenAiResponses,
    #[serde(rename = "openai_chat_completions")]
    OpenAiChatCompletions,
    GeminiNative,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PresetAuth {
    Bearer,
    XApiKey,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PresetEndpoint {
    Literal {
        url: String,
    },
    Parameterized {
        template: String,
        parameters: Vec<PresetParameter>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct PresetParameter {
    pub id: String,
    pub label: String,
    pub placeholder: String,
    pub required: bool,
    pub default_value: Option<String>,
}

pub fn catalog() -> Result<ProviderPresetCatalog, serde_json::Error> {
    serde_json::from_str(include_str!("catalog.json"))
}

#[tauri::command]
pub fn provider_presets() -> Result<ProviderPresetCatalog, String> {
    catalog().map_err(|_| "built-in Provider catalog is invalid".to_string())
}
