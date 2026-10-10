//! Runtime side of the projects sidebar section.
//!
//! A project is a named herdr session. The shell only renders a cached list;
//! this loop-owned runtime refreshes it off the loop (`spawn_blocking`): it
//! lists the sessions (which probes their sockets), reads the visible labels
//! from `state_dir()/client/project-labels.json`, and reads the workspace
//! labels of each session from its saved `session.json`. Label changes and
//! deletes run in the same background job, followed by a fresh listing.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use super::events::ClientLoopEvent;
use super::shell::{ProjectEntry, ProjectsCommand};

/// How often a visible projects section re-lists the sessions.
const REFRESH_INTERVAL: Duration = Duration::from_secs(3);

/// What a background projects job hands back to the loop.
#[derive(Debug)]
pub(super) struct ProjectsListing {
    pub(super) projects: Result<Vec<ProjectEntry>, String>,
    /// A user-facing error from the command that ran before the listing.
    pub(super) notice: Option<String>,
}

/// Loop-owned scheduler for the background projects jobs.
#[derive(Default)]
pub(super) struct ProjectsRuntime {
    event_tx: Option<tokio::sync::mpsc::Sender<ClientLoopEvent>>,
    /// Generation of the newest job started.
    started: u64,
    /// Generation of the newest listing applied to the shell.
    applied: u64,
    in_flight: usize,
    last_started_at: Option<Instant>,
}

impl ProjectsRuntime {
    pub(super) fn new(event_tx: tokio::sync::mpsc::Sender<ClientLoopEvent>) -> Self {
        Self {
            event_tx: Some(event_tx),
            ..Self::default()
        }
    }

    /// Periodic refresh while the shell shows the section.
    pub(super) fn tick(&mut self, now: Instant, wanted: bool) {
        let due = self
            .last_started_at
            .is_none_or(|last| now.saturating_duration_since(last) >= REFRESH_INTERVAL);
        if wanted && due && self.in_flight == 0 {
            self.run(ProjectsCommand::Refresh);
        }
    }

    /// Runs `command` and then re-lists the projects, off the loop.
    pub(super) fn run(&mut self, command: ProjectsCommand) {
        let Some(event_tx) = self.event_tx.clone() else {
            return;
        };
        self.started += 1;
        self.in_flight += 1;
        self.last_started_at = Some(Instant::now());
        let generation = self.started;
        // The active session only changes on the loop thread, so read it here.
        let current = crate::session::active_name();
        let labels_path = project_labels_path();
        tokio::spawn(async move {
            let listing = tokio::task::spawn_blocking(move || {
                run_projects_job(command, current.as_deref(), &labels_path)
            })
            .await
            .unwrap_or_else(|error| ProjectsListing {
                projects: Err(format!("project listing stopped unexpectedly: {error}")),
                notice: None,
            });
            let _ = event_tx
                .send(ClientLoopEvent::ProjectsListed {
                    generation,
                    listing,
                })
                .await;
        });
    }

    /// A job finished. Returns whether its listing is the newest one and
    /// should replace the shell's list.
    pub(super) fn finish(&mut self, generation: u64) -> bool {
        self.in_flight = self.in_flight.saturating_sub(1);
        if generation <= self.applied {
            return false;
        }
        self.applied = generation;
        true
    }
}

pub(super) fn project_labels_path() -> PathBuf {
    crate::config::state_dir()
        .join("client")
        .join("project-labels.json")
}

/// Blocking: applies `command`, then lists every session as a project.
fn run_projects_job(
    command: ProjectsCommand,
    current: Option<&str>,
    labels_path: &Path,
) -> ProjectsListing {
    let notice = match command {
        ProjectsCommand::Refresh => None,
        ProjectsCommand::SetLabel { name, label } => {
            let session = crate::session::display_name(name.as_deref()).to_owned();
            set_project_label(labels_path, &session, label.as_deref())
                .err()
                .map(|error| format!("Could not rename {session}: {error}"))
        }
        ProjectsCommand::Delete { name } => delete_project(labels_path, &name, current)
            .err()
            .map(|error| format!("Could not delete {name}: {error}")),
    };
    let projects = crate::session::list_sessions()
        .map_err(|error| error.to_string())
        .map(|sessions| {
            let labels = load_project_labels(labels_path);
            project_entries(sessions, &labels, current, |session| {
                saved_workspace_labels(&Path::new(&session.session_dir).join("session.json"))
            })
        });
    ProjectsListing { projects, notice }
}

