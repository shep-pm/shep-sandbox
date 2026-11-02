# shep-sandbox

Run a shep sheep inside a crabpot sandbox. MIT OR Apache-2.0.

The plan is a split. shep spawns a sandboxed sheep through a per-sheep `crabpot -s <settings> -- <cmd>` launcher, because crabpot has no daemon and a sandbox lives in the process that started it. This dog handles the rest: the presets and surfacing violations. The v2 dog launches a sheep from its own settings table, so the v1 `init` and `doctor` verbs and their `sandbox.json` file are removed. They return once ported to the settings table. The design is still being charted on this repository's issues (the `wayfinder:map` issue).

So far it is an adoptable dog and `launch`. `shep-sandbox launch <sheep> -- <cmd>` is how the shepherd spawns a sandboxed sheep: it asks the shepherd for the sheep's `[app.dogs.sandbox]` table, adds the cwd to `filesystem.allowWrite`, and hands crabpot the effective settings through an unlinked file at `/dev/fd/<n>`, so the command runs under crabpot or not at all. Every refusal exits 125. Adopted (`$SHEP_DOG_NAME` set), it reads its `[sandbox]` section and idles until the shepherd stops it. Named but not supervised (`shep sandbox`, which hands it no connection), it exits 2 saying it runs only as a supervised dog.

## Commands

- `cargo test --locked` is the test shape.
- The `integration` feature gates `tests/integration.rs`, which drives a real shepherd: `SHEP_BIN=$(command -v shep) cargo test --features integration --locked`. CI builds shep from `main` for it.
- CI's gates, all required: `cargo fmt --all -- --check`, `cargo clippy --all-targets --all-features --locked -- -D warnings`, `RUSTDOCFLAGS='-D warnings' cargo doc --no-deps --all-features --locked`, and `cargo +1.88 check --all-targets --all-features --locked` for the MSRV.
- One cargo command at a time: they share the target-dir lock.
- `rust-toolchain.toml` floats on stable, like CI. When CI's clippy flags something a local run did not, `rustup update stable` first.

## Where things live

- `src/main.rs`: `crabpot::run_from_args()` first (the Linux sandbox re-enters this binary), the probes, the `launch` route and the idle loop. `src/launch.rs` is `launch`, `src/follow.rs` the thread it starts that follows the sheep's table and feeds crabpot's control pipe. `src/config.rs` is the `[sandbox]` section, `src/settings.rs` a sheep's table, which crabpot's `Config` parses and whose schema the dog serves, `src/error.rs` what stops the dog.
- `tests/probe.rs` runs the binary with shep's probe flags. `tests/integration.rs` needs a built shep.
- `docs/design-notes.md`: the discussion that started the effort. Decisions after it live on the map's tickets, not here.

## Style

- Invoke the `rust-house-style` skill before writing or reviewing Rust. The rules are shep-pm/rust-house-style, IR-1..IR-48. This repo's exceptions go in `docs/rust-house-style-addendum.md`, which wins where the two disagree.
- `#![forbid(unsafe_code)]` holds through `[lints]` in Cargo.toml.
- shep's vocabulary: a `sheep` is one managed process, the plural is `flock`, dogs are plugin processes, and the daemon is only ever "the shepherd".
- Committed text says "the maintainer", never a name, and uses repo-relative paths.

## Commits and pull requests

- Conventional subjects: `type(scope): summary`, with types `feat` `fix` `perf` `refactor` `docs` `test` `ci` `chore` `style`, and `!` on the commit that breaks something. `.githooks/commit-msg` checks locally (run `git config core.hooksPath .githooks` once per clone) and `.github/workflows/commits.yml` checks every pull request.
- Pull request titles are conventional too. `merge_commit_title` is set to the PR title, so a merge commit's subject is the title.
- Merge with a merge commit, never a squash. Release pull requests are the exception, and their workflow squashes them itself.
- One commit per item. Bodies carry the full reasoning.
- release-plz writes CHANGELOG.md from commit subjects, so never hand-write an entry. A merged release pull request publishes to crates.io on its own.

## Agent skills

### Issue tracker

GitHub Issues on `shep-pm/shep-sandbox`, via `gh`. See `docs/agents/issue-tracker.md`.

### Triage labels

The five default roles, label string equal to role name. See `docs/agents/triage-labels.md`.

### Domain docs

Single-context: `CONTEXT.md` at the root and ADRs in `docs/adr/`, both created when a term or decision is first settled. See `docs/agents/domain.md`.
