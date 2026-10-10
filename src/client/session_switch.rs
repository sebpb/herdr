//! Runtime side of switching the Local endpoint to another named session.
//!
//! Flow: a UI emits `ClientShellAction::RequestSessionSwitch`; the runtime
//! asks the current session's JSON API (off the loop) which work a suspend
//! would lose and returns `ClientLoopEvent::SessionSwitchWarnings`. The shell
//! either confirms with the user or emits `ClientShellAction::SwitchSession`
//! directly. The runtime then (off the loop) starts the target server when it
//! is not running, stops the current one, and returns
//! `ClientLoopEvent::SessionSwitchPrepared`. The loop finally re-targets the
//! Local supervisor at the new session and activates it once it connects.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::api::client::{ApiClient, ConnectionTarget};
use crate::api::schema::{
    AgentStatus, EmptyParams, Method, PaneInfo, PaneListParams, PaneProcessInfo,
    PaneProcessInfoParams, Request, ResponseResult, WorkspaceInfo,
};

use super::events::ClientLoopEvent;
use super::shell::{ClientShellState, SessionSwitchRequest};

const WARNING_REQUEST_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Clone, Debug, Default, PartialEq, Eq)]
enum SessionSwitchPhase {
    #[default]
    Idle,
    /// Asking the current session what a suspend would lose.
    Checking,
    /// Starting the target server and stopping the current one.
    Switching,
    /// The Local supervisor targets the new session and awaits its connection.
    Connecting,
}

/// Loop-owned coordinator for session switches.
#[derive(Default)]
pub(super) struct SessionSwitchRuntime {
    event_tx: Option<tokio::sync::mpsc::Sender<ClientLoopEvent>>,
    phase: SessionSwitchPhase,
}

impl SessionSwitchRuntime {
    pub(super) fn new(event_tx: tokio::sync::mpsc::Sender<ClientLoopEvent>) -> Self {
        Self {
            event_tx: Some(event_tx),
            phase: SessionSwitchPhase::Idle,
        }
    }

    /// While true, losing the Local connection is part of the switch and must
    /// not end a client that otherwise exits with its server.
    pub(super) fn keeps_local_alive(&self) -> bool {
        matches!(
            self.phase,
            SessionSwitchPhase::Switching | SessionSwitchPhase::Connecting
        )
    }

    fn reject_busy(&self, shell: Option<&mut ClientShellState>) -> bool {
        if !matches!(
            self.phase,
            SessionSwitchPhase::Checking | SessionSwitchPhase::Switching
        ) {
            return false;
        }
        if let Some(shell) = shell {
            shell.show_session_switch_notice("A session switch is already in progress");
        }
        true
    }

    fn reject_current(
        request: &SessionSwitchRequest,
        shell: Option<&mut ClientShellState>,
    ) -> bool {
        if request.target != crate::session::active_name() {
            return false;
        }
        if let Some(shell) = shell {
            shell.show_session_switch_notice(format!(
                "Already in session {}",
                request.target_label()
            ));
        }
        true
    }

    /// Handles `ClientShellAction::RequestSessionSwitch`. Returns whether the
    /// shell needs a repaint.
    pub(super) fn request_check(
        &mut self,
        request: SessionSwitchRequest,
        mut shell: Option<&mut ClientShellState>,
    ) -> bool {
        if self.reject_busy(shell.as_deref_mut()) || Self::reject_current(&request, shell) {
            return true;
        }
        let Some(event_tx) = self.event_tx.clone() else {
            return false;
        };
        self.phase = SessionSwitchPhase::Checking;
        let api_socket = crate::session::active_api_socket_path();
        tokio::spawn(async move {
            let warnings =
                tokio::task::spawn_blocking(move || collect_session_switch_warnings(&api_socket))
                    .await
                    .unwrap_or_else(|error| {
                        Err(format!("warning check stopped unexpectedly: {error}"))
                    });
            let _ = event_tx
                .send(ClientLoopEvent::SessionSwitchWarnings { request, warnings })
                .await;
        });
        false
    }

