//! Client-shell side of the projects sidebar section.
//!
//! A project is a named herdr session. The runtime (`crate::client::projects`)
//! lists the sessions off the loop and hands them over through
//! [`ClientShellState::receive_projects`]. The shell only keeps that cached
//! list, renders it, and turns clicks into session switches or into
//! [`ProjectsCommand`]s that the runtime runs in the background.

use super::*;

/// One project row: a herdr session and its visible label.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ProjectEntry {
    /// Session name; `None` is the default session.
    pub(crate) name: Option<String>,
    pub(crate) label: String,
    pub(crate) running: bool,
    pub(crate) current: bool,
    /// Workspace labels from the session's saved layout. The current project
    /// renders the live snapshot instead.
    pub(crate) workspaces: Vec<String>,
}

impl ProjectEntry {
    pub(crate) fn session_name(&self) -> &str {
        crate::session::display_name(self.name.as_deref())
    }
}

/// Work the runtime runs off the loop, always followed by a fresh listing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ProjectsCommand {
    Refresh,
    /// Stores the visible label of a session; `None` goes back to its name.
    SetLabel {
        name: Option<String>,
        label: Option<String>,
    },
    /// Deletes a stopped, non-current session and its label.
    Delete {
        name: String,
    },
}

#[derive(Debug, Default)]
pub(super) struct ClientProjectsState {
    /// Empty until the first listing arrives; the section stays hidden until then.
    pub(super) entries: Vec<ProjectEntry>,
    /// First visible project row.
    pub(super) scroll: usize,
}

/// The text prompts of the projects section, shown in the rename modal.
#[derive(Debug)]
pub(super) enum ClientProjectPrompt {
    /// Step 1 of a new project: its name.
    NewName { error: Option<String> },
    /// Step 2 of a new project: the folder of its first space.
    NewFolder {
        name: String,
        label: String,
        error: Option<String>,
    },
    /// Changes only the visible label of an existing project.
    Rename { name: Option<String> },
}

impl ClientProjectPrompt {
    /// The dim hint or red error drawn above the input.
    pub(super) fn message(&self) -> Option<(&str, bool)> {
        match self {
            Self::NewName { error: Some(error) }
            | Self::NewFolder {
                error: Some(error), ..
            } => Some((error, true)),
            Self::NewName { error: None } => Some(("Name of the new session", false)),
            Self::NewFolder { error: None, .. } => Some(("Folder of its first space", false)),
            Self::Rename { .. } => Some(("Only changes the label in the sidebar", false)),
        }
    }
}

#[derive(Debug)]
pub(crate) struct ClientProjectDeleteConfirmOverlay {
    pub(super) name: String,
    pub(super) label: String,
}

/// Session name for a project typed as `label`: ASCII lowercase, accents
/// stripped, spaces and other symbols as `-`.
pub(super) fn project_session_name(label: &str) -> String {
    let mut slug = String::new();
    for character in label.trim().chars().flat_map(char::to_lowercase) {
        let folded = fold_accent(character);
        if folded.is_empty() {
            if !slug.ends_with('-') {
                slug.push('-');
            }
            continue;
        }
        for character in folded.chars() {
            if character.is_ascii_alphanumeric() || matches!(character, '.' | '_') {
                slug.push(character);
            } else if !slug.ends_with('-') {
                slug.push('-');
            }
        }
    }
    let mut slug = slug.trim_matches('-').to_owned();
    // Session names are at most 64 bytes; the slug is ASCII.
    slug.truncate(64);
    slug.trim_end_matches('-').to_owned()
}

/// ASCII spelling of a lowercase character; empty for separators.
fn fold_accent(character: char) -> std::borrow::Cow<'static, str> {
    let ascii = match character {
        'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' | 'ā' | 'ă' | 'ą' => "a",
        'ç' | 'ć' | 'ĉ' | 'ċ' | 'č' => "c",
        'ď' | 'đ' => "d",
        'è' | 'é' | 'ê' | 'ë' | 'ē' | 'ĕ' | 'ė' | 'ę' | 'ě' => "e",
        'ĝ' | 'ğ' | 'ġ' | 'ģ' => "g",
        'ĥ' | 'ħ' => "h",
        'ì' | 'í' | 'î' | 'ï' | 'ĩ' | 'ī' | 'ĭ' | 'į' | 'ı' => "i",
        'ĵ' => "j",
        'ķ' => "k",
        'ĺ' | 'ļ' | 'ľ' | 'ŀ' | 'ł' => "l",
        'ñ' | 'ń' | 'ņ' | 'ň' => "n",
        'ò' | 'ó' | 'ô' | 'õ' | 'ö' | 'ø' | 'ō' | 'ŏ' | 'ő' => "o",
        'ŕ' | 'ŗ' | 'ř' => "r",
        'ś' | 'ŝ' | 'ş' | 'š' => "s",
        'ţ' | 'ť' | 'ŧ' => "t",
        'ù' | 'ú' | 'û' | 'ü' | 'ũ' | 'ū' | 'ŭ' | 'ů' | 'ű' | 'ų' => "u",
        'ŵ' => "w",
        'ý' | 'ÿ' | 'ŷ' => "y",
        'ź' | 'ż' | 'ž' => "z",
        'ß' => "ss",
        'æ' => "ae",
        'œ' => "oe",
        'þ' => "th",
        'ð' => "d",
        character if character.is_whitespace() => "",
        character => return character.to_string().into(),
    };
    ascii.into()
}

