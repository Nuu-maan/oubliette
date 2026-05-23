# Contributing

This is an educational project, so contributions that **improve
clarity, correctness, tests, or documentation** are the most welcome.
PRs that simply add features without docs or tests will probably be
politely closed.

## What's a good contribution

Yes, please:

- **Bug fixes** with a clear repro.
- **Unit tests** for the pure-function modules (`crypto`, `chunker`,
  `inode`, path parsing). Lots of room here.
- **Property tests** with `proptest` for round-trip correctness.
- **Clarification edits** to README/ARCHITECTURE/INTERNALS — if you
  read something confusing, that's a doc bug worth fixing.
- **Linux/macOS port** behind a `cfg(unix)` gate using `fuser`.
- **Performance investigations** with measurements. "I switched from
  sync to async file reads and got 1.4× faster on N MB files."
- **Better error messages**, especially around the Discord auth path.
- **Multi-message large directories** — currently capped at ~50 root
  entries; see roadmap.

No thank you:

- Code that makes it easier to use this **at scale or commercially**.
- Anything that **removes the ToS disclaimers**.
- Public bot hosting / shared-bot patterns.
- Telemetry, analytics, "auto-updates", or anything else that phones
  home.

## Code style

- `cargo fmt` before pushing. The repo follows default rustfmt.
- `cargo clippy --all-targets` should be clean for the modules you
  touch.
- Comments are sparse: explain *why*, not *what*. The code already
  shows *what*.
- Module-level docstrings are welcome, especially in `src/fs.rs`
  where the WinFSP API is unfamiliar to most readers.

## Commits

- Commit messages: present tense, lowercase, no trailing period.
  `add streaming reads to put_file`, not `Added streaming reads.`
- Squash before merging unless your branch tells a meaningful story
  in 2-4 commits.
- Tests and docs in the same commit as the code change they cover.

## Tests

Right now testing is mostly manual. A good first contribution would
be to add real unit tests. Suggested order:

1. `src/chunker.rs::plan` — pure function, easy to property-test
   (every chunk's `offset + length` should equal the file size).
2. `src/crypto.rs::encrypt_chunk` + `decrypt_chunk` — round-trip
   property test.
3. `src/inode.rs` — serde round-trip via `serde_json`.
4. `src/store.rs::parse_path` — table-driven tests for valid /
   invalid paths.

## How to run the project from source

```powershell
# Prereqs
# - Rust 1.88+
# - WinFSP installed from https://winfsp.dev
# - A Discord bot you've created (see README)

git clone https://github.com/Nuu-maan/oubliette.git
cd oubliette
cargo build
cargo run --bin oubliette -- setup     # CLI wizard
# or
cargo run --bin oubliette-gui          # GUI wizard
```

## How to add a CLI subcommand

1. Add a variant to `Cmd` in `src/cli.rs`.
2. Add a `match` arm in `src/main.rs`.
3. Implement the actual logic as a method on `Store` (or wherever
   makes sense).
4. Update the CLI table in `README.md`.

## How to add a WinFSP callback

1. Add the `fn` to `impl FileSystemContext for OublietteFs` in
   `src/fs.rs`. Look at the trait definition in
   `winfsp::filesystem::context` for the signature.
2. Bridge sync ↔ async via `self.runtime.block_on(async { … })`.
3. Map `oubliette::Error` to an appropriate `NTSTATUS` via
   `STATUS_*.into()` — see the trait impl for examples.
4. Update the state diagram in `README.md` if the new callback
   introduces a new state transition.

## Code of conduct

Be kind, be specific, assume good faith. Don't @ anyone unrelated
to your problem. If a maintainer points out their bandwidth is
limited, believe them — this is a learning project, not a job.

## License

By contributing, you agree your contributions are licensed under
the same [Educational Research License](LICENSE) as the rest of the
project. If that's a problem, please don't contribute.