    /// The pre-suspend check has been delivered to the shell.
    pub(super) fn finish_check(&mut self) {
        if self.phase == SessionSwitchPhase::Checking {
            self.phase = SessionSwitchPhase::Idle;
        }
    }

    /// Handles `ClientShellAction::SwitchSession`. Returns whether the shell
    /// needs a repaint.
    pub(super) fn begin_switch(
        &mut self,
        request: SessionSwitchRequest,
        mut shell: Option<&mut ClientShellState>,
    ) -> bool {
        if self.phase == SessionSwitchPhase::Switching {
            return self.reject_busy(shell);
        }
        if Self::reject_current(&request, shell.as_deref_mut()) {
            return true;
        }
        let Some(event_tx) = self.event_tx.clone() else {
            return false;
        };
        self.phase = SessionSwitchPhase::Switching;
        let current_label = crate::session::active_display_name();
        let plan = SessionSwitchPlan {
            current_api_socket: crate::session::active_api_socket_path(),
            current_client_socket: crate::server::socket_paths::client_socket_path(),
            current_label: current_label.clone(),
            target: request.target.clone(),
            create_cwd: request.create_cwd.clone(),
        };
        if let Some(shell) = shell {
            shell.show_session_switch_notice(format!(
                "Suspending {current_label} and opening {}…",
                request.target_label()
            ));
        }
        tokio::spawn(async move {
            let result = tokio::task::spawn_blocking(move || prepare_session_switch(&plan))
                .await
                .unwrap_or_else(|error| {
                    Err(format!("session switch stopped unexpectedly: {error}"))
                });
            let _ = event_tx
                .send(ClientLoopEvent::SessionSwitchPrepared { request, result })
                .await;
        });
        true
    }

    /// Local now targets the new session; keep it alive until it connects.
    pub(super) fn await_local_connection(&mut self) {
        self.phase = SessionSwitchPhase::Connecting;
    }

    /// A replacement Local connection is online, so normal Local failure
    /// handling applies again.
    pub(super) fn local_connected(&mut self) {
        if self.phase == SessionSwitchPhase::Connecting {
            self.phase = SessionSwitchPhase::Idle;
        }
    }

    pub(super) fn fail(&mut self) {
        self.phase = SessionSwitchPhase::Idle;
    }
}

struct SessionSwitchPlan {
    current_api_socket: PathBuf,
    current_client_socket: PathBuf,
    current_label: String,
    target: Option<String>,
    create_cwd: Option<PathBuf>,
}

/// Blocking: makes the target session's server available, then suspends the
/// current one. The target starts first so a failed start leaves the current
/// session untouched.
fn prepare_session_switch(plan: &SessionSwitchPlan) -> Result<(), String> {
    let target = plan.target.as_deref();
    let target_label = crate::session::display_name(target);
    if !crate::session::is_server_running_at(&crate::session::api_socket_path_for(target)) {
        crate::server::autodetect::spawn_server_daemon_for(target, plan.create_cwd.as_deref())
            .map_err(|error| format!("could not start {target_label}: {error}"))?;
        crate::server::autodetect::wait_for_server_socket(
            &crate::session::client_socket_path_for(target),
            crate::server::autodetect::server_ready_timeout(),
        )
        .map_err(|error| format!("could not start {target_label}: {error}"))?;
    }
    if let Err(error) = crate::session::stop_server_at(
        plan.current_api_socket.clone(),
        plan.current_client_socket.clone(),
        &format!("session {}", plan.current_label),
    ) {
        // Only a server that is still answering keeps the client in place.
        if crate::session::is_server_running_at(&plan.current_api_socket) {
            return Err(format!("could not suspend {}: {error}", plan.current_label));
        }
    }
    Ok(())
}