impl ClientShellState {
    /// Whether the runtime should keep the project list fresh: the local,
    /// expanded sidebar shows the section, or it has never been listed.
    pub(crate) fn projects_refresh_wanted(&self) -> bool {
        self.config.projects_enabled
            && self.endpoints.len() == 1
            && !self.sidebar_collapsed
            && (self.projects.entries.is_empty()
                || !self
                    .collapsed_sidebar_sections
                    .contains(&crate::ui::SidebarSection::Projects))
    }

    /// Applies a background listing. Returns whether the shell needs a repaint.
    pub(crate) fn receive_projects(
        &mut self,
        projects: Result<Vec<ProjectEntry>, String>,
        notice: Option<String>,
    ) -> bool {
        let mut repaint = false;
        match projects {
            Ok(entries) if entries != self.projects.entries => {
                self.projects.entries = entries;
                repaint = true;
            }
            Ok(_) => {}
            Err(error) => tracing::warn!(%error, "could not list projects"),
        }
        if let Some(notice) = notice {
            self.show_projects_notice(notice);
            repaint = true;
        }
        repaint
    }

    /// Marks `target` as the current project right after a switch, before the
    /// next listing confirms it.
    pub(crate) fn mark_current_project(&mut self, target: Option<&str>) {
        for entry in &mut self.projects.entries {
            entry.current = entry.name.as_deref() == target;
            entry.running |= entry.current;
        }
        crate::client::projects::sort_projects(&mut self.projects.entries);
        self.projects.scroll = 0;
    }

    fn show_projects_notice(&mut self, body: String) {
        self.visible_endpoint_notice = Some(ClientVisibleEndpointNotice {
            key: ClientEndpointNoticeKey {
                boot_id: "projects".to_owned(),
                kind: ClientEndpointNoticeKind::Rejected,
                code: "projects".to_owned(),
            },
            title: "Projects".to_owned(),
            body,
            deadline: std::time::Instant::now() + std::time::Duration::from_secs(8),
        });
    }

    fn project(&self, name: &Option<String>) -> Option<&ProjectEntry> {
        self.projects
            .entries
            .iter()
            .find(|entry| &entry.name == name)
    }

    fn project_at(&self, point: (u16, u16)) -> Option<Option<String>> {
        self.hits
            .projects
            .iter()
            .find(|(rect, _)| super::contains(*rect, point))
            .map(|(_, name)| name.clone())
    }

    /// Left button down in the projects section: `new` or a project row.
    pub(super) fn handle_projects_click(
        &mut self,
        point: (u16, u16),
        outcome: &mut ClientShellInput,
    ) -> bool {
        if super::contains(self.hits.projects_new, point) {
            self.open_new_project_overlay();
            outcome.repaint = true;
            return true;
        }
        let Some(name) = self.project_at(point) else {
            return false;
        };
        if self.project(&name).is_some_and(|project| !project.current) {
            self.switch_to_project(crate::session::display_name(name.as_deref()), None, outcome);
        }
        true
    }

    /// Right button down on a project row opens its context menu.
    pub(super) fn handle_projects_right_click(
        &mut self,
        point: (u16, u16),
        x: u16,
        y: u16,
    ) -> bool {
        let Some(name) = self.project_at(point) else {
            return false;
        };
        let Some(project) = self.project(&name) else {
            return true;
        };
        // The default session cannot be deleted, and a running one must be stopped first.
        let deletable = name.is_some() && !project.current && !project.running;
        self.overlay = Some(ClientShellOverlay::ContextMenu(ClientContextMenuOverlay {
            target: ClientContextMenuTarget::Project { name, deletable },
            x,
            y,
            highlighted: 0,
        }));
        true
    }

    /// Wheel over the project rows. Returns whether the list moved.
    pub(super) fn scroll_projects(&mut self, down: bool) -> bool {
        let next = if down {
            self.projects
                .scroll
                .saturating_add(1)
                .min(self.hits.project_max_scroll)
        } else {
            self.projects.scroll.saturating_sub(1)
        };
        let changed = next != self.projects.scroll;
        self.projects.scroll = next;
        changed
    }

    pub(super) fn open_new_project_overlay(&mut self) {
        self.overlay = Some(ClientShellOverlay::Rename(ClientRenameOverlay {
            title: "new project",
            input: TextEditor::new("", false),
            target: ClientRenameTarget::Project(ClientProjectPrompt::NewName { error: None }),
        }));
    }

