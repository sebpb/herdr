use super::*;
use crate::ui::SidebarSection;

fn project(name: Option<&str>, label: &str, running: bool, current: bool) -> ProjectEntry {
    ProjectEntry {
        name: name.map(str::to_owned),
        label: label.to_owned(),
        running,
        current,
        workspaces: vec![format!("{label} web"), format!("{label} api")],
    }
}

fn projects_state() -> ClientShellState {
    // Session names in notices and the current session follow the process env.
    std::env::remove_var(crate::session::SESSION_ENV_VAR);
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    state.set_snapshot(Box::new(snapshot()));
    state.set_pane_surface(surface());
    assert!(state.receive_projects(
        Ok(vec![
            project(Some("cliente-alamo"), "Cliente Álamo", true, true),
            project(None, "default", true, false),
            project(Some("old"), "old", false, false),
        ]),
        None,
    ));
    state.compose(106, 30).expect("composed frame");
    state
}

fn mouse(state: &mut ClientShellState, button: MouseButton, rect: Rect) -> ClientShellInput {
    state.handle_raw_events(vec![RawInputEvent::Mouse(crossterm::event::MouseEvent {
        kind: MouseEventKind::Down(button),
        column: rect.x + 1,
        row: rect.y,
        modifiers: KeyModifiers::empty(),
    })])
}

fn project_rect(state: &ClientShellState, name: Option<&str>) -> Rect {
    state
        .hits
        .projects
        .iter()
        .find(|(_, candidate)| candidate.as_deref() == name)
        .map(|(rect, _)| *rect)
        .unwrap_or_else(|| panic!("missing project {name:?}"))
}

fn sidebar_rows(state: &mut ClientShellState) -> Vec<String> {
    let frame = state.compose(106, 30).expect("composed frame");
    let width = state.hits.sidebar_divider.x as usize;
    frame_rows(&frame)
        .into_iter()
        .map(|row| row.chars().take(width).collect::<String>())
        .collect()
}

fn session_switch_targets(outcome: &ClientShellInput) -> Vec<&SessionSwitchRequest> {
    outcome
        .actions
        .iter()
        .filter_map(|action| match action {
            ClientShellAction::RequestSessionSwitch(request) => Some(request),
            _ => None,
        })
        .collect()
}

fn projects_commands(outcome: &ClientShellInput) -> Vec<&ProjectsCommand> {
    outcome
        .actions
        .iter()
        .filter_map(|action| match action {
            ClientShellAction::Projects(command) => Some(command),
            _ => None,
        })
        .collect()
}

fn set_overlay_input(state: &mut ClientShellState, text: &str) {
    match state.overlay.as_mut() {
        Some(ClientShellOverlay::Rename(rename)) => rename.input = TextEditor::new(text, false),
        other => panic!("expected a text prompt, got {other:?}"),
    }
}

fn overlay_title(state: &ClientShellState) -> &'static str {
    match state.overlay.as_ref() {
        Some(ClientShellOverlay::Rename(rename)) => rename.title,
        other => panic!("expected a text prompt, got {other:?}"),
    }
}

#[test]
fn projects_section_renders_above_spaces_like_space_rows() {
    let mut state = projects_state();
    let rows = sidebar_rows(&mut state);

    assert!(rows[0].starts_with(" projects"), "{rows:?}");
    assert!(rows[0].trim_end().ends_with("new"), "{rows:?}");
    assert_eq!(rows[1].trim(), "", "{rows:?}");
    assert!(rows[2].starts_with(" ● Cliente Álamo"), "{rows:?}");
    // The current project lists the live snapshot's spaces.
    assert!(rows[3].starts_with("   client-shell"), "{rows:?}");
    assert!(rows[4].starts_with(" ○ default"), "{rows:?}");
    assert!(rows[5].starts_with("   default web, def"), "{rows:?}");
    assert!(rows[5].trim_end().ends_with('…'), "{rows:?}");
    assert!(rows[6].starts_with("   old"), "{rows:?}");
    assert!(rows[7].starts_with("   old web, old api"), "{rows:?}");
    assert!(rows[8].starts_with('─'), "{rows:?}");
    assert!(rows[9].starts_with(" spaces"), "{rows:?}");

    let current = project_rect(&state, Some("cliente-alamo"));
    assert_eq!((current.y, current.height), (2, 2));
    let frame = state.compose(106, 30).expect("composed frame");
    let buffer = frame.to_ratatui_buffer().expect("frame should reconstruct");
    let active = state.config.palette.active_row_bg;
    assert_eq!(buffer[(current.x + 4, current.y + 1)].bg, active);
    let suspended = project_rect(&state, Some("old"));
    assert_ne!(buffer[(suspended.x + 4, suspended.y)].bg, active);
}

