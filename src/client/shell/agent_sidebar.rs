use std::collections::HashMap;

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Modifier, Style},
    text::Line,
    widgets::{Paragraph, Widget},
};

use super::*;

pub(super) struct AgentRow {
    pub(super) pane_id: String,
    pub(super) workspace_id: String,
    pub(super) status: crate::api::schema::AgentStatus,
    pub(super) focused: bool,
    pub(super) rows: Vec<Vec<crate::ui::ResolvedToken>>,
}

pub(super) fn ordered_agent_pane_ids(
    snapshot: &ClientShellSnapshot,
    sort: crate::config::AgentPanelSortConfig,
) -> Vec<String> {
    if snapshot.agent_view_label.is_some() {
        return snapshot
            .agent_order
            .iter()
            .filter(|pane_id| {
                snapshot
                    .agents
                    .iter()
                    .any(|agent| agent.pane_id == pane_id.as_str())
            })
            .cloned()
            .collect();
    }
    let mut agents = snapshot.agents.iter().collect::<Vec<_>>();
    if sort == crate::config::AgentPanelSortConfig::Priority {
        agents.sort_by_key(|agent| {
            (
                std::cmp::Reverse(status_priority(agent.agent_status)),
                std::cmp::Reverse(agent.state_change_seq),
            )
        });
    }
    agents
        .into_iter()
        .map(|agent| agent.pane_id.clone())
        .collect()
}

/// Whether the agent panel nests agents under space headers.
pub(super) fn agent_panel_grouped(
    snapshot: &ClientShellSnapshot,
    config: &ClientShellConfig,
) -> bool {
    config.agents.group_by_space
        && config.agent_panel_sort == crate::config::AgentPanelSortConfig::Spaces
        && snapshot.agent_view_label.is_none()
}

/// Agent panel entries in display order: `None` for a space header, `Some(pane_id)` for a
/// visible agent. Agents of collapsed spaces are omitted.
fn agent_panel_layout(
    snapshot: &ClientShellSnapshot,
    config: &ClientShellConfig,
    collapsed_agent_groups: &HashSet<String>,
) -> Vec<Option<String>> {
    let grouped = agent_panel_grouped(snapshot, config);
    let mut entries = Vec::new();
    let mut current: Option<String> = None;
    for pane_id in ordered_agent_pane_ids(snapshot, config.agent_panel_sort) {
        let Some(workspace_id) = snapshot
            .agents
            .iter()
            .find(|agent| agent.pane_id == pane_id)
            .map(|agent| agent.workspace_id.as_str())
            .filter(|workspace_id| {
                snapshot
                    .workspaces
                    .iter()
                    .any(|workspace| workspace.workspace_id == *workspace_id)
            })
        else {
            continue;
        };
        if !grouped {
            entries.push(Some(pane_id));
            continue;
        }
        if current.as_deref() != Some(workspace_id) {
            entries.push(None);
            current = Some(workspace_id.to_owned());
        }
        if !collapsed_agent_groups.contains(workspace_id) {
            entries.push(Some(pane_id));
        }
    }
    entries
}

/// Agents the panel shows, in order; agents of collapsed spaces are skipped.
pub(super) fn visible_agent_pane_ids(
    snapshot: &ClientShellSnapshot,
    config: &ClientShellConfig,
    collapsed_agent_groups: &HashSet<String>,
) -> Vec<String> {
    agent_panel_layout(snapshot, config, collapsed_agent_groups)
        .into_iter()
        .flatten()
        .collect()
}

/// Index of the panel entry showing `pane_id`, counting space headers.
pub(super) fn agent_entry_index(
    snapshot: &ClientShellSnapshot,
    config: &ClientShellConfig,
    collapsed_agent_groups: &HashSet<String>,
    pane_id: &str,
) -> Option<usize> {
    agent_panel_layout(snapshot, config, collapsed_agent_groups)
        .iter()
        .position(|entry| entry.as_deref() == Some(pane_id))
}

