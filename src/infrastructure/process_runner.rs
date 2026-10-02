//! External process execution, supervision, and mirror pid tracking.
//!
//! Short-lived commands (niri queries, validation) run through
//! [`CommandRunner`] with a timeout. The long-lived wl-mirror helper runs
//! through [`ProcessSupervisor`], which owns the child handle and shuts it
//! down cleanly with SIGTERM followed by a SIGKILL fallback.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use nix::errno::Errno;
use nix::sys::signal::{Signal, kill};
use nix::unistd::Pid;

/// Process execution and supervision errors.
#[derive(Debug, thiserror::Error)]
pub enum ProcessError {
    #[error("failed to execute {program}: {source}")]
    Spawn {
        program: String,
        source: std::io::Error,
    },
    #[error("command {program} timed out after {timeout:?}")]
    Timeout { program: String, timeout: Duration },
    #[error("failed to run command: {0}")]
    Io(#[from] std::io::Error),
    #[error("command output reader thread panicked")]
    ReaderPanicked,
    #[error("internal process state lock is poisoned")]
    LockPoisoned,
    #[error("invalid process id {0}")]
    InvalidPid(u32),
    #[error("failed to signal process {pid}: {source}")]
    Signal { pid: i32, source: Errno },
    #[error("process {pid} did not exit within {grace:?}")]
    TerminationTimeout { pid: i32, grace: Duration },
    #[error("failed to read mirror pid file {}: {source}", path.display())]
    PidFileRead {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("failed to write mirror pid file {}: {source}", path.display())]
    PidFileWrite {
        path: PathBuf,
        source: std::io::Error,
    },
}

/// Specification of a short-lived command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandSpec {
    pub program: String,
    pub args: Vec<String>,
    pub timeout: Duration,
}

impl CommandSpec {
    pub fn new(program: impl Into<String>, args: Vec<String>) -> Self {
        Self {
            program: program.into(),
            args,
            timeout: Duration::from_secs(10),
        }
    }

    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }
}

/// Captured result of a short-lived command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandOutput {
    pub success: bool,
    pub stdout: String,
    pub stderr: String,
}

/// Runs short-lived commands with a timeout.
pub trait CommandRunner: Send + Sync {
    fn run(&self, spec: &CommandSpec) -> Result<CommandOutput, ProcessError>;
}

/// Production command runner backed by `std::process`.
pub struct SystemCommandRunner;

impl CommandRunner for SystemCommandRunner {
    fn run(&self, spec: &CommandSpec) -> Result<CommandOutput, ProcessError> {
        let mut child = Command::new(&spec.program)
            .args(&spec.args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|source| ProcessError::Spawn {
                program: spec.program.clone(),
                source,
            })?;

        let stdout_reader = spawn_reader(child.stdout.take());
        let stderr_reader = spawn_reader(child.stderr.take());

        let deadline = Instant::now() + spec.timeout;
        let status = loop {
            if let Some(status) = child.try_wait()? {
                break status;
            }
            if Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                return Err(ProcessError::Timeout {
                    program: spec.program.clone(),
                    timeout: spec.timeout,
                });
            }
            std::thread::sleep(Duration::from_millis(10));
        };

        let stdout = join_reader(stdout_reader)?;
        let stderr = join_reader(stderr_reader)?;
        Ok(CommandOutput {
            success: status.success(),
            stdout,
            stderr,
        })
    }
}

/// Supervises a single long-lived child process (the wl-mirror instance).
pub trait ProcessSupervisor: Send + Sync {
    /// Spawn the process, replacing and terminating any previously supervised one.
    fn spawn(&self, spec: &CommandSpec) -> Result<u32, ProcessError>;

    fn is_running(&self) -> Result<bool, ProcessError>;

    /// Terminate the supervised process: SIGTERM, then SIGKILL after `grace`.
    fn terminate(&self, grace: Duration) -> Result<(), ProcessError>;

    /// Terminate an arbitrary tracked pid (used for orphan cleanup).
    fn terminate_pid(&self, pid: u32, grace: Duration) -> Result<(), ProcessError>;
}

/// Production supervisor backed by `std::process::Child`.
#[derive(Default)]
pub struct SystemSupervisor {
    child: Mutex<Option<Child>>,
}

impl SystemSupervisor {
    pub fn new() -> Self {
        Self::default()
    }
}