fn delete_project(labels_path: &Path, name: &str, current: Option<&str>) -> Result<(), String> {
    if current == Some(name) {
        return Err("it is the current project".to_owned());
    }
    crate::session::delete_session(name)?;
    set_project_label(labels_path, name, None)
}

/// Builds the sorted project rows: the current project first, then the rest
/// alphabetically by visible label.
pub(super) fn project_entries(
    sessions: Vec<crate::session::SessionInfo>,
    labels: &BTreeMap<String, String>,
    current: Option<&str>,
    mut workspaces: impl FnMut(&crate::session::SessionInfo) -> Vec<String>,
) -> Vec<ProjectEntry> {
    let mut entries = sessions
        .into_iter()
        .map(|session| {
            let name = (!session.default).then(|| session.name.clone());
            let is_current = name.as_deref() == current;
            ProjectEntry {
                label: labels
                    .get(&session.name)
                    .cloned()
                    .unwrap_or_else(|| session.name.clone()),
                running: session.running || is_current,
                current: is_current,
                workspaces: workspaces(&session),
                name,
            }
        })
        .collect::<Vec<_>>();
    sort_projects(&mut entries);
    entries
}

pub(crate) fn sort_projects(entries: &mut [ProjectEntry]) {
    entries.sort_by(|left, right| {
        right
            .current
            .cmp(&left.current)
            .then_with(|| left.label.to_lowercase().cmp(&right.label.to_lowercase()))
            .then_with(|| left.name.cmp(&right.name))
    });
}

pub(super) fn load_project_labels(path: &Path) -> BTreeMap<String, String> {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|content| serde_json::from_str(&content).ok())
        .unwrap_or_default()
}

/// Stores (or with `None`, removes) the visible label of `session`.
pub(super) fn set_project_label(
    path: &Path,
    session: &str,
    label: Option<&str>,
) -> Result<(), String> {
    let mut labels = load_project_labels(path);
    let changed = match label.filter(|label| *label != session) {
        Some(label) => {
            labels
                .insert(session.to_owned(), label.to_owned())
                .as_deref()
                != Some(label)
        }
        None => labels.remove(session).is_some(),
    };
    if !changed {
        return Ok(());
    }
    let parent = path
        .parent()
        .ok_or_else(|| format!("invalid project labels path: {}", path.display()))?;
    std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    let content = serde_json::to_vec_pretty(&labels).map_err(|error| error.to_string())?;
    let temp_path = parent.join(format!("project-labels.json.tmp-{}", std::process::id()));
    std::fs::write(&temp_path, content).map_err(|error| error.to_string())?;
    std::fs::rename(&temp_path, path).map_err(|error| {
        let _ = std::fs::remove_file(&temp_path);
        error.to_string()
    })
}

#[derive(serde::Deserialize)]
struct SavedSession {
    #[serde(default)]
    workspaces: Vec<SavedWorkspace>,
}

#[derive(serde::Deserialize)]
struct SavedWorkspace {
    #[serde(default)]
    custom_name: Option<String>,
    #[serde(default)]
    identity_cwd: Option<PathBuf>,
}

