use lazytmux::config::parse_config;

#[test]
fn parses_remote_and_local_plugins() {
    let input = r#"
options {
    auto-install #true
}
plugin "tmux-plugins/tmux-sensible"
plugin "~/dev/my-plugin" local=#true name="my-plugin-dev"
    "#;

    let cfg = parse_config(input).unwrap();
    assert_eq!(cfg.plugins.len(), 2);
    assert!(cfg.plugins[0].is_remote());
    assert!(cfg.plugins[1].is_local());
    assert_eq!(cfg.plugins[1].name, "my-plugin-dev");
}

#[test]
fn parses_options() {
    let input = r#"
options {
    concurrency 4
    auto-install #false
    auto-clean #true
    bind-ui #true
    ui-key "T"
}
    "#;
    let cfg = parse_config(input).unwrap();
    assert_eq!(cfg.options.concurrency, 4);
    assert!(!cfg.options.auto_install);
    assert!(cfg.options.auto_clean);
    assert!(cfg.options.bind_ui);
    assert_eq!(cfg.options.ui_key, "T");
}

#[test]
fn parses_opts_and_opt_prefix() {
    let input = r##"
plugin "catppuccin/tmux" opt-prefix="catppuccin_" {
    opt "flavor" "mocha"
    opt "window_text" "#W"
}
    "##;
    let cfg = parse_config(input).unwrap();
    let p = &cfg.plugins[0];
    assert_eq!(p.opt_prefix, "catppuccin_");
    assert_eq!(p.opts.len(), 2);
    assert_eq!(p.opts[0], ("flavor".into(), "mocha".into()));
    assert_eq!(p.opts[1], ("window_text".into(), "#W".into()));
}

#[test]
fn parses_tracking_selectors() {
    let branch = parse_config(r#"plugin "user/repo" branch="main""#).unwrap();
    let tag = parse_config(r#"plugin "user/repo" tag="v2.3""#).unwrap();
    let commit = parse_config(r#"plugin "user/repo" commit="abc123""#).unwrap();
    let default = parse_config(r#"plugin "user/repo""#).unwrap();

    assert!(matches!(
        branch.plugins[0].tracking,
        lazytmux::model::Tracking::Branch(_)
    ));
    assert!(matches!(
        tag.plugins[0].tracking,
        lazytmux::model::Tracking::Tag(_)
    ));
    assert!(matches!(
        commit.plugins[0].tracking,
        lazytmux::model::Tracking::Commit(_)
    ));
    assert!(matches!(
        default.plugins[0].tracking,
        lazytmux::model::Tracking::DefaultBranch
    ));
}

#[test]
fn rejects_multiple_tracking_selectors() {
    let input = r#"plugin "tmux-plugins/tmux-yank" branch="main" tag="v1.0.0""#;
    assert!(parse_config(input).is_err());
}

#[test]
fn rejects_local_plugin_with_tracking_selector() {
    let input = r#"plugin "~/dev/my-plugin" local=#true branch="main""#;
    assert!(parse_config(input).is_err());
}

#[test]
fn parses_build_property() {
    let input = r#"plugin "tmux-plugins/tmux-resurrect" build="make install""#;
    let cfg = parse_config(input).unwrap();
    assert_eq!(cfg.plugins[0].build.as_deref(), Some("make install"));
}

#[test]
fn defaults_are_applied() {
    let cfg = parse_config("").unwrap();
    assert_eq!(cfg.options.concurrency, 8);
    assert!(cfg.options.auto_install);
    assert!(!cfg.options.auto_clean);
    assert!(!cfg.options.bind_ui);
    assert_eq!(cfg.options.ui_key, "L");
    assert!(cfg.plugins.is_empty());
}