pub(super) fn render_agent_panel(
    buffer: &mut Buffer,
    area: Rect,
    snapshot: &ClientShellSnapshot,
    config: &ClientShellConfig,
    agent_scroll: &mut usize,
    dragged_pane_id: Option<&str>,
    drop_indicator_row: Option<u16>,
    collapsed_agent_groups: &HashSet<String>,
    hits: &mut ShellHitMap,
) {
    if !render_agent_panel_header(
        buffer,
        area,
        snapshot.agent_view_label.as_deref(),
        config,
        hits,
    ) {
        return;
    }

    let rows = agent_rows(snapshot, config, None);
    let grouped = agent_panel_grouped(snapshot, config);
    let entries = if grouped {
        grouped_agent_entries(snapshot, rows, collapsed_agent_groups)
    } else {
        rows.into_iter().map(AgentPanelEntry::Agent).collect()
    };
    render_agent_list(
        buffer,
        area,
        &entries,
        snapshot
            .agent_view_label
            .as_ref()
            .map(|_| " no matching agents"),
        config,
        agent_scroll,
        hits,
        |entry| match entry {
            AgentPanelEntry::Header { .. } => 1,
            AgentPanelEntry::Agent(row) => row.rows.len(),
        },
        |buffer, rect, entry, hits| match entry {
            AgentPanelEntry::Header(header) => {
                hits.agent_entries.push(AgentEntryHit {
                    rect,
                    workspace_id: header.workspace_id.clone(),
                    pane_id: None,
                });
                render_space_header(buffer, rect, header, config);
            }
            AgentPanelEntry::Agent(row) => {
                hits.agents.push((rect, row.pane_id.clone()));
                if grouped {
                    hits.agent_entries.push(AgentEntryHit {
                        rect,
                        workspace_id: row.workspace_id.clone(),
                        pane_id: Some(row.pane_id.clone()),
                    });
                    if dragged_pane_id == Some(row.pane_id.as_str()) {
                        buffer.set_style(rect, Style::default().bg(config.palette.surface1));
                    } else if row.focused {
                        buffer.set_style(rect, Style::default().bg(config.palette.active_row_bg));
                    }
                    let nested = Rect::new(
                        rect.x.saturating_add(GROUP_INDENT),
                        rect.y,
                        rect.width.saturating_sub(GROUP_INDENT),
                        rect.height,
                    );
                    render_agent_row(buffer, nested, row, config);
                } else {
                    render_agent_row(buffer, rect, row, config);
                }
            }
        },
    );
    if let Some(row) = drop_indicator_row
        .filter(|row| *row >= hits.agent_body.y && *row < hits.agent_body.bottom())
    {
        put_text(
            buffer,
            hits.agent_body.x,
            row,
            hits.agent_body.width,
            &"─".repeat(hits.agent_body.width as usize),
            Style::default().fg(config.palette.accent),
        );
    }
}

const GROUP_INDENT: u16 = 2;

struct SpaceHeader {
    workspace_id: String,
    label: String,
    focused: bool,
    collapsed: bool,
    /// Agents hidden by the collapse, their most urgent status, and whether one is focused.
    hidden_agents: usize,
    hidden_status: Option<crate::api::schema::AgentStatus>,
    hidden_focused: bool,
}

enum AgentPanelEntry {
    Header(SpaceHeader),
    Agent(AgentRow),
}

/// Inserts a header before each run of agents that share a space; agents of collapsed spaces
/// fold into their header.
fn grouped_agent_entries(
    snapshot: &ClientShellSnapshot,
    rows: Vec<AgentRow>,
    collapsed_agent_groups: &HashSet<String>,
) -> Vec<AgentPanelEntry> {
    let mut entries = Vec::with_capacity(rows.len() * 2);
    let mut current: Option<String> = None;
    for row in rows {
        if current.as_deref() != Some(row.workspace_id.as_str()) {
            let workspace = snapshot
                .workspaces
                .iter()
                .find(|workspace| workspace.workspace_id == row.workspace_id);
            entries.push(AgentPanelEntry::Header(SpaceHeader {
                workspace_id: row.workspace_id.clone(),
                label: workspace.map_or_else(String::new, |workspace| workspace.label.clone()),
                focused: workspace.is_some_and(|workspace| workspace.focused),
                collapsed: collapsed_agent_groups.contains(&row.workspace_id),
                hidden_agents: 0,
                hidden_status: None,
                hidden_focused: false,
            }));
            current = Some(row.workspace_id.clone());
        }
        if let Some(AgentPanelEntry::Header(header)) = entries
            .last_mut()
            .filter(|_| collapsed_agent_groups.contains(&row.workspace_id))
        {
            header.hidden_agents += 1;
            header.hidden_focused |= row.focused;
            header.hidden_status = header
                .hidden_status
                .into_iter()
                .chain([row.status])
                .max_by_key(|status| status_priority(*status));
            continue;
        }
        entries.push(AgentPanelEntry::Agent(row));
    }
    entries
}

