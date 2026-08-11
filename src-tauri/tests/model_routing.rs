use grillforge_lib::core::routing::{ModelRoute, RouteTable};

#[test]
fn main_and_worker_aliases_resolve_to_different_targets() {
    let routes = RouteTable::new([
        ModelRoute::new("grillforge-main", "anthropic", "claude-opus"),
        ModelRoute::new("grillforge-worker-review", "deepseek", "deepseek-reasoner"),
    ])
    .expect("valid route table");

    let main = routes.resolve("grillforge-main").expect("main route");
    let worker = routes
        .resolve("grillforge-worker-review")
        .expect("worker route");

    assert_eq!(main.provider_id(), "anthropic");
    assert_eq!(main.upstream_model(), "claude-opus");
    assert_eq!(worker.provider_id(), "deepseek");
    assert_eq!(worker.upstream_model(), "deepseek-reasoner");
}

#[test]
fn duplicate_alias_is_rejected_before_routing_starts() {
    let result = RouteTable::new([
        ModelRoute::new("grillforge-worker", "openai", "gpt-worker"),
        ModelRoute::new("grillforge-worker", "deepseek", "deepseek-worker"),
    ]);

    assert!(result.is_err());
}
