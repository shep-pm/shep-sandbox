//! The checks, one function each. A check that does not apply here is
//! [`Status::Skip`] with the reason.

use std::{io, path::Path};

use super::{Check, Command, Env, Os, SandboxedSheep, Status};

/// crabpot's exit code when it refuses to start, as opposed to the
/// command it runs failing.
const REFUSED: i32 = 125;
const SANDBOX_EXEC: &str = "/usr/bin/sandbox-exec";
const OS_RELEASE: &str = "/etc/os-release";
const USERNS_SYSCTL: &str = "/proc/sys/kernel/apparmor_restrict_unprivileged_userns";
const APPARMOR_PROFILES: &str = "/sys/kernel/security/apparmor/profiles";
const BWRAP_PROFILE: &str = "bwrap-userns-restrict";
/// The first Ubuntu release that restricts unprivileged user namespaces.
const UBUNTU_RESTRICTS_FROM: (u32, u32) = (24, 4);

fn check(name: &str, status: Status) -> Check {
    Check {
        name: name.to_owned(),
        status,
    }
}

fn fail(detail: impl Into<String>, fix: impl Into<String>) -> Status {
    Status::Fail {
        detail: detail.into(),
        fix: fix.into(),
    }
}

fn describe(err: &io::Error) -> String {
    if err.kind() == io::ErrorKind::NotFound {
        "not found".to_owned()
    } else {
        err.to_string()
    }
}

/// The first line of `<program> --version`, or why there is none.
fn version_of(env: &dyn Env, program: &str) -> Result<String, String> {
    let output = env
        .run(&Command::new(program, &["--version"]))
        .map_err(|err| format!("{program} could not run: {}", describe(&err)))?;
    match output.code {
        Some(0) => Ok(output.stdout.lines().next().unwrap_or("").trim().to_owned()),
        code => Err(format!(
            "`{program} --version` ended with {}",
            code.map_or("a signal".to_owned(), |code| format!("exit {code}"))
        )),
    }
}

/// crabpot is on `PATH`.
pub fn crabpot(env: &dyn Env) -> Check {
    let status = match version_of(env, "crabpot") {
        Ok(version) => Status::Pass(version),
        Err(detail) => fail(
            detail,
            "Install crabpot and put it on the shepherd's PATH:\n\
             cargo install crabpot --locked",
        ),
    };
    check("crabpot", status)
}

/// bubblewrap is on `PATH`, on Linux.
pub fn bwrap(env: &dyn Env) -> Check {
    let status = if env.os() != Os::Linux {
        Status::Skip("Linux only".to_owned())
    } else {
        match version_of(env, "bwrap") {
            Ok(version) => Status::Pass(version),
            Err(detail) => fail(
                detail,
                "Install bubblewrap, for example:\nsudo apt install bubblewrap",
            ),
        }
    };
    check("bwrap", status)
}

/// `sandbox-exec` is installed, on macOS.
pub fn sandbox_exec(env: &dyn Env) -> Check {
    let status = if env.os() != Os::Mac {
        Status::Skip("macOS only".to_owned())
    } else if env.exists(Path::new(SANDBOX_EXEC)) {
        Status::Pass(SANDBOX_EXEC.to_owned())
    } else {
        fail(
            format!("{SANDBOX_EXEC} does not exist"),
            "macOS ships sandbox-exec; restore it by reinstalling or updating macOS.",
        )
    };
    check("sandbox-exec", status)
}

/// Whether this is Ubuntu 24.04 or later, which restricts user namespaces.
fn restricts_userns(env: &dyn Env) -> Result<(), String> {
    if env.os() != Os::Linux {
        return Err("Linux only".to_owned());
    }
    let release = env
        .read(Path::new(OS_RELEASE))
        .map_err(|_| "not Ubuntu 24.04 or later".to_owned())?;
    let value = |key: &str| {
        release.lines().find_map(|line| {
            let rest = line.strip_prefix(key)?.strip_prefix('=')?;
            Some(rest.trim_matches('"'))
        })
    };
    let version = value("VERSION_ID").and_then(|id| {
        let (major, minor) = id.split_once('.')?;
        Some((major.parse::<u32>().ok()?, minor.parse::<u32>().ok()?))
    });
    match (value("ID"), version) {
        (Some("ubuntu"), Some(version)) if version >= UBUNTU_RESTRICTS_FROM => Ok(()),
        _ => Err("not Ubuntu 24.04 or later".to_owned()),
    }
}

/// `kernel.apparmor_restrict_unprivileged_userns` is 0, on Ubuntu 24.04+.
pub fn userns_sysctl(env: &dyn Env) -> Check {
    let name = "apparmor_restrict_unprivileged_userns";
    if let Err(reason) = restricts_userns(env) {
        return check(name, Status::Skip(reason));
    }
    let status = match env.read(Path::new(USERNS_SYSCTL)) {
        Ok(value) if value.trim() == "0" => Status::Pass("0".to_owned()),
        Ok(value) => fail(
            format!(
                "kernel.apparmor_restrict_unprivileged_userns is {}",
                value.trim()
            ),
            "Turn it off now and at every boot:\n\
             echo 'kernel.apparmor_restrict_unprivileged_userns = 0' | \
             sudo tee /etc/sysctl.d/60-apparmor-userns.conf\n\
             sudo sysctl --system",
        ),
        Err(err) => Status::Skip(format!("cannot read {USERNS_SYSCTL}: {}", describe(&err))),
    };
    check(name, status)
}