/// Blocking: lists the work in the session at `api_socket` that suspending it
/// would lose. An unreachable session has nothing left to lose.
fn collect_session_switch_warnings(api_socket: &Path) -> Result<Vec<String>, String> {
    if !crate::session::is_server_running_at(api_socket) {
        return Ok(Vec::new());
    }
    let client = ApiClient::for_target(ConnectionTarget::SocketPath(api_socket.to_path_buf()));
    let workspaces = match api_request(&client, Method::WorkspaceList(EmptyParams::default()))? {
        ResponseResult::WorkspaceList { workspaces } => workspaces,
        other => return Err(format!("unexpected workspace.list result: {other:?}")),
    };
    let panes = match api_request(
        &client,
        Method::PaneList(PaneListParams { workspace_id: None }),
    )? {
        ResponseResult::PaneList { panes } => panes,
        other => return Err(format!("unexpected pane.list result: {other:?}")),
    };
    let mut processes = HashMap::new();
    for pane in panes.iter().filter(|pane| pane.agent_session.is_none()) {
        let method = Method::PaneProcessInfo(PaneProcessInfoParams {
            pane_id: Some(pane.pane_id.clone()),
        });
        match api_request(&client, method) {
            Ok(ResponseResult::PaneProcessInfo { process_info }) => {
                processes.insert(pane.pane_id.clone(), process_info);
            }
            Ok(other) => {
                tracing::warn!(pane = %pane.pane_id, result = ?other, "unexpected pane.process_info result");
            }
            Err(error) => {
                tracing::warn!(pane = %pane.pane_id, %error, "pane process info is unavailable");
            }
        }
    }
    Ok(session_switch_warnings(&workspaces, &panes, &processes))
}

fn api_request(client: &ApiClient, method: Method) -> Result<ResponseResult, String> {
    let request = Request {
        id: "client:session-switch".into(),
        method,
    };
    let value = client
        .request_value_with_timeout(&request, WARNING_REQUEST_TIMEOUT)
        .map_err(|error| error.to_string())?;
    crate::api::client::parse_response_value(value)
        .map(|response| response.result)
        .map_err(|error| error.to_string())
}

/// Builds the suspend warnings, one line per affected pane (duplicates merged):
/// - a pane with a resumable agent session that is working:
///   `"<workspace>: agent working (<title>)"`;
/// - any other pane whose foreground process group is not its shell:
///   `"<workspace>: <unique foreground process names>"`.
pub(super) fn session_switch_warnings(
    workspaces: &[WorkspaceInfo],
    panes: &[PaneInfo],
    processes: &HashMap<String, PaneProcessInfo>,
) -> Vec<String> {
    let labels = workspaces
        .iter()
        .map(|workspace| (workspace.workspace_id.as_str(), workspace.label.as_str()))
        .collect::<HashMap<_, _>>();
    let mut seen = HashSet::new();
    let mut warnings = Vec::new();
    for pane in panes {
        let workspace = labels
            .get(pane.workspace_id.as_str())
            .copied()
            .unwrap_or(pane.workspace_id.as_str());
        let warning = if pane.agent_session.is_some() {
            if pane.agent_status != AgentStatus::Working {
                continue;
            }
            format!("{workspace}: agent working ({})", agent_title(pane))
        } else {
            let Some(info) = processes.get(&pane.pane_id) else {
                continue;
            };
            let (Some(shell_pid), Some(foreground)) =
                (info.shell_pid, info.foreground_process_group_id)
            else {
                continue;
            };
            if shell_pid == foreground {
                continue;
            }
            let mut names = Vec::new();
            for process in &info.foreground_processes {
                if !process.name.is_empty() && !names.contains(&process.name.as_str()) {
                    names.push(process.name.as_str());
                }
            }
            let names = if names.is_empty() {
                pane.agent
                    .clone()
                    .unwrap_or_else(|| format!("process {foreground}"))
            } else {
                names.join(", ")
            };
            format!("{workspace}: {names}")
        };
        if seen.insert(warning.clone()) {
            warnings.push(warning);
        }
    }
    warnings
}

