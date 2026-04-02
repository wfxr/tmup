mod utils;

use tempfile::tempdir;
use tmup::config_tpm::load_config_from_path;
use tmup::model::Tracking;
use utils::write_file;

#[test]
fn config_tpm_parses_single_plugin_declaration() {
    let dir = tempdir().unwrap();
    let tmux_conf = dir.path().join("tmux.conf");
    write_file(&tmux_conf, "set -g @plugin 'tmux-plugins/tmux-sensible'\n");

    let cfg = load_config_from_path(&tmux_conf).unwrap();

    assert_eq!(cfg.plugins.len(), 1);
    assert_eq!(cfg.plugins[0].remote_id().unwrap(), "github.com/tmux-plugins/tmux-sensible");
    assert!(matches!(cfg.plugins[0].tracking, Tracking::DefaultBranch));
}

#[test]
fn config_tpm_accepts_set_option_form() {
    let dir = tempdir().unwrap();
    let tmux_conf = dir.path().join("tmux.conf");
    write_file(&tmux_conf, "set-option -g @plugin 'tmux-plugins/tmux-sensible'\n");

    let cfg = load_config_from_path(&tmux_conf).unwrap();

    assert_eq!(cfg.plugins.len(), 1);
    assert_eq!(cfg.plugins[0].remote_id().unwrap(), "github.com/tmux-plugins/tmux-sensible");
}

#[test]
fn config_tpm_accepts_combined_set_flags() {
    let dir = tempdir().unwrap();
    let tmux_conf = dir.path().join("tmux.conf");
    write_file(&tmux_conf, "set -gq @plugin 'tmux-plugins/tmux-sensible'\n");

    let cfg = load_config_from_path(&tmux_conf).unwrap();

    assert_eq!(cfg.plugins.len(), 1);
    assert_eq!(cfg.plugins[0].remote_id().unwrap(), "github.com/tmux-plugins/tmux-sensible");
}

#[test]
fn config_tpm_parses_branch_suffix() {
    let dir = tempdir().unwrap();
    let tmux_conf = dir.path().join("tmux.conf");
    write_file(&tmux_conf, "set -g @plugin 'tmux-plugins/tmux-resurrect#feature'\n");

    let cfg = load_config_from_path(&tmux_conf).unwrap();

    assert_eq!(cfg.plugins.len(), 1);
    assert!(matches!(&cfg.plugins[0].tracking, Tracking::Branch(branch) if branch == "feature"));
}

#[test]
fn config_tpm_parses_unquoted_branch_suffix() {
    let dir = tempdir().unwrap();
    let tmux_conf = dir.path().join("tmux.conf");
    write_file(&tmux_conf, "set -g @plugin tmux-plugins/tmux-resurrect#feature\n");

    let cfg = load_config_from_path(&tmux_conf).unwrap();

    assert_eq!(cfg.plugins.len(), 1);
    assert!(matches!(&cfg.plugins[0].tracking, Tracking::Branch(branch) if branch == "feature"));
}

#[test]
fn config_tpm_accepts_https_and_ssh_sources() {
    let dir = tempdir().unwrap();
    let tmux_conf = dir.path().join("tmux.conf");
    write_file(
        &tmux_conf,
        concat!(
            "set -g @plugin 'https://github.com/user/one.git'\n",
            "set -g @plugin 'git@github.com:user/two.git'\n",
        ),
    );

    let cfg = load_config_from_path(&tmux_conf).unwrap();

    assert_eq!(cfg.plugins.len(), 2);
    assert_eq!(cfg.plugins[0].remote_id().unwrap(), "github.com/user/one");
    assert_eq!(cfg.plugins[1].remote_id().unwrap(), "github.com/user/two");
}

