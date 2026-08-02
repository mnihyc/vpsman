#[test]
fn invalid_hot_reload_keeps_the_last_known_good_suite_config() {
    let path = std::env::temp_dir().join(format!(
        "vpsman-suite-config-last-known-good-{}.toml",
        uuid::Uuid::new_v4()
    ));
    std::fs::write(&path, "version = 1\n\n[capacity]\ndispatcher_batch = 17\n").unwrap();
    let initial = super::load_suite_config_last_known_good(&path).unwrap();
    assert_eq!(initial.capacity.dispatcher_batch, Some(17));

    std::fs::remove_file(&path).unwrap();
    let missing_fallback = super::load_suite_config_last_known_good(&path).unwrap();
    assert_eq!(missing_fallback.capacity.dispatcher_batch, Some(17));

    std::fs::write(&path, "version = 1\n\n[capacity\n").unwrap();
    let fallback = super::load_suite_config_last_known_good(&path).unwrap();
    assert_eq!(fallback.capacity.dispatcher_batch, Some(17));

    let _ = std::fs::remove_file(path);
}
