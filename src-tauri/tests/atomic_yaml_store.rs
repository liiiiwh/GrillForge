use grillforge_lib::storage::YamlStore;
use serde::{Deserialize, Serialize};
use std::fs;

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
struct Settings {
    worker_mode: bool,
}

struct BrokenSettings;

impl Serialize for BrokenSettings {
    fn serialize<S>(&self, _serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        Err(serde::ser::Error::custom(
            "deliberate serialization failure",
        ))
    }
}

#[test]
fn failed_write_preserves_the_previous_valid_yaml() {
    let directory = tempfile::tempdir().expect("temp directory");
    let path = directory.path().join("config.yaml");
    let store = YamlStore::new(&path);

    store
        .write(&Settings { worker_mode: true })
        .expect("initial write");
    let before = fs::read(&path).expect("stored YAML");

    let error = store
        .write(&BrokenSettings)
        .expect_err("serialization must fail");

    assert_eq!(error.to_string(), "could not serialize config.yaml");
    assert_eq!(fs::read(&path).expect("preserved YAML"), before);
    assert_eq!(
        store.read::<Settings>().expect("valid state remains"),
        Settings { worker_mode: true }
    );
}