#[test]
fn projects_section_waits_for_a_listing_and_follows_the_config() {
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    state.set_snapshot(Box::new(snapshot()));
    state.set_pane_surface(surface());
    assert!(state.projects_refresh_wanted());
    let rows = sidebar_rows(&mut state);
    assert!(rows[0].starts_with(" spaces"), "{rows:?}");
    assert!(state.hits.projects.is_empty());

    let mut config = Config::default();
    config.ui.sidebar.projects.enabled = false;
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&config));
    state.set_snapshot(Box::new(snapshot()));
    state.set_pane_surface(surface());
    state.receive_projects(Ok(vec![project(None, "default", true, true)]), None);
    assert!(!state.projects_refresh_wanted());
    let rows = sidebar_rows(&mut state);
    assert!(rows[0].starts_with(" spaces"), "{rows:?}");
}

#[test]
fn clicking_a_project_requests_a_switch_except_for_the_current_one() {
    let mut state = projects_state();

    let current = project_rect(&state, Some("cliente-alamo"));
    let outcome = mouse(&mut state, MouseButton::Left, current);
    assert!(outcome.actions.is_empty());
    assert!(state.overlay.is_none());

    let default = project_rect(&state, None);
    let outcome = mouse(&mut state, MouseButton::Left, default);
    let requests = session_switch_targets(&outcome);
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].target, None);
    assert_eq!(requests[0].create_cwd, None);

    let old = project_rect(&state, Some("old"));
    let outcome = mouse(
        &mut state,
        MouseButton::Left,
        Rect {
            y: old.y + 1,
            ..old
        },
    );
    let requests = session_switch_targets(&outcome);
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].target.as_deref(), Some("old"));
}

#[test]
fn new_project_asks_for_a_name_then_a_folder() {
    let folder = std::env::temp_dir().join(format!("herdr-new-project-{}", std::process::id()));
    std::fs::create_dir_all(&folder).unwrap();
    let mut state = projects_state();

    let new = state.hits.projects_new;
    assert_ne!(new, Rect::default());
    let outcome = mouse(&mut state, MouseButton::Left, new);
    assert!(outcome.actions.is_empty());
    assert_eq!(overlay_title(&state), "new project");
    assert!(
        !state
            .collapsed_sidebar_sections
            .contains(&SidebarSection::Projects),
        "new must not collapse the section"
    );
    let text = frame_rows(&state.compose(106, 30).unwrap()).join("\n");
    assert!(text.contains("new project"), "{text}");

    // An empty name keeps the prompt open with an error.
    state.handle_input_bytes(b"\r");
    assert_eq!(overlay_title(&state), "new project");
    let text = frame_rows(&state.compose(106, 30).unwrap()).join("\n");
    assert!(text.contains("Enter a project name"), "{text}");

    set_overlay_input(&mut state, "Señor Café");
    let outcome = state.handle_input_bytes(b"\r");
    assert!(outcome.actions.is_empty());
    assert_eq!(overlay_title(&state), "project folder");
    match state.overlay.as_ref() {
        Some(ClientShellOverlay::Rename(rename)) => assert_eq!(rename.input.as_str(), "~"),
        other => panic!("folder prompt: {other:?}"),
    }

    set_overlay_input(&mut state, "/definitely/not/a/folder");
    let outcome = state.handle_input_bytes(b"\r");
    assert!(outcome.actions.is_empty());
    assert_eq!(overlay_title(&state), "project folder");
    let text = frame_rows(&state.compose(106, 30).unwrap()).join("\n");
    assert!(text.contains("Folder not found"), "{text}");

    set_overlay_input(&mut state, folder.to_str().unwrap());
    let outcome = state.handle_input_bytes(b"\r");
    assert!(state.overlay.is_none());
    assert_eq!(
        projects_commands(&outcome),
        [&ProjectsCommand::SetLabel {
            name: Some("senor-cafe".into()),
            label: Some("Señor Café".into()),
        }]
    );
    let requests = session_switch_targets(&outcome);
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].target.as_deref(), Some("senor-cafe"));
    assert_eq!(requests[0].create_cwd.as_deref(), Some(folder.as_path()));
    std::fs::remove_dir_all(folder).unwrap();
}

#[test]
fn new_project_with_an_existing_name_switches_to_it() {
    let mut state = projects_state();
    let new = state.hits.projects_new;
    mouse(&mut state, MouseButton::Left, new);
    set_overlay_input(&mut state, "OLD");

    let outcome = state.handle_input_bytes(b"\r");

    assert!(state.overlay.is_none());
    assert!(projects_commands(&outcome).is_empty());
    let requests = session_switch_targets(&outcome);
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].target.as_deref(), Some("old"));
    assert_eq!(requests[0].create_cwd, None);
}

