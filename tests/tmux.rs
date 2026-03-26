use std::path::PathBuf;

use lazytmux::tmux::TmuxCommand;

#[test]
fn run_shell_quotes_paths_with_spaces() {
    let cmd = TmuxCommand::RunShell { script: PathBuf::from("/tmp/with space/plugin.tmux") };

    assert_eq!(
        cmd.to_args(),
        vec!["run-shell".to_string(), "'/tmp/with space/plugin.tmux'".to_string(),]
    );
}

#[test]
fn run_shell_escapes_single_quotes() {
    let cmd = TmuxCommand::RunShell { script: PathBuf::from("/tmp/it's/plugin.tmux") };

    assert_eq!(
        cmd.to_args(),
        vec!["run-shell".to_string(), "'/tmp/it'\"'\"'s/plugin.tmux'".to_string(),]
    );
}

#[test]
fn parse_tmux_version_formats() {
    use lazytmux::tmux::parse_tmux_version;

    let cases = [
        ("tmux 3.2", Some((3, 2, None))),
        ("tmux 3.3a", Some((3, 3, Some('a')))),
        ("tmux next-3.4", Some((3, 4, None))),
        ("tmux master", None),
    ];

    for (input, expected) in cases {
        let parsed = parse_tmux_version(input).map(|v| (v.major, v.minor, v.suffix));
        assert_eq!(parsed, expected, "{} should parse as {expected:?}", input);
    }
}
