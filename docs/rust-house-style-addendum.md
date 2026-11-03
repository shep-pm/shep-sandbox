# Rust house style addendum

This repo's exceptions to shep-pm/rust-house-style. Where this file and the rules disagree, this file wins.

## IR-7: `unsafe_code` is `deny`, not `forbid`

`launch` keeps the shepherd's link from the sandboxed command. Two calls need `unsafe` and have no safe form in std: marking the inherited descriptor close-on-exec (`BorrowedFd::borrow_raw`, for a descriptor this process holds only by number) and removing `SHEP_DOG_FD` from the environment (`std::env::remove_var`, `unsafe` in edition 2024).

- Both live in `src/inherit.rs`, each with a `// SAFETY:` comment and an `#[allow(unsafe_code)]` on the narrowest item (IR-8, IR-22).
- Nothing else in the crate uses `unsafe`. A new use is a review question, not a precedent.
