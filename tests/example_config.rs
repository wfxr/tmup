#[test]
fn example_config_parses() {
    let input = std::fs::read_to_string("examples/lazy.kdl").unwrap();
    let cfg = lazytmux::config::parse_config(&input).unwrap();
    assert_eq!(cfg.options.concurrency, 8);
    assert!(cfg.options.auto_install);
    assert!(!cfg.options.auto_clean);
    assert_eq!(cfg.plugins.len(), 6); // continuum disabled via slashdash
    // tmux-sensible
    assert_eq!(
        cfg.plugins[0].remote_id().unwrap(),
        "github.com/tmux-plugins/tmux-sensible"
    );
    // tmux-yank pinned to tag
    assert!(matches!(
        cfg.plugins[1].tracking,
        lazytmux::model::Tracking::Tag(_)
    ));
    // tmux-resurrect with branch + build + opts
    assert!(matches!(
        cfg.plugins[2].tracking,
        lazytmux::model::Tracking::Branch(_)
    ));
    assert_eq!(cfg.plugins[2].build.as_deref(), Some("make install"));
    assert_eq!(cfg.plugins[2].opts.len(), 2);
    // catppuccin with opt-prefix
    assert_eq!(cfg.plugins[3].opt_prefix, "catppuccin_");
    assert_eq!(cfg.plugins[3].opts[0], ("flavor".into(), "mocha".into()));
    // gitlab plugin
    assert_eq!(
        cfg.plugins[4].remote_id().unwrap(),
        "gitlab.com/user/my-plugin"
    );
    // local plugin
    assert!(cfg.plugins[5].is_local());
    assert_eq!(cfg.plugins[5].name, "my-plugin-dev");
}
