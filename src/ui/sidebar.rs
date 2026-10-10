mod tokens;

use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::Span,
};

pub(crate) use self::tokens::{
    agent_rows as sidebar_agent_rows, space_rows as sidebar_space_rows, AgentTokenContext,
    ResolvedToken, ResolvedTokenKind, SpaceTokenContext,
};
use super::text::{display_width, truncate_end};
use crate::app::state::Palette;
use crate::app::AppState;
use crate::detect::AgentState;
use crate::terminal::TerminalRuntimeRegistry;

pub(crate) struct AgentPanelEntry {
    pub ws_idx: usize,
    pub tab_idx: usize,
    pub pane_id: crate::layout::PaneId,
    pub agent_kind_label: Option<String>,
    pub state: AgentState,
    pub seen: bool,
    pub last_agent_state_change_seq: Option<u64>,
    pub tokens: std::collections::HashMap<String, String>,
}

fn sidebar_section_heights(total_height: u16, split_ratio: f32) -> (u16, u16) {
    if total_height == 0 {
        return (0, 0);
    }
    if total_height < 6 {
        let workspace_height = total_height.div_ceil(2);
        return (
            workspace_height,
            total_height.saturating_sub(workspace_height),
        );
    }

    let workspace_height = ((total_height as f32) * split_ratio.clamp(0.1, 0.9)).round() as u16;
    let workspace_height = workspace_height.clamp(3, total_height.saturating_sub(3));
    (
        workspace_height,
        total_height.saturating_sub(workspace_height),
    )
}

/// A titled expanded-sidebar section that can be collapsed to its title row.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum SidebarSection {
    Projects,
    Spaces,
    Agents,
}

impl SidebarSection {
    /// Stable name used by persisted client preferences.
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Projects => "projects",
            Self::Spaces => "spaces",
            Self::Agents => "agents",
        }
    }

    pub(crate) fn from_name(name: &str) -> Option<Self> {
        match name {
            "projects" => Some(Self::Projects),
            "spaces" => Some(Self::Spaces),
            "agents" => Some(Self::Agents),
            _ => None,
        }
    }
}

/// Rows kept by a collapsed projects or spaces section: the title row.
pub(crate) const COLLAPSED_SECTION_ROWS: u16 = 1;
/// Rows kept by a collapsed agents section: the `─` rule above it plus its title row.
pub(crate) const COLLAPSED_AGENT_SECTION_ROWS: u16 = 2;

/// Geometry of the expanded sidebar sections, top to bottom.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct SidebarSectionRects {
    /// Projects section with its title on row 0; `None` when there is no projects section.
    pub(crate) projects: Option<Rect>,
    /// One-row `─` rule between projects and spaces; not draggable.
    pub(crate) projects_divider: Option<Rect>,
    /// Spaces section with its title on row 0.
    pub(crate) spaces: Rect,
    /// Agents section: row 0 is the `─` rule, row 1 its title.
    pub(crate) agents: Rect,
    /// Draggable spaces/agents divider (agents row 0); only when both are expanded.
    pub(crate) divider: Option<Rect>,
    /// Rows shared by spaces and agents; the split ratio is relative to this area.
    pub(crate) split_area: Rect,
}

