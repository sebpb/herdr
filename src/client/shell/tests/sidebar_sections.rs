use super::*;
use crate::ui::SidebarSection;

fn preferences_path(name: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!(
        "herdr-shell-{name}-{}-{}.json",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos()
    ))
}

fn agent(pane_id: &str, name: &str) -> ClientShellAgent {
    ClientShellAgent {
        pane_id: pane_id.into(),
        workspace_id: "ws_1".into(),
        tab_id: "tab_1".into(),
        name: Some(name.into()),
        display_agent: None,
        agent: Some("pi".into()),
        title: None,
        terminal_title: None,
        terminal_title_stripped: None,
        agent_status: AgentStatus::Working,
        state_change_seq: 1,
        state_labels: Vec::new(),
        tokens: Vec::new(),
        focused: true,
    }
}

fn section_state(path: Option<std::path::PathBuf>) -> ClientShellState {
    let mut projected = snapshot();
    projected.agents.push(agent("pane_1", "alpha"));
    let mut config = ClientShellConfig::from_config(&Config::default());
    if let Some(path) = path {
        config = config.with_preferences_path(path);
    }
    let mut state = ClientShellState::new(config);
    state.set_snapshot(Box::new(projected));
    state.set_pane_surface(surface());
    state
}

fn title_rect(state: &ClientShellState, section: SidebarSection) -> Rect {
    state
        .hits
        .section_titles
        .iter()
        .find(|(_, candidate)| *candidate == section)
        .map(|(rect, _)| *rect)
        .unwrap_or_else(|| panic!("missing {section:?} title"))
}

fn click(state: &mut ClientShellState, column: u16, row: u16) -> ClientShellInput {
    state.handle_raw_events(vec![RawInputEvent::Mouse(crossterm::event::MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column,
        row,
        modifiers: KeyModifiers::empty(),
    })])
}

fn sidebar_rows(frame: &FrameData, width: u16) -> Vec<String> {
    frame_rows(frame)
        .into_iter()
        .map(|row| row.chars().take(width as usize).collect())
        .collect()
}

#[test]
fn spaces_title_click_collapses_expands_and_persists() {
    let path = preferences_path("spaces-section");
    let mut state = section_state(Some(path.clone()));
    let frame = state.compose(106, 30).expect("expanded sidebar");
    let width = state.hits.sidebar_divider.x;
    let rows = sidebar_rows(&frame, width);
    assert!(rows[0].starts_with(" spaces"), "{rows:?}");
    assert!(!rows.iter().any(|row| row.contains('▸')), "{rows:?}");
    assert!(!state.hits.workspaces.is_empty());
    assert_ne!(state.hits.sidebar_section_divider, Rect::default());
    let spaces_title = title_rect(&state, SidebarSection::Spaces);
    assert_eq!(spaces_title.y, 0);

    let outcome = click(&mut state, spaces_title.x + 2, spaces_title.y);
    assert!(outcome.repaint);
    assert!(outcome.actions.is_empty());
    assert!(state
        .collapsed_sidebar_sections
        .contains(&SidebarSection::Spaces));

    let frame = state.compose(106, 30).expect("collapsed spaces");
    let rows = sidebar_rows(&frame, width);
    assert!(rows[0].starts_with("▸ spaces"), "{rows:?}");
    assert!(rows[0].trim_end().ends_with("menu"), "{rows:?}");
    assert!(rows[1].starts_with('─'), "{rows:?}");
    assert!(rows[2].starts_with(" agents"), "{rows:?}");
    assert!(!rows.iter().any(|row| row.starts_with(" new")));
    assert!(state.hits.workspaces.is_empty());
    assert_eq!(state.hits.new_workspace, Rect::default());
    assert_eq!(state.hits.global_launcher.y, 0);
    assert_eq!(state.hits.sidebar_section_divider, Rect::default());
    assert_eq!(state.hits.agent_body.y, 4);
    assert!(!state.hits.agents.is_empty());

    let reloaded = ClientShellState::new(
        ClientShellConfig::from_config(&Config::default()).with_preferences_path(path.clone()),
    );
    assert_eq!(
        reloaded
            .collapsed_sidebar_sections
            .iter()
            .copied()
            .collect::<Vec<_>>(),
        [SidebarSection::Spaces]
    );

    let spaces_title = title_rect(&state, SidebarSection::Spaces);
    click(&mut state, spaces_title.x, spaces_title.y);
    assert!(state.collapsed_sidebar_sections.is_empty());
    let frame = state.compose(106, 30).expect("re-expanded spaces");
    let rows = sidebar_rows(&frame, width);
    assert!(rows[0].starts_with(" spaces"), "{rows:?}");
    assert!(!state.hits.workspaces.is_empty());
    assert_ne!(state.hits.sidebar_section_divider, Rect::default());
    let reloaded = ClientShellState::new(
        ClientShellConfig::from_config(&Config::default()).with_preferences_path(path.clone()),
    );
    assert!(reloaded.collapsed_sidebar_sections.is_empty());
    std::fs::remove_file(path).expect("remove section preferences");
}