#[test]
fn config_tpm_deduplicates_equivalent_remote_plugin_ids() {
    let dir = tempdir().unwrap();
    let tmux_conf = dir.path().join("tmux.conf");
    write_file(
        &tmux_conf,
        concat!(
            "set -g @plugin 'tmux-plugins/tmux-sensible'\n",
            "set -g @plugin 'https://github.com/tmux-plugins/tmux-sensible.git'\n",
        ),
    );

    let cfg = load_config_from_path(&tmux_conf).unwrap();

    assert_eq!(cfg.plugins.len(), 1);
    assert_eq!(cfg.plugins[0].remote_id().unwrap(), "github.com/tmux-plugins/tmux-sensible");
}

#[test]
fn config_tpm_reads_direct_sourced_file() {
    let dir = tempdir().unwrap();
    let tmux_conf = dir.path().join("tmux.conf");
    let sourced = dir.path().join("plugins.conf");
    write_file(&tmux_conf, &format!("source-file '{}'\n", sourced.display()));
    write_file(&sourced, "set -g @plugin 'tmux-plugins/tmux-yank'\n");

    let cfg = load_config_from_path(&tmux_conf).unwrap();

    assert_eq!(cfg.plugins.len(), 1);
    assert_eq!(cfg.plugins[0].remote_id().unwrap(), "github.com/tmux-plugins/tmux-yank");
}

#[test]
fn config_tpm_reads_relative_sourced_file_from_config_dir() {
    let dir = tempdir().unwrap();
    let tmux_conf = dir.path().join("tmux.conf");
    let sourced = dir.path().join("conf.d/plugins.conf");
    write_file(&tmux_conf, "source-file conf.d/plugins.conf\n");
    write_file(&sourced, "set -g @plugin 'tmux-plugins/tmux-yank'\n");

    let cfg = load_config_from_path(&tmux_conf).unwrap();

    assert_eq!(cfg.plugins.len(), 1);
    assert_eq!(cfg.plugins[0].remote_id().unwrap(), "github.com/tmux-plugins/tmux-yank");
}

#[test]
fn config_tpm_reads_source_alias() {
    let dir = tempdir().unwrap();
    let tmux_conf = dir.path().join("tmux.conf");
    let sourced = dir.path().join("plugins.conf");
    write_file(&tmux_conf, &format!("source '{}'\n", sourced.display()));
    write_file(&sourced, "set -g @plugin 'tmux-plugins/tmux-yank'\n");

    let cfg = load_config_from_path(&tmux_conf).unwrap();

    assert_eq!(cfg.plugins.len(), 1);
    assert_eq!(cfg.plugins[0].remote_id().unwrap(), "github.com/tmux-plugins/tmux-yank");
}

#[test]
fn config_tpm_reads_direct_sourced_file_with_quiet_flag() {
    let dir = tempdir().unwrap();
    let tmux_conf = dir.path().join("tmux.conf");
    let sourced = dir.path().join("plugins.conf");
    write_file(&tmux_conf, &format!("source-file -q '{}'\n", sourced.display()));
    write_file(&sourced, "set -g @plugin 'tmux-plugins/tmux-yank'\n");

    let cfg = load_config_from_path(&tmux_conf).unwrap();

    assert_eq!(cfg.plugins.len(), 1);
    assert_eq!(cfg.plugins[0].remote_id().unwrap(), "github.com/tmux-plugins/tmux-yank");
}

#[test]
fn config_tpm_expands_globbed_sourced_files() {
    let dir = tempdir().unwrap();
    let tmux_conf = dir.path().join("tmux.conf");
    let conf_d = dir.path().join("conf.d");
    let first = conf_d.join("one.conf");
    let second = conf_d.join("two.conf");
    write_file(&tmux_conf, &format!("source-file '{}'\n", conf_d.join("*.conf").display()));
    write_file(&first, "set -g @plugin 'tmux-plugins/tmux-sensible'\n");
    write_file(&second, "set -g @plugin 'tmux-plugins/tmux-yank'\n");

    let cfg = load_config_from_path(&tmux_conf).unwrap();

    assert_eq!(cfg.plugins.len(), 2);
    assert_eq!(cfg.plugins[0].remote_id().unwrap(), "github.com/tmux-plugins/tmux-sensible");
    assert_eq!(cfg.plugins[1].remote_id().unwrap(), "github.com/tmux-plugins/tmux-yank");
}