fn render_space_header(
    buffer: &mut Buffer,
    rect: Rect,
    header: &SpaceHeader,
    config: &ClientShellConfig,
) {
    let palette = &config.palette;
    if header.hidden_focused {
        buffer.set_style(rect, Style::default().bg(palette.active_row_bg));
    }
    let style = Style::default()
        .fg(if header.focused {
            palette.text
        } else {
            palette.overlay1
        })
        .add_modifier(Modifier::BOLD);
    let marker = if header.collapsed { '▸' } else { '▾' };
    let mut label_width = rect.width.saturating_sub(1);
    if header.collapsed {
        let count = header.hidden_agents.to_string();
        let count_width = display_width(&count) as u16;
        let count_x = rect.right().saturating_sub(count_width.saturating_add(1));
        put_text(
            buffer,
            count_x,
            rect.y,
            count_width,
            &count,
            Style::default()
                .fg(palette.overlay0)
                .add_modifier(Modifier::DIM),
        );
        let mut suffix_x = count_x;
        if let Some(status) = header.hidden_status.filter(|status| {
            matches!(
                status,
                crate::api::schema::AgentStatus::Blocked
                    | crate::api::schema::AgentStatus::Done
                    | crate::api::schema::AgentStatus::Working
            )
        }) {
            let icon = status_icon(status, config.status_indicators);
            let icon_width = display_width(icon) as u16;
            suffix_x = count_x.saturating_sub(icon_width.saturating_add(1));
            put_text(
                buffer,
                suffix_x,
                rect.y,
                icon_width,
                icon,
                Style::default().fg(status_color(status, palette)),
            );
        }
        label_width = suffix_x
            .saturating_sub(rect.x.saturating_add(2))
            .min(label_width);
    }
    put_text(
        buffer,
        rect.x.saturating_add(1),
        rect.y,
        label_width,
        &format!("{marker} {}", header.label),
        style,
    );
}

pub(super) fn render_agent_panel_header(
    buffer: &mut Buffer,
    area: Rect,
    agent_view_label: Option<&str>,
    config: &ClientShellConfig,
    hits: &mut ShellHitMap,
) -> bool {
    if area.height == 0 {
        return false;
    }
    put_text(
        buffer,
        area.x,
        area.y,
        area.width,
        &"─".repeat(area.width as usize),
        Style::default().fg(config.palette.surface_dim),
    );
    if area.height < 2 {
        return false;
    }
    put_text(
        buffer,
        area.x,
        area.y + 1,
        area.width,
        " agents",
        Style::default()
            .fg(config.palette.overlay0)
            .add_modifier(Modifier::BOLD),
    );
    let sort_label = agent_view_label.unwrap_or(match config.agent_panel_sort {
        crate::config::AgentPanelSortConfig::Spaces => "grouped",
        crate::config::AgentPanelSortConfig::Priority => "priority",
    });
    let sort_width = display_width(sort_label).min(area.width as usize) as u16;
    let sort_rect = Rect::new(
        area.right().saturating_sub(sort_width),
        area.y + 1,
        sort_width,
        1,
    );
    hits.agent_sort_toggle = if config.mouse_capture && agent_view_label.is_none() {
        sort_rect
    } else {
        Rect::default()
    };
    put_text(
        buffer,
        sort_rect.x,
        sort_rect.y,
        sort_rect.width,
        sort_label,
        Style::default()
            .fg(if agent_view_label.is_some() {
                config.palette.accent
            } else {
                config.palette.overlay0
            })
            .add_modifier(Modifier::BOLD),
    );
    true
}

