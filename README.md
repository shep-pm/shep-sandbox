# shep-sandbox

[![crates.io](https://img.shields.io/crates/v/shep-sandbox.svg)](https://crates.io/crates/shep-sandbox)
[![License](https://img.shields.io/crates/l/shep-sandbox.svg)](https://github.com/shep-pm/shep-sandbox#license)

Run a [shep](https://github.com/shep-pm/shep) sheep inside a [crabpot](https://github.com/shep-pm/crabpot) sandbox.

Status: being built. The design lives on this repository's issues, as a map labelled `wayfinder:map`. [docs/design-notes.md](docs/design-notes.md) holds the discussion that started it.

## Adopt it

```sh
cargo install --git https://github.com/shep-pm/shep-sandbox
shep adopt shep-sandbox --yes
```

shep names the dog `sandbox`. Adopted, it connects to the shepherd, reads its `[sandbox]` section of `dogs.toml` (which has no keys yet) and idles until the shepherd stops it. It also checks each sheep's `[app.dogs.sandbox]` table before `shep` writes it.

The `init` and `doctor` verbs of the first design are removed, and return once ported to the settings table the dog launches a sheep from.

## Launch a sheep

```sh
shep-sandbox launch <sheep> -- <command> [args...]
```

The shepherd spawns a sandboxed sheep this way. `launch` reads `$SHEP_HOME`, which the shepherd passes, asks the shepherd for the sheep's `[app.dogs.sandbox]` table, adds the current directory (the sheep's cwd) to `filesystem.allowWrite`, and runs the command under crabpot with that table. The command's exit status is `launch`'s.

- Every refusal exits 125, as crabpot does, with one line on stderr naming the cause: no sheep name before `--`, no `$SHEP_HOME`, a shepherd that cannot be reached, no table for the sheep, a table crabpot refuses, no command after `--`, or crabpot's own refusal.
- A command that crabpot never reached never runs. `launch` has no fallback that runs it outside the sandbox.
- On Windows `launch` exits 125 saying the platform is not supported.
- On Linux the sandbox runs this binary again inside itself, so the dog's binary must be readable inside the sandbox.
- The command sees `$SHEP_HOME` unchanged, because crabpot passes the environment through.

Do not let a sheep reach the shepherd's control socket. That socket trusts any process of the same user, so a sheep that can connect to it holds the operator's authority over the whole flock. `network.allowAllUnixSockets`, or a `network.allowUnixSockets` entry that reaches `$SHEP_HOME/run/shep.sock`, grants exactly that.

## A sheep's keys

A sheep's `[app.dogs.sandbox]` table takes crabpot's settings keys, spelled as crabpot spells them (`network.allowedDomains`, `filesystem.denyRead`, `enableWeakerNetworkIsolation`). The dog serves crabpot's own JSON Schema for them as `x-shep-sheep` in its `--schema` answer, so the schema is the list of keys.

- The five lists crabpot requires (`network.allowedDomains`, `network.deniedDomains`, `filesystem.denyRead`, `filesystem.allowWrite`, `filesystem.denyWrite`) may be left out. The dog fills each with an empty list.
- `shep` asks the dog to validate a table before it writes it, and the dog hands the table to crabpot. What crabpot refuses is refused with crabpot's own message, including a key the platform cannot honour, which the schema does not describe.
- A key crabpot does not know is dropped, as crabpot drops it.
- A macOS sheep that runs file watchers needs `allowMachLookup: ["com.apple.FSEvents"]` under `network`. The dog does not add it.

## Editing a running sheep's table

`launch` stays connected to the shepherd for as long as the sheep runs and follows its `[app.dogs.sandbox]` table. It reads the table again on every change event.

- Live: `network.allowedDomains`, `network.deniedDomains` and `network.deniedResolvedAddresses`. A change reaches the running sandbox through crabpot's `--control-fd`, with no restart.
- Everything else needs a restart, `network.allowTcp` and every `filesystem` key included. The running sandbox keeps its old value, and the notice repeats on a later edit while the key still differs from it. `launch` writes one line to stderr, which lands in the sheep's err log, naming the key: `shep-sandbox: web: filesystem.denyRead changed, restart the sheep to apply it`.
- A table crabpot refuses is logged the same way, and the settings in force stay.
- A removed table is logged, saying the next start is refused. The running sandbox stays as it is.
- A lost shepherd connection is logged once. The sandbox keeps its last lists while `launch` reconnects, waiting 1 second and doubling up to 30, and reads the table again once it is back.
- Nothing restarts the sheep.

`shep-sandbox --version`, `--schema` and `--shep-manifest` answer without a shepherd running.

## License

MIT OR Apache-2.0, at your option.
