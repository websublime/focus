# Focus

A Rust engine for AI-DLC (AI-Driven Development Life Cycle), targeting Claude
Code, Cursor and GitHub Copilot.

AI-DLC turns a coding assistant into a structured delivery workflow: stages
grouped into phases, selected by scopes, with human approval gates and an audit
trail. Focus started as a port of
[awslabs/aidlc-workflows](https://github.com/awslabs/aidlc-workflows) (TypeScript
on Bun) and is now a fork of it, frozen at the commit named in
`fixtures/upstream/PIN`: that commit is the reference every behaviour here is
checked against, and upstream's later changes are not followed. Everything in
a Focus project carries Focus's name: the command, the `FOCUS_*` variables, the
`focus/` workspace directory, the `focus-<name>.md` files. Upstream says `aidlc`
in each of those places, so a project set up for upstream is not a Focus
project.

**Status: the stage graph is complete (milestones M0 and M1).** Compiling the
graph, including plugin selection and composed scopes, and every `graph` verb
are ported and held to parity with the reference. State, audit, guards, hooks
and the orchestrator are not written yet. The plan for the rest, module by
module, is in [`docs/plano-de-migracao.md`](docs/plano-de-migracao.md)
(Portuguese).

## What works

```sh
cargo build --release
cd some/project/with/a/harness/installed

focus engine graph compile            # recompile in place, under the workspace lock
focus engine graph compile --check    # fail when the compiled files are out of date
focus engine graph scope bugfix       # the stages a scope executes
focus engine graph validate-scope feature
focus engine graph validate-grid --proposal plan.json --custom
focus engine graph ars --iae 0.3 --csu 0.75 --ve 0.2 --r 0.55 --ua 0.4
focus engine graph export             # graph, scopes, artifacts and agents as JSON
```

`focus engine graph --help` lists every verb and flag. The project is, in
order: `--project-dir`; `FOCUS_PROJECT_DIR`; the directory the host names
(`CLAUDE_PROJECT_DIR`, which Claude Code sets for hooks); the nearest directory
at or above the current one with a harness installed. The harness is
`--harness`, or `FOCUS_HARNESS`, or the only one installed there.

To compile outside an installed project, from the authored Markdown:

```sh
focus engine graph compile --harness claude --core fixtures/upstream/core --out /tmp/compiled
```

## Layout

| Crate | Owns |
| --- | --- |
| `focus-js` | JavaScript semantics the on-disk formats depend on: whitespace, sort order and collation, `Object.keys` order, `JSON.parse`/`stringify`, number printing and rounding, ECMAScript regex |
| `focus-frontmatter` | The YAML-subset frontmatter readers, ported pattern for pattern |
| `focus-harness` | What differs between Claude Code, Cursor and GitHub Copilot, as data |
| `focus-workspace` | Which project and harness a command acts on, every path in them, atomic writes, the workspace lock |
| `focus-graph` | Stage, scope, agent, sensor and rule schemas; the compile; composed scopes; graph queries; plan validation; ARS |
| `focus-cli` | The `focus` binary (the package is named `focus`) |

`fixtures/upstream/` holds the reference's Markdown, its compiled output and an
installed project tree, all at the pinned commit and all under Focus's names.
`fixtures/oracle/` and `fixtures/recorded/` hold inputs with what the reference
did for each.

## How parity is checked

Nothing here is ported by reading alone. The reference engine runs locally, so
the same inputs go through both engines and the results are compared as text.

The two engines differ in one word. `tools/oracle/names.py` is the map between
them (`aidlc` to `focus`, a plain substitution): the reference is always asked
in its own vocabulary, and its trees, questions and answers are translated
before they reach Focus or `fixtures/`. Focus's own output is never translated,
so a name left unchanged shows up as a difference.

- **Oracle corpus** (`fixtures/oracle/`): generated inputs for individual
  functions, with the reference's answers.
- **Recorded differential** (`fixtures/recorded/`): whole commands run against
  edited project trees, with the reference's exit code, output and written
  files.
- **Goldens** (`fixtures/upstream/golden/`): the compiled files the reference's
  packager produced.

`cargo test` replays all three. It needs neither Bun nor a checkout of the
reference. To regenerate them, and to run both engines live side by side:

```sh
git clone https://github.com/awslabs/aidlc-workflows ../aidlc-workflows
git -C ../aidlc-workflows checkout "$(sed -n 's/^commit: //p' fixtures/upstream/PIN)"
tools/oracle/refresh.sh ../aidlc-workflows                                  # needs bun
python3 tools/oracle/differential.py ../aidlc-workflows --fuzz 600          # live, with random cases
```

Where Focus differs from the reference on purpose, the migration plan lists it
(section 10). A difference in what a command does to the same project has a
test marked "Deliberate deviation" that says why, and the differential lists it
by name with its reason. Differences of surface (Focus's own lock, its
environment variables, refusing unknown flags) are covered by the CLI tests
without that mark.

## Platforms and releases

Focus builds for Linux, macOS and Windows. CI (`.github/workflows/ci.yml`) runs
the lints and the tests on all three; the recorded differential replays on
Linux and macOS only.

Releases are packaged by [cargo-dist](https://opensource.axo.dev/cargo-dist/)
(`dist-workspace.toml`): archives for five targets plus shell and PowerShell
installers, built by `.github/workflows/release.yml` when a version tag is
pushed to [websublime/focus](https://github.com/websublime/focus). On a pull
request the same workflow only checks that a release could be planned.

## Contributing

The repository follows GitHub Flow: `main` is always green, every change is a
short-lived branch and a pull request, and a pull request merges only with CI
passing on Linux, macOS and Windows. `CLAUDE.md` lists the rules a change has
to keep.

## License

MIT, copyright Websublime; see `LICENSE`. The reference is MIT-0, which places
no conditions on derived work. `fixtures/upstream/` is derived from it under
that license (`fixtures/upstream/LICENSE`).
