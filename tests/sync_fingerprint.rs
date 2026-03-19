use lazytmux::config::parse_config;
use lazytmux::lockfile::{config_fingerprint, remote_plugin_config_hash};

#[test]
fn default_branch_hash_uses_declared_selector_semantics() {
    let default_cfg = parse_config(r#"plugin "user/repo""#).unwrap();
    let branch_cfg = parse_config(r#"plugin "user/repo" branch="main""#).unwrap();

    let default_hash = remote_plugin_config_hash(&default_cfg.plugins[0]).unwrap();
    let default_hash_again = remote_plugin_config_hash(&default_cfg.plugins[0]).unwrap();
    let branch_hash = remote_plugin_config_hash(&branch_cfg.plugins[0]).unwrap();

    assert_eq!(default_hash, default_hash_again);
    assert_ne!(default_hash, branch_hash);
}

#[test]
fn config_fingerprint_ignores_non_lock_affecting_changes() {
    let cfg_a = parse_config(
        r#"
plugin "user/beta" name="beta" opt-prefix="beta_" {
    opt "flavor" "mocha"
}
plugin "https://github.com/user/alpha.git" name="alpha"
plugin "/tmp/local-a" local=#true name="local-a"
"#,
    )
    .unwrap();

    let cfg_b = parse_config(
        r#"
plugin "/tmp/local-b" local=#true name="local-b"
plugin "git@github.com:user/alpha.git" name="renamed-alpha" opt-prefix="ignored_" {
    opt "theme" "light"
}
plugin "https://github.com/user/beta.git"
"#,
    )
    .unwrap();

    assert_eq!(config_fingerprint(&cfg_a), config_fingerprint(&cfg_b));
}

#[test]
fn config_fingerprint_changes_when_build_changes() {
    let cfg_a = parse_config(r#"plugin "user/repo" build="make install""#).unwrap();
    let cfg_b = parse_config(r#"plugin "user/repo" build="just build""#).unwrap();

    assert_ne!(config_fingerprint(&cfg_a), config_fingerprint(&cfg_b));
}
