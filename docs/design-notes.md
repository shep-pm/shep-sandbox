# Design notes

From a working session on 2026-10-28, while kelpie's first Windows worker ran on crabpot (crabpot #218). Nothing here is decided.

## Starting point

crabpot sandboxes any process: filesystem allow and deny lists, a network allowlist through a filtering proxy, and the OS's own primitives (`sandbox-exec` on macOS, bubblewrap and seccomp on Linux, a separate sandbox user behind a WFP fence on Windows). No container.

Its defaults are shaped for AI agents, because it is a port of Anthropic's sandbox-runtime (`srt`), written for Claude Code:

- mandatory write denies on `.git/hooks`, shell rc files, `.vscode`, `.idea`, `.claude/commands` and `.claude/agents`, whatever `allowWrite` says (`crates/crabpot-core/src/dangerous.rs`)
- credential denies (`~/.ssh`, `~/.aws`, key stores, cargo `credentials`)
- config pointers so git and cargo work in the sandbox (`GIT_CONFIG_GLOBAL`, `git.safeDirectories`, `CARGO_HOME`)
- small Claude Code hooks, such as `CLAUDE_CODE_TMPDIR` on Linux

## Idea 1: agent policy as an optional layer in crabpot

After crabpot reaches parity with upstream, split it in three:

| Layer | What |
|---|---|
| Mechanism | the platform backends, filesystem allow and deny, the proxy, the config |
| Agent policy | the mandatory denies, credential denies, git and cargo env pointers |
| Host install (Windows) | sandbox user, WFP, profile read grant. Setup, not policy, so unchanged |

- The switch is runtime config (a preset or a settings key), not a Cargo feature. Cargo unifies features across a dependency graph, so one crate turning it off would change every other crate's security.
- On by default, so turning protections off is explicit.
- Cost: the agent defaults stop matching upstream line for line, so parity checks name the layer. Another reason to wait for parity.

## Idea 2: shep-sandbox

A way for shep to run a sheep inside a crabpot sandbox.

### Where it lives

crabpot has no daemon. A `Sandbox` lives in the process that started the command, and its grants and proxy go away when that process drops it. That decides the shape. shep-docker can be a dog because Docker's daemon owns the container; a dog cannot wrap a spawn the shepherd does.

| Option | Verdict |
|---|---|
| crabpot linked into the shepherd | No. The shepherd's binary would have to route `--srt-win` and `--crabpot-helper` and be readable by the sandbox user. A crabpot panic in FFI takes down the daemon for the whole flock, and `shep daemon reload` drops every sandbox at once |
| A dog alone | No. Dogs run beside a sheep's lifecycle and cannot wrap its spawn |
| Spawn in shep through a launcher, the rest in a dog | Recommended |

### The recommended split

- In shep, small: a `sandbox = "<settings>"` key on a sheep. The shepherd spawns `crabpot -s <settings> -- <cmd>`, so each sheep gets its own launcher process.
  - A restart is a new sandbox, a crash stays inside one sheep, and on Windows the runner is `crabpot.exe`, not `shep.exe`.
  - shep reads the launcher's refusal (`StartError`) and shows it as an event, so a sandboxed sheep doesn't just look like it crashed.
- The `shep-sandbox` dog, shaped like shep-docker (an external dog you adopt): install and doctor (`windows-install`, status, the profile grant's health, sizing the proxy port range for the flock), policy presets, and surfacing violations.
- Kelpie wraps its own workers in crabpot today. With this in place it could move onto it, which removes one copy of the glue.

### Facts from the kelpie Windows run that bear on it

- Lifetime: a `Sandbox` holds grants and the proxy until it drops. On Windows every new start makes its grants again.
- Scale: the Windows proxy range defaults to `60080-60089`, two ports per sandbox, so five sandboxes at once. A larger flock gets `StartError::ProxyRangeFull` unless the install sizes the range (`--proxy-port-range`).
- Every Windows session runs as one sandbox user (`srt-sandbox`). Isolation between sessions comes from a per-session SID on each grant. Anything a session creates that is keyed to its owner is reachable from every other session.
- A file under the profile made after the profile read grant stays unreadable to the sandbox until the grant runs again.
- The Windows sandbox refuses session 0, so a shepherd running as a service cannot start one. It has to run in the user's desktop session.

## Settled on 2026-10-30

Parity shipped in the meantime, so neither idea waits on it now. The six open questions were answered, and the effort moved to a wayfinder map on this repository's issues, which holds every decision from here on.

1. Signals: on Unix the `crabpot` launcher forwards SIGINT and SIGTERM, and the Linux helper also SIGHUP and SIGQUIT. shep sends its stop signal to the sheep's whole process group. bwrap's `--new-session` keeps that group signal off the command on Linux; on macOS the command gets it twice, from shep and from the launcher. A SIGQUIT or SIGUSR2 stop kills the launcher on its default action (shep-pm/crabpot#283).
2. Windows grants: srt-win reaps a dead holder's grants on every `acl` call, so the next sandbox start cleans up after a killed launcher.
3. Settings: a path per sheep, and presets are files the dog ships.
4. Violations: the launcher's stderr, which already lands in the sheep's log, for v1.
5. Idea 1 does not block shep-sandbox. It is built when a real service hits one of the agent denies.
6. A wayfinder map in this repository, with shep and crabpot tickets hanging off it. macOS and Linux first; Windows waits on a desktop-session story for the shepherd.
