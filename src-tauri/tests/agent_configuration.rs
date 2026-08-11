use grillforge_lib::core::agent::{AgentConfiguration, MainSelection};
use grillforge_lib::core::model::ModelRegistry;

#[test]
fn worker_mode_cannot_be_enabled_with_an_empty_pool() {
    let models =
        ModelRegistry::new([], std::iter::empty::<String>()).expect("empty registry is valid");

    let error = AgentConfiguration::new(
        MainSelection::Native,
        true,
        std::iter::empty::<String>(),
        &models,
    )
    .expect_err("empty worker mode must fail");

    assert_eq!(
        error.to_string(),
        "worker mode requires at least one valid enabled model"
    );
}