pub(super) fn render_agent_list<T>(
    buffer: &mut Buffer,
    area: Rect,
    rows: &[T],
    empty_message: Option<&str>,
    config: &ClientShellConfig,
    agent_scroll: &mut usize,
    hits: &mut ShellHitMap,
    row_lines: impl Fn(&T) -> usize,
    mut render_row: impl FnMut(&mut Buffer, Rect, &T, &mut ShellHitMap),
) {
    let body = Rect::new(
        area.x,
        area.y.saturating_add(3),
        area.width,
        area.height.saturating_sub(3),
    );
    hits.agent_body = body;
    if body.is_empty() || rows.is_empty() {
        *agent_scroll = 0;
        if let Some(message) = empty_message.filter(|_| !body.is_empty()) {
            put_text(
                buffer,
                body.x,
                body.y,
                body.width,
                message,
                Style::default()
                    .fg(config.palette.overlay0)
                    .add_modifier(Modifier::DIM),
            );
        }
        return;
    }

    let row_heights = rows
        .iter()
        .map(|row| row_lines(row).max(1).min(u16::MAX as usize) as u16)
        .collect::<Vec<_>>();
    let gaps = rows
        .iter()
        .enumerate()
        .map(|(index, _)| {
            if index + 1 < rows.len() {
                config.agents.row_gap
            } else {
                0
            }
        })
        .collect::<Vec<_>>();
    let metrics =
        super::scroll::list_scroll_metrics(&row_heights, &gaps, body.height, *agent_scroll);
    hits.agent_max_scroll = metrics.max_offset_from_bottom;
    hits.agent_scroll_metrics = Some(metrics);
    *agent_scroll = metrics
        .max_offset_from_bottom
        .saturating_sub(metrics.offset_from_bottom);
    let show_scrollbar = metrics.max_offset_from_bottom > 0 && body.width > 1;
    let content_width = body.width.saturating_sub(u16::from(show_scrollbar));
    let mut y = body.y;
    for (index, row) in rows.iter().enumerate().skip(*agent_scroll) {
        let height = row_heights[index].min(body.height);
        if y.saturating_add(height) > body.bottom() {
            break;
        }
        let rect = Rect::new(body.x, y, content_width, height);
        render_row(buffer, rect, row, hits);
        y = y
            .saturating_add(height)
            .saturating_add(if index + 1 < rows.len() {
                config.agents.row_gap
            } else {
                0
            });
    }

    if show_scrollbar {
        let track = Rect::new(body.right().saturating_sub(1), body.y, 1, body.height);
        hits.agent_scrollbar = track;
        super::scroll::render_list_scrollbar(buffer, track, metrics, &config.palette);
    }
}

pub(super) fn agent_rows(
    snapshot: &ClientShellSnapshot,
    config: &ClientShellConfig,
    machine: Option<&str>,
) -> Vec<AgentRow> {
    ordered_agent_pane_ids(snapshot, config.agent_panel_sort)
        .into_iter()
        .filter_map(|pane_id| agent_row(snapshot, &pane_id, config, machine))
        .collect()
}