fn agent_title(pane: &PaneInfo) -> &str {
    [
        pane.label.as_deref(),
        pane.title.as_deref(),
        pane.terminal_title_stripped.as_deref(),
        pane.display_agent.as_deref(),
        pane.agent.as_deref(),
    ]
    .into_iter()
    .flatten()
    .find(|title| !title.trim().is_empty())
    .unwrap_or("agent")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> (
        Vec<WorkspaceInfo>,
        Vec<PaneInfo>,
        HashMap<String, PaneProcessInfo>,
    ) {
        let workspaces: Vec<WorkspaceInfo> = serde_json::from_value(serde_json::json!([
            {
                "workspace_id": "w1", "number": 1, "label": "api", "focused": true,
                "pane_count": 4, "tab_count": 1, "active_tab_id": "t1", "agent_status": "working"
            },
            {
                "workspace_id": "w2", "number": 2, "label": "web", "focused": false,
                "pane_count": 2, "tab_count": 1, "active_tab_id": "t2", "agent_status": "idle"
            }
        ]))
        .unwrap();
        let pane = |id: &str, workspace: &str, extra: serde_json::Value| {
            let mut value = serde_json::json!({
                "pane_id": id, "terminal_id": format!("term-{id}"), "workspace_id": workspace,
                "tab_id": "t", "focused": false, "agent_status": "idle", "revision": 1
            });
            value
                .as_object_mut()
                .unwrap()
                .extend(extra.as_object().unwrap().clone());
            value
        };
        let session = serde_json::json!({
            "source": "test", "agent": "claude", "kind": "id", "value": "abc"
        });
        let panes: Vec<PaneInfo> = serde_json::from_value(serde_json::json!([
            // Resumable agent at work: warned with its title.
            pane(
                "p1",
                "w1",
                serde_json::json!({
                    "agent": "claude", "agent_status": "working", "title": "fix login",
                    "agent_session": session
                })
            ),
            // Resumable idle agent: comes back on reopen, no warning.
            pane(
                "p2",
                "w1",
                serde_json::json!({
                    "agent": "codex", "agent_status": "idle", "agent_session": session
                })
            ),
            // Shell at its prompt: no warning.
            pane("p3", "w1", serde_json::json!({})),
            // Foreground job: warned with unique process names.
            pane("p4", "w2", serde_json::json!({})),
            // Same job twice in one workspace collapses to one line.
            pane("p5", "w2", serde_json::json!({})),
            // Agent without native resume running in the foreground.
            pane(
                "p6",
                "w3",
                serde_json::json!({ "agent": "aider", "agent_status": "working" })
            ),
            // No process facts (e.g. unsupported platform): no warning.
            pane("p7", "w2", serde_json::json!({}))
        ]))
        .unwrap();
        let processes: Vec<PaneProcessInfo> = serde_json::from_value(serde_json::json!([
            { "pane_id": "p3", "shell_pid": 10, "foreground_process_group_id": 10 },
            {
                "pane_id": "p4", "shell_pid": 20, "foreground_process_group_id": 21,
                "foreground_processes": [
                    { "pid": 21, "name": "npm" }, { "pid": 22, "name": "node" },
                    { "pid": 23, "name": "node" }
                ]
            },
            {
                "pane_id": "p5", "shell_pid": 30, "foreground_process_group_id": 31,
                "foreground_processes": [{ "pid": 31, "name": "npm" }, { "pid": 32, "name": "node" }]
            },
            { "pane_id": "p6", "shell_pid": 40, "foreground_process_group_id": 41 },
            { "pane_id": "p7" }
        ]))
        .unwrap();
        let processes = processes
            .into_iter()
            .map(|info| (info.pane_id.clone(), info))
            .collect();
        (workspaces, panes, processes)
    }

    #[test]
    fn warnings_list_working_agents_and_foreground_processes() {
        let (workspaces, panes, processes) = sample();

        assert_eq!(
            session_switch_warnings(&workspaces, &panes, &processes),
            vec![
                "api: agent working (fix login)".to_string(),
                "web: npm, node".to_string(),
                "w3: aider".to_string(),
            ]
        );
    }

    #[test]
    fn idle_session_has_no_warnings() {
        let (workspaces, panes, processes) = sample();
        let panes = panes
            .into_iter()
            .filter(|pane| ["p2", "p3", "p7"].contains(&pane.pane_id.as_str()))
            .collect::<Vec<_>>();

        assert!(session_switch_warnings(&workspaces, &panes, &processes).is_empty());
    }

    #[test]
    fn unreachable_session_has_nothing_to_lose() {
        let path = std::env::temp_dir().join(format!(
            "herdr-session-switch-missing-{}.sock",
            std::process::id()
        ));
        assert_eq!(collect_session_switch_warnings(&path), Ok(Vec::new()));
    }

    fn isolated_session_env(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("herdr-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::env::set_var("XDG_CONFIG_HOME", root.join("config"));
        std::env::set_var("XDG_STATE_HOME", root.join("state"));
        std::env::remove_var(crate::session::SESSION_ENV_VAR);
        std::env::remove_var(crate::api::SOCKET_PATH_ENV_VAR);
        std::env::remove_var(crate::server::socket_paths::CLIENT_SOCKET_PATH_ENV_VAR);
        crate::session::clear_explicit_session_for_test();
        root
    }

    #[test]
    fn switch_check_reports_back_through_the_loop() {
        let root = isolated_session_env("switch-check");
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async {
            let (tx, mut rx) = tokio::sync::mpsc::channel(4);
            let mut runtime = SessionSwitchRuntime::new(tx);
            let request = SessionSwitchRequest::new("work").unwrap();

            assert!(!runtime.request_check(request.clone(), None));
            assert_eq!(runtime.phase, SessionSwitchPhase::Checking);
            // A second request while checking is rejected rather than queued.
            assert!(runtime.request_check(request.clone(), None));

            match rx.recv().await {
                Some(ClientLoopEvent::SessionSwitchWarnings {
                    request: received,
                    warnings,
                }) => {
                    assert_eq!(received, request);
                    // The current (default) session is not running: nothing to lose.
                    assert_eq!(warnings, Ok(Vec::new()));
                }
                _ => panic!("expected session switch warnings"),
            }
            runtime.finish_check();
            assert!(rx.try_recv().is_err());
        });
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn switching_to_the_current_session_is_a_notice_not_a_switch() {
        let root = isolated_session_env("switch-current");
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async {
            let (tx, mut rx) = tokio::sync::mpsc::channel(4);
            let mut runtime = SessionSwitchRuntime::new(tx);
            let mut shell =
                ClientShellState::new(super::super::shell::ClientShellConfig::from_config(
                    &crate::config::Config::default(),
                ));
            let request = SessionSwitchRequest::new("default").unwrap();

            assert!(runtime.begin_switch(request.clone(), Some(&mut shell)));
            assert!(runtime.request_check(request, Some(&mut shell)));

            assert_eq!(runtime.phase, SessionSwitchPhase::Idle);
            assert!(!runtime.keeps_local_alive());
            tokio::task::yield_now().await;
            assert!(rx.try_recv().is_err());
        });
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn switch_runtime_keeps_local_alive_only_while_switching() {
        let mut runtime = SessionSwitchRuntime::default();
        assert!(!runtime.keeps_local_alive());
        runtime.phase = SessionSwitchPhase::Checking;
        assert!(!runtime.keeps_local_alive());
        runtime.finish_check();
        assert_eq!(runtime.phase, SessionSwitchPhase::Idle);
        runtime.phase = SessionSwitchPhase::Switching;
        assert!(runtime.keeps_local_alive());
        runtime.await_local_connection();
        assert!(runtime.keeps_local_alive());
        runtime.local_connected();
        assert!(!runtime.keeps_local_alive());
        runtime.phase = SessionSwitchPhase::Switching;
        runtime.fail();
        assert!(!runtime.keeps_local_alive());
    }
}
