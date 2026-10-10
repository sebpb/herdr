//! Renders the projects section of the single-machine sidebar from the cached
//! project list. No I/O happens here: the list is refreshed by the runtime and
//! the current project's spaces come from the live snapshot.

use super::*;
use crate::ui::SidebarSection;

/// Title row and a blank row, then two rows per project.
const PROJECT_HEADER_ROWS: u16 = 2;
const PROJECT_ROWS: u16 = 2;
const NEW_BUTTON: &str = "new";

pub(super) fn projects_natural_rows(projects: &[super::ProjectEntry]) -> u16 {
    let rows = u16::try_from(projects.len()).unwrap_or(u16::MAX);
    PROJECT_HEADER_ROWS.saturating_add(rows.saturating_mul(PROJECT_ROWS))
}

#[allow(clippy::too_many_arguments)] // Mirrors the other sidebar section renderers.
pub(super) fn render_projects_section(
    buffer: &mut Buffer,
    area: Rect,
    projects: &[super::ProjectEntry],
    snapshot: &ClientShellSnapshot,
    config: &ClientShellConfig,
    collapsed: bool,
    scroll: &mut usize,
    hits: &mut ShellHitMap,
) {
    if area.is_empty() {
        return;
    }
    let palette = &config.palette;
    // Keep `new` out of the title hit so it does not collapse the section.
    let new_width = if config.mouse_capture {
        (display_width(NEW_BUTTON) + 1).min(area.width)
    } else {
        0
    };
    super::sidebar::render_section_title(
        buffer,
        Rect::new(area.x, area.y, area.width - new_width, area.height),
        SidebarSection::Projects,
        "projects",
        collapsed,
        palette,
        hits,
    );
    if new_width > 0 {
        hits.projects_new = Rect::new(area.right() - new_width, area.y, new_width, 1);
        put_right_text(
            buffer,
            area,
            area.y,
            NEW_BUTTON,
            Style::default().fg(palette.overlay0),
        );
    }
    if collapsed {
        return;
    }

    // Drop the blank spacer row when the section is capped below its natural height.
    let header = if area.height >= projects_natural_rows(projects) {
        PROJECT_HEADER_ROWS
    } else {
        1
    };
    let body = Rect::new(
        area.x,
        area.y.saturating_add(header),
        area.width,
        area.height.saturating_sub(header),
    );
    hits.projects_body = body;
    let visible = usize::from(body.height / PROJECT_ROWS);
    let max_scroll = projects.len().saturating_sub(visible);
    *scroll = (*scroll).min(max_scroll);
    hits.project_max_scroll = max_scroll;
    for (row, project) in projects.iter().skip(*scroll).take(visible).enumerate() {
        let rect = Rect::new(
            body.x,
            body.y + PROJECT_ROWS * row as u16,
            body.width,
            PROJECT_ROWS,
        );
        render_project_rows(buffer, rect, project, snapshot, palette);
        hits.projects.push((rect, project.name.clone()));
    }
}

fn render_project_rows(
    buffer: &mut Buffer,
    rect: Rect,
    project: &super::ProjectEntry,
    snapshot: &ClientShellSnapshot,
    palette: &Palette,
) {
    let current = project.current;
    if current {
        buffer.set_style(rect, Style::default().bg(palette.active_row_bg));
    }
    let text_right = rect.right().saturating_sub(2);
    // Line 1: status mark and label, like a space row.
    let (mark, mark_style) = if current {
        ("●", Style::default().fg(palette.accent))
    } else if project.running {
        ("○", Style::default().fg(palette.overlay1))
    } else {
        (" ", Style::default())
    };
    let x = put_segment(
        buffer,
        rect.x.saturating_add(1),
        rect.y,
        text_right,
        mark,
        mark_style,
    );
    let x = put_segment(buffer, x, rect.y, text_right, " ", Style::default());
    put_segment(
        buffer,
        x,
        rect.y,
        text_right,
        &crate::ui::truncate_end(&project.label, usize::from(text_right.saturating_sub(x))),
        Style::default()
            .fg(if current {
                palette.text
            } else {
                palette.subtext0
            })
            .add_modifier(if current {
                Modifier::BOLD
            } else {
                Modifier::empty()
            }),
    );
    // Line 2: dim comma-separated spaces.
    let workspaces = if current {
        snapshot
            .workspaces
            .iter()
            .map(|workspace| workspace.label.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    } else {
        project.workspaces.join(", ")
    };
    let x = rect.x.saturating_add(3);
    put_segment(
        buffer,
        x,
        rect.y.saturating_add(1),
        text_right,
        &crate::ui::truncate_end(&workspaces, usize::from(text_right.saturating_sub(x))),
        Style::default().fg(if current {
            palette.mauve
        } else {
            palette.overlay0
        }),
    );
}
