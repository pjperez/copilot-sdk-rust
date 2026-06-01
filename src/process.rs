// Copyright (c) 2026 Elias Bachaalany
// SPDX-License-Identifier: MIT

//! Process management for the Copilot SDK.
//!
//! Provides async subprocess spawning and management for the Copilot CLI.
//!
//! ## Process-tree cleanup (Job Object / process group)
//!
//! The Copilot CLI subprocess spawns its own children for stdio MCP
//! servers. If we kill only the CLI (TerminateProcess on Windows, SIGKILL
//! on Unix) those MCP grandchildren are reparented to PID 1 / `csrss.exe`
//! and live on as orphans. Repeated SDK restarts then leak MCP processes
//! indefinitely (observed: 94 stranded `node.exe` MCP procs on a long-lived
//! Windows session).
//!
//! Fix:
//!   - **Windows**: wrap the spawned CLI in a Win32 **Job Object** with
//!     `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`. When the SDK drops the
//!     `CopilotProcess` (or its `JobHandle`), the kernel kills every
//!     process still assigned to the job — the CLI plus all its MCP
//!     grandchildren — atomically.
//!   - **Unix**: spawn the CLI as the leader of its own process group
//!     (`setpgid(0, 0)` via `Command::process_group(0)`), and on kill /
//!     drop send the signal to the whole group via `killpg`.
//!
//! There is a small race on Windows between `CreateProcess` returning and
//! `AssignProcessToJobObject` succeeding: if the CLI spawns MCP children
//! before we assign the job, those grandchildren are NOT in the job and
//! will not be killed by `KILL_ON_JOB_CLOSE`. In practice the CLI does
//! nothing of substance until it receives an RPC over stdin, so the race
//! window is microseconds and has not been observed. Eliminating it
//! fully would require `CREATE_SUSPENDED`/`ResumeThread`, which `tokio`
//! and `std` do not expose ergonomically.

use crate::error::{CopilotError, Result};
use crate::transport::StdioTransport;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use tokio::process::{Child, Command};

#[cfg(windows)]
#[allow(unsafe_code)]
mod job {
    //! Win32 Job Object helper. Owns a HANDLE and closes it on Drop, which
    //! triggers `KILL_ON_JOB_CLOSE` on every process assigned to the job.

    use std::io;
    use std::mem;
    use std::os::windows::io::RawHandle;
    use std::ptr;

    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
        SetInformationJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
        JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    };

    pub(super) struct JobHandle(HANDLE);

    // HANDLE is just a numeric kernel handle; safe to move across threads.
    unsafe impl Send for JobHandle {}
    unsafe impl Sync for JobHandle {}

    impl JobHandle {
        /// Create a new anonymous Job Object configured to kill every
        /// assigned process when the last handle to the job is closed.
        pub(super) fn with_kill_on_close() -> io::Result<Self> {
            unsafe {
                let job = CreateJobObjectW(ptr::null(), ptr::null());
                if job.is_null() || job == INVALID_HANDLE_VALUE {
                    return Err(io::Error::last_os_error());
                }
                let job = JobHandle(job);

                let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = mem::zeroed();
                info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;

                let ok = SetInformationJobObject(
                    job.0,
                    JobObjectExtendedLimitInformation,
                    &info as *const _ as *const _,
                    mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                );
                if ok == 0 {
                    return Err(io::Error::last_os_error());
                }
                Ok(job)
            }
        }

        /// Assign a process to this job. After assignment, every child
        /// the process spawns inherits the job (Windows 8+; nested jobs
        /// are allowed by default on modern Windows).
        pub(super) fn assign(&self, process_handle: RawHandle) -> io::Result<()> {
            unsafe {
                let ok = AssignProcessToJobObject(self.0, process_handle as HANDLE);
                if ok == 0 {
                    return Err(io::Error::last_os_error());
                }
            }
            Ok(())
        }
    }

    impl Drop for JobHandle {
        fn drop(&mut self) {
            unsafe {
                if !self.0.is_null() && self.0 != INVALID_HANDLE_VALUE {
                    // Closing the last handle to the job fires
                    // JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE, killing every
                    // process still in the job (CLI + MCP children).
                    CloseHandle(self.0);
                }
            }
        }
    }
}

