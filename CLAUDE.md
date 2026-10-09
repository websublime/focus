# Focus

A Rust engine for the AI-DLC method: a fork of `awslabs/aidlc-workflows`
(TypeScript on Bun), frozen at the commit named in `fixtures/upstream/PIN`.
This repository is the source of truth for the code, the plan and the fixtures.

## Where things are

- `docs/plano-de-migracao.md` (Portuguese): architecture, the module-by-module
  migration map, milestones M0 to M8, risks, decisions, and what each finished
  milestone verified. Read the milestone's section before working on it, and
  update the plan in the same pull request that changes what it describes.
- `crates/`: one crate per responsibility (see the table in `README.md`).
- `tools/oracle/`: the parity tooling. `fixtures/`: what it produces.

## Workflow: GitHub Flow

- `main` is always green and releasable. Nothing is committed to it directly.
- One short-lived branch per change, cut from `main`
  (`feat/…`, `fix/…`, `chore/…`, `docs/…`), then a pull request.
- A pull request merges only with CI green on Linux, macOS and Windows.
- A milestone lands as several pull requests, each one verified on its own.

## Rules that are easy to break

- **Every name is `focus`**: command, `FOCUS_*` variables, `focus/`,
  `.focus-engine`, `focus-<name>.md`. Upstream's `aidlc` appears only in the
  tools that drive upstream and in references to its source files.
- **Nothing is ported by reading alone.** Each behaviour is compared with
  upstream running on the same input: the oracle corpus for functions, the
  differential for whole commands. Upstream is asked in its own names and
  translated by `tools/oracle/names.py`; Focus's output is never translated.
- **`cargo test` needs neither Bun nor an upstream checkout.** Whatever a
  comparison establishes is recorded under `fixtures/` and replayed.
- **`focus-js` owns JavaScript semantics** (whitespace, ordering, collation,
  JSON, numbers, regex). Ported code does not call `str::trim`, `sort` or the
  `regex` crate where the result reaches disk or a message.
- **A deliberate deviation is declared three times**: a test marked
  "Deliberate deviation", an entry in `DEVIATIONS` in
  `tools/oracle/differential.py`, and a row in the plan.
- **Input that is not ported is refused, never ignored.**

## Toolchain

`rust-toolchain.toml` pins the toolchain lints and tests run on, locally and in
CI, so a new stable release cannot fail `main` with a new lint. `rust-version`
in `Cargo.toml` is the oldest toolchain the workspace builds with; CI checks it
in its own job. CI repeats the lines that explain a failure as annotations on
the run, so a failure is readable from the run's summary.

## Commands

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace

# With Bun and an upstream checkout at the pinned commit:
tools/oracle/refresh.sh ../aidlc-workflows                                          # regenerate fixtures
python3 -I tools/oracle/differential.py ../aidlc-workflows target/release/focus --fuzz 600
```