#[test]
fn agents_title_click_collapses_without_touching_the_sort_toggle() {
    let mut state = section_state(None);
    state.compose(106, 30).expect("expanded sidebar");
    let width = state.hits.sidebar_divider.x;
    let agents_title = title_rect(&state, SidebarSection::Agents);
    let sort_toggle = state.hits.agent_sort_toggle;
    assert_eq!(agents_title.y, sort_toggle.y);
    assert!(agents_title.right() <= sort_toggle.x);
    let expanded_body = state.hits.workspace_body;

    click(&mut state, sort_toggle.x, sort_toggle.y);
    assert!(state.collapsed_sidebar_sections.is_empty());
    assert_eq!(
        state.config.agent_panel_sort,
        crate::config::AgentPanelSortConfig::Priority
    );

    state.compose(106, 30).expect("priority sidebar");
    let agents_title = title_rect(&state, SidebarSection::Agents);
    click(&mut state, agents_title.x + 1, agents_title.y);
    assert!(state
        .collapsed_sidebar_sections
        .contains(&SidebarSection::Agents));
    let frame = state.compose(106, 30).expect("collapsed agents");
    let rows = sidebar_rows(&frame, width);
    assert!(rows[29].starts_with("▸ agents"), "{rows:?}");
    assert!(rows[28].starts_with('─'), "{rows:?}");
    assert!(!rows.iter().any(|row| row.contains("priority")), "{rows:?}");
    assert!(!rows.iter().any(|row| row.contains("alpha")), "{rows:?}");
    assert!(state.hits.agents.is_empty());
    assert_eq!(state.hits.agent_sort_toggle, Rect::default());
    assert_eq!(state.hits.sidebar_section_divider, Rect::default());
    assert!(state.hits.workspace_body.height > expanded_body.height);
    assert_eq!(state.hits.workspace_body.bottom(), 27);

    let agents_title = title_rect(&state, SidebarSection::Agents);
    assert_eq!(agents_title.y, 29);
    click(&mut state, agents_title.x, agents_title.y);
    assert!(state.collapsed_sidebar_sections.is_empty());
    state.compose(106, 30).expect("re-expanded agents");
    assert!(!state.hits.agents.is_empty());
    assert_ne!(state.hits.agent_sort_toggle, Rect::default());
}

#[test]
fn collapsed_sections_stack_and_keep_the_sidebar_toggle() {
    let mut state = section_state(None);
    state.collapsed_sidebar_sections = [SidebarSection::Agents].into_iter().collect();
    state.compose(106, 30).expect("collapsed agents");
    let toggle = state.hits.sidebar_toggle;
    let agents_title = title_rect(&state, SidebarSection::Agents);
    assert_eq!(toggle.y, agents_title.y);
    click(&mut state, toggle.x, toggle.y);
    assert!(state.sidebar_collapsed);
    assert!(state
        .collapsed_sidebar_sections
        .contains(&SidebarSection::Agents));

    state.sidebar_collapsed = false;
    state.collapsed_sidebar_sections = [SidebarSection::Spaces, SidebarSection::Agents]
        .into_iter()
        .collect();
    let frame = state.compose(106, 30).expect("both collapsed");
    let width = state.hits.sidebar_divider.x;
    let rows = sidebar_rows(&frame, width);
    assert!(rows[0].starts_with("▸ spaces"), "{rows:?}");
    assert!(rows[1].starts_with('─'), "{rows:?}");
    assert!(rows[2].starts_with("▸ agents"), "{rows:?}");
    assert!(
        rows[3..29].iter().all(|row| row.trim().is_empty()),
        "{rows:?}"
    );
    assert!(state.hits.workspaces.is_empty());
    assert!(state.hits.agents.is_empty());
    assert_eq!(state.hits.sidebar_section_divider, Rect::default());
}

#[test]
fn machines_title_collapses_in_the_multi_endpoint_sidebar() {
    let mut state = section_state(None);
    let profile = crate::client::endpoint::SavedSshEndpoint {
        id: crate::client::endpoint::ProfileId::parse("0123456789abcdef0123456789abcdef")
            .expect("profile id"),
        label: "Build".into(),
        target: "dev@build.example".into(),
        session: "agents".into(),
        enabled: true,
    };
    let endpoint_id = ClientEndpointId::Ssh(profile.id.clone());
    state.set_endpoint_catalog(&[profile]);
    state.set_endpoint_status(&endpoint_id, ClientEndpointStatus::Online);
    state.set_endpoint_snapshot(&endpoint_id, Box::new(snapshot()));
    state.compose(120, 40).expect("machines sidebar");
    assert!(!state.hits.machines.is_empty());
    let machines_title = title_rect(&state, SidebarSection::Spaces);

    click(&mut state, machines_title.x + 1, machines_title.y);
    assert!(state
        .collapsed_sidebar_sections
        .contains(&SidebarSection::Spaces));
    let frame = state.compose(120, 40).expect("collapsed machines");
    let width = state.hits.sidebar_divider.x;
    let rows = sidebar_rows(&frame, width);
    assert!(rows[0].starts_with("▸ machines"), "{rows:?}");
    assert!(state.hits.machines.is_empty());
    assert!(state.hits.workspaces.is_empty());
    assert!(!state.hits.endpoint_agents.is_empty());
    assert_eq!(state.hits.global_launcher.y, 0);
}
