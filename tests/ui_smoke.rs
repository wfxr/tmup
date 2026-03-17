use ratatui::{Terminal, backend::TestBackend};

use lazytmux::{
    planner::{LastResult, PluginState, PluginStatus},
    ui::{App, render},
};

fn make_status(
    id: &str,
    name: &str,
    kind: &str,
    state: PluginState,
    last_result: LastResult,
) -> PluginStatus {
    PluginStatus {
        id: id.into(),
        name: name.into(),
        source: id.into(),
        kind: kind.into(),
        state,
        last_result,
        current_commit: None,
        lock_commit: Some("abc1234".into()),
    }
}

#[test]
fn ui_renders_state_and_last_result() {
    let rows = vec![
        make_status(
            "github.com/user/repo-a",
            "repo-a",
            "remote",
            PluginState::Installed,
            LastResult::Ok,
        ),
        make_status(
            "github.com/user/repo-b",
            "repo-b",
            "remote",
            PluginState::Installed,
            LastResult::BuildFailed,
        ),
    ];

    let app = App::new(rows, false);
    let backend = TestBackend::new(80, 20);
    let mut terminal = Terminal::new(backend).unwrap();

    terminal.draw(|frame| render(&app, frame)).unwrap();

    let buffer = terminal.backend().buffer().clone();
    let content = buffer_to_string(&buffer);

    assert!(content.contains("repo-a"), "should show repo-a name");
    assert!(content.contains("repo-b"), "should show repo-b name");
    assert!(content.contains("installed"), "should show installed state");
    assert!(
        content.contains("build-failed"),
        "should show build-failed result"
    );
}

#[test]
fn ui_shows_busy_banner_when_writer_is_active() {
    let rows = vec![make_status(
        "github.com/user/repo",
        "repo",
        "remote",
        PluginState::Installed,
        LastResult::Ok,
    )];

    let app = App::new(rows, true);
    let backend = TestBackend::new(80, 20);
    let mut terminal = Terminal::new(backend).unwrap();

    terminal.draw(|frame| render(&app, frame)).unwrap();

    let buffer = terminal.backend().buffer().clone();
    let content = buffer_to_string(&buffer);

    assert!(content.contains("BUSY"), "should show BUSY banner");
}

#[test]
fn ui_shows_summary_counts() {
    let rows = vec![
        make_status("a", "a", "remote", PluginState::Installed, LastResult::Ok),
        make_status("b", "b", "remote", PluginState::Missing, LastResult::None),
        make_status("c", "c", "remote", PluginState::PinnedTag, LastResult::Ok),
    ];

    let app = App::new(rows, false);
    let backend = TestBackend::new(80, 20);
    let mut terminal = Terminal::new(backend).unwrap();

    terminal.draw(|frame| render(&app, frame)).unwrap();

    let buffer = terminal.backend().buffer().clone();
    let content = buffer_to_string(&buffer);

    assert!(
        content.contains("Installed 1"),
        "should show installed count"
    );
    assert!(content.contains("Missing 1"), "should show missing count");
    assert!(content.contains("Pinned 1"), "should show pinned count");
}

fn buffer_to_string(buffer: &ratatui::buffer::Buffer) -> String {
    let mut result = String::new();
    for y in 0..buffer.area.height {
        for x in 0..buffer.area.width {
            let cell = &buffer[(x, y)];
            result.push_str(cell.symbol());
        }
        result.push('\n');
    }
    result
}
