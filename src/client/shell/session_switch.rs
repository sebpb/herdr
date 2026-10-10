//! Client-shell side of switching the Local endpoint to another named session.
//!
//! The shell only owns presentation: it turns a UI request into runtime
//! actions, shows the suspend confirmation when the runtime reports work that
//! would be lost, and reloads the per-session chrome preferences once the
//! runtime has moved Local to the new session.

use super::*;

/// A request to move the Local endpoint to another session.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SessionSwitchRequest {
    /// Target session; `None` is the default session.
    pub(crate) target: Option<String>,
    /// Directory that seeds the first workspace when the target session is new.
    /// An existing session keeps its restored layout and ignores it.
    pub(crate) create_cwd: Option<std::path::PathBuf>,
}

impl SessionSwitchRequest {
    /// Builds a request from a user-facing session name (`"default"` is the default session).
    pub(crate) fn new(target: &str) -> Result<Self, String> {
        Ok(Self {
            target: crate::session::parse_target_name(target)?,
            create_cwd: None,
        })
    }

    pub(crate) fn with_create_cwd(mut self, cwd: impl Into<std::path::PathBuf>) -> Self {
        self.create_cwd = Some(cwd.into());
        self
    }

    /// User-facing target name.
    pub(crate) fn target_label(&self) -> &str {
        crate::session::display_name(self.target.as_deref())
    }
}

#[derive(Debug)]
pub(crate) struct ClientSessionSwitchConfirmOverlay {
    pub(crate) request: SessionSwitchRequest,
    pub(crate) title: String,
    pub(crate) warnings: Vec<String>,
}

impl ClientShellState {
    /// Entry point for UI surfaces: asks the runtime to check the current
    /// session for work that suspending it would lose. The runtime answers
    /// through [`Self::receive_session_switch_warnings`].
    pub(crate) fn request_session_switch(
        &mut self,
        request: SessionSwitchRequest,
        outcome: &mut ClientShellInput,
    ) {
        outcome
            .actions
            .push(ClientShellAction::RequestSessionSwitch(request));
    }

    /// Applies the runtime's pre-suspend check. Without warnings the switch
    /// starts immediately; otherwise the confirmation lists what would be lost.
    pub(crate) fn receive_session_switch_warnings(
        &mut self,
        request: SessionSwitchRequest,
        warnings: Result<Vec<String>, String>,
    ) -> ClientShellInput {
        let mut outcome = ClientShellInput::default();
        let warnings = match warnings {
            Ok(warnings) => warnings,
            Err(error) => vec![format!("Could not check running work: {error}")],
        };
        if warnings.is_empty() {
            outcome
                .actions
                .push(ClientShellAction::SwitchSession(request));
            return outcome;
        }
        self.overlay = Some(ClientShellOverlay::ConfirmSessionSwitch(
            ClientSessionSwitchConfirmOverlay {
                title: format!("Suspend {}?", crate::session::active_display_name()),
                request,
                warnings,
            },
        ));
        outcome.repaint = true;
        outcome
    }

    pub(super) fn accept_session_switch_confirmation(&mut self, outcome: &mut ClientShellInput) {
        let Some(ClientShellOverlay::ConfirmSessionSwitch(confirm)) = self.overlay.take() else {
            return;
        };
        outcome
            .actions
            .push(ClientShellAction::SwitchSession(confirm.request));
        outcome.repaint = true;
    }

    pub(super) fn cancel_session_switch_confirmation(&mut self, outcome: &mut ClientShellInput) {
        if matches!(
            self.overlay,
            Some(ClientShellOverlay::ConfirmSessionSwitch(_))
        ) {
            self.overlay = None;
            outcome.repaint = true;
        }
    }

    /// Shows a transient client-owned notice about a session switch.
    pub(crate) fn show_session_switch_notice(&mut self, body: impl Into<String>) {
        self.visible_endpoint_notice = Some(ClientVisibleEndpointNotice {
            key: ClientEndpointNoticeKey {
                boot_id: "session-switch".to_owned(),
                kind: ClientEndpointNoticeKind::Unavailable,
                code: "session_switch".to_owned(),
            },
            title: "Session".to_owned(),
            body: body.into(),
            deadline: std::time::Instant::now() + std::time::Duration::from_secs(8),
        });
    }

    /// Points chrome persistence at another Local endpoint's preference file
    /// and re-applies the saved chrome layout from it.
    pub(crate) fn reload_local_preferences(&mut self, socket_path: &std::path::Path) {
        let path = preferences::path_for_local_endpoint(socket_path);
        let saved = preferences::load(&path).unwrap_or_default();
        self.config.preferences_path = Some(path);
        self.apply_chrome_preferences(saved);
    }

    // Mirrors the preference handling in `ClientShellState::new`. Every saved
    // chrome field must be re-applied here so switching sessions restores it.
    fn apply_chrome_preferences(&mut self, saved: preferences::ClientChromePreferences) {
        self.config.preferences = saved.clone();
        self.sidebar_collapsed = saved
            .sidebar_collapsed
            .unwrap_or(self.config.sidebar_start_collapsed);
        self.sidebar_collapsed_manual = saved.sidebar_collapsed.is_some();
        let (min_width, max_width) = crate::config::validated_sidebar_bounds(
            self.config.sidebar_min_width,
            self.config.sidebar_max_width,
        )
        .unwrap_or((18, 36));
        self.sidebar_width = saved
            .sidebar_width
            .unwrap_or(self.config.sidebar_width)
            .clamp(min_width, max_width);
        self.sidebar_width_manual = saved.sidebar_width.is_some();
        self.sidebar_section_split = saved
            .sidebar_section_split
            .filter(|split| split.is_finite())
            .map(|split| split.clamp(0.1, 0.9))
            .unwrap_or(0.5);
        self.sidebar_section_split_manual = saved.sidebar_section_split.is_some();
        if let Some(sort) = saved.agent_panel_sort {
            self.config.agent_panel_sort = sort;
        }
        self.agent_panel_sort_manual = saved.agent_panel_sort.is_some();
        self.collapsed_groups = saved.collapsed_groups.into_iter().collect();
        self.remote_collapsed_groups.clear();
        for group in saved.remote_collapsed_groups {
            let Ok(profile_id) = crate::client::endpoint::ProfileId::parse(group.profile_id) else {
                continue;
            };
            self.remote_collapsed_groups
                .entry(ClientEndpointId::Ssh(profile_id))
                .or_default()
                .extend(group.collapsed_groups);
        }
        self.collapsed_agent_groups = saved.collapsed_agent_groups.into_iter().collect();
        self.collapsed_sidebar_sections = saved
            .collapsed_sidebar_sections
            .iter()
            .filter_map(|name| crate::ui::SidebarSection::from_name(name))
            .collect();
        self.workspace_scroll = 0;
        self.agent_scroll = 0;
        self.reveal_focused_workspace = true;
    }
}