#[test]
fn config_tpm_ignores_missing_quiet_sourced_file() {
    let dir = tempdir().unwrap();
    let tmux_conf = dir.path().join("tmux.conf");
    let missing = dir.path().join("missing.conf");
    write_file(
        &tmux_conf,
        &format!(
            concat!("set -g @plugin 'tmux-plugins/tmux-sensible'\n", "source-file -q '{}'\n"),
            missing.display()
        ),
    );

    let cfg = load_config_from_path(&tmux_conf).unwrap();

    assert_eq!(cfg.plugins.len(), 1);
    assert_eq!(cfg.plugins[0].remote_id().unwrap(), "github.com/tmux-plugins/tmux-sensible");
}

#[test]
fn config_tpm_ignores_blank_and_comment_lines() {
    let dir = tempdir().unwrap();
    let tmux_conf = dir.path().join("tmux.conf");
    write_file(
        &tmux_conf,
        concat!("\n", "  \n", "# comment only\n", "set -g @plugin 'tmux-plugins/tmux-sensible'\n"),
    );

    let cfg = load_config_from_path(&tmux_conf).unwrap();

    assert_eq!(cfg.plugins.len(), 1);
    assert_eq!(cfg.plugins[0].remote_id().unwrap(), "github.com/tmux-plugins/tmux-sensible");
}

#[test]
fn config_tpm_skips_malformed_plugin_line_without_value() {
    let dir = tempdir().unwrap();
    let tmux_conf = dir.path().join("tmux.conf");
    write_file(
        &tmux_conf,
        concat!("set -g @plugin\n", "set -g @plugin 'tmux-plugins/tmux-sensible'\n"),
    );

    let cfg = load_config_from_path(&tmux_conf).unwrap();

    assert_eq!(cfg.plugins.len(), 1);
    assert_eq!(cfg.plugins[0].remote_id().unwrap(), "github.com/tmux-plugins/tmux-sensible");
}

#[test]
fn config_tpm_deduplicates_multiple_equivalent_remote_plugin_ids() {
    let dir = tempdir().unwrap();
    let tmux_conf = dir.path().join("tmux.conf");
    write_file(
        &tmux_conf,
        concat!(
            "set -g @plugin 'tmux-plugins/tmux-sensible'\n",
            "set -g @plugin 'https://github.com/tmux-plugins/tmux-sensible.git'\n",
            "set -g @plugin 'git@github.com:tmux-plugins/tmux-sensible.git'\n",
        ),
    );

    let cfg = load_config_from_path(&tmux_conf).unwrap();

    assert_eq!(cfg.plugins.len(), 1);
}

#[test]
fn config_tpm_does_not_recurse_into_nested_sourced_files() {
    let dir = tempdir().unwrap();
    let tmux_conf = dir.path().join("tmux.conf");
    let direct = dir.path().join("plugins.conf");
    let nested = dir.path().join("nested.conf");
    write_file(&tmux_conf, &format!("source-file '{}'\n", direct.display()));
    write_file(
        &direct,
        &format!(
            concat!("set -g @plugin 'tmux-plugins/tmux-sensible'\n", "source-file '{}'\n",),
            nested.display()
        ),
    );
    write_file(&nested, "set -g @plugin 'tmux-plugins/tmux-yank'\n");

    let cfg = load_config_from_path(&tmux_conf).unwrap();

    // Intentionally matches current TPM behavior: only direct sourced files are scanned.
    assert_eq!(cfg.plugins.len(), 1);
    assert_eq!(cfg.plugins[0].remote_id().unwrap(), "github.com/tmux-plugins/tmux-sensible");
}