// =============================================================================
// Process Options
// =============================================================================

/// Options for spawning a subprocess.
#[derive(Debug, Clone)]
pub struct ProcessOptions {
    /// Working directory for the subprocess (None = inherit from parent).
    pub working_directory: Option<PathBuf>,

    /// Environment variables to set.
    pub environment: HashMap<String, String>,

    /// Whether to inherit the parent's environment variables.
    pub inherit_environment: bool,

    /// Whether to redirect stdin (pipe to subprocess).
    pub redirect_stdin: bool,

    /// Whether to redirect stdout (pipe from subprocess).
    pub redirect_stdout: bool,

    /// Whether to redirect stderr (pipe from subprocess).
    pub redirect_stderr: bool,
}

impl Default for ProcessOptions {
    fn default() -> Self {
        Self::new()
    }
}

impl ProcessOptions {
    /// Create new process options with default values.
    pub fn new() -> Self {
        Self {
            working_directory: None,
            environment: HashMap::new(),
            inherit_environment: true,
            redirect_stdin: true,
            redirect_stdout: true,
            redirect_stderr: false,
        }
    }

    /// Set working directory.
    pub fn working_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.working_directory = Some(dir.into());
        self
    }

    /// Add environment variable.
    pub fn env(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.environment.insert(key.into(), value.into());
        self
    }

    /// Set whether to inherit parent environment.
    pub fn inherit_env(mut self, inherit: bool) -> Self {
        self.inherit_environment = inherit;
        self
    }

    /// Set stdin redirection.
    pub fn stdin(mut self, redirect: bool) -> Self {
        self.redirect_stdin = redirect;
        self
    }

    /// Set stdout redirection.
    pub fn stdout(mut self, redirect: bool) -> Self {
        self.redirect_stdout = redirect;
        self
    }

    /// Set stderr redirection.
    pub fn stderr(mut self, redirect: bool) -> Self {
        self.redirect_stderr = redirect;
        self
    }
}

// =============================================================================
// Copilot Process
// =============================================================================

/// A running Copilot CLI process.
pub struct CopilotProcess {
    child: Child,
    transport: Option<StdioTransport>,
    stdout: Option<tokio::process::ChildStdout>,
    stderr: Option<tokio::process::ChildStderr>,
    /// Win32 Job Object that owns the CLI subprocess and all its children
    /// (notably MCP servers). Dropping this handle triggers
    /// `KILL_ON_JOB_CLOSE`, killing the whole tree atomically. `None` only
    /// if Job Object setup failed at spawn time (a warning is logged then).
    #[cfg(windows)]
    _job: Option<job::JobHandle>,
}