impl ProcessSupervisor for SystemSupervisor {
    fn spawn(&self, spec: &CommandSpec) -> Result<u32, ProcessError> {
        let mut guard = self.child.lock().map_err(|_| ProcessError::LockPoisoned)?;
        if let Some(mut existing) = guard.take() {
            let _ = existing.kill();
            let _ = existing.wait();
        }
        let child = Command::new(&spec.program)
            .args(&spec.args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|source| ProcessError::Spawn {
                program: spec.program.clone(),
                source,
            })?;
        let pid = child.id();
        *guard = Some(child);
        Ok(pid)
    }

    fn is_running(&self) -> Result<bool, ProcessError> {
        let mut guard = self.child.lock().map_err(|_| ProcessError::LockPoisoned)?;
        match guard.as_mut() {
            Some(child) => match child.try_wait()? {
                Some(_status) => {
                    *guard = None;
                    Ok(false)
                }
                None => Ok(true),
            },
            None => Ok(false),
        }
    }

    fn terminate(&self, grace: Duration) -> Result<(), ProcessError> {
        let mut guard = self.child.lock().map_err(|_| ProcessError::LockPoisoned)?;
        let Some(mut child) = guard.take() else {
            return Ok(());
        };
        terminate_child(&mut child, grace)
    }

    fn terminate_pid(&self, pid: u32, grace: Duration) -> Result<(), ProcessError> {
        terminate_pid_raw(pid, grace)
    }
}

/// Tracks the pid of the wl-mirror process spawned by this manager.
///
/// The file lives in `$XDG_RUNTIME_DIR` and lets a new session clean up an
/// orphaned wl-mirror left behind by a crash.
#[derive(Debug, Clone)]
pub struct MirrorPidFile {
    path: PathBuf,
}

impl MirrorPidFile {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn write(&self, pid: u32) -> Result<(), ProcessError> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent).map_err(|source| ProcessError::PidFileWrite {
                path: self.path.clone(),
                source,
            })?;
        }
        std::fs::write(&self.path, format!("{pid}\n")).map_err(|source| {
            ProcessError::PidFileWrite {
                path: self.path.clone(),
                source,
            }
        })
    }

    pub fn read(&self) -> Result<Option<u32>, ProcessError> {
        match std::fs::read_to_string(&self.path) {
            Ok(contents) => Ok(contents.trim().parse::<u32>().ok()),
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(source) => Err(ProcessError::PidFileRead {
                path: self.path.clone(),
                source,
            }),
        }
    }

    pub fn remove(&self) -> Result<(), ProcessError> {
        match std::fs::remove_file(&self.path) {
            Ok(()) => Ok(()),
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(source) => Err(ProcessError::PidFileWrite {
                path: self.path.clone(),
                source,
            }),
        }
    }

    /// Terminate a leftover wl-mirror recorded by a previous session.
    ///
    /// `is_ours` guards against pid reuse: only processes that look like our
    /// wl-mirror instance are signalled. Returns whether a process was stopped.
    pub fn cleanup_orphan(&self, is_ours: impl Fn(u32) -> bool) -> Result<bool, ProcessError> {
        let Some(pid) = self.read()? else {
            return Ok(false);
        };
        let cleaned = is_ours(pid);
        if cleaned {
            terminate_pid_raw(pid, Duration::from_secs(2))?;
        }
        self.remove()?;
        Ok(cleaned)
    }
}

/// Check whether a pid belongs to a process named `wl-mirror`.
pub fn process_is_wl_mirror(pid: u32) -> bool {
    std::fs::read_to_string(format!("/proc/{pid}/comm"))
        .map(|name| name.trim() == "wl-mirror")
        .unwrap_or(false)
}

fn spawn_reader<R: Read + Send + 'static>(
    reader: Option<R>,
) -> std::thread::JoinHandle<Result<String, std::io::Error>> {
    std::thread::spawn(move || {
        let mut buffer = String::new();
        if let Some(mut reader) = reader {
            reader.read_to_string(&mut buffer)?;
        }
        Ok(buffer)
    })
}

fn join_reader(
    handle: std::thread::JoinHandle<Result<String, std::io::Error>>,
) -> Result<String, ProcessError> {
    match handle.join() {
        Ok(Ok(text)) => Ok(text),
        Ok(Err(source)) => Err(ProcessError::Io(source)),
        Err(_) => Err(ProcessError::ReaderPanicked),
    }
}

