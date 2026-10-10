use super::*;
use crossterm::event::{MouseButton, MouseEventKind};

fn switch_state() -> ClientShellState {
    // Titles name the current session; tests run in their own process under nextest.
    std::env::remove_var(crate::session::SESSION_ENV_VAR);
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    state.set_snapshot(Box::new(snapshot()));
    state.set_pane_surface(surface());
    state.compose(106, 24).unwrap();
    state
}

fn request() -> SessionSwitchRequest {
    SessionSwitchRequest::new("work")
        .unwrap()
        .with_create_cwd("/tmp/project")
}

fn click(state: &mut ClientShellState, rect: Rect) -> ClientShellInput {
    state.handle_raw_events(vec![crate::raw_input::RawInputEvent::Mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: rect.x,
        row: rect.y,
        modifiers: KeyModifiers::empty(),
    })])
}

fn switch_requests(outcome: &ClientShellInput) -> Vec<&SessionSwitchRequest> {
    outcome
        .actions
        .iter()
        .filter_map(|action| match action {
            ClientShellAction::SwitchSession(request) => Some(request),
            _ => None,
        })
        .collect()
}

#[test]
fn session_switch_request_normalizes_the_default_session() {
    assert_eq!(SessionSwitchRequest::new("default").unwrap().target, None);
    let named = SessionSwitchRequest::new("work").unwrap();
    assert_eq!(named.target.as_deref(), Some("work"));
    assert_eq!(named.target_label(), "work");
    assert_eq!(
        SessionSwitchRequest::new("default").unwrap().target_label(),
        "default"
    );
    assert!(SessionSwitchRequest::new("../escape").is_err());
}

#[test]
fn requesting_a_session_switch_asks_the_runtime_to_check_first() {
    let mut state = switch_state();
    let mut outcome = ClientShellInput::default();

    state.request_session_switch(request(), &mut outcome);

    assert!(matches!(
        outcome.actions.as_slice(),
        [ClientShellAction::RequestSessionSwitch(requested)] if *requested == request()
    ));
    assert!(state.overlay.is_none());
}

#[test]
fn session_switch_without_warnings_switches_immediately() {
    let mut state = switch_state();

    let outcome = state.receive_session_switch_warnings(request(), Ok(Vec::new()));

    assert_eq!(switch_requests(&outcome), vec![&request()]);
    assert!(state.overlay.is_none());
}

#[test]
fn session_switch_with_warnings_waits_for_keyboard_or_mouse_confirmation() {
    for mouse in [false, true] {
        let mut state = switch_state();
        let warnings = vec![
            "api: agent working (fix login)".to_string(),
            "web: npm, node".to_string(),
        ];

        let outcome = state.receive_session_switch_warnings(request(), Ok(warnings));

        assert!(outcome.repaint);
        assert!(switch_requests(&outcome).is_empty());
        assert!(matches!(
            &state.overlay,
            Some(ClientShellOverlay::ConfirmSessionSwitch(confirm))
                if confirm.title == "Suspend default?" && confirm.warnings.len() == 2
        ));
        let frame = state.compose(106, 24).unwrap();
        let text = frame_rows(&frame).join("\n");
        assert!(text.contains("Suspend default?"));
        assert!(text.contains("Switching to work stops"));
        assert!(text.contains("api: agent working (fix login)"));
        assert!(text.contains("web: npm, node"));
        assert!(text.contains("switch"));

        let accepted = if mouse {
            let primary = state.hits.overlay_primary;
            click(&mut state, primary)
        } else {
            state.handle_input_bytes(b"\r")
        };

        assert_eq!(switch_requests(&accepted), vec![&request()]);
        assert!(state.overlay.is_none());
    }
}

#[test]
fn cancelling_the_session_switch_confirmation_emits_nothing() {
    for mouse in [false, true] {
        let mut state = switch_state();
        state.receive_session_switch_warnings(request(), Ok(vec!["web: vim".to_string()]));
        state.compose(106, 24).unwrap();

        let cancelled = if mouse {
            let cancel = state.hits.overlay_cancel;
            click(&mut state, cancel)
        } else {
            state.handle_input_bytes(b"\x1b")
        };

        assert!(cancelled.actions.is_empty());
        assert!(cancelled.requests.is_empty());
        assert!(state.overlay.is_none());
    }
}

#[test]
fn failed_session_switch_check_still_asks_for_confirmation() {
    let mut state = switch_state();

    let outcome = state.receive_session_switch_warnings(request(), Err("timed out".to_string()));

    assert!(switch_requests(&outcome).is_empty());
    assert!(matches!(
        &state.overlay,
        Some(ClientShellOverlay::ConfirmSessionSwitch(confirm))
            if confirm.warnings == ["Could not check running work: timed out"]
    ));
}

#[test]
fn many_session_switch_warnings_are_summarized() {
    let mut state = switch_state();
    let warnings = (1..=12)
        .map(|n| format!("ws{n}: job{n}"))
        .collect::<Vec<_>>();
    state.receive_session_switch_warnings(request(), Ok(warnings));

    let frame = state.compose(106, 30).unwrap();
    let text = frame_rows(&frame).join("\n");

    assert!(text.contains("ws8: job8"));
    assert!(!text.contains("ws9: job9"));
    assert!(text.contains("and 4 more"));
    assert!(state.hits.overlay_primary.width > 0);
}

#[test]
fn reloading_local_preferences_follows_the_new_session_file() {
    let root = std::env::temp_dir().join(format!(
        "herdr-shell-session-preferences-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::env::set_var("XDG_STATE_HOME", &root);
    let old_socket = std::path::Path::new("/tmp/herdr-old/herdr-client.sock");
    let new_socket = std::path::Path::new("/tmp/herdr-new/herdr-client.sock");
    let new_path = preferences::path_for_local_endpoint(new_socket);
    preferences::store(
        &new_path,
        preferences::ClientChromePreferences {
            sidebar_width: Some(30),
            sidebar_collapsed: Some(true),
            collapsed_groups: vec!["/repo".to_string()],
            ..preferences::ClientChromePreferences::default()
        },
    )
    .unwrap();
    let mut state = ClientShellState::new(
        ClientShellConfig::from_config(&Config::default()).with_local_endpoint(old_socket),
    );
    state.sidebar_width = 22;
    state.sidebar_width_manual = true;
    state.collapsed_groups.insert("/old".to_string());

    state.reload_local_preferences(new_socket);

    assert_eq!(
        state.config.preferences_path.as_deref(),
        Some(new_path.as_path())
    );
    assert_eq!(state.sidebar_width, 30);
    assert!(state.sidebar_width_manual);
    assert!(state.sidebar_collapsed);
    assert!(state.sidebar_collapsed_manual);
    assert_eq!(state.collapsed_groups, HashSet::from(["/repo".to_string()]));

    // A session without saved chrome falls back to the configured defaults.
    state.reload_local_preferences(old_socket);
    assert_eq!(state.sidebar_width, Config::default().ui.sidebar_width);
    assert!(!state.sidebar_width_manual);
    assert!(!state.sidebar_collapsed_manual);
    assert!(state.collapsed_groups.is_empty());

    let _ = std::fs::remove_dir_all(&root);
    std::env::remove_var("XDG_STATE_HOME");
}