#[test]
fn project_context_menu_renames_and_only_deletes_stopped_projects() {
    let mut state = projects_state();
    let menu_actions = |state: &ClientShellState| match state.overlay.as_ref() {
        Some(ClientShellOverlay::ContextMenu(menu)) => menu
            .items()
            .iter()
            .map(|item| item.action)
            .collect::<Vec<_>>(),
        other => panic!("context menu: {other:?}"),
    };

    for name in [Some("cliente-alamo"), None] {
        let rect = project_rect(&state, name);
        let outcome = mouse(&mut state, MouseButton::Right, rect);
        assert!(outcome.actions.is_empty());
        assert_eq!(menu_actions(&state), [ClientContextMenuAction::Rename]);
        state.handle_input_bytes(b"\x1b");
        state.compose(106, 30).unwrap();
    }

    // Rename changes only the label, right away and through the runtime.
    let old = project_rect(&state, Some("old"));
    mouse(&mut state, MouseButton::Right, old);
    assert_eq!(
        menu_actions(&state),
        [
            ClientContextMenuAction::Rename,
            ClientContextMenuAction::DeleteProject
        ]
    );
    state.compose(106, 30).unwrap();
    let rename = state.hits.context_menu_rows[0].0;
    mouse(&mut state, MouseButton::Left, rename);
    assert_eq!(overlay_title(&state), "rename project");
    set_overlay_input(&mut state, "Archive");
    let outcome = state.handle_input_bytes(b"\r");
    assert_eq!(
        projects_commands(&outcome),
        [&ProjectsCommand::SetLabel {
            name: Some("old".into()),
            label: Some("Archive".into()),
        }]
    );
    assert!(session_switch_targets(&outcome).is_empty());
    let rows = sidebar_rows(&mut state);
    assert!(rows[4].starts_with("   Archive"), "{rows:?}");

    // Delete asks first; Esc cancels, Enter deletes.
    for confirm in [false, true] {
        let old = project_rect(&state, Some("old"));
        mouse(&mut state, MouseButton::Right, old);
        state.compose(106, 30).unwrap();
        let delete = state.hits.context_menu_rows[1].0;
        let outcome = mouse(&mut state, MouseButton::Left, delete);
        assert!(outcome.actions.is_empty());
        assert!(matches!(
            state.overlay,
            Some(ClientShellOverlay::ConfirmProjectDelete(_))
        ));
        let text = frame_rows(&state.compose(106, 30).unwrap()).join("\n");
        assert!(text.contains("Delete project Archive?"), "{text}");
        let outcome = state.handle_input_bytes(if confirm { b"\r" } else { b"\x1b" });
        assert!(state.overlay.is_none());
        let expected = ProjectsCommand::Delete { name: "old".into() };
        if confirm {
            assert_eq!(projects_commands(&outcome), [&expected]);
        } else {
            assert!(outcome.actions.is_empty());
        }
        state.compose(106, 30).unwrap();
    }
}

#[test]
fn projects_title_click_collapses_the_section() {
    let mut state = projects_state();
    let title = state
        .hits
        .section_titles
        .iter()
        .find(|(_, section)| *section == SidebarSection::Projects)
        .map(|(rect, _)| *rect)
        .expect("projects title");
    assert!(title.right() <= state.hits.projects_new.x);

    let outcome = mouse(&mut state, MouseButton::Left, title);
    assert!(outcome.actions.is_empty());
    assert!(state
        .collapsed_sidebar_sections
        .contains(&SidebarSection::Projects));
    assert!(!state.projects_refresh_wanted());
    let rows = sidebar_rows(&mut state);
    assert!(rows[0].starts_with("▸ projects"), "{rows:?}");
    assert!(rows[1].starts_with('─'), "{rows:?}");
    assert!(rows[2].starts_with(" spaces"), "{rows:?}");
    assert!(state.hits.projects.is_empty());

    let title = state.hits.section_titles[0].0;
    mouse(&mut state, MouseButton::Left, title);
    assert!(state.collapsed_sidebar_sections.is_empty());
    assert!(state.projects_refresh_wanted());
}

#[test]
fn many_projects_scroll_inside_their_capped_section() {
    let mut state = projects_state();
    let entries = (0..12)
        .map(|index| {
            project(
                Some(&format!("p{index:02}")),
                &format!("p{index:02}"),
                false,
                index == 0,
            )
        })
        .collect();
    state.receive_projects(Ok(entries), None);
    state.compose(106, 30).unwrap();

    // 40% of 30 rows leaves the title plus five project rows.
    assert_eq!(state.hits.projects.len(), 5);
    assert_eq!(state.hits.project_max_scroll, 7);
    let body = state.hits.projects_body;
    for _ in 0..10 {
        state.handle_raw_events(vec![RawInputEvent::Mouse(crossterm::event::MouseEvent {
            kind: MouseEventKind::ScrollDown,
            column: body.x + 1,
            row: body.y,
            modifiers: KeyModifiers::empty(),
        })]);
    }
    state.compose(106, 30).unwrap();
    assert_eq!(state.projects.scroll, 7);
    assert_eq!(state.hits.projects[0].1.as_deref(), Some("p07"));
}

#[test]
fn a_switch_marks_the_new_current_project_first() {
    let mut state = projects_state();

    state.mark_current_project(Some("old"));

    let names = state
        .projects
        .entries
        .iter()
        .map(|entry| (entry.name.as_deref(), entry.current, entry.running))
        .collect::<Vec<_>>();
    assert_eq!(
        names,
        [
            (Some("old"), true, true),
            (Some("cliente-alamo"), false, true),
            (None, false, true),
        ]
    );
}
