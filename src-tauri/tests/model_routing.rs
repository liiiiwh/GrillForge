use grillforge_lib::core::routing::{ModelRoute, RouteTable};

#[test]
fn independent_aliases_resolve_to_different_targets() {
    let routes = RouteTable::new([
        ModelRoute::new("grillforge-main", "anthropic", "claude-opus"),
        ModelRoute::new(
            "grillforge-extension-review",
            "deepseek",
            "deepseek-reasoner",
        ),
    ])
    .expect("valid route table");

    let main = routes.resolve("grillforge-main").expect("main route");
    let extension = routes
        .resolve("grillforge-extension-review")
        .expect("extension route");

    assert_eq!(main.provider_id(), "anthropic");
    assert_eq!(main.upstream_model(), "claude-opus");
    assert_eq!(extension.provider_id(), "deepseek");
    assert_eq!(extension.upstream_model(), "deepseek-reasoner");
}

#[test]
fn duplicate_alias_is_rejected_before_routing_starts() {
    let result = RouteTable::new([
        ModelRoute::new("grillforge-extension", "openai", "gpt-worker"),
        ModelRoute::new("grillforge-extension", "deepseek", "deepseek-worker"),
    ]);

    assert!(result.is_err());
}