impl CopilotProcess {
    /// Spawn a new Copilot CLI process.
    pub fn spawn(
        executable: impl AsRef<Path>,
        args: &[&str],
        options: ProcessOptions,
    ) -> Result<Self> {
        let executable = executable.as_ref();

        // Build command
        let mut cmd = Command::new(executable);
        cmd.args(args);

        // Set working directory
        if let Some(dir) = &options.working_directory {
            cmd.current_dir(dir);
        }

        // Set environment
        if !options.inherit_environment {
            cmd.env_clear();
        }
        for (key, value) in &options.environment {
            cmd.env(key, value);
        }

        // Configure stdio
        cmd.stdin(if options.redirect_stdin {
            Stdio::piped()
        } else {
            Stdio::null()
        });
        cmd.stdout(if options.redirect_stdout {
            Stdio::piped()
        } else {
            Stdio::null()
        });
        cmd.stderr(if options.redirect_stderr {
            Stdio::piped()
        } else {
            Stdio::null()
        });

        // On Windows, prevent child console processes from creating visible
        // console windows when the parent is a GUI application (e.g. Tauri app
        // built with `#![windows_subsystem = "windows"]`).
        #[cfg(windows)]
        {
            const CREATE_NO_WINDOW: u32 = 0x08000000;
            cmd.creation_flags(CREATE_NO_WINDOW);
        }

        // On Unix, make the child the leader of its own process group so
        // we can deliver signals to the whole tree (CLI + MCP children)
        // via killpg. Without this, killing only the CLI orphans every
        // MCP server it spawned.
        #[cfg(unix)]
        {
            cmd.process_group(0);
        }

        // Spawn the process
        let mut child = cmd.spawn().map_err(CopilotError::ProcessStart)?;

        // Wrap the spawned process in a Job Object on Windows so its MCP
        // grandchildren are killed atomically when we drop / force-stop
        // the CLI. See module docs for the small race-window caveat.
        #[cfg(windows)]
        let _job = {
            match job::JobHandle::with_kill_on_close() {
                Ok(handle) => match child.raw_handle() {
                    Some(raw) => match handle.assign(raw as _) {
                        Ok(()) => Some(handle),
                        Err(e) => {
                            tracing::warn!(
                                "Failed to assign Copilot CLI subprocess to Job Object; \
                                 MCP-server children may be orphaned on kill: {}",
                                e
                            );
                            // Drop the unused job handle (no processes assigned,
                            // so KILL_ON_JOB_CLOSE is a no-op).
                            None
                        }
                    },
                    None => {
                        tracing::warn!(
                            "Spawned Copilot CLI subprocess has no raw handle; \
                             MCP-server children may be orphaned on kill"
                        );
                        None
                    }
                },
                Err(e) => {
                    tracing::warn!(
                        "Failed to create Job Object for Copilot CLI subprocess; \
                         MCP-server children may be orphaned on kill: {}",
                        e
                    );
                    None
                }
            }
        };

        // Create transport from stdio handles
        let transport = if options.redirect_stdin && options.redirect_stdout {
            let stdin = child
                .stdin
                .take()
                .ok_or_else(|| CopilotError::InvalidConfig("Failed to capture stdin".into()))?;
            let stdout = child
                .stdout
                .take()
                .ok_or_else(|| CopilotError::InvalidConfig("Failed to capture stdout".into()))?;
            Some(StdioTransport::new(stdin, stdout))
        } else {
            None
        };

        // Capture stdout if redirected but not used for stdio transport.
        let stdout = if transport.is_none() && options.redirect_stdout {
            child.stdout.take()
        } else {
            None
        };

        // Capture stderr if redirected
        let stderr = if options.redirect_stderr {
            child.stderr.take()
        } else {
            None
        };

        Ok(Self {
            child,
            transport,
            stdout,
            stderr,
            #[cfg(windows)]
            _job,
        })
    }

    /// Spawn the Copilot CLI with default options for stdio mode.
    pub fn spawn_stdio(cli_path: impl AsRef<Path>) -> Result<Self> {
        let options = ProcessOptions::new().stdin(true).stdout(true).stderr(false);

        Self::spawn(cli_path, &["--stdio"], options)
    }

    /// Take the transport (can only be called once).
    ///
    /// Returns the stdio transport for communication with the CLI.
    pub fn take_transport(&mut self) -> Option<StdioTransport> {
        self.transport.take()
    }

    /// Take stdout (can only be called once).
    pub fn take_stdout(&mut self) -> Option<tokio::process::ChildStdout> {
        self.stdout.take()
    }

    /// Get the process ID.
    pub fn id(&self) -> Option<u32> {
        self.child.id()
    }

    /// Check if the process is still running.
    pub async fn is_running(&mut self) -> bool {
        self.child.try_wait().ok().flatten().is_none()
    }

    /// Try to get the exit status without blocking.
    pub async fn try_wait(&mut self) -> Result<Option<i32>> {
        match self.child.try_wait() {
            Ok(Some(status)) => Ok(Some(status.code().unwrap_or(-1))),
            Ok(None) => Ok(None),
            Err(e) => Err(CopilotError::Transport(e)),
        }
    }

    /// Wait for the process to exit.
    pub async fn wait(&mut self) -> Result<i32> {
        let status = self.child.wait().await.map_err(CopilotError::Transport)?;
        Ok(status.code().unwrap_or(-1))
    }

    /// Request termination of the process.
    ///
    /// On Unix, sends `SIGKILL` to the whole process group (CLI + MCP
    /// children). On Windows, the Job Object's `KILL_ON_JOB_CLOSE` is
    /// what actually cleans up the tree when this `CopilotProcess` is
    /// dropped, but `start_kill` is also issued so the immediate parent
    /// (the CLI) is terminated eagerly.
    pub fn terminate(&mut self) -> Result<()> {
        self.kill()
    }

