//! `shep sandbox doctor`: what a sandboxed sheep needs on this host.
//!
//! Each check is a function in [`checks`] over [`Env`], the dog's only way to
//! run a command or read a file, so tests answer for the host with a fake.

mod checks;
mod flock;
#[cfg(test)]
mod tests;

pub use flock::FlockError;

use std::{
    fmt, fs, io,
    path::{Path, PathBuf},
    process::{self, Stdio},
};

/// The operating system a check applies to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Os {
    /// bubblewrap and seccomp.
    Linux,
    /// `sandbox-exec`.
    Mac,
    /// Anything else, where no sandbox check applies.
    Other,
}

/// A command for [`Env::run`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Command {
    /// Found on `PATH` unless it holds a separator.
    pub program: String,
    /// The arguments after the program.
    pub args: Vec<String>,
    /// The directory to run in, or the dog's own.
    pub cwd: Option<PathBuf>,
    /// Whether the command starts with a pipe open on descriptor 3.
    pub pipe_on_fd3: bool,
}

impl Command {
    fn new(program: &str, args: &[&str]) -> Self {
        Self {
            program: program.to_owned(),
            args: args.iter().map(|arg| (*arg).to_owned()).collect(),
            cwd: None,
            pipe_on_fd3: false,
        }
    }

    fn in_dir(mut self, cwd: &Path) -> Self {
        self.cwd = Some(cwd.to_owned());
        self
    }

    fn with_pipe_on_fd3(mut self) -> Self {
        self.pipe_on_fd3 = true;
        self
    }
}

/// How a command ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Output {
    /// The exit code, or `None` when a signal ended it.
    pub code: Option<i32>,
    /// Standard output as text.
    pub stdout: String,
    /// Standard error as text.
    pub stderr: String,
}

/// A sheep in the flock with `sandbox` set.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SandboxedSheep {
    /// The sheep's name.
    pub name: String,
    /// The settings file, resolved against `cwd` when relative.
    pub settings: PathBuf,
    /// The directory the sheep runs in.
    pub cwd: PathBuf,
}

/// The host as the checks see it.
pub trait Env {
    /// The operating system the dog runs on.
    fn os(&self) -> Os;
    /// Runs `command` to the end.
    fn run(&self, command: &Command) -> io::Result<Output>;
    /// Reads a file as text.
    fn read(&self, path: &Path) -> io::Result<String>;
    /// Whether `path` exists.
    fn exists(&self, path: &Path) -> bool;
    /// Writes `contents` to a new file in the temp directory.
    fn write_temp(&self, contents: &str) -> io::Result<PathBuf>;
    /// Removes a file [`Env::write_temp`] made.
    fn remove(&self, path: &Path);
    /// The sheep in the flock with `sandbox` set, or why they are unknown.
    fn sandboxed_sheep(&self) -> Result<Vec<SandboxedSheep>, FlockError>;
}

/// How one check ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    /// The host has what the check looks for.
    Pass(String),
    /// The host lacks it, with the command or step that fixes it.
    Fail {
        /// What was found.
        detail: String,
        /// How to fix it.
        fix: String,
    },
    /// The check does not apply here, with the reason.
    Skip(String),
}

/// One named check and how it ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Check {
    /// What the check looks at.
    pub name: String,
    /// How it ended.
    pub status: Status,
}

/// Every check, in the order they ran.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Report {
    /// The checks.
    pub checks: Vec<Check>,
}

impl Report {
    /// 1 when any check failed, else 0, so a deploy script can gate on it.
    pub fn exit_code(&self) -> u8 {
        let failed = self
            .checks
            .iter()
            .any(|check| matches!(check.status, Status::Fail { .. }));
        u8::from(failed)
    }
}

impl fmt::Display for Report {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for check in &self.checks {
            match &check.status {
                Status::Pass(detail) => writeln!(f, "ok    {}: {detail}", check.name)?,
                Status::Skip(reason) => writeln!(f, "skip  {}: {reason}", check.name)?,
                Status::Fail { detail, fix } => {
                    writeln!(f, "FAIL  {}: {detail}", check.name)?;
                    for line in fix.lines() {
                        writeln!(f, "      {line}")?;
                    }
                }
            }
        }
        Ok(())
    }
}

/// Runs every check against `env`.
pub fn run(env: &dyn Env) -> Report {
    let mut checks = vec![
        checks::crabpot(env),
        checks::bwrap(env),
        checks::sandbox_exec(env),
        checks::userns_sysctl(env),
        checks::userns_profile(env),
        checks::shepherd_channel(env),
    ];
    checks.extend(checks::flock(env));
    Report { checks }
}

/// Reads the flock from the shepherd, for [`System`] to answer with.
pub async fn read_flock() -> Result<Vec<SandboxedSheep>, FlockError> {
    flock::read().await
}

/// The real host, with the flock as [`read_flock`] found it.
#[derive(Debug, Clone)]
pub struct System {
    flock: Result<Vec<SandboxedSheep>, FlockError>,
}

impl System {
    /// The host whose flock is `flock`.
    pub fn new(flock: Result<Vec<SandboxedSheep>, FlockError>) -> Self {
        Self { flock }
    }
}
impl Env for System {
    fn os(&self) -> Os {
        if cfg!(target_os = "linux") {
            Os::Linux
        } else if cfg!(target_os = "macos") {
            Os::Mac
        } else {
            Os::Other
        }
    }

    fn run(&self, command: &Command) -> io::Result<Output> {
        let mut process = if command.pipe_on_fd3 {
            // The shell dups the stdin pipe onto descriptor 3 before it
            // execs, which sets up the descriptor without `unsafe`.
            let mut process = process::Command::new("sh");
            process
                .args(["-c", "exec 3<&0; exec \"$@\"", "sh", &command.program])
                .args(&command.args)
                .stdin(Stdio::piped());
            process
        } else {
            let mut process = process::Command::new(&command.program);
            process.args(&command.args).stdin(Stdio::null());
            process
        };
        if let Some(cwd) = &command.cwd {
            process.current_dir(cwd);
        }
        let output = process.output()?;
        Ok(Output {
            code: output.status.code(),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        })
    }

    fn read(&self, path: &Path) -> io::Result<String> {
        fs::read_to_string(path)
    }

    fn exists(&self, path: &Path) -> bool {
        path.exists()
    }

    fn write_temp(&self, contents: &str) -> io::Result<PathBuf> {
        use std::io::Write;

        let path = std::env::temp_dir().join(format!("shep-sandbox-doctor-{}.json", process::id()));
        fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)?
            .write_all(contents.as_bytes())?;
        Ok(path)
    }

    fn remove(&self, path: &Path) {
        let _ = fs::remove_file(path);
    }

    fn sandboxed_sheep(&self) -> Result<Vec<SandboxedSheep>, FlockError> {
        self.flock.clone()
    }
}