/// Lays out the expanded sidebar sections.
///
/// `projects_rows` is the natural height of an optional projects section above spaces (0 means
/// no section). Expanded projects take at most 40% of the height and ignore the split ratio.
/// Collapsed sections keep only their title row (agents also keeps its `─` rule). Spaces and
/// agents share the rest with `split_ratio`; when one is collapsed the other takes all of it,
/// and when both are collapsed they stack at the top, leaving the rest empty.
pub(crate) fn sidebar_section_rects(
    area: Rect,
    split_ratio: f32,
    collapsed: &std::collections::BTreeSet<SidebarSection>,
    projects_rows: u16,
) -> SidebarSectionRects {
    let content = Rect::new(area.x, area.y, area.width.saturating_sub(1), area.height);
    if content.is_empty() {
        return SidebarSectionRects::default();
    }
    let row = |y: u16, height: u16| Rect::new(content.x, y, content.width, height);
    let mut y = content.y;
    let mut remaining = content.height;
    let mut projects = None;
    let mut projects_divider = None;
    if projects_rows > 0 {
        let height = if collapsed.contains(&SidebarSection::Projects) {
            COLLAPSED_SECTION_ROWS
        } else {
            projects_rows.min(content.height * 2 / 5).max(1)
        }
        .min(remaining);
        projects = Some(row(y, height));
        y += height;
        remaining -= height;
        if remaining > 0 {
            projects_divider = Some(row(y, 1));
            y += 1;
            remaining -= 1;
        }
    }

    let split_area = row(y, remaining);
    let (spaces_height, agents_height) = match (
        collapsed.contains(&SidebarSection::Spaces),
        collapsed.contains(&SidebarSection::Agents),
    ) {
        (false, false) => sidebar_section_heights(remaining, split_ratio),
        (true, false) => {
            let spaces = COLLAPSED_SECTION_ROWS.min(remaining);
            (spaces, remaining - spaces)
        }
        (false, true) => {
            let agents = COLLAPSED_AGENT_SECTION_ROWS.min(remaining);
            (remaining - agents, agents)
        }
        (true, true) => {
            let spaces = COLLAPSED_SECTION_ROWS.min(remaining);
            (spaces, COLLAPSED_AGENT_SECTION_ROWS.min(remaining - spaces))
        }
    };
    let spaces = row(y, spaces_height);
    let agents = row(y + spaces_height, agents_height);
    let both_expanded = !collapsed.contains(&SidebarSection::Spaces)
        && !collapsed.contains(&SidebarSection::Agents);
    let divider = (both_expanded && remaining >= 6).then(|| row(agents.y, 1));
    SidebarSectionRects {
        projects,
        projects_divider,
        spaces,
        agents,
        divider,
        split_area,
    }
}

pub(crate) fn agent_panel_entries_from(
    app: &AppState,
    _terminal_runtimes: &TerminalRuntimeRegistry,
) -> Vec<AgentPanelEntry> {
    let mut entries = app
        .workspaces
        .iter()
        .enumerate()
        .flat_map(|(ws_idx, workspace)| {
            workspace
                .pane_details(&app.terminals)
                .into_iter()
                .map(move |detail| AgentPanelEntry {
                    ws_idx,
                    tab_idx: detail.tab_idx,
                    pane_id: detail.pane_id,
                    agent_kind_label: detail.agent_kind_label,
                    state: detail.state,
                    seen: detail.seen,
                    last_agent_state_change_seq: detail.last_agent_state_change_seq,
                    tokens: detail.tokens,
                })
        })
        .collect();
    crate::app::agent_view::apply_agent_view(app, &mut entries);
    entries
}

