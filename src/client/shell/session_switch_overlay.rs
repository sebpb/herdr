use super::*;

const MAX_LISTED_WARNINGS: usize = 8;

pub(super) fn render_session_switch_confirm_overlay(
    b: &mut Buffer,
    c: &ClientSessionSwitchConfirmOverlay,
    p: &Palette,
) -> Option<OverlayRender> {
    let listed = c.warnings.len().min(MAX_LISTED_WARNINGS);
    let hidden = c.warnings.len() - listed;
    let body_rows = listed + usize::from(hidden > 0);
    // Title, explanation, warnings, blank row, buttons, plus the panel border.
    let height = u16::try_from(body_rows)
        .unwrap_or(u16::MAX)
        .saturating_add(6);
    let q = popup(b.area, 72, height.min(b.area.height))?;
    let i = panel(b, q, p.red, p.panel_bg)?;
    put_text(
        b,
        i.x,
        i.y,
        i.width,
        &format!(" {}", c.title),
        Style::default()
            .fg(p.red)
            .bg(p.panel_bg)
            .add_modifier(Modifier::BOLD),
    );
    put_text(
        b,
        i.x,
        i.y + 1,
        i.width,
        &format!(
            " Switching to {} stops this session's terminals:",
            c.request.target_label()
        ),
        Style::default().fg(p.text).bg(p.panel_bg),
    );
    // Keep the blank row and the button row free on short terminals.
    let list_end = i.bottom().saturating_sub(2);
    let mut y = i.y + 2;
    for warning in c.warnings.iter().take(listed) {
        if y >= list_end {
            break;
        }
        put_text(
            b,
            i.x,
            y,
            i.width,
            &format!("  • {warning}"),
            Style::default().fg(p.subtext0).bg(p.panel_bg),
        );
        y += 1;
    }
    if hidden > 0 && y < list_end {
        put_text(
            b,
            i.x,
            y,
            i.width,
            &format!("  … and {hidden} more"),
            Style::default().fg(p.subtext0).bg(p.panel_bg),
        );
    }
    let offset = i.height.saturating_sub(1);
    let rs = row(i, &[12, 12], 2, offset);
    let [ok, cancel] = rs.as_slice() else {
        return None;
    };
    button(
        b,
        *ok,
        " ↵ switch ",
        Style::default()
            .fg(contrast(p))
            .bg(p.red)
            .add_modifier(Modifier::BOLD),
    );
    button(
        b,
        *cancel,
        " esc cancel ",
        Style::default()
            .fg(p.text)
            .bg(p.surface0)
            .add_modifier(Modifier::BOLD),
    );
    Some(OverlayRender {
        area: q,
        primary: *ok,
        cancel: *cancel,
        ..OverlayRender::default()
    })
}
