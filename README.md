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

It declares no verb. The `init` and `doctor` verbs of the first design are removed, and return once ported to the settings table the dog launches a sheep from.

## A sheep's keys

A sheep's `[app.dogs.sandbox]` table takes crabpot's settings keys, spelled as crabpot spells them (`network.allowedDomains`, `filesystem.denyRead`, `enableWeakerNetworkIsolation`). The dog serves crabpot's own JSON Schema for them as `x-shep-sheep` in its `--schema` answer, so the schema is the list of keys.

- The five lists crabpot requires (`network.allowedDomains`, `network.deniedDomains`, `filesystem.denyRead`, `filesystem.allowWrite`, `filesystem.denyWrite`) may be left out. The dog fills each with an empty list.
- `shep` asks the dog to validate a table before it writes it, and the dog hands the table to crabpot. What crabpot refuses is refused with crabpot's own message, including a key the platform cannot honour, which the schema does not describe.
- A key crabpot does not know is dropped, as crabpot drops it.
- A macOS sheep that runs file watchers needs `allowMachLookup: ["com.apple.FSEvents"]` under `network`. The dog does not add it.

`shep-sandbox --version`, `--schema` and `--shep-manifest` answer without a shepherd running.

## License

MIT OR Apache-2.0, at your option.