pub(crate) fn resolved_token_spans(
    resolved: &[ResolvedToken],
    state_icon: (&str, Style),
    state_text_style: Style,
    workspace_style: Style,
    secondary_style: Style,
    custom_style: Style,
    palette: &Palette,
    max_width: usize,
) -> Vec<Span<'static>> {
    let fixed_widths = resolved
        .iter()
        .map(|token| match &token.kind {
            ResolvedTokenKind::StateIcon => display_width(state_icon.0),
            ResolvedTokenKind::GitStatus { ahead, behind } => {
                usize::from(*ahead > 0) * display_width(&format!("↑{ahead}"))
                    + usize::from(*behind > 0) * display_width(&format!("↓{behind}"))
                    + usize::from(*ahead > 0 && *behind > 0)
            }
            _ => 0,
        })
        .collect::<Vec<_>>();
    let flexible_widths = resolved
        .iter()
        .map(|token| match &token.kind {
            ResolvedTokenKind::StateText(text)
            | ResolvedTokenKind::Machine(text)
            | ResolvedTokenKind::Workspace(text)
            | ResolvedTokenKind::Tab(text)
            | ResolvedTokenKind::Pane(text)
            | ResolvedTokenKind::Agent(text)
            | ResolvedTokenKind::TerminalTitle(text)
            | ResolvedTokenKind::Branch(text)
            | ResolvedTokenKind::Custom(text) => display_width(text),
            _ => 0,
        })
        .collect::<Vec<_>>();
    let minimum_width = |active: &[bool]| {
        let indices = active
            .iter()
            .enumerate()
            .filter_map(|(index, active)| active.then_some(index))
            .collect::<Vec<_>>();
        let content = indices
            .iter()
            .map(|index| fixed_widths[*index] + usize::from(flexible_widths[*index] > 0))
            .sum::<usize>();
        let separators = indices
            .windows(2)
            .map(|pair| display_width(tokens::separator(&resolved[pair[0]], &resolved[pair[1]])))
            .sum::<usize>();
        content + separators
    };
    let mut active = resolved.iter().map(|_| true).collect::<Vec<_>>();
    if minimum_width(&active) > max_width {
        for (index, width) in flexible_widths.iter().enumerate() {
            if *width > 0 {
                active[index] = false;
            }
        }
        for index in (0..resolved.len()).rev() {
            if flexible_widths[index] == 0 {
                continue;
            }
            active[index] = true;
            if minimum_width(&active) > max_width {
                active[index] = false;
            }
        }
    }
    let visible_indices = active
        .iter()
        .enumerate()
        .filter_map(|(index, active)| active.then_some(index))
        .collect::<Vec<_>>();
    let separator_width = visible_indices
        .windows(2)
        .map(|pair| display_width(tokens::separator(&resolved[pair[0]], &resolved[pair[1]])))
        .sum::<usize>();
    let fixed_width = visible_indices
        .iter()
        .map(|index| fixed_widths[*index])
        .sum::<usize>();
    let mut budgets = flexible_widths
        .iter()
        .enumerate()
        .map(|(index, width)| usize::from(active[index] && *width > 0))
        .collect::<Vec<_>>();
    let minimum = budgets.iter().sum::<usize>();
    let mut remaining = max_width
        .saturating_sub(separator_width + fixed_width)
        .saturating_sub(minimum);
    while remaining > 0 {
        let mut grew = false;
        for (budget, width) in budgets.iter_mut().zip(&flexible_widths) {
            if *budget > 0 && *budget < *width {
                *budget += 1;
                remaining -= 1;
                grew = true;
                if remaining == 0 {
                    break;
                }
            }
        }
        if !grew {
            break;
        }
    }

    let mut spans = Vec::new();
    for (position, index) in visible_indices.iter().copied().enumerate() {
        let token = &resolved[index];
        if position > 0 {
            let previous = &resolved[visible_indices[position - 1]];
            spans.push(Span::styled(
                tokens::separator(previous, token),
                Style::default().fg(palette.overlay0),
            ));
        }
        match &token.kind {
            ResolvedTokenKind::StateIcon => spans.push(Span::styled(
                state_icon.0.to_string(),
                apply_token_style(state_icon.1, token.style),
            )),
            ResolvedTokenKind::StateText(text) => spans.push(Span::styled(
                truncate_end(text, budgets[index]),
                apply_token_style(state_text_style, token.style),
            )),
            ResolvedTokenKind::Workspace(text) => spans.push(Span::styled(
                truncate_end(text, budgets[index]),
                apply_token_style(workspace_style, token.style),
            )),
            ResolvedTokenKind::Machine(text)
            | ResolvedTokenKind::Tab(text)
            | ResolvedTokenKind::Pane(text)
            | ResolvedTokenKind::Agent(text)
            | ResolvedTokenKind::Branch(text) => spans.push(Span::styled(
                truncate_end(text, budgets[index]),
                apply_token_style(secondary_style, token.style),
            )),
            ResolvedTokenKind::GitStatus { ahead, behind } => {
                if *ahead > 0 {
                    spans.push(Span::styled(
                        format!("↑{ahead}"),
                        apply_token_style(Style::default().fg(palette.green), token.style),
                    ));
                }
                if *ahead > 0 && *behind > 0 {
                    spans.push(Span::styled(
                        " ",
                        apply_token_style(Style::default(), token.style),
                    ));
                }
                if *behind > 0 {
                    spans.push(Span::styled(
                        format!("↓{behind}"),
                        apply_token_style(Style::default().fg(palette.red), token.style),
                    ));
                }
            }
            ResolvedTokenKind::TerminalTitle(text) | ResolvedTokenKind::Custom(text) => {
                spans.push(Span::styled(
                    truncate_end(text, budgets[index]),
                    apply_token_style(custom_style, token.style),
                ));
            }
        }
    }
    spans
}

