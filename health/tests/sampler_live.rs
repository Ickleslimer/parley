use parley_health::sampler::{AppServerSampler, CodexSampler};

#[test]
#[ignore = "requires the installed authenticated Codex app-server"]
fn live_app_server_sample_is_bounded_and_authoritative() {
    let sample = AppServerSampler::new()
        .sample()
        .expect("bounded app-server sample")
        .expect("usage sample");
    assert!(sample.as_of_ms > 0);
    assert!(sample.plan_type.is_some());
    if let Some(percent) = sample.used_percent {
        assert!((0.0..=100.0).contains(&percent));
    }
}
