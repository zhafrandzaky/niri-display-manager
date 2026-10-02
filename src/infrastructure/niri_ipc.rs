//! niri IPC adapter built on the `niri msg` CLI.
//!
//! The CLI is version-locked to the running compositor and is the supported
//! interface, so the manager drives it rather than reimplementing the private
//! Unix-socket JSON protocol. Types here deserialize the `--json` output and
//! are intentionally hand-rolled to keep the project dependency-light and
//! license-clean.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use serde::Deserialize;

use super::process_runner::{CommandRunner, CommandSpec, ProcessError};
use crate::domain::display::{DisplayOutput, OutputId};

/// Errors surfaced by the niri adapter.
#[derive(Debug, thiserror::Error)]
pub enum NiriError {
    #[error("niri command failed ({command}): {stderr}")]
    CommandFailed { command: String, stderr: String },
    #[error("failed to parse niri JSON output: {0}")]
    Json(#[from] serde_json::Error),
    #[error("process error while running niri: {0}")]
    Process(#[from] ProcessError),
}

/// A toplevel window as reported by `niri msg --json windows`.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct WindowInfo {
    pub id: u64,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub app_id: Option<String>,
    #[serde(default)]
    pub workspace_id: Option<u64>,
    #[serde(default)]
    pub is_focused: bool,
}

/// A workspace as reported by `niri msg --json workspaces`.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct WorkspaceInfo {
    pub id: u64,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub output: Option<String>,
    #[serde(default)]
    pub is_active: bool,
}

/// Read and control niri through its CLI.
pub trait NiriClient: Send + Sync {
    fn outputs(&self) -> Result<Vec<DisplayOutput>, NiriError>;
    fn windows(&self) -> Result<Vec<WindowInfo>, NiriError>;
    fn workspaces(&self) -> Result<Vec<WorkspaceInfo>, NiriError>;
    /// Reload the compositor configuration from disk.
    fn load_config(&self) -> Result<(), NiriError>;
    fn move_window_to_output(&self, window_id: u64, output: &OutputId) -> Result<(), NiriError>;
    fn focus_window(&self, window_id: u64) -> Result<(), NiriError>;
    fn focus_output(&self, output: &OutputId) -> Result<(), NiriError>;
    /// Enable a temporarily disabled output (`niri msg output NAME on`).
    fn enable_output(&self, output: &OutputId) -> Result<(), NiriError>;
    /// Validate a config file without applying it.
    fn validate_config(&self, path: &Path) -> Result<(), NiriError>;
}

/// Production client invoking the `niri` binary.
pub struct NiriCliClient {
    runner: Arc<dyn CommandRunner>,
    program: String,
}

impl NiriCliClient {
    const COMMAND_TIMEOUT: Duration = Duration::from_secs(5);

    pub fn new(runner: Arc<dyn CommandRunner>) -> Self {
        Self {
            runner,
            program: "niri".to_owned(),
        }
    }

    fn run_checked(&self, args: Vec<String>) -> Result<String, NiriError> {
        let command = format!("{} {}", self.program, args.join(" "));
        let spec = CommandSpec::new(self.program.clone(), args).with_timeout(Self::COMMAND_TIMEOUT);
        let output = self.runner.run(&spec)?;
        if !output.success {
            let stderr = output.stderr.trim();
            let stdout = output.stdout.trim();
            let detail = if !stderr.is_empty() {
                stderr.to_owned()
            } else if !stdout.is_empty() {
                stdout.to_owned()
            } else {
                "no error output".to_owned()
            };
            return Err(NiriError::CommandFailed {
                command,
                stderr: detail,
            });
        }
        Ok(output.stdout)
    }

    fn query_json<T: serde::de::DeserializeOwned>(
        &self,
        args: Vec<String>,
    ) -> Result<T, NiriError> {
        let raw = self.run_checked(args)?;
        Ok(serde_json::from_str(&raw)?)
    }

    fn action(&self, args: Vec<String>) -> Result<(), NiriError> {
        let mut full_args = vec!["msg".to_owned(), "action".to_owned()];
        full_args.extend(args);
        self.run_checked(full_args).map(|_| ())
    }
}

impl NiriClient for NiriCliClient {
    fn outputs(&self) -> Result<Vec<DisplayOutput>, NiriError> {
        let by_name: HashMap<String, DisplayOutput> = self.query_json(vec![
            "msg".to_owned(),
            "--json".to_owned(),
            "outputs".to_owned(),
        ])?;
        let mut outputs: Vec<DisplayOutput> = by_name.into_values().collect();
        outputs.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(outputs)
    }

    fn windows(&self) -> Result<Vec<WindowInfo>, NiriError> {
        self.query_json(vec![
            "msg".to_owned(),
            "--json".to_owned(),
            "windows".to_owned(),
        ])
    }

    fn workspaces(&self) -> Result<Vec<WorkspaceInfo>, NiriError> {
        self.query_json(vec![
            "msg".to_owned(),
            "--json".to_owned(),
            "workspaces".to_owned(),
        ])
    }

    fn load_config(&self) -> Result<(), NiriError> {
        self.action(vec!["load-config-file".to_owned()])
    }

    fn move_window_to_output(&self, window_id: u64, output: &OutputId) -> Result<(), NiriError> {
        self.action(vec![
            "move-window-to-monitor".to_owned(),
            "--id".to_owned(),
            window_id.to_string(),
            output.as_str().to_owned(),
        ])
    }

    fn focus_window(&self, window_id: u64) -> Result<(), NiriError> {
        self.action(vec![
            "focus-window".to_owned(),
            "--id".to_owned(),
            window_id.to_string(),
        ])
    }