fn terminate_child(child: &mut Child, grace: Duration) -> Result<(), ProcessError> {
    let pid = child.id();
    let raw = i32::try_from(pid).map_err(|_| ProcessError::InvalidPid(pid))?;
    match kill(Pid::from_raw(raw), Signal::SIGTERM) {
        Ok(()) | Err(Errno::ESRCH) => {}
        Err(source) => return Err(ProcessError::Signal { pid: raw, source }),
    }
    let deadline = Instant::now() + grace;
    while Instant::now() < deadline {
        if child.try_wait()?.is_some() {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let _ = child.kill();
    let _ = child.wait();
    Ok(())
}

fn terminate_pid_raw(pid: u32, grace: Duration) -> Result<(), ProcessError> {
    let raw = i32::try_from(pid).map_err(|_| ProcessError::InvalidPid(pid))?;
    let target = Pid::from_raw(raw);
    match kill(target, Signal::SIGTERM) {
        Ok(()) | Err(Errno::ESRCH) => {}
        Err(source) => return Err(ProcessError::Signal { pid: raw, source }),
    }
    let deadline = Instant::now() + grace;
    while Instant::now() < deadline {
        if !process_exists(pid) {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    match kill(target, Signal::SIGKILL) {
        Ok(()) | Err(Errno::ESRCH) => {}
        Err(source) => return Err(ProcessError::Signal { pid: raw, source }),
    }
    for _ in 0..25 {
        if !process_exists(pid) {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    Err(ProcessError::TerminationTimeout { pid: raw, grace })
}

/// Whether a pid is alive and not a zombie.
fn process_exists(pid: u32) -> bool {
    match std::fs::read_to_string(format!("/proc/{pid}/stat")) {
        Ok(stat) => {
            // The state field follows the parenthesised command name.
            stat.rsplit(')')
                .next()
                .and_then(|rest| rest.split_whitespace().next())
                .map(|state| state != "Z")
                .unwrap_or(false)
        }
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sleep_spec(seconds: u32) -> CommandSpec {
        CommandSpec::new("sleep", vec![seconds.to_string()])
    }

    #[test]
    fn runner_captures_stdout_and_status() {
        let runner = SystemCommandRunner;
        let spec = CommandSpec::new("sh", vec!["-c".to_owned(), "printf hello".to_owned()]);
        let output = runner.run(&spec).unwrap();
        assert!(output.success);
        assert_eq!(output.stdout, "hello");

        let failing = CommandSpec::new("sh", vec!["-c".to_owned(), "exit 3".to_owned()]);
        let output = runner.run(&failing).unwrap();
        assert!(!output.success);
    }

    #[test]
    fn runner_times_out_and_kills_the_child() {
        let runner = SystemCommandRunner;
        let spec = sleep_spec(30).with_timeout(Duration::from_millis(200));
        let error = runner.run(&spec).unwrap_err();
        match error {
            ProcessError::Timeout { program, .. } => assert_eq!(program, "sleep"),
            other => panic!("expected timeout, got {other}"),
        }
    }

    #[test]
    fn supervisor_terminates_child_with_sigterm() {
        let supervisor = SystemSupervisor::new();
        let pid = supervisor.spawn(&sleep_spec(30)).unwrap();
        assert!(supervisor.is_running().unwrap());
        assert!(process_exists(pid));
        supervisor.terminate(Duration::from_millis(800)).unwrap();
        assert!(!supervisor.is_running().unwrap());
        assert!(!process_exists(pid));
    }

    #[test]
    fn terminate_pid_stops_an_arbitrary_process() {
        let mut child = Command::new("sleep").arg("30").spawn().unwrap();
        let pid = child.id();
        let supervisor = SystemSupervisor::new();
        supervisor
            .terminate_pid(pid, Duration::from_millis(800))
            .unwrap();
        assert!(!process_exists(pid));
        let _ = child.wait();
    }

    #[test]
    fn terminate_pid_is_idempotent_for_dead_processes() {
        let supervisor = SystemSupervisor::new();
        supervisor
            .terminate_pid(99_999_999, Duration::from_millis(100))
            .unwrap();
    }

    #[test]
    fn pid_file_round_trips_and_cleans_up_orphans() {
        let directory = tempfile::tempdir().unwrap();
        let pid_file = MirrorPidFile::new(directory.path().join("mirror.pid"));
        assert_eq!(pid_file.read().unwrap(), None);

        pid_file.write(4242).unwrap();
        assert_eq!(pid_file.read().unwrap(), Some(4242));

        let cleaned = pid_file
            .cleanup_orphan(|pid| {
                assert_eq!(pid, 4242);
                false
            })
            .unwrap();
        assert!(!cleaned);
        assert_eq!(pid_file.read().unwrap(), None);
    }
}