    /// Forcefully kill the process *and its children* (MCP servers etc.).
    ///
    /// On Unix this delivers `SIGKILL` to the whole process group via
    /// `killpg` (the CLI was spawned as the leader of its own group).
    /// On Windows this calls `start_kill` on the CLI directly; the
    /// associated Job Object will additionally kill any survivors when
    /// the `CopilotProcess` is dropped.
    pub fn kill(&mut self) -> Result<()> {
        #[cfg(unix)]
        kill_process_group(&self.child);
        self.child.start_kill().map_err(CopilotError::Transport)
    }

    /// Take stderr (can only be called once).
    pub fn take_stderr(&mut self) -> Option<tokio::process::ChildStderr> {
        self.stderr.take()
    }
}

#[cfg(unix)]
#[allow(unsafe_code)]
fn kill_process_group(child: &Child) {
    if let Some(pid) = child.id() {
        // Negative PID = process group. We're the group leader because
        // we spawned the child with `Command::process_group(0)`.
        unsafe {
            libc::kill(-(pid as i32), libc::SIGKILL);
        }
    }
}

/// Drop backstop. On Unix, deliver `SIGKILL` to the whole process group
/// so MCP children don't outlive the SDK's `CopilotProcess`. On Windows,
/// the `_job` field's `Drop` impl fires `KILL_ON_JOB_CLOSE` and does the
/// equivalent atomically — no work needed here.
impl Drop for CopilotProcess {
    fn drop(&mut self) {
        #[cfg(unix)]
        kill_process_group(&self.child);
    }
}

// =============================================================================
// Utility Functions
// =============================================================================

/// Find an executable in the system PATH.
///
/// Returns the full path to the executable if found.
pub fn find_executable(name: &str) -> Option<PathBuf> {
    which::which(name).ok()
}

/// Check if a path looks like a Node.js script.
pub fn is_node_script(path: &Path) -> bool {
    path.extension()
        .is_some_and(|ext| ext == "js" || ext == "mjs")
}

/// Get the system's Node.js executable path.
pub fn find_node() -> Option<PathBuf> {
    find_executable("node")
}