    fn focus_output(&self, output: &OutputId) -> Result<(), NiriError> {
        self.action(vec!["focus-monitor".to_owned(), output.as_str().to_owned()])
    }

    fn enable_output(&self, output: &OutputId) -> Result<(), NiriError> {
        self.run_checked(vec![
            "msg".to_owned(),
            "output".to_owned(),
            output.as_str().to_owned(),
            "on".to_owned(),
        ])
        .map(|_| ())
    }

    fn validate_config(&self, path: &Path) -> Result<(), NiriError> {
        self.run_checked(vec![
            "validate".to_owned(),
            "-c".to_owned(),
            path.to_string_lossy().into_owned(),
        ])
        .map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    use std::sync::Mutex;

    struct FakeRunner {
        responses: Mutex<VecDeque<(bool, String, String)>>,
        calls: Mutex<Vec<CommandSpec>>,
    }

    impl FakeRunner {
        fn new(responses: Vec<(bool, String, String)>) -> Self {
            Self {
                responses: Mutex::new(responses.into()),
                calls: Mutex::new(Vec::new()),
            }
        }

        fn calls(&self) -> Vec<CommandSpec> {
            self.calls.lock().unwrap().clone()
        }
    }

    impl CommandRunner for FakeRunner {
        fn run(
            &self,
            spec: &CommandSpec,
        ) -> Result<crate::infrastructure::process_runner::CommandOutput, ProcessError> {
            self.calls.lock().unwrap().push(spec.clone());
            let (success, stdout, stderr) = self
                .responses
                .lock()
                .unwrap()
                .pop_front()
                .expect("a prepared response must exist for every call");
            Ok(crate::infrastructure::process_runner::CommandOutput {
                success,
                stdout,
                stderr,
            })
        }
    }

    fn client(responses: Vec<(bool, String, String)>) -> (NiriCliClient, Arc<FakeRunner>) {
        let runner = Arc::new(FakeRunner::new(responses));
        (NiriCliClient::new(runner.clone()), runner)
    }

    #[test]
    fn parses_outputs_and_sorts_by_name() {
        let (client, runner) = client(vec![(
            true,
            include_str!("../../tests/fixtures/niri_outputs_dual.json").to_owned(),
            String::new(),
        )]);
        let outputs = client.outputs().unwrap();
        assert_eq!(outputs.len(), 2);
        assert_eq!(outputs[0].name, "HDMI-A-1");
        assert_eq!(outputs[1].name, "eDP-1");
        assert_eq!(runner.calls()[0].args, vec!["msg", "--json", "outputs"]);
    }

    #[test]
    fn failed_commands_expose_the_error_output() {
        let (client, _runner) = client(vec![(
            false,
            String::new(),
            "compositor unreachable".to_owned(),
        )]);
        let error = client.outputs().unwrap_err();
        match error {
            NiriError::CommandFailed { command, stderr } => {
                assert!(command.contains("msg --json outputs"));
                assert_eq!(stderr, "compositor unreachable");
            }
            other => panic!("expected command failure, got {other}"),
        }
    }

    #[test]
    fn parses_windows_and_workspaces() {
        let windows_json = r#"[
            {"id": 7, "title": "Wayland Mirror", "app_id": "wl-mirror", "workspace_id": 1, "is_focused": true}
        ]"#;
        let workspaces_json = r#"[
            {"id": 1, "name": null, "output": "HDMI-A-1", "is_active": true}
        ]"#;
        let (client, _runner) = client(vec![
            (true, windows_json.to_owned(), String::new()),
            (true, workspaces_json.to_owned(), String::new()),
        ]);
        let windows = client.windows().unwrap();
        assert_eq!(windows[0].app_id.as_deref(), Some("wl-mirror"));
        assert_eq!(windows[0].workspace_id, Some(1));
        let workspaces = client.workspaces().unwrap();
        assert_eq!(workspaces[0].output.as_deref(), Some("HDMI-A-1"));
    }

    #[test]
    fn builds_expected_action_commands() {
        let (client, runner) = client(vec![
            (true, String::new(), String::new()),
            (true, String::new(), String::new()),
            (true, String::new(), String::new()),
            (true, String::new(), String::new()),
            (true, String::new(), String::new()),
        ]);
        client
            .move_window_to_output(7, &OutputId::new("HDMI-A-1"))
            .unwrap();
        client.focus_window(7).unwrap();
        client.focus_output(&OutputId::new("HDMI-A-1")).unwrap();
        client.enable_output(&OutputId::new("eDP-1")).unwrap();
        client.load_config().unwrap();

        let calls = runner.calls();
        assert_eq!(
            calls[0].args,
            vec![
                "msg",
                "action",
                "move-window-to-monitor",
                "--id",
                "7",
                "HDMI-A-1"
            ]
        );
        assert_eq!(
            calls[1].args,
            vec!["msg", "action", "focus-window", "--id", "7"]
        );
        assert_eq!(
            calls[2].args,
            vec!["msg", "action", "focus-monitor", "HDMI-A-1"]
        );
        assert_eq!(calls[3].args, vec!["msg", "output", "eDP-1", "on"]);
        assert_eq!(calls[4].args, vec!["msg", "action", "load-config-file"]);
    }

    #[test]
    fn validates_config_files_and_propagates_failures() {
        let (client, runner) = client(vec![
            (true, String::new(), String::new()),
            (false, String::new(), "expected `}`".to_owned()),
        ]);
        let path = Path::new("/tmp/display.kdl");
        client.validate_config(path).unwrap();
        assert_eq!(
            runner.calls()[0].args,
            vec!["validate", "-c", "/tmp/display.kdl"]
        );

        let error = client.validate_config(path).unwrap_err();
        assert!(error.to_string().contains("expected `}`"));
    }
}
