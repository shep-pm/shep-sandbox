<p align="center">
  <img src="https://raw.githubusercontent.com/shep-pm/shep-sandbox/main/assets/shep-sandbox-logo.png" alt="A sheep sitting in a crab pot" width="220">
</p>

# shep-sandbox

[![crates.io](https://img.shields.io/crates/v/shep-sandbox.svg)](https://crates.io/crates/shep-sandbox)
[![License](https://img.shields.io/crates/l/shep-sandbox.svg)](https://github.com/shep-pm/shep-sandbox#license)

Run a [shep](https://github.com/shep-pm/shep) sheep inside a [crabpot](https://github.com/shep-pm/crabpot) sandbox. Linux and macOS.

## Adopt it

```sh
cargo install shep-sandbox --locked
shep adopt shep-sandbox --yes
```

shep names the dog `sandbox`. It needs shep 0.22 or newer, the first that spawns a sheep through it. From 0.23 the old `sandbox` key is refused, and this dog replaces it.

## Sandbox a sheep

Give the sheep an `[app.dogs.sandbox]` table. The shepherd then spawns it as `shep-sandbox launch <sheep> -- <command>`, and the dog runs the command under crabpot with that table.

```toml
[[app]]
name = "reactmap"
script = "node"
args = ["."]
cwd = "~/ReactMap"
user = "app1"
env = { NODE_USE_ENV_PROXY = "1" }

[app.dogs.sandbox.network]
allowedDomains = ["raw.githubusercontent.com", "github.com"]
# Linux. On macOS, use allowLocalBinding = true instead (see below).
publish = ["127.0.0.1:8080"]
allowTcp = ["db.example.com:3306"]

[app.dogs.sandbox.filesystem]
# The toolchain lives under home, so re-open it.
allowRead = ["~/.nvm"]
```

The table takes crabpot's settings keys, spelled as crabpot spells them. `shep-sandbox --schema` serves crabpot's JSON Schema for them as `x-shep-sheep`, and `shep` checks a table against it before writing it.

- The sheep's cwd is added to `filesystem.allowWrite`. Writes go only there, to the rest of `allowWrite` and to a temp folder.
- Reads under home are denied except the cwd. The dog adds `~` and `$SHEP_HOME` to `filesystem.denyRead` and the cwd to `filesystem.allowRead`, so a sheep cannot read other apps' configs, `.env` files or `$SHEP_HOME`'s Flockfile. Reads outside home are open.
- Re-open what the sheep needs with `filesystem.allowRead`, such as a toolchain at `~/.nvm`. `allowRead = ["~"]` opens all of home, but `$SHEP_HOME` stays hidden, because crabpot keeps a `denyRead` entry that is more specific than the `allowRead` around it.
- `denyRead` still takes your own entries, kept alongside the two the dog adds.
- The five lists crabpot requires may be left out. The dog fills each with an empty list.
- A key crabpot does not know is dropped, as crabpot drops it.

## Editing a running sheep

The dog follows the sheep's table while it runs.

```sh
shep set reactmap.dogs.sandbox.network.allowedDomains '["raw.githubusercontent.com", "cdn.jsdelivr.net"]'
```

- `network.allowedDomains`, `network.deniedDomains` and `network.deniedResolvedAddresses` apply at once, with no restart.
- Any other key needs a restart. The sheep's err log says so, naming the key: `shep-sandbox: reactmap: filesystem.allowRead changed, restart the sheep to apply it`.
- A table crabpot refuses is logged the same way, and the settings in force stay.
- If the shepherd connection drops, the sandbox keeps its last lists while the dog reconnects. Over the shepherd's launch link (see below) there is nothing to reconnect to: the err log says that the next change needs a restart, and the settings in force stay.

## Deploying with shep-deploy

shep-deploy moves a sheep's cwd to `$SHEP_HOME/deploy/<sheep>/current` and symlinks every gitignored file of the original checkout (`.env`, a database, uploads) into each release. The checkout is under home, so the read-deny hides it. Tracked files still work, but the links fail with "no such file".

Re-open the checkout:

```toml
[app.dogs.sandbox.filesystem]
allowRead = ["/srv/apps/web"]
allowWrite = ["/srv/apps/web"]
```

- `allowWrite` is needed only when the app writes a linked file, such as a database.
- The dog does not re-open link targets on its own. The sheep can write its cwd, so a link it plants there would re-open whatever it names, `$SHEP_HOME` included, at its next restart.
- shep refuses `listen` together with an `[app.dogs.sandbox]` table, so a sheep that takes its sockets from the shepherd cannot be sandboxed yet.

## Running as another user

A sheep with `user` set runs as that user, and the dog needs no access to the shepherd's control socket for it. The shepherd spawns `launch` holding a link to itself, named by `$SHEP_DOG_FD`. `launch` fetches the table and follows it over that link. The command never sees it: the descriptor is closed on exec and `SHEP_DOG_FD` is removed from the environment the command gets. `launch` holds no more authority than the one table it asks for.

## Platform notes

- Node ignores proxy variables by default, so its outbound requests fail with `ENOTFOUND`. Set `NODE_USE_ENV_PROXY = "1"` in the sheep's `env` (Node 22.21 or 24.5 and newer).
- macOS: a sheep that watches files (chokidar, nodemon, a config reloader) needs `allowMachLookup = ["com.apple.FSEvents"]` under `network`. Without it the watcher fails silently.
- macOS: `allowTcp` is refused, since Seatbelt cannot limit it to one host. `allowLocalBinding = true` lets the sheep listen and also dial any port on localhost, such as a local database.
- Linux: a sheep that listens needs its address in `network.publish`, and a sheep that dials a host directly, such as a database, needs it in `network.allowTcp`.
- Windows is not supported. `launch` exits 125 there.

## Refusals

A refusal at launch exits 125 with one line on stderr naming the cause, and shep marks the sheep errored without restarting it. A refusal while the sheep runs is only logged, as above. A command crabpot never reached never runs: there is no fallback outside the sandbox.

With a shepherd older than the launch link, `launch` dials the control socket instead. That socket trusts any process of the same user, so do not let such a sheep reach it: `network.allowAllUnixSockets`, or a `network.allowUnixSockets` entry that reaches `$SHEP_HOME/run/shep.sock`, grants the operator's authority over the whole flock.

## License

MIT OR Apache-2.0, at your option. The logo is crabpot's, with shep's sheep in place of Ferris.
