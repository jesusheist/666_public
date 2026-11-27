//! `init` with `VICTORY_LOG_DIR`: the files get their subjects (own process,
//! so the global subscriber is ours).

#[test]
fn log_dir_gets_subject_files() {
    let dir = tempfile::tempdir().unwrap();
    // SAFETY: single test in this process, nothing else reads the environment meanwhile.
    unsafe {
        std::env::set_var("VICTORY_LOG_DIR", dir.path());
        std::env::set_var("VICTORY_LOG", "warn");
        std::env::set_var("VICTORY_LOG_SUBJECTS", "memory=info");
    }
    victory_core::logging::init("warn");
    victory_core::logging::set_frame_source(|| 77);
    assert!(victory_core::logging::files_on());
    tracing::info!(target: "victory::memory", "evicted {} textures", 3);
    tracing::info!("not wanted: info outside any subject");
    tracing::warn!(target: "victory_gpu::draw", slot = 9, "a texture picked by index changed after its upload");
    tracing::error!(target: "victory_gpu::host", "when the host device went: x");
    for n in 0..50 {
        tracing::warn!(target: "victory_gpu::draw", slot = n, "a texture picked by index changed after its upload");
    }
    victory_core::logging::publish_perf(victory_core::logging::PerfSample { fps: 60.0, ..Default::default() });
    victory_core::logging::publish_memory(victory_core::logging::MemSample::default());
    victory_core::logging::device_removed();
    let read = |n: &str| std::fs::read_to_string(dir.path().join(n)).unwrap();
    let memory = read("memory.log");
    assert!(memory.contains("INFO  victory::memory: evicted 3 textures"), "{memory}");
    assert!(memory.contains("f=77"), "{memory}");
    assert!(read("gpu-crash.log").contains("when the host device went"));
    let textures = read("textures.log");
    assert_eq!(textures.lines().count(), 5, "{textures}");
    assert!(read("perf.log").contains("fps 60.0"));
    let all = read("all.log");
    assert!(!all.contains("not wanted"), "{all}");
    assert!(all.contains("evicted 3 textures"));
}