/// Find the Copilot CLI executable.
///
/// Searches for the Copilot CLI in common locations and the system PATH.
pub fn find_copilot_cli() -> Option<PathBuf> {
    // First, allow an explicit override to match the upstream SDKs.
    if let Ok(cli_path) = std::env::var("COPILOT_CLI_PATH") {
        let cli_path = cli_path.trim();
        if !cli_path.is_empty() {
            let path = PathBuf::from(cli_path);
            if path.exists() {
                return Some(path);
            }
        }
    }

    // First, try the system PATH
    if let Some(path) = find_executable("copilot") {
        return Some(path);
    }

    // On Windows, also try "copilot.cmd" and "copilot.exe"
    #[cfg(windows)]
    {
        if let Some(path) = find_executable("copilot.cmd") {
            return Some(path);
        }
        if let Some(path) = find_executable("copilot.exe") {
            return Some(path);
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_process_options_builder() {
        let options = ProcessOptions::new()
            .working_dir("/tmp")
            .env("FOO", "bar")
            .inherit_env(false)
            .stdin(true)
            .stdout(true)
            .stderr(true);

        assert_eq!(options.working_directory, Some(PathBuf::from("/tmp")));
        assert_eq!(options.environment.get("FOO"), Some(&"bar".to_string()));
        assert!(!options.inherit_environment);
        assert!(options.redirect_stdin);
        assert!(options.redirect_stdout);
        assert!(options.redirect_stderr);
    }

    #[test]
    fn test_process_options_default() {
        let options = ProcessOptions::default();

        assert!(options.working_directory.is_none());
        assert!(options.environment.is_empty());
        assert!(options.inherit_environment);
        assert!(options.redirect_stdin);
        assert!(options.redirect_stdout);
        assert!(!options.redirect_stderr);
    }

    #[test]
    fn test_is_node_script() {
        assert!(is_node_script(Path::new("script.js")));
        assert!(is_node_script(Path::new("script.mjs")));
        assert!(is_node_script(Path::new("/path/to/script.js")));
        assert!(!is_node_script(Path::new("script.ts")));
        assert!(!is_node_script(Path::new("script")));
        assert!(!is_node_script(Path::new("script.py")));
    }

    #[test]
    fn test_find_node() {
        // This test just verifies the function doesn't panic
        // Whether it finds node depends on the system
        let _ = find_node();
    }

    #[test]
    fn test_find_copilot_cli() {
        // This test just verifies the function doesn't panic
        // Whether it finds copilot depends on the system
        let _ = find_copilot_cli();
    }

    /// End-to-end regression test for the MCP-orphan leak.
    ///
    /// Spawns `powershell.exe` as the "CLI" and has it spawn a long-sleeping
    /// `powershell` grandchild (the stand-in for an MCP server). On drop,
    /// the Job Object's `KILL_ON_JOB_CLOSE` flag must cascade the kill so
    /// the grandchild dies — proving real MCP servers (workiq, enghub, ...)
    /// won't be orphaned when Hindsight calls `force_stop` or exits.
    #[cfg(windows)]
    #[tokio::test]
    async fn windows_drop_kills_grandchildren_via_job_object() {
        use std::time::{Duration, Instant};
        use tokio::io::{AsyncBufReadExt, BufReader};

        // Parent prints the grandchild PID, then sleeps. Grandchild sleeps
        // 60s; long enough that any non-cascading kill leaves it alive when
        // we poll. `Start-Process -PassThru` returns the new process object
        // so we can echo its Id before either party exits.
        let script = "$p = Start-Process powershell -ArgumentList '-NoProfile','-Command','Start-Sleep -Seconds 60' -PassThru -WindowStyle Hidden; \
                      Write-Host \"CHILDPID:$($p.Id)\"; \
                      Start-Sleep -Seconds 60";

        let options = ProcessOptions::new()
            .stdin(false)
            .stdout(true)
            .stderr(false);

        let mut proc = CopilotProcess::spawn(
            "powershell.exe",
            &["-NoProfile", "-NonInteractive", "-Command", script],
            options,
        )
        .expect("spawn powershell");

        let stdout = proc.take_stdout().expect("stdout pipe");
        let mut reader = BufReader::new(stdout).lines();

        // Wait up to 10s for the grandchild PID line.
        let grandchild_pid: u32 = {
            let deadline = Instant::now() + Duration::from_secs(10);
            loop {
                if Instant::now() >= deadline {
                    panic!("timed out waiting for CHILDPID line from powershell");
                }
                match tokio::time::timeout(Duration::from_millis(500), reader.next_line()).await {
                    Ok(Ok(Some(line))) => {
                        if let Some(rest) = line.trim().strip_prefix("CHILDPID:") {
                            break rest.trim().parse().expect("parse CHILDPID");
                        }
                    }
                    Ok(Ok(None)) => panic!("powershell stdout closed before CHILDPID"),
                    Ok(Err(e)) => panic!("read error: {e}"),
                    Err(_elapsed) => continue,
                }
            }
        };

        // Sanity: grandchild is alive right now.
        assert!(
            is_process_alive(grandchild_pid),
            "grandchild PID {grandchild_pid} should be alive before drop"
        );

        // The whole point of the fix: dropping the SDK handle must kill the
        // grandchild via KILL_ON_JOB_CLOSE.
        drop(proc);

        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            if !is_process_alive(grandchild_pid) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }

        panic!(
            "grandchild PID {grandchild_pid} survived drop of CopilotProcess — \
             Job Object KILL_ON_JOB_CLOSE did not cascade"
        );
    }

    /// `OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION)` returns NULL once the
    /// kernel has fully reaped a process. That's a stable signal across all
    /// Windows versions we care about and avoids spawning a tasklist probe.
    #[cfg(windows)]
    #[allow(unsafe_code)]
    fn is_process_alive(pid: u32) -> bool {
        use windows_sys::Win32::Foundation::CloseHandle;
        use windows_sys::Win32::System::Threading::{
            OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
        };
        unsafe {
            let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
            if h.is_null() {
                false
            } else {
                CloseHandle(h);
                true
            }
        }
    }
}
