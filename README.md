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

shep names the dog `sandbox`. Adopted, it has no work of its own yet: it connects to the shepherd, reads its `[sandbox]` section of `dogs.toml` (which has no keys yet) and idles until the shepherd stops it.

`shep sandbox init` and `shep sandbox doctor` are declared in the dog's manifest, so `shep --help` and completion list them.

## Start a sandbox

```sh
shep sandbox init            # writes ./sandbox.json
shep sandbox init web.json   # writes ./web.json
```

`init` writes a settings file for [crabpot](https://github.com/shep-pm/crabpot) and refuses to overwrite one that exists. The file opens the sheep's cwd for writing and allows no network. Each setting has a `"//"` comment key above it that says what it does and gives an example: `filesystem.allowWrite`, `denyRead` and `denyWrite`, and `network.allowedDomains`, `deniedDomains`, `publish` and `allowTcp`. crabpot reads plain JSON and drops keys it does not know, so the comments cost nothing.

### Start from a running sheep

```sh
shep sandbox init --from web           # writes ./sandbox.json
shep sandbox init web.json --from web
```

`--from <sheep>` asks the shepherd for the running sheep, then reads the TCP sockets held by its process and its children: `/proc` on Linux, `lsof` on macOS. Windows has no socket reader yet, so `--from` is refused there.

- Listening sockets become `network.publish` on Linux and macOS.
- Connections the sheep dialed become `network.allowTcp` on Linux. On macOS, where crabpot refuses a non-empty `allowTcp`, they go in a `"// dialed directly"` comment and `allowTcp` stays empty. A connection accepted on one of its listening ports is not a dial, and neither is a loopback dial to one of its own listeners.
- A dial to port 443 is listed in a `"// dialed over HTTPS"` comment on every platform that has a socket reader, since `allowedDomains` covers HTTPS by name.
- The sheep's `cwd` becomes `filesystem.allowWrite`.
- The rest is the plain skeleton. The file says which sheep and pid it was taken from.

It is a snapshot. A port the sheep opens later, or a host it dials only now and then, is not in it. A sheep that is not running is refused by name.

Run a command under it with `crabpot -s sandbox.json -- <command>`.

## Check the host

```sh
shep sandbox doctor
```

Prints each check as `ok`, `FAIL` or `skip`, with the fix under every failure, and exits 1 if any failed so a deploy script can gate on it. A check that does not apply to this host is skipped and says why.

- crabpot is on `PATH`, with its version
- Linux: `bwrap` is on `PATH`. macOS: `/usr/bin/sandbox-exec` exists
- Ubuntu 24.04 and later: `kernel.apparmor_restrict_unprivileged_userns` is 0 and the `bwrap-userns-restrict` AppArmor profile is not loaded. The profile list is readable by root only, so the profile check is skipped for any other user
- Linux: a descriptor the shepherd opens (fd 3) reaches a sandboxed command. crabpot 0.7.0 and 0.8.0 close it ([shep-pm/crabpot#307](https://github.com/shep-pm/crabpot/issues/307)). The check runs a probe instead of comparing versions
- Each sheep in the flock with `sandbox` set: its settings file exists and `crabpot -s <file> -- true` exits 0 from its `cwd`. A shepherd older than protocol 14 does not run `sandbox`, so the check is skipped for it. A sheep with an unset or relative `cwd` is skipped by name, because doctor cannot tell where the shepherd reads `sandbox` from

`shep-sandbox --version`, `--schema` and `--shep-manifest` answer without a shepherd running.

## License

MIT OR Apache-2.0, at your option.