/// Ubuntu's `bwrap-userns-restrict` AppArmor profile is not loaded.
pub fn userns_profile(env: &dyn Env) -> Check {
    let name = BWRAP_PROFILE;
    if let Err(reason) = restricts_userns(env) {
        return check(name, Status::Skip(reason));
    }
    let status = match env.read(Path::new(APPARMOR_PROFILES)) {
        Ok(profiles) => {
            let loaded = profiles
                .lines()
                .any(|line| line.split_whitespace().next() == Some(BWRAP_PROFILE));
            if loaded {
                fail(
                    format!("the {BWRAP_PROFILE} AppArmor profile is loaded"),
                    format!(
                        "Unload it and keep it unloaded at boot:\n\
                         sudo ln -s /etc/apparmor.d/{BWRAP_PROFILE} /etc/apparmor.d/disable/\n\
                         sudo apparmor_parser -R /etc/apparmor.d/{BWRAP_PROFILE}"
                    ),
                )
            } else {
                Status::Pass("not loaded".to_owned())
            }
        }
        Err(err) => Status::Skip(format!(
            "cannot read {APPARMOR_PROFILES}: {}; run doctor as root to check",
            describe(&err)
        )),
    };
    check(name, status)
}

fn crabpot_in(settings: &Path, cwd: Option<&Path>, command: &[&str]) -> Command {
    let settings = settings.to_string_lossy();
    let mut args = vec!["-s", &settings, "--"];
    args.extend(command);
    let command = Command::new("crabpot", &args);
    match cwd {
        Some(cwd) => command.in_dir(cwd),
        None => command,
    }
}

/// A descriptor the shepherd opens reaches a sandboxed command, on Linux.
pub fn shepherd_channel(env: &dyn Env) -> Check {
    let name = "shepherd channel";
    if env.os() != Os::Linux {
        return check(name, Status::Skip("Linux only".to_owned()));
    }
    let settings = match env.write_temp("{}") {
        Ok(path) => path,
        Err(err) => {
            return check(
                name,
                fail(
                    format!("could not write the probe's settings file: {err}"),
                    "Make the temp directory writable for this user.",
                ),
            );
        }
    };
    let probe =
        crabpot_in(&settings, None, &["sh", "-c", "test -e /proc/self/fd/3"]).with_pipe_on_fd3();
    let result = env.run(&probe);
    env.remove(&settings);
    let status = match result {
        Ok(output) if output.code == Some(0) => {
            Status::Pass("descriptor 3 reaches a sandboxed command".to_owned())
        }
        Ok(output) if output.code == Some(REFUSED) => fail(
            format!(
                "crabpot refused to start the probe: {}",
                output.stderr.trim()
            ),
            "Fix the failures above, then run doctor again.",
        ),
        Ok(_) => fail(
            "descriptor 3 does not reach a sandboxed command",
            "crabpot 0.7.0 and 0.8.0 replace and close an inherited descriptor 3\n\
             (shep-pm/crabpot#307), which cuts a sheep off from the shepherd:\n\
             wait_ready and shutdown_with_message fail.\n\
             Upgrade crabpot to a release that closes that issue.",
        ),
        Err(err) => fail(
            format!("the probe could not run: {}", describe(&err)),
            "Fix the crabpot check above, then run doctor again.",
        ),
    };
    check(name, status)
}

fn sheep_check(env: &dyn Env, sheep: &SandboxedSheep) -> Check {
    let name = format!("sheep {}", sheep.name);
    let settings = if sheep.settings.is_absolute() {
        sheep.settings.clone()
    } else {
        sheep.cwd.join(&sheep.settings)
    };
    if !env.exists(&settings) {
        return check(
            &name,
            fail(
                format!("settings file {} does not exist", settings.display()),
                format!(
                    "Create it, for example:\nshep sandbox init {}",
                    settings.display()
                ),
            ),
        );
    }
    // crabpot runs in `cwd`, so a relative path is passed as written.
    let command = crabpot_in(&sheep.settings, Some(&sheep.cwd), &["true"]);
    let status = match env.run(&command) {
        Ok(output) if output.code == Some(0) => {
            Status::Pass(format!("{} loads", settings.display()))
        }
        Ok(output) if output.code == Some(REFUSED) => fail(
            format!("crabpot refused to start: {}", output.stderr.trim()),
            format!("Fix {} as crabpot's message says.", settings.display()),
        ),
        Ok(output) => fail(
            format!(
                "`true` under crabpot ended with {:?}: {}",
                output.code,
                output.stderr.trim()
            ),
            format!("Check {} and the sandbox checks above.", settings.display()),
        ),
        Err(err) => fail(
            format!("crabpot could not run: {}", describe(&err)),
            "Fix the crabpot check above, then run doctor again.",
        ),
    };
    check(&name, status)
}

/// Each sheep with `sandbox` set has settings crabpot loads.
pub fn flock(env: &dyn Env) -> Vec<Check> {
    let sheep = env.sandboxed_sheep();
    if sheep.is_empty() {
        return vec![check(
            "flock",
            Status::Skip("no sheep in the flock has `sandbox` set".to_owned()),
        )];
    }
    sheep.iter().map(|sheep| sheep_check(env, sheep)).collect()
}
