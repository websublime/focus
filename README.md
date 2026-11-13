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

**Status: the stage graph, the audit ledger, the state of a workflow, the
guards and the harness hooks are ported (milestones M0 to M4); the
orchestrator is not.** Compiling the graph, including plugin selection and
composed scopes, and every `graph` verb are held to parity with the reference.
So are the ledger (the format, the shards, the `audit` command: append, notes,
history), the settings files, the Markdown reader, the runtime graph a
workflow's ledger compiles to, the state file and its transitions, gates and
jumps, the creation of a piece of work, and the hooks a host runs at each
event of a conversation: the observers, the session start, the person's turn
and the four checks before a tool call. What those checks read off the
engine's last step (its directive, an approval and its receipts) belongs to
the orchestrator, and is refused by name until it is written. Of the
orchestrator's milestone (M5) the first slices are in: the identity of a
project's source, the baseline a row that opens work on it carries, and the
questions the agent asks and the person's replies, recorded and read back
(`log decision`, `log answer`, `log answers`). The plan for
the rest, module by module, is in
[`docs/plano-de-migracao.md`](docs/plano-de-migracao.md) (Portuguese).

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

focus engine audit append ERROR_LOGGED --field Stage=code-generation --field "Note=what happened"
focus engine audit append-raw "A decision" "why we chose it"
focus engine audit history --stage code-generation --limit 20

focus engine runtime compile          # write the active record's runtime-graph.json
focus engine runtime read code-generation
focus engine runtime summary          # stages, diaries, sensors, learnings, duration

focus engine state get "Current Stage" # one field of the active record's state file
focus engine state count completed
FOCUS_ALLOW_DIRECT_STATE_TRANSITIONS=1 focus engine state set Scope=bugfix Completed=+1
focus engine state resume              # where the work stands, as JSON
focus engine state advance intent-capture       # close a stage, open the next in-scope one
focus engine state skip market-research --route --reason "no market to research"
focus engine state complete-workflow feedback-optimization --reason "done"
focus engine state gate-start intent-capture    # put a stage to the person for approval
focus engine state approve intent-capture --user-input "Approve"
focus engine state reject intent-capture --feedback "the title is wrong"
focus engine state park                       # stop the work where it is; unpark takes it up
focus engine jump resolve --stage feasibility # where a jump would land
focus engine jump execute --target intent-capture --direction backward
focus engine log decision --stage intent-capture --decision "Which database?" --options "Postgres,SQLite"
focus engine log answer --stage intent-capture --details "Postgres"   # the person's reply, as the agent read it
focus engine log answer --stage intent-capture --details-file .focus-engine/answer-text/answer.txt   # the same, from a file read once and removed
focus engine log answers --stage intent-capture   # the stage's questions, with the answers they have

focus engine utility detect --json     # project type, languages, frameworks, build system
focus engine utility intent-create --scope feature --arguments "Add login" --label "add login"
focus engine utility now
focus engine utility status            # the workflow as a person reads it