pub(super) fn agent_row(
    snapshot: &ClientShellSnapshot,
    pane_id: &str,
    config: &ClientShellConfig,
    machine: Option<&str>,
) -> Option<AgentRow> {
    let agent = snapshot
        .agents
        .iter()
        .find(|agent| agent.pane_id == pane_id)?;
    let workspace = snapshot
        .workspaces
        .iter()
        .find(|workspace| workspace.workspace_id == agent.workspace_id)?;
    let tab = snapshot.tabs.iter().find(|tab| tab.tab_id == agent.tab_id);
    let pane = snapshot
        .panes
        .iter()
        .find(|pane| pane.pane_id == agent.pane_id);
    let tab_count = snapshot
        .tabs
        .iter()
        .filter(|candidate| candidate.workspace_id == agent.workspace_id)
        .count();
    let tab_label = tab
        .filter(|tab| tab_count > 1 || tab.custom_label)
        .map(|tab| tab.label.as_str());
    let agent_label = agent
        .display_agent
        .as_deref()
        .or(agent.name.as_deref())
        .or(agent.agent.as_deref())
        .or(agent.title.as_deref());
    let labels = agent
        .state_labels
        .iter()
        .cloned()
        .collect::<HashMap<_, _>>();
    let tokens = agent.tokens.iter().cloned().collect::<HashMap<_, _>>();
    let state_text = labels
        .get(status_text(agent.agent_status))
        .map(String::as_str)
        .unwrap_or_else(|| sidebar_status_text(agent.agent_status));
    let canonical_agent = agent
        .agent
        .as_deref()
        .and_then(crate::detect::parse_agent_label);
    let rows = crate::ui::sidebar_agent_rows(
        &config.agents,
        crate::ui::AgentTokenContext {
            machine,
            workspace: &workspace.label,
            tab: tab_label,
            pane: agent
                .title
                .as_deref()
                .or_else(|| pane.and_then(|pane| pane.label.as_deref())),
            agent_label,
            terminal_title: agent.terminal_title.as_deref(),
            terminal_title_stripped: agent.terminal_title_stripped.as_deref(),
            canonical_agent,
            tokens: &tokens,
        },
        state_text,
    );
    Some(AgentRow {
        pane_id: agent.pane_id.clone(),
        workspace_id: agent.workspace_id.clone(),
        status: agent.agent_status,
        focused: agent.focused,
        rows,
    })
}

pub(super) fn render_agent_row(
    buffer: &mut Buffer,
    rect: Rect,
    row: &AgentRow,
    config: &ClientShellConfig,
) {
    let palette = &config.palette;
    let row_style = if row.focused {
        Style::default().bg(palette.active_row_bg)
    } else {
        Style::default()
    };
    let name_style = if row.focused {
        Style::default()
            .fg(palette.text)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default()
            .fg(palette.subtext0)
            .add_modifier(Modifier::BOLD)
    };
    let status_style = Style::default().fg(status_color(row.status, palette));
    let secondary = Style::default().fg(palette.overlay0);
    let icon = (
        status_icon(row.status, config.status_indicators),
        Style::default().fg(status_color(row.status, palette)),
    );
    let rows = if row.rows.is_empty() {
        vec![vec![crate::ui::ResolvedToken {
            kind: crate::ui::ResolvedTokenKind::StateIcon,
            style: Default::default(),
        }]]
    } else {
        row.rows.clone()
    };
    for (index, tokens) in rows.iter().take(rect.height as usize).enumerate() {
        let indent = if index == 0 { 1 } else { 3 };
        let mut spans = vec![ratatui::text::Span::raw(" ".repeat(indent))];
        spans.extend(crate::ui::resolved_token_spans(
            tokens,
            icon,
            status_style,
            name_style,
            secondary,
            secondary,
            palette,
            rect.width.saturating_sub(indent as u16) as usize,
        ));
        Paragraph::new(Line::from(spans)).style(row_style).render(
            Rect::new(rect.x, rect.y + index as u16, rect.width, 1),
            buffer,
        );
    }
}

fn put_text(buffer: &mut Buffer, x: u16, y: u16, width: u16, text: &str, style: Style) {
    for (offset, character) in text.chars().take(width as usize).enumerate() {
        if let Some(cell) = buffer.cell_mut((x + offset as u16, y)) {
            cell.set_char(character).set_style(style);
        }
    }
}

fn display_width(text: &str) -> usize {
    unicode_width::UnicodeWidthStr::width(text)
}

fn sidebar_status_text(status: crate::api::schema::AgentStatus) -> &'static str {
    use crate::api::schema::AgentStatus;
    match status {
        AgentStatus::Blocked => "blocked",
        AgentStatus::Done => "done",
        AgentStatus::Working => "working",
        AgentStatus::Idle | AgentStatus::Unknown => "idle",
    }
}
