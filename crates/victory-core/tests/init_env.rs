//! `tunables::init` in a start like `victory play`'s: the registry is looked at
//! before the launcher fills the environment, and init must neither add
//! variables of its own nor lose what the launcher set.

use victory_core::tunables;

fn victory_env() -> Vec<(String, String)> {
    let mut v: Vec<_> = std::env::vars().filter(|(k, _)| k.starts_with("VICTORY_")).collect();
    v.sort();
    v
}

#[test]
fn init_adds_nothing_to_the_environment_and_reads_what_the_launcher_set() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("victory-live.toml");
    // Saved on purpose: one default value (never written as text), one other value.
    std::fs::write(&file, "[tunables]\nthin = false\ncard_steps = \"2\"\n").unwrap();
    // SAFETY: the only test of this process; no other thread reads the environment yet.
    unsafe { std::env::set_var("VICTORY_LIVE_SETTINGS", &file) };
    // The launcher reads the saved values first (this registers the switches) ...
    let saved = tunables::saved_env_values();
    assert!(saved.contains(&("VICTORY_THIN", None)), "{saved:?}");
    assert!(saved.contains(&("VICTORY_CARD_STEPS", Some("2".to_owned()))), "{saved:?}");
    // ... then fills the environment with its presets and --set values.
    // SAFETY: as above.
    unsafe {
        std::env::set_var("VICTORY_RELAY", "2");
        std::env::set_var("VICTORY_CARD_STEPS", "1");
        std::env::set_var("VICTORY_BVH_BUILDER", "1");
    }
    let before = victory_env();
    tunables::init();
    assert_eq!(victory_env(), before, "init changed the environment");
    let source = |name: &str| tunables::get(name).unwrap().to_json().get("source").and_then(|s| s.as_str().map(str::to_owned));
    assert_eq!(source("card_steps").as_deref(), Some("env"));
    assert_eq!(source("bvh_builder").as_deref(), Some("env"));
    assert_eq!(source("relay").as_deref(), Some("env"));
    assert_eq!(tunables::get("card_steps").unwrap().value_text(), "1", "the launcher's value wins over the file");
    // thin: only in the file, as the default, and its variable was not set.
    assert!(std::env::var_os("VICTORY_THIN").is_none());
}
