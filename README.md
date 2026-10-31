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

`shep sandbox init` and `shep sandbox doctor` are declared in the dog's manifest, so `shep --help` and completion list them. `init` prints "not built yet" and exits 2 for now.

## Check the host

```sh
shep sandbox doctor
```

Prints each check as `ok`, `FAIL` or `skip`, with the fix under every failure, and exits 1 if any failed so a deploy script can gate on it. A check that does not apply to this host is skipped and says why.

- crabpot is on `PATH`, with its version
- Linux: `bwrap` is on `PATH`. macOS: `/usr/bin/sandbox-exec` exists
- Ubuntu 24.04 and later: `kernel.apparmor_restrict_unprivileged_userns` is 0 and the `bwrap-userns-restrict` AppArmor profile is not loaded. The profile list is readable by root only, so the profile check is skipped for any other user
- Linux: a descriptor the shepherd opens (fd 3) reaches a sandboxed command. crabpot 0.7.0 and 0.8.0 close it ([shep-pm/crabpot#307](https://github.com/shep-pm/crabpot/issues/307)). The check runs a probe instead of comparing versions
- Each sheep in the flock with `sandbox` set: its settings file exists and `crabpot -s <file> -- true` exits 0 from its `cwd`. The shepherd has no `sandbox` key to read yet, so this check is skipped for now

`shep-sandbox --version`, `--schema` and `--shep-manifest` answer without a shepherd running.

## License

MIT OR Apache-2.0, at your option.
