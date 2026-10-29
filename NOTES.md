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

## Open questions

1. Does the `crabpot` CLI forward shep's stop and reload signals to its child, on every platform?
2. On Windows, a forced kill of the launcher skips `Drop`, so its grants stay behind. crabpot's `crates/crabpot/src/windows/held.rs` suggests it tracks grants it could not revoke. Check whether a later start cleans them up.
3. Where does a sheep's sandbox settings file live, and is it per sheep or shared presets?
4. How do violations reach shep: the launcher's stderr, or a structured channel?
5. Does the agent-policy layer (idea 1) land before shep-sandbox, so a plain service can opt out of agent defaults?
6. Does shep-sandbox start as a shep design issue, a crabpot ADR, or both?
