use grillforge_lib::core::provider::{
    ApiKeyPlacement, Auth, EndpointMode, Protocol, Provider, ProviderDraft, build_request_endpoint,
};
use url::Url;

#[test]
fn remote_http_endpoint_is_rejected_before_provider_is_created() {
    let result = Provider::try_from(ProviderDraft {
        id: "deepseek".into(),
        name: "DeepSeek".into(),
        enabled: true,
        protocol: Protocol::OpenAiChatCompletions,
        endpoint: "http://api.deepseek.com".into(),
        endpoint_mode: EndpointMode::BaseUrl,
        auth: Auth::api_key(ApiKeyPlacement::Bearer, "not-a-real-key"),
        models_url: None,
    });

    let error = result.expect_err("remote HTTP must fail");
    assert_eq!(
        error.to_string(),
        "provider endpoint must use HTTPS unless it is loopback: http://api.deepseek.com/"
    );
}

#[test]
fn provider_id_must_be_a_stable_slug() {
    let result = Provider::try_from(ProviderDraft {
        id: "Deep Seek".into(),
        name: "DeepSeek".into(),
        enabled: true,
        protocol: Protocol::OpenAiChatCompletions,
        endpoint: "https://api.deepseek.com".into(),
        endpoint_mode: EndpointMode::BaseUrl,
        auth: Auth::api_key(ApiKeyPlacement::Bearer, "not-a-real-key"),
        models_url: None,
    });

    assert_eq!(
        result.expect_err("invalid id must fail").to_string(),
        "provider id must be a lowercase slug: Deep Seek"
    );
}

#[test]
fn empty_api_key_is_rejected_without_echoing_it() {
    let result = Provider::try_from(ProviderDraft {
        id: "openai".into(),
        name: "OpenAI".into(),
        enabled: true,
        protocol: Protocol::OpenAiResponses,
        endpoint: "https://api.openai.com".into(),
        endpoint_mode: EndpointMode::BaseUrl,
        auth: Auth::api_key(ApiKeyPlacement::Bearer, "  "),
        models_url: None,
    });

    assert_eq!(
        result.expect_err("blank key must fail").to_string(),
        "provider API key must not be empty"
    );
}

#[test]
fn responses_protocol_rejects_anthropic_header_auth() {
    let result = Provider::try_from(ProviderDraft {
        id: "openai".into(),
        name: "OpenAI".into(),
        enabled: true,
        protocol: Protocol::OpenAiResponses,
        endpoint: "https://api.openai.com".into(),
        endpoint_mode: EndpointMode::BaseUrl,
        auth: Auth::api_key(ApiKeyPlacement::XApiKey, "not-a-real-key"),
        models_url: None,
    });

    assert_eq!(
        result.expect_err("incompatible auth must fail").to_string(),
        "OpenAI-compatible providers require Bearer authentication"
    );
}

#[test]
fn api_key_with_header_injection_is_rejected_and_redacted() {
    let result = Provider::try_from(ProviderDraft {
        id: "anthropic".into(),
        name: "Anthropic".into(),
        enabled: true,
        protocol: Protocol::AnthropicMessages,
        endpoint: "https://api.anthropic.com".into(),
        endpoint_mode: EndpointMode::BaseUrl,
        auth: Auth::api_key(ApiKeyPlacement::XApiKey, "secret\r\ninjected: value"),
        models_url: None,
    });

    let message = result.expect_err("CRLF key must fail").to_string();
    assert_eq!(
        message,
        "provider API key contains invalid header characters"
    );
    assert!(!message.contains("secret"));
}

#[test]
fn endpoint_rejects_embedded_credentials() {
    let result = Provider::try_from(ProviderDraft {
        id: "local".into(),
        name: "Local".into(),
        enabled: true,
        protocol: Protocol::OpenAiChatCompletions,
        endpoint: "http://user:password@localhost:8080".into(),
        endpoint_mode: EndpointMode::BaseUrl,
        auth: Auth::api_key(ApiKeyPlacement::Bearer, "not-a-real-key"),
        models_url: None,
    });

    assert_eq!(
        result.expect_err("URL credentials must fail").to_string(),
        "provider endpoint must not contain credentials or a fragment"
    );
}

