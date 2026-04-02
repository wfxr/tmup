mod utils;

use tempfile::tempdir;
use tmup::config_mode::{ConfigMode, load_from_sources};
use tmup::model::{PluginSource, Tracking};
use utils::write_file;

#[test]
fn config_mode_tmup_loads_only_kdl() {
    let dir = tempdir().unwrap();
    let kdl = dir.path().join("tmup.kdl");
    let tpm = dir.path().join("tmux.conf");
    write_file(&kdl, r#"plugin "tmux-plugins/tmux-sensible""#);
    write_file(&tpm, "set -g @plugin 'tmux-plugins/tmux-yank'\n");

    let loaded = load_from_sources(ConfigMode::Tmup, Some(&kdl), Some(&tpm)).unwrap();

    assert_eq!(loaded.config.plugins.len(), 1);
    assert_eq!(
        loaded.config.plugins[0].remote_id().unwrap(),
        "github.com/tmux-plugins/tmux-sensible"
    );
    assert!(loaded.warnings.is_empty());
}

#[test]
fn config_mode_mixed_merges_tpm_plugins_into_kdl() {
    let dir = tempdir().unwrap();
    let kdl = dir.path().join("tmup.kdl");
    let tpm = dir.path().join("tmux.conf");
    write_file(&kdl, r#"plugin "tmux-plugins/tmux-sensible""#);
    write_file(&tpm, "set -g @plugin 'tmux-plugins/tmux-yank'\n");

    let loaded = load_from_sources(ConfigMode::Mixed, Some(&kdl), Some(&tpm)).unwrap();

    assert_eq!(loaded.config.plugins.len(), 2);
    assert_eq!(loaded.config.plugins[0].remote_id().unwrap(), "github.com/tmux-plugins/tmux-yank");
    assert_eq!(
        loaded.config.plugins[1].remote_id().unwrap(),
        "github.com/tmux-plugins/tmux-sensible"
    );
}

#[test]
fn config_mode_mixed_prefers_kdl_for_duplicate_remote_plugin() {
    let dir = tempdir().unwrap();
    let kdl = dir.path().join("tmup.kdl");
    let tpm = dir.path().join("tmux.conf");
    write_file(&kdl, r#"plugin "tmux-plugins/tmux-sensible" branch="feature""#);
    write_file(&tpm, "set -g @plugin 'tmux-plugins/tmux-sensible'\n");

    let loaded = load_from_sources(ConfigMode::Mixed, Some(&kdl), Some(&tpm)).unwrap();

    assert_eq!(loaded.config.plugins.len(), 1);
    assert!(
        matches!(&loaded.config.plugins[0].tracking, Tracking::Branch(branch) if branch == "feature")
    );
    assert_eq!(loaded.warnings.len(), 1);
    assert!(loaded.warnings[0].contains("github.com/tmux-plugins/tmux-sensible"));
}

#[test]
fn config_mode_mixed_keeps_kdl_local_plugins() {
    let dir = tempdir().unwrap();
    let kdl = dir.path().join("tmup.kdl");
    let tpm = dir.path().join("tmux.conf");
    let local = dir.path().join("local-plugin");
    std::fs::create_dir_all(&local).unwrap();
    write_file(&kdl, &format!(r#"plugin "{}" local=#true"#, local.display()));
    write_file(&tpm, "set -g @plugin 'tmux-plugins/tmux-sensible'\n");

    let loaded = load_from_sources(ConfigMode::Mixed, Some(&kdl), Some(&tpm)).unwrap();

    assert_eq!(loaded.config.plugins.len(), 2);
    assert!(matches!(loaded.config.plugins[1].source, PluginSource::Local { .. }));
}

#[test]
fn config_mode_mixed_requires_kdl_source() {
    let dir = tempdir().unwrap();
    let tpm = dir.path().join("tmux.conf");
    write_file(&tpm, "set -g @plugin 'tmux-plugins/tmux-yank'\n");

    let err = load_from_sources(ConfigMode::Mixed, None, Some(&tpm)).unwrap_err();
    assert!(err.to_string().contains("tmup config file not found"));
}

#[test]
fn config_mode_mixed_allows_missing_tpm_config() {
    let dir = tempdir().unwrap();
    let kdl = dir.path().join("tmup.kdl");
    write_file(&kdl, r#"plugin "tmux-plugins/tmux-sensible""#);

    let loaded = load_from_sources(ConfigMode::Mixed, Some(&kdl), None).unwrap();

    assert_eq!(loaded.config.plugins.len(), 1);
    assert!(loaded.warnings.is_empty());
}

#[test]
fn config_mode_mixed_supports_empty_kdl_with_tpm_plugins() {
    let dir = tempdir().unwrap();
    let kdl = dir.path().join("tmup.kdl");
    let tpm = dir.path().join("tmux.conf");
    write_file(&kdl, "");
    write_file(&tpm, "set -g @plugin 'tmux-plugins/tmux-yank'\n");

    let loaded = load_from_sources(ConfigMode::Mixed, Some(&kdl), Some(&tpm)).unwrap();

    assert_eq!(loaded.config.plugins.len(), 1);
    assert_eq!(loaded.config.plugins[0].remote_id().unwrap(), "github.com/tmux-plugins/tmux-yank");
}

#[test]
fn config_mode_mixed_supports_empty_sources() {
    let dir = tempdir().unwrap();
    let kdl = dir.path().join("tmup.kdl");
    let tpm = dir.path().join("tmux.conf");
    write_file(&kdl, "");
    write_file(&tpm, "");

    let loaded = load_from_sources(ConfigMode::Mixed, Some(&kdl), Some(&tpm)).unwrap();

    assert!(loaded.config.plugins.is_empty());
    assert!(loaded.warnings.is_empty());
}