    pub(super) fn activate_project_context_action(
        &mut self,
        name: Option<String>,
        action: ClientContextMenuAction,
    ) {
        let Some(project) = self.project(&name) else {
            return;
        };
        match action {
            ClientContextMenuAction::Rename => {
                let label = project.label.clone();
                self.overlay = Some(ClientShellOverlay::Rename(ClientRenameOverlay {
                    title: "rename project",
                    input: TextEditor::new(&label, false),
                    target: ClientRenameTarget::Project(ClientProjectPrompt::Rename { name }),
                }));
            }
            ClientContextMenuAction::DeleteProject => {
                let label = project.label.clone();
                if let Some(name) = name {
                    self.overlay = Some(ClientShellOverlay::ConfirmProjectDelete(
                        ClientProjectDeleteConfirmOverlay { name, label },
                    ));
                }
            }
            _ => {}
        }
    }

    pub(super) fn accept_project_delete_confirmation(&mut self, outcome: &mut ClientShellInput) {
        let Some(ClientShellOverlay::ConfirmProjectDelete(confirm)) = self.overlay.take() else {
            return;
        };
        outcome
            .actions
            .push(ClientShellAction::Projects(ProjectsCommand::Delete {
                name: confirm.name,
            }));
        outcome.repaint = true;
    }

    /// Enter in a projects prompt. Invalid input keeps the prompt open with an error.
    pub(super) fn save_project_prompt(
        &mut self,
        prompt: ClientProjectPrompt,
        input: TextEditor,
        title: &'static str,
        outcome: &mut ClientShellInput,
    ) {
        outcome.repaint = true;
        let text = input.trim().to_owned();
        let reopen = |state: &mut Self, prompt| {
            state.overlay = Some(ClientShellOverlay::Rename(ClientRenameOverlay {
                title,
                input,
                target: ClientRenameTarget::Project(prompt),
            }));
        };
        match prompt {
            ClientProjectPrompt::NewName { .. } => {
                let name = project_session_name(&text);
                let error = if text.is_empty() {
                    Some("Enter a project name".to_owned())
                } else if name.is_empty() {
                    Some("Use letters or numbers in the name".to_owned())
                } else {
                    crate::session::validate_name(&name).err()
                };
                if let Some(error) = error {
                    reopen(self, ClientProjectPrompt::NewName { error: Some(error) });
                    return;
                }
                let existing = self
                    .projects
                    .entries
                    .iter()
                    .find(|entry| entry.session_name() == name);
                if let Some(existing) = existing {
                    if existing.current {
                        self.show_projects_notice(format!("Already in {}", existing.label));
                    } else {
                        self.switch_to_project(&name, None, outcome);
                    }
                    return;
                }
                self.overlay = Some(ClientShellOverlay::Rename(ClientRenameOverlay {
                    title: "project folder",
                    input: TextEditor::new("~", false),
                    target: ClientRenameTarget::Project(ClientProjectPrompt::NewFolder {
                        name,
                        label: text,
                        error: None,
                    }),
                }));
            }
            ClientProjectPrompt::NewFolder { name, label, .. } => {
                let folder = crate::worktree::expand_tilde_absolute_path(if text.is_empty() {
                    "~"
                } else {
                    &text
                });
                if !folder.is_dir() {
                    reopen(
                        self,
                        ClientProjectPrompt::NewFolder {
                            name,
                            label,
                            error: Some(format!("Folder not found: {}", folder.display())),
                        },
                    );
                    return;
                }
                if label != name {
                    outcome
                        .actions
                        .push(ClientShellAction::Projects(ProjectsCommand::SetLabel {
                            name: Some(name.clone()),
                            label: Some(label),
                        }));
                }
                self.switch_to_project(&name, Some(folder), outcome);
            }
            ClientProjectPrompt::Rename { name } => {
                let session = crate::session::display_name(name.as_deref()).to_owned();
                let label = (!text.is_empty() && text != session).then_some(text);
                if let Some(entry) = self
                    .projects
                    .entries
                    .iter_mut()
                    .find(|entry| entry.name == name)
                {
                    entry.label = label.clone().unwrap_or(session);
                }
                crate::client::projects::sort_projects(&mut self.projects.entries);
                outcome
                    .actions
                    .push(ClientShellAction::Projects(ProjectsCommand::SetLabel {
                        name,
                        label,
                    }));
            }
        }
    }

    fn switch_to_project(
        &mut self,
        name: &str,
        create_cwd: Option<std::path::PathBuf>,
        outcome: &mut ClientShellInput,
    ) {
        match SessionSwitchRequest::new(name) {
            Ok(request) => {
                let request = match create_cwd {
                    Some(cwd) => request.with_create_cwd(cwd),
                    None => request,
                };
                self.request_session_switch(request, outcome);
            }
            Err(error) => self.show_projects_notice(format!("Cannot open {name}: {error}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn project_names_become_session_names() {
        assert_eq!(project_session_name("Cliente Álamo"), "cliente-alamo");
        assert_eq!(project_session_name("  Ñandú  Größe "), "nandu-grosse");
        assert_eq!(project_session_name("api/v2 (beta)"), "api-v2-beta");
        assert_eq!(project_session_name("my_app.v1"), "my_app.v1");
        assert_eq!(project_session_name("日本"), "");
        assert_eq!(project_session_name(&"á".repeat(80)).len(), 64);
        for name in ["Cliente Álamo", "a--b", "x y z"] {
            crate::session::validate_name(&project_session_name(name)).unwrap();
        }
    }
}
