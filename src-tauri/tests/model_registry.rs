use grillforge_lib::core::model::{Model, ModelDraft, ModelRegistry};

#[test]
fn model_registry_rejects_missing_provider_reference() {
    let model = Model::try_from(ModelDraft {
        id: "gpt-5".into(),
        provider_id: "openai".into(),
        upstream_id: "gpt-5".into(),
        display_name: "GPT-5".into(),
        capabilities: vec!["coding".into(), "reasoning".into()],
        protocol_capabilities: vec![],
    })
    .expect("model draft is valid");

    let error = ModelRegistry::new([model], std::iter::empty::<String>())
        .expect_err("missing provider must fail");
    assert_eq!(
        error.to_string(),
        "model gpt-5 references unknown provider openai"
    );
}

#[test]
fn valid_model_metadata_is_available_from_the_registry() {
    let model = Model::try_from(ModelDraft {
        id: "qwen-coder".into(),
        provider_id: "dashscope".into(),
        upstream_id: "qwen3-coder-plus".into(),
        display_name: "Qwen Coder".into(),
        capabilities: vec!["coding".into(), "review".into()],
        protocol_capabilities: vec![],
    })
    .expect("valid model");

    let registry = ModelRegistry::new([model], ["dashscope"]).expect("known provider");
    let stored = registry.get("qwen-coder").expect("registered model");

    assert_eq!(stored.upstream_id(), "qwen3-coder-plus");
    assert_eq!(stored.display_name(), "Qwen Coder");
    assert_eq!(stored.capabilities(), ["coding", "review"]);
}