fn apply_token_style(mut style: Style, patch: crate::config::SidebarTokenStyle) -> Style {
    if let Some(foreground) = patch.fg {
        style = style.fg(foreground.ratatui());
    }
    if let Some(bold) = patch.bold {
        style = if bold {
            style.add_modifier(Modifier::BOLD)
        } else {
            style.remove_modifier(Modifier::BOLD)
        };
    }
    if let Some(dim) = patch.dim {
        style = if dim {
            style.add_modifier(Modifier::DIM)
        } else {
            style.remove_modifier(Modifier::DIM)
        };
    }
    style
}

#[cfg(test)]
mod section_layout_tests {
    use std::collections::BTreeSet;

    use super::*;

    fn collapsed(sections: &[SidebarSection]) -> BTreeSet<SidebarSection> {
        sections.iter().copied().collect()
    }

    #[test]
    fn expanded_spaces_and_agents_keep_the_split_layout() {
        let area = Rect::new(0, 0, 26, 30);
        let rects = sidebar_section_rects(area, 0.5, &collapsed(&[]), 0);
        assert_eq!(rects.projects, None);
        assert_eq!(rects.projects_divider, None);
        assert_eq!(rects.spaces, Rect::new(0, 0, 25, 15));
        assert_eq!(rects.agents, Rect::new(0, 15, 25, 15));
        assert_eq!(rects.divider, Some(Rect::new(0, 15, 25, 1)));
        assert_eq!(rects.split_area, Rect::new(0, 0, 25, 30));

        let skewed = sidebar_section_rects(area, 0.95, &collapsed(&[]), 0);
        assert_eq!(skewed.spaces.height, 27);
        assert_eq!(skewed.agents.height, 3);
    }

    #[test]
    fn collapsed_section_gives_its_rows_to_the_other() {
        let area = Rect::new(2, 1, 26, 30);
        let spaces = sidebar_section_rects(area, 0.3, &collapsed(&[SidebarSection::Spaces]), 0);
        assert_eq!(spaces.spaces, Rect::new(2, 1, 25, 1));
        assert_eq!(spaces.agents, Rect::new(2, 2, 25, 29));
        assert_eq!(spaces.divider, None);

        let agents = sidebar_section_rects(area, 0.3, &collapsed(&[SidebarSection::Agents]), 0);
        assert_eq!(agents.spaces, Rect::new(2, 1, 25, 28));
        assert_eq!(agents.agents, Rect::new(2, 29, 25, 2));
        assert_eq!(agents.divider, None);
    }

    #[test]
    fn both_collapsed_stack_at_the_top() {
        let area = Rect::new(0, 0, 26, 30);
        let rects = sidebar_section_rects(
            area,
            0.5,
            &collapsed(&[SidebarSection::Spaces, SidebarSection::Agents]),
            0,
        );
        assert_eq!(rects.spaces, Rect::new(0, 0, 25, 1));
        assert_eq!(rects.agents, Rect::new(0, 1, 25, 2));
        assert_eq!(rects.divider, None);
    }