#[test]
fn loopback_http_provider_is_valid_and_secret_debug_is_redacted() {
    let provider = Provider::try_from(ProviderDraft {
        id: "ollama".into(),
        name: "Ollama".into(),
        enabled: true,
        protocol: Protocol::OpenAiChatCompletions,
        endpoint: "http://127.0.0.1:11434/v1".into(),
        endpoint_mode: EndpointMode::BaseUrl,
        auth: Auth::api_key(ApiKeyPlacement::Bearer, "local-secret"),
        models_url: None,
    })
    .expect("loopback HTTP is explicitly supported");

    assert_eq!(provider.id(), "ollama");
    assert_eq!(provider.endpoint(), "http://127.0.0.1:11434/v1");
    assert!(!format!("{provider:?}").contains("local-secret"));
}

#[test]
fn endpoint_requires_http_or_https() {
    let result = Provider::try_from(ProviderDraft {
        id: "custom".into(),
        name: "Custom".into(),
        enabled: true,
        protocol: Protocol::AnthropicMessages,
        endpoint: "ftp://models.example.com".into(),
        endpoint_mode: EndpointMode::ExactUrl,
        auth: Auth::api_key(ApiKeyPlacement::Bearer, "not-a-real-key"),
        models_url: None,
    });

    assert_eq!(
        result
            .expect_err("unsupported scheme must fail")
            .to_string(),
        "provider endpoint must use HTTP or HTTPS: ftp://models.example.com/"
    );
}

#[test]
fn provider_name_must_not_be_blank() {
    let result = Provider::try_from(ProviderDraft {
        id: "custom".into(),
        name: "   ".into(),
        enabled: true,
        protocol: Protocol::AnthropicMessages,
        endpoint: "https://models.example.com".into(),
        endpoint_mode: EndpointMode::BaseUrl,
        auth: Auth::api_key(ApiKeyPlacement::Bearer, "not-a-real-key"),
        models_url: None,
    });

    assert_eq!(
        result.expect_err("blank name must fail").to_string(),
        "provider name must not be empty"
    );
}

#[test]
fn base_endpoint_preserves_custom_prefix_and_deduplicates_v1() {
    let prefixed = build_request_endpoint(
        &Url::parse("https://example.com/openai").unwrap(),
        EndpointMode::BaseUrl,
        "/v1/responses",
    )
    .unwrap();
    let versioned = build_request_endpoint(
        &Url::parse("https://example.com/openai/v1").unwrap(),
        EndpointMode::BaseUrl,
        "/v1/responses",
    )
    .unwrap();

    assert_eq!(prefixed.as_str(), "https://example.com/openai/v1/responses");
    assert_eq!(
        versioned.as_str(),
        "https://example.com/openai/v1/responses"
    );
}

#[test]
fn no_auth_is_allowed_only_for_loopback_providers() {
    let local = Provider::try_from(ProviderDraft {
        id: "ollama".into(),
        name: "Ollama".into(),
        enabled: true,
        protocol: Protocol::OpenAiChatCompletions,
        endpoint: "http://localhost:11434/v1".into(),
        endpoint_mode: EndpointMode::BaseUrl,
        auth: Auth::none(),
        models_url: None,
    });
    assert!(local.is_ok());

    let remote = Provider::try_from(ProviderDraft {
        id: "unsafe".into(),
        name: "Unsafe".into(),
        enabled: true,
        protocol: Protocol::OpenAiChatCompletions,
        endpoint: "https://api.example.com/v1".into(),
        endpoint_mode: EndpointMode::BaseUrl,
        auth: Auth::none(),
        models_url: None,
    });
    assert_eq!(
        remote.expect_err("remote no-auth must fail").to_string(),
        "no-auth providers must use a loopback endpoint"
    );
}

#[test]
fn remote_models_url_requires_https() {
    let result = Provider::try_from(ProviderDraft {
        id: "deepseek".into(),
        name: "DeepSeek".into(),
        enabled: true,
        protocol: Protocol::AnthropicMessages,
        endpoint: "https://api.deepseek.com/anthropic".into(),
        endpoint_mode: EndpointMode::BaseUrl,
        auth: Auth::api_key(ApiKeyPlacement::Bearer, "not-a-real-key"),
        models_url: Some("http://api.deepseek.com/models".into()),
    });

    assert_eq!(
        result.expect_err("remote models URL must fail").to_string(),
        "provider models URL must use HTTPS unless it is loopback: http://api.deepseek.com/models"
    );
}