echo '{"session_id":"s1","reason":"exit"}' | focus engine hook session-end   # a harness hook, on the host's payload
```

`focus engine graph --help`, `focus engine audit --help`,
`focus engine runtime --help`, `focus engine state --help`, `focus engine jump
--help`, `focus engine log --help`, `focus engine utility --help` and
`focus engine hook --help` list every
verb and flag. The `audit` command is
the diagnostic way into the ledger: events that carry authority (a gate
approval, a person's turn, a unit's completion) are written only by the
engine's own tools and hooks, and are refused there. The `runtime` command
reads the ledger back as a graph of what the workflow has done; the
`rebuild-stage-graph` hook compiles it after every event that moves a
workflow, and it can be run by hand. The `state` command reads and writes the state file as data, and moves
a workflow through its stages (`advance`, `finalize`, `complete-workflow`,
`skip`; `resume` reads where it stands) and through its approval gates
(`gate-start` puts a stage to the person, `approve` and `reject` record their
decision, `revise` puts it to them again): each transition passes the
admission chain first (the workflow is not archived, the stage's declared
artifacts exist, a code-producing stage left source work, the evidence the
stage owes is there) and writes its ledger rows before the state file. A gate
decision is the person's: it needs their turn on the ledger since the gate was
shown, and an input that names the agent as its author is refused for the
agent, with the guidance upstream prints beside the sentence. `park` stops the
work where it is and records whose stop it was; the `jump` command moves it to
another stage, skipping or resetting what lies between. A write that is not
the orchestrator's passes the state-transition fence first. The
`utility` command is the workflow from outside: `intent-create` starts a piece
of work (its record, its ledger's first rows, its state file built from the
scope's plan, its registry row), `detect` is the scan that classifies the
project for it, and `status` reads the workflow as a person does: where it
stands, who is on it, its settings, its progress, and a finished stage whose
inputs moved since (the validity receipts against the tree).

The project is, in
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
| `focus-js` | JavaScript semantics the on-disk formats depend on: whitespace, sort order and collation, `Object.keys` order, `JSON.parse`/`stringify`, number printing and rounding, ECMAScript regex, `Date.parse` for the timestamps the engine writes, and Node's `path` in both flavours for the paths a command names |
| `focus-frontmatter` | The YAML-subset frontmatter readers, ported pattern for pattern |
| `focus-harness` | What differs between Claude Code, Cursor and GitHub Copilot, as data |
| `focus-workspace` | Which project, harness, space and intent record a command acts on (and which conversation, by the process tree), every path in them, the settings of the machine, the project and one clone, atomic writes, the workspace lock, opening a file whose path is not trusted, and the scan that tells a new project from existing code |
| `focus-markdown` | The block structure of a Markdown document, line by line, and the lines of it a reader of the engine's files may act on: not what sits in a comment, a code block or an HTML block |
| `focus-graph` | Stage, scope, agent, sensor and rule schemas; the compile; composed scopes; graph queries; plan validation; ARS |
| `focus-audit` | The audit ledger: the event vocabulary, how an entry is validated and written, how entries are read back, the shard each clone writes, appending to it under the lock, and a record's history from all its shards. And what a ledger reads as: the runtime graph of a workflow, its summary, the counts of a stage's diary, and the order units can be built in |
| `focus-state` | The state file of a workflow: its bulleted fields and checkbox lines, read and written as text, and the file on disk |
| `focus-evidence` | What a decision is weighed against. So far the identity of a project's source: the walk that reads a tree into a listing (every path with what it held) and a fingerprint, deciding by name what is not source and reading the commit an embedded git repository is on without running git, the text a snapshot keeps a listing as, and the baseline a row that opens work on the source carries (`Source Baseline`), with its listing kept beside the record. |
| `focus-guards` | Guard Policy, the five fences and the authority behind an action: what the guards decide before anything moves. And what the hooks read before any fence: the shell reader (what a `Bash` command may write, in POSIX shell or PowerShell, and where the shell may be when it writes), runtime integrity (whether a tool call touches a runtime record, the audit trail, an installed enforcement file or a hook, directly or through a script), the engine's own invocations as a hook reads them off a command (the transition tools that recompile the runtime graph, one literal call of the orchestrator), the dispatched reviewer's read scope, the state transitions a command calls directly or through a wrapper, a nested shell or the dispatcher (what a conductor may not call and what a delegated agent may not run), and what a tool call would change while a plan waits for its approval (the files it names as written to, whether a command could change something the reader cannot see, what the wait admits, and where a write target lies) |
| `focus-intent` | Creating a piece of work: the record's name and id, the plan it runs, the state file built from it, the registry row and the cursors |
| `focus-hooks` | The bodies of the harness hooks (`focus engine hook <name>`): which workflow a hook fires in and whether this conversation has joined it, the heartbeat and the drops each hook leaves, and the observers: the compaction breadcrumb, a session's end, a subagent's completion, the task list and the IDE mirror moving the stage, a write into the record as its artifact, the runtime graph recompiled after a transition and a creation binding the session that made it; the session start: the conversation bound to its record and stamped, the process map, the active-memory copy kept for the harness's includes and handed to the chat, the rules the chat holds, the rebind offer, the switched-off checks said once, and the context the model is handed; the person's turn (the `HUMAN_TURN` row and what a gate later reads of it); and the guards: the review freeze, the reviewer's read scope, the state-transition fence and the Plan Approval guard, each refusing with the host's own deny decision or standing aside with one line and one ledger row, and the one registration (`guard-tool-call`) that runs the four in one process, each for the tools its own row matched |
| `focus-lifecycle` | Moving a piece of work through its stages: the transitions, the gate verbs and the parking of the state command, the jumps, the admission chain each one passes, where a stage's artifacts are, the validity receipt a completion leaves, whether the person has acted since a gate opened, and what a gate asks with |
| `focus-log` | The interactions of a piece of work written to its ledger (`focus engine log`). So far the questions the agent asks: what is refused before a question is recorded (a value that reached the engine in pieces, the person's confirmation claimed by the question itself), the row, the proof of the message that already answered it; the person's reply to a question, which needs a message of theirs that has answered nothing yet, with its text in a file the engine reads once and removes when a shell would mangle it; a choice the person left to the agent, recorded with the words that left it; and the questions of a stage read back with the answers they have. |
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

One kind of answer is judged instead of compared. The reference has no
Markdown parser that reports source positions, so it works a document's
structure out by rendering it with probe words in it; Focus reads the
structure from a parser. On documents people write the two agree line for
line, and the corpus holds Focus to that. Where they differ, two other parsers
(`tools/oracle/referee/`: micromark, and commonmark.js as a second opinion)
are the judge: a line Focus reads otherwise than the reference, or that the
reference did not place at all, must be one the referee reads as Focus does. `tools/oracle/markdown_rule.py` is the rule, and
`differential.py --markdown N` applies it live to documents written with no
care for the syntax.

The reference's regular expressions are ported as written and run on an
ECMAScript engine (`regress`). That engine ignores case otherwise than
JavaScript for a few characters (it takes long `ſ` for `s`); `focus-js`
corrects it, and refuses a pattern the correction would not cover. The corpus
holds it to that with every character that has a case as a pattern, and with
the reference's own case-insensitive patterns (`tools/oracle/patterns.ts`).

`cargo test` replays all three. It needs neither Bun nor a checkout of the
reference. To regenerate them, and to run both engines live side by side:

```sh
git clone https://github.com/awslabs/aidlc-workflows ../aidlc-workflows
git -C ../aidlc-workflows checkout "$(sed -n 's/^commit: //p' fixtures/upstream/PIN)"
tools/oracle/refresh.sh ../aidlc-workflows                                  # needs bun
python3 tools/oracle/differential.py ../aidlc-workflows --fuzz 600          # live, with random cases
python3 tools/oracle/differential.py ../aidlc-workflows --markdown 2500     # live, hostile Markdown
```

Where Focus differs from the reference on purpose, the migration plan lists it
(sections 10 and 13). A difference in what a command does to the same project has a
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