    #[test]
    fn projects_take_natural_height_capped_at_forty_percent() {
        let area = Rect::new(0, 0, 26, 30);
        let small = sidebar_section_rects(area, 0.5, &collapsed(&[]), 6);
        assert_eq!(small.projects, Some(Rect::new(0, 0, 25, 6)));
        assert_eq!(small.projects_divider, Some(Rect::new(0, 6, 25, 1)));
        assert_eq!(small.split_area, Rect::new(0, 7, 25, 23));
        assert_eq!(small.spaces.y, 7);
        assert_eq!(small.agents.bottom(), 30);
        assert_eq!(small.divider, Some(Rect::new(0, small.agents.y, 25, 1)));

        let large = sidebar_section_rects(area, 0.5, &collapsed(&[]), 40);
        assert_eq!(large.projects, Some(Rect::new(0, 0, 25, 12)));
        assert_eq!(large.split_area, Rect::new(0, 13, 25, 17));
        assert_eq!(large.spaces.height + large.agents.height, 17);

        let folded = sidebar_section_rects(area, 0.5, &collapsed(&[SidebarSection::Projects]), 40);
        assert_eq!(folded.projects, Some(Rect::new(0, 0, 25, 1)));
        assert_eq!(folded.projects_divider, Some(Rect::new(0, 1, 25, 1)));
        assert_eq!(folded.split_area, Rect::new(0, 2, 25, 28));
    }

    #[test]
    fn projects_with_collapsed_spaces_and_agents() {
        let area = Rect::new(0, 0, 26, 30);
        let rects = sidebar_section_rects(
            area,
            0.5,
            &collapsed(&[SidebarSection::Spaces, SidebarSection::Agents]),
            8,
        );
        assert_eq!(rects.projects, Some(Rect::new(0, 0, 25, 8)));
        assert_eq!(rects.spaces, Rect::new(0, 9, 25, 1));
        assert_eq!(rects.agents, Rect::new(0, 10, 25, 2));
        assert_eq!(rects.divider, None);
    }

    #[test]
    fn tiny_heights_never_overflow_the_sidebar() {
        let all = [
            SidebarSection::Projects,
            SidebarSection::Spaces,
            SidebarSection::Agents,
        ];
        for height in 0..8 {
            for mask in 0..8u8 {
                let set = all
                    .iter()
                    .enumerate()
                    .filter(|(index, _)| mask & (1 << index) != 0)
                    .map(|(_, section)| *section)
                    .collect::<BTreeSet<_>>();
                for projects_rows in [0, 1, 4] {
                    let area = Rect::new(0, 3, 20, height);
                    let rects = sidebar_section_rects(area, 0.5, &set, projects_rows);
                    let used = rects.projects.map_or(0, |rect| rect.height)
                        + rects.projects_divider.map_or(0, |rect| rect.height)
                        + rects.spaces.height
                        + rects.agents.height;
                    assert!(used <= height, "height {height} mask {mask}: {rects:?}");
                    assert!(rects.agents.bottom() <= area.bottom() || rects.agents.height == 0);
                    if height < 6 {
                        assert_eq!(rects.divider, None);
                    }
                }
            }
        }
        assert_eq!(
            sidebar_section_rects(Rect::new(0, 0, 1, 10), 0.5, &BTreeSet::new(), 3),
            SidebarSectionRects::default()
        );
    }

    #[test]
    fn section_names_round_trip() {
        for section in [
            SidebarSection::Projects,
            SidebarSection::Spaces,
            SidebarSection::Agents,
        ] {
            assert_eq!(SidebarSection::from_name(section.name()), Some(section));
        }
        assert_eq!(SidebarSection::from_name("machines"), None);
    }
}
