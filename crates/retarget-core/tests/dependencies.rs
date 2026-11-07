//! Keeps the core free of I/O dependencies.
//!
//! The adapters (telemetry, event store, settings files, git) live in the
//! umbrella crate so that changing one of them never changes the core (see the
//! crate doc). A crate that brings I/O, a runtime or an outside format into
//! `[dependencies]` breaks that, and this test names it.

/// Crates that would bring I/O, an async runtime or an outside format into the core.
const DENIED: &[&str] = &[
    "eventsdb",
    "eventsdb-core",
    "eventsdb-sqlite",
    "rusqlite",
    "sqlx",
    "tokio",
    "hyper",
    "axum",
    "reqwest",
    "serde_json",
    "toml",
    "git2",
    "opentelemetry",
    "opentelemetry-proto",
];

/// Names of the entries of `[dependencies]` in a Cargo.toml, ignoring other tables.
fn dependency_names(manifest: &str) -> Vec<String> {
    let mut names = Vec::new();
    let mut in_dependencies = false;
    for line in manifest.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_dependencies = line == "[dependencies]";
            continue;
        }
        if !in_dependencies || line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some((key, _)) = line.split_once('=') {
            let key = key.trim();
            let name = key.split('.').next().unwrap_or(key);
            names.push(name.trim_matches('"').to_string());
        }
    }
    names
}

#[test]
fn core_has_no_io_dependency() {
    let manifest = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml"))
        .expect("the core's Cargo.toml is readable");
    let found: Vec<String> = dependency_names(&manifest)
        .into_iter()
        .filter(|name| DENIED.contains(&name.as_str()))
        .collect();
    assert!(
        found.is_empty(),
        "retarget-core must not depend on {found:?}; put the adapter in the umbrella crate `retarget`"
    );
}

#[test]
fn reads_only_the_dependencies_table() {
    let manifest = "[package]\nname = \"x\"\n\n[dependencies]\nfoo = \"1\"\nbar.workspace = true\n\n[dev-dependencies]\ntokio = \"1\"\n";
    assert_eq!(dependency_names(manifest), ["foo", "bar"]);
}