/// Workspace labels from a saved session layout: the custom name, else the
/// basename of the workspace folder. A missing or unreadable file has none.
pub(super) fn saved_workspace_labels(path: &Path) -> Vec<String> {
    let Ok(content) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    let Ok(saved) = serde_json::from_str::<SavedSession>(&content) else {
        return Vec::new();
    };
    saved
        .workspaces
        .into_iter()
        .filter_map(|workspace| {
            workspace
                .custom_name
                .filter(|name| !name.trim().is_empty())
                .or_else(|| {
                    workspace.identity_cwd.and_then(|cwd| {
                        cwd.file_name()
                            .map(|name| name.to_string_lossy().into_owned())
                    })
                })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "herdr-projects-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock after epoch")
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn session(name: &str, running: bool) -> crate::session::SessionInfo {
        crate::session::SessionInfo {
            name: name.to_owned(),
            default: name == "default",
            running,
            socket_path: String::new(),
            session_dir: format!("/sessions/{name}"),
        }
    }

    #[test]
    fn entries_put_the_current_project_first_then_sort_by_label() {
        let labels = BTreeMap::from([
            ("cliente-alamo".to_owned(), "Cliente Álamo".to_owned()),
            ("zeta".to_owned(), "a zeta".to_owned()),
        ]);
        let entries = project_entries(
            vec![
                session("default", true),
                session("cliente-alamo", false),
                session("work", true),
                session("zeta", false),
            ],
            &labels,
            Some("work"),
            |session| vec![format!("{} ws", session.name)],
        );

        let rows = entries
            .iter()
            .map(|entry| {
                (
                    entry.name.as_deref(),
                    entry.label.as_str(),
                    entry.running,
                    entry.current,
                )
            })
            .collect::<Vec<_>>();
        assert_eq!(
            rows,
            [
                (Some("work"), "work", true, true),
                (Some("zeta"), "a zeta", false, false),
                (Some("cliente-alamo"), "Cliente Álamo", false, false),
                (None, "default", true, false),
            ]
        );
        assert_eq!(entries[0].workspaces, ["work ws"]);
    }

    #[test]
    fn labels_round_trip_and_clear() {
        let dir = temp_dir("labels");
        let path = dir.join("client").join("project-labels.json");
        assert!(load_project_labels(&path).is_empty());

        set_project_label(&path, "cliente-alamo", Some("Cliente Álamo")).unwrap();
        set_project_label(&path, "default", Some("Personal")).unwrap();
        assert_eq!(
            load_project_labels(&path),
            BTreeMap::from([
                ("cliente-alamo".to_owned(), "Cliente Álamo".to_owned()),
                ("default".to_owned(), "Personal".to_owned()),
            ])
        );

        // A label equal to the session name is the same as no label.
        set_project_label(&path, "default", Some("default")).unwrap();
        set_project_label(&path, "cliente-alamo", None).unwrap();
        assert!(load_project_labels(&path).is_empty());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn saved_workspace_labels_use_custom_names_or_folder_names() {
        let dir = temp_dir("saved");
        let path = dir.join("session.json");
        std::fs::write(
            &path,
            r#"{"version":5,"workspaces":[
                {"id":"a","custom_name":"Self dev","identity_cwd":"/home/u/ai"},
                {"id":"b","custom_name":null,"identity_cwd":"/home/u/herdr"},
                {"id":"c"}
            ],"active":0}"#,
        )
        .unwrap();

        assert_eq!(saved_workspace_labels(&path), ["Self dev", "herdr"]);
        assert!(saved_workspace_labels(&dir.join("missing.json")).is_empty());
        std::fs::write(&path, "not json").unwrap();
        assert!(saved_workspace_labels(&path).is_empty());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn runtime_applies_only_the_newest_listing_and_waits_between_refreshes() {
        let (tx, _rx) = tokio::sync::mpsc::channel(4);
        let mut runtime = ProjectsRuntime::new(tx);
        // Without a tokio runtime, account for the jobs by hand.
        runtime.started = 2;
        runtime.in_flight = 2;
        assert!(runtime.finish(2));
        assert!(!runtime.finish(1));
        assert_eq!(runtime.in_flight, 0);

        runtime.last_started_at = Some(Instant::now());
        runtime.tick(Instant::now(), true);
        assert_eq!(runtime.started, 2, "refreshes wait for the interval");
    }
}
