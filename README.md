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

It declares no verb. The `init` and `doctor` verbs of the first design are removed, and return once ported to the settings table the dog launches a sheep from.

`shep-sandbox --version`, `--schema` and `--shep-manifest` answer without a shepherd running.

## License

MIT OR Apache-2.0, at your option.
