#!/usr/bin/env python3
"""Run upstream's engine and focus on the same project trees and compare.

    differential.py <upstream checkout with dist/ built> [focus binary]
                    [--fuzz N] [--seed S] [--only PATTERN] [--record DIR] [--explain] [--show]
                    [--markdown N]

Every check builds a Claude Code project from upstream's own distribution,
edits it in one specific way, gives each engine its own copy, runs the same
command in both, and compares what a caller can observe: exit code, standard
output, standard error, and (for a compile) the files left on disk.

The two engines do not share a vocabulary: upstream's tree says `aidlc` where
Focus's says `focus` (names.py). Upstream gets the tree as its packager built
it; Focus gets the same tree with the map applied, and the same arguments,
input and environment with the map applied. What upstream then says and
writes, with the map applied, is what Focus must say and write. Focus's own
output is never mapped, so a name it failed to change shows as a difference.
Checks are written once, in upstream's names.

A check is `same`, a listed `deviation` (focus differs on purpose; the reason
is printed, and with --explain so is where the two differ), or `DIFFERENT`.
Exits non-zero when anything is DIFFERENT.

With --record, what upstream did in each check is written to DIR (the tree it
started from, the command, the outcome), all under Focus's names, so
`cargo test` can replay the same checks against focus with no upstream and no
Bun present.

With --markdown N, N Markdown documents written with no care for what the
syntax allows are read by upstream's analysis, by Focus's and by a third
parser, and every line is judged (see the Markdown section below). It needs
the referee's packages: `bun install` in tools/oracle/referee.

What is normalised before comparing, besides the map, and nothing else:
  - how each engine is invoked (`aidlc-graph <verb>:` / `focus graph <verb>:`,
    the command it tells the reader to run, "AI-DLC" / "Focus" as the subject
    of a refusal);
  - the location of the temporary tree;
  - error text produced by the runtime, not by the engine (the operating
    system's message for a missing file, the JSON parser's message);
  - for the audit command, which writes the time and may mint an identity:
    a timestamp written during the run (`<now>`), and the host and token of
    a clone identity minted during the run;
  - for the audit command, the shape of a refusal upstream lets reach its
    runtime (see SHAPES).
"""
import hashlib
import json
import os
import random
import re
import shutil
import subprocess
import sys
import tempfile
import time

# `python3 -I` leaves the script's directory off the path.
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import markdown_docs  # noqa: E402
import markdown_rule  # noqa: E402
from names import rename_tree, to_focus, to_upstream  # noqa: E402

argv = sys.argv[1:]


def option(name, default=None):
    if name in argv:
        at = argv.index(name)
        value = argv[at + 1]
        del argv[at:at + 2]
        return value
    return default


RECORD = option("--record")
# --show prints what upstream wrote to stdout in full for every check that is the same, to read what a check
# covers rather than trust its name.
SHOW = "--show" in argv and not argv.remove("--show")
FUZZ = int(option("--fuzz", "0"))
MARKDOWN = int(option("--markdown", "0"))
SEED = int(option("--seed", "1"))
ONLY = option("--only")
EXPLAIN = "--explain" in argv and not argv.remove("--explain")
UPSTREAM = os.path.abspath(argv[0])
FOCUS = os.path.abspath(argv[1] if len(argv) > 1 else "target/release/focus")
BUN = shutil.which("bun") or os.path.expanduser("~/.bun/bin/bun")
WORK = tempfile.mkdtemp(prefix="focus-diff-")
DIST = os.path.join(UPSTREAM, "dist", "claude")

# Checks where focus differs from upstream on purpose.
DEVIATIONS = {
    "compile/duplicate_slug":
        "which file upstream calls 'already declared' follows filesystem order; focus walks phases in lifecycle order",
    "compile/composed_named_constructor":
        "upstream reports a scope named after an Object.prototype member as a duplicate; focus treats it as a name",
    "compile/plugins_manifest_not_json":
        "upstream reads an unparseable harness.json as 'no selection' and compiles; focus refuses, because a "
        "selection it cannot read would silently enable every plugin",
    r"compile/composed_grid_entry_(null|array)_stages":
        "upstream back-fills a record whose `stages` is not an object, which then fails every later compile; "
        "focus writes none",
    r"compile/composed_(record_then_compile_error|data_dir_is_link)":
        "upstream leaves the identity file it restored in place when the compile is then refused, and with no "
        "grid column behind it the scope skips every stage; focus takes the file back",
    "compile/grid_file_is_link":
        "upstream checks only the data directory and renames over a linked compiled file; focus refuses before "
        "writing either file",
    "compile/check_missing_graph":
        "upstream surfaces the raw ENOENT; focus reports the file as out of date",
    r"verbs/memory_span_after_escape/grid_.*":
        "a code span that opens after an escaped backtick and closes two lines down hides the `Mode:` line "
        "between; upstream's analysis does not record the span and reads the line, focus reads what CommonMark "
        "renders (markdown/commonmark)",
    r"verbs/memory_past_budget/grid_.*":
        "upstream stops analysing a document after 256 renders and reads the rest as text, so a `Mode:` line "
        "inside a comment declares a policy when enough of the file comes before it; focus reads the end of a "
        "file as it reads its start (markdown/commonmark)",
    r"verbs/bad_scope/grid_matched_.*":
        "with a scope file that does not load, upstream falls back to 'strict' inside a try/catch and prints a "
        "verdict about Guard Policy; focus reports the scope file",
}

# Differences of shape that are settled before comparing, and counted.
SHAPES = {
    "group-thrown":
        "a member of a hook group that fails on its own (it throws: refused settings, say) refuses nothing, and "
        "the group carries what it said. Upstream's member says the runtime's stack trace, whose frames differ "
        "from member to member, so the same sentence is carried once per failing member; focus's member says the "
        "sentence as every command does (`focus: sentence`), the same line from each, and the group carries it "
        "once. Each of upstream's traces is read as that one line and repeats are dropped before comparing; the "
        "exit, the sentence and the files are compared",
    "gate-words-offset":
        "the gate-words record indexes the audit shard by byte offset, and upstream's shard header is one byte "
        "longer than focus's (\"# AI-DLC Audit Log\" is the one phrase of the name map whose length changes), so "
        "every offset upstream writes is one more. Upstream's offsets are read one byte back before comparing; "
        "the messages, the session and the shard are compared as they are",
    "uncaught":
        "upstream lets a refusal thrown inside an audit verb reach the runtime, which prints a source excerpt "
        "and a stack trace around the sentence; focus prints the same sentence as {\"error\": ...}, like every "
        "other refusal of the command. The sentence is compared",
    "guard-ask":
        "a guard that refuses a transition prints its sentence, then (upstream) the guard-recovery ask as a JSON "
        "line: the remedies the conductor may offer, read off the review attempt and kept with the refusal's "
        "streak under the engine directory (M5). Focus prints the sentence. The ask line and the streak record "
        "are left out before comparing; the sentence, the error row and everything else are compared",
    "process-map-budget":
        "the session start writes the process map (its ancestors' pids, with each one's start time) within a 50 ms "
        "budget and retires a slot to null before it reads the process; under load one engine misses the budget "
        "and leaves a null or an unwritten slot where the other wrote the entry. The map is compared live only; "
        "an entry one engine did not reach in time is left out before comparing, and everything else is compared",
    "validation-basis":
        "a completion row carries the validity receipt: per artifact, a hash of the files it resolved to and a "
        "hash of their contents, both taken over paths that carry the workspace directory's name (aidlc/ there, "
        "focus/ here), so the two engines' hashes differ by construction. Each engine's hashes are recomputed "
        "here from its own tree and names, and a hash that matches is compared as <verified>; one that does not "
        "is compared as it is",
    "summary-authorization":
        "a summary's authorization id is a hash over the attempt, the stage, the workflow, the questions file's "
        "name, what was confirmed and the choice, and the name carries the workspace directory's (aidlc/ there, "
        "focus/ here), so the two engines' ids differ by construction. Each engine's id is recomputed here from "
        "the authorization its own record keeps, whose every other field is compared as it is, and an id that "
        "matches is compared as <verified> wherever it is written; one that does not is compared as it is",
}

results = {"same": 0, "deviation": 0, "DIFFERENT": 0}
shapes = {name: 0 for name in SHAPES}
# A timestamp at or after this instant was written during the run.
STARTED = time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime(time.time() - 2))
STARTED_MS = int((time.time() - 2) * 1000)
# The day a record minted during the run is named after.
TODAY = time.strftime("%y%m%d", time.gmtime())
UUID_V7 = r"[0-9a-f]{8}-[0-9a-f]{4}-7[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}"


class Tree:
    """One engine's copy of a project.

    Paths and text are given and returned in upstream's names for both
    engines, so one edit serves both. On Focus's copy (`mapped`) they are
    translated on the way to and from disk.
    """

    def __init__(self, root, mapped=False):
        self.root = root
        self.mapped = mapped

    def path(self, rel):
        return os.path.join(self.root, to_focus(rel) if self.mapped else rel)

    def raw(self, rel):
        """The file as it is on disk, in this engine's own names."""
        with open(self.path(rel), encoding="utf-8") as handle:
            return handle.read()

    def read(self, rel):
        return to_upstream(self.raw(rel)) if self.mapped else self.raw(rel)

    def write(self, rel, text):
        os.makedirs(os.path.dirname(self.path(rel)), exist_ok=True)
        with open(self.path(rel), "w", encoding="utf-8") as handle:
            handle.write(to_focus(text) if self.mapped else text)

    def edit(self, rel, change):
        self.write(rel, change(self.read(rel)))

    def sub(self, rel, pattern, replacement, count=1):
        self.edit(rel, lambda text: re.sub(pattern, replacement, text, count=count, flags=re.M))

    def remove(self, rel):
        os.remove(self.path(rel))

    def json(self, rel):
        return json.loads(self.read(rel))

    def write_json(self, rel, value):
        self.write(rel, json.dumps(value, indent=2) + "\n")

    def stage(self, phase, slug, *extra):
        lines = ["---", f"slug: {slug}", f"phase: {phase}", "execution: ALWAYS", "condition: c",
                 "lead_agent: orchestrator", "support_agents: []", "mode: inline", *extra]
        if not any(line.startswith("produces:") for line in extra):
            lines.append("produces: []")
        if not any(line.startswith("requires_stage:") for line in extra):
            lines.append("requires_stage: []")
        if not any(line.startswith("consumes:") for line in extra):
            lines.append("consumes: []")
        lines += ["inputs: i", "outputs: o", "---", "", f"# {slug}", ""]
        self.write(f".claude/aidlc-common/stages/{phase}/{slug}.md", "\n".join(lines))

    def scope_file(self, file, name, *extra):
        self.write(f".claude/scopes/{file}",
                   "\n".join(["---", f"name: {name}", "depth: Minimal", "description: d", *extra, "---", "",
                              f"# {name}", ""]))

    def plugins(self, value):
        self.edit(".claude/tools/data/harness.json",
                  lambda text: json.dumps({**json.loads(text), "plugins": value}, indent=2) + "\n")


BEGIN = ("<!-- BEGIN aidlc composed-scope-grid: generated by `aidlc engine graph compile` — reshape the plan "
         "through /aidlc compose, not by editing here -->")
END = "<!-- END aidlc composed-scope-grid -->"


def identity(name, extra=""):
    return f"---\nname: {name}\ndepth: Minimal\ndescription: composed\nkeywords:\n  - {name}\n{extra}---\n\n# {name}\n\nA tailored plan.\n"


def record(name, stages, extra=""):
    grid = json.dumps({"stages": stages}, indent=2)
    return f"{identity(name, extra).rstrip(chr(10))}\n\n{BEGIN}\n\n```json\n{grid}\n```\n\n{END}\n"


def some_stages(tree, execute):
    graph = tree.json(".claude/tools/data/stage-graph.json")
    return {s["slug"]: ("EXECUTE" if s["phase"] == "initialization" or s["slug"] in execute else "SKIP")
            for s in graph}


def focus_base():
    """Upstream's distribution under Focus's names, built once. Its TypeScript is left out: Focus runs none."""
    base = os.path.join(WORK, "_focus_base")
    if not os.path.isdir(base):
        shutil.copytree(DIST, base, symlinks=True, ignore=shutil.ignore_patterns("*.ts"))
        rename_tree(base)
    return base


class Pair(list):
    """The two trees of a check: laid out when first used, removed when the checks queued on them are done. Every
    check is queued before any runs, and each tree is a copy of the distribution: laid out at once, a few thousand
    of them do not fit on a disk."""

    def __init__(self, name, edits):
        super().__init__()
        self.name, self.edits, self.made = name, edits, False

    def make(self):
        if not self.made:
            self.made = True
            for engine in ("ts", "rs"):
                root = os.path.join(WORK, self.name.replace("/", "_"), engine)
                shutil.copytree(DIST if engine == "ts" else focus_base(), root, symlinks=True)
                tree = Tree(root, mapped=engine == "rs")
                for edit in self.edits:
                    edit(tree)
                self.append(tree)
        return self

    def __iter__(self):
        self.make()
        return super().__iter__()

    def __getitem__(self, at):
        self.make()
        return super().__getitem__(at)

    def __len__(self):
        return 2

    def dispose(self):
        if self.made:
            shutil.rmtree(os.path.join(WORK, self.name.replace("/", "_")), ignore_errors=True)


def pair(name, *edits):
    """The same tree twice, one per engine in its own names, with `edits` applied to both."""
    return Pair(name, edits)


def run(engine, tree, args, env=None, stdin=None):
    env = env or {}
    full = dict(os.environ)
    for key in list(full):
        if key.startswith(("AIDLC_", "FOCUS_")):
            del full[key]
    if engine == "ts":
        full.update({"AIDLC_SRC": tree.root, "AIDLC_HARNESS_DIR": ".claude", "AIDLC_HARNESS_NAME": "claude",
                     "AIDLC_RULES_DIR": tree.path("aidlc/spaces/default/memory")})
        full.update({f"AIDLC_{k}": v for k, v in env.items()})
        command = [BUN, ".claude/tools/aidlc-graph.ts", *args]
    else:
        full.update({f"FOCUS_{k}": to_focus(v) for k, v in env.items()})
        command = [FOCUS, "engine", "graph", *map(to_focus, args), "--project-dir", tree.root, "--harness", "claude"]
        stdin = stdin if stdin is None else to_focus(stdin)
    done = subprocess.run(command, cwd=tree.root, env=full, input=stdin, capture_output=True, text=True,
                          timeout=120)
    return done.returncode, done.stdout, done.stderr


RUNTIME_TEXT = [
    # (prefix the engine wrote, what follows the runtime's own text)
    (r"(Stage graph not readable at [^:]+: ).*?(\. Reinstall)", r"\1<runtime>\2"),
    (r"(is not valid JSON: ).*", r"\1<runtime>"),
    (r"(: invalid JSON: ).*", r"\1<runtime>"),
    (r"(has an unparseable grid fence: ).*", r"\1<runtime>"),
    (r"(cannot read ars priors at [^:]+: ).*", r"\1<runtime>"),
    (r"(validate-grid: cannot read [^:]+: ).*?(\. Write the grid|$)", r"\1<runtime>\2"),
    # A plan written where there is no directory: each runtime's own words for "no such file".
    (r"(graph resolve: )(?:ENOENT: no such file or directory, .*|.*: No such file or directory \(os error 2\))",
     r"\1<runtime>"),
]


def without_tree(text, tree):
    """`text` with the tree's location taken out, as given and with links resolved (an engine prints a path it
    resolved, such as the install root, resolved)."""
    for root in dict.fromkeys((os.path.realpath(tree.root), tree.root)):
        text = text.replace(root + "/", "").replace(root, "<tree>")
    return text


def normalise(text, tree, engine):
    text = without_tree(text, tree)
    text = re.sub(r"^(aidlc-graph|focus graph) ", "graph ", text, flags=re.M)
    text = text.replace("bun aidlc-graph.ts", "<cli> graph").replace("focus engine graph", "<cli> graph")
    # The composed-scope sentinel names the command; quoted in a message, it is the same command.
    text = text.replace("aidlc engine graph", "<cli> graph")
    text = text.replace("'bun .claude/tools/aidlc.ts config'", "'<cli> config'").replace("'focus config'", "'<cli> config'")
    text = text.replace("aidlc-graph resolve is gated", "graph resolve is gated")
    text = text.replace("focus graph resolve is gated", "graph resolve is gated")
    # Each engine's own prefix only: a variable Focus named in upstream's way is a difference.
    text = re.sub(r"\b%s_([A-Z_]+)\b" % ("AIDLC" if engine == "ts" else "FOCUS"), r"<ENV>_\1", text)
    text = text.replace("so AI-DLC changed nothing", "so <engine> changed nothing")
    text = text.replace("so Focus changed nothing", "so <engine> changed nothing")
    text = text.replace("treated as an AI-DLC persona", "treated as <engine> persona")
    text = text.replace("treated as a Focus persona", "treated as <engine> persona")
    for pattern, replacement in RUNTIME_TEXT:
        text = re.sub(pattern, replacement, text, flags=re.M)
    return text


def snapshot(tree):
    """The files a compile may write, under Focus's names: upstream's are mapped, Focus's are taken as they are."""
    say = (lambda text: text) if tree.mapped else to_focus
    files = {}
    for rel in (".claude/tools/data/stage-graph.json", ".claude/tools/data/scope-grid.json"):
        files[rel] = say(tree.raw(rel)) if os.path.exists(tree.path(rel)) else None
    for directory in (".claude/scopes", "aidlc/scopes"):
        real = tree.path(directory)
        if os.path.isdir(real):
            for file in sorted(os.listdir(real)):
                with open(os.path.join(real, file), encoding="utf-8") as handle:
                    files[f"{to_focus(directory)}/{say(file)}"] = say(handle.read())
    # The plan `resolve` writes into the record it finds, wherever that is.
    for folder, _, names in os.walk(tree.path("aidlc/spaces")):
        if "plan.json" in names and os.path.basename(folder) == os.path.basename(tree.path(".aidlc-engine")):
            with open(os.path.join(folder, "plan.json"), encoding="utf-8") as handle:
                files[to_focus(os.path.relpath(os.path.join(folder, "plan.json"), tree.root))] = say(handle.read())
    return files


def first_difference(a, b):
    a_lines, b_lines = a.split("\n"), b.split("\n")
    for index in range(max(len(a_lines), len(b_lines))):
        left = a_lines[index] if index < len(a_lines) else "<end>"
        right = b_lines[index] if index < len(b_lines) else "<end>"
        if left != right:
            # Long lines (a JSON document on one line) are shown from where they first differ.
            at = next((i for i, (x, y) in enumerate(zip(left, right)) if x != y), min(len(left), len(right)))
            start = max(0, at - 80)
            left, right = left[start:start + 300], right[start:start + 300]
            return f"line {index + 1}\n             upstream: {left}\n             focus:    {right}"
    return "identical"


JOBS = []

# The part of an installed project the graph verbs read or write. Recording
# stores each tree as its difference from this subset of upstream's
# distribution, which refresh.sh vendors as fixtures/upstream/installed/claude.
BASE_DIRS = (".claude/aidlc-common/stages", ".claude/agents", ".claude/scopes", ".claude/sensors",
             ".claude/tools/data/memory-seed", "aidlc")
# The include stub (`.claude/rules/aidlc.md`) names the memory files the session start hashes into the rules record.
# The two files the distribution lays at the project's root are source like any other: a baseline lists them.
BASE_FILES = (*(f".claude/tools/data/{name}" for name in ("harness.json", "stage-graph.json", "scope-grid.json", "ars-priors.json")),
              ".claude/rules/aidlc.md", ".gitignore", ".mcp.json")
# The tools whose verbs read the project beyond the engine's own files.
WHOLE_PROJECT = ("utility", "jump", "state")
RECORDED_TREES = {}
RECORDED_CHECKS = {}
LARGE = 3000


def relevant_files(root, hidden=False, everything=False):
    """The files of a tree a recording keeps. The graph verbs read none of the engine's own dot-files, so those
    are left out unless `hidden`: the runtime command reads the clone's identity. The utility's scan reads the
    whole project, so its checks keep `everything`."""
    found = {}
    # A settings path can hold a directory where the file goes; walking a file finds nothing.
    for directory in ("",) if everything else (*BASE_DIRS, "p", "machine", "aidlc.settings.json", "aidlc.settings.local.json"):
        for folder, _, names in os.walk(os.path.join(root, directory)):
            for name in names:
                path = os.path.join(folder, name)
                rel = os.path.relpath(path, root)
                if os.path.islink(path) or (not hidden and "/.aidlc-" in f"/{rel}" and "composer-proposal" not in rel):
                    continue
                found[rel] = path
    for rel in (*BASE_FILES, "aidlc.settings.json", "aidlc.settings.local.json"):
        if os.path.isfile(os.path.join(root, rel)):
            found[rel] = os.path.join(root, rel)
    return found


def has_links(root):
    for folder, dirs, names in os.walk(root):
        if any(os.path.islink(os.path.join(folder, name)) for name in (*dirs, *names)):
            return True
    return False


_base_cache = {}


def overlay(tree, hidden=False, everything=False):
    """`tree` as its difference from the base: {relative path: text, or None when removed}."""
    import hashlib
    base = _base_cache.setdefault((hidden, everything), {})
    if not base:
        for rel, path in relevant_files(DIST, hidden, everything).items():
            with open(path, "rb") as handle:
                base[rel] = handle.read()
    now = {}
    for rel, path in relevant_files(tree.root, hidden, everything).items():
        with open(path, "rb") as handle:
            now[rel] = handle.read()
    diff = {}
    for rel, data in now.items():
        if base.get(rel) != data:
            diff[rel] = data.decode("utf-8")
    for rel in base:
        if rel not in now:
            diff[rel] = None
    # Stored under Focus's names, like the base the replay starts from, and
    # with a record minted today named by its day placeholder, so the replay
    # lays the same tree out on any other day.
    diff = {dated(to_focus(rel)): text if text is None else to_focus(text) for rel, text in diff.items()}
    key = hashlib.sha256(json.dumps(diff, sort_keys=True).encode()).hexdigest()[:16]
    RECORDED_TREES[key] = diff
    return key


def hand_set_times(tree, key, everything=False):
    """{relative path: modification time in nanoseconds} of the files of the overlay `key` that were not written
    during this run: a check set their time, and what it compares depends on it."""
    times = {}
    for rel, path in relevant_files(tree.root, True, everything).items():
        name = dated(to_focus(rel))
        if RECORDED_TREES[key].get(name) is not None and abs(time.time() - os.stat(path).st_mtime) > 3600:
            times[name] = str(os.stat(path).st_mtime_ns)
    return times


def compact(text):
    """Long output is recorded as its digest; the live comparison is where a diff is read."""
    import hashlib
    if len(text) <= LARGE:
        return text
    return {"sha256": hashlib.sha256(text.encode("utf-8")).hexdigest(), "bytes": len(text.encode("utf-8"))}


def check(name, trees, args, env=None, stdin=None, files=False):
    """Queue one comparison. Checks on the same pair of trees run in the order queued."""
    if ONLY and not re.search(ONLY, name):
        return
    JOBS.append((name, trees, args, env, stdin, files))


def run_queued(quiet=False):
    """Run every queued check, trees in parallel, and report in queue order."""
    from concurrent.futures import ThreadPoolExecutor
    groups = {}
    for index, job in enumerate(JOBS):
        groups.setdefault(id(job[1]), []).append((index, job))
    lines = {}

    def work(group):
        for index, job in group:
            lines[index] = compare(*job)
        trees = group[0][1][1]
        if isinstance(trees, Pair):
            trees.dispose()

    # Built once, before the trees that copy it are laid out side by side.
    focus_base()
    with ThreadPoolExecutor(max_workers=os.cpu_count() or 2) as pool:
        list(pool.map(work, groups.values()))
    for index in sorted(lines):
        verdict, name, detail = lines[index]
        results[verdict] += 1
        if verdict != "same" or not quiet:
            print(f"{verdict:<9} {name:<46} {detail}")
    JOBS.clear()


def compare(name, trees, args, env, stdin, files):
    if files in ("audit", "runtime", "state", "utility", "jump", "log", "hook"):
        return compare_audit(name, trees, args, env, files)
    ts, rs = trees
    outcome = {}
    started_from = overlay(ts) if RECORD and not has_links(ts.root) and name not in UNRECORDED else None
    for engine, tree in (("ts", ts), ("rs", rs)):
        code, out, err = run(engine, tree, args, env, stdin)
        say = to_focus if engine == "ts" else (lambda text: text)
        outcome[engine] = {
            "exit": str(code),
            "stdout": say(normalise(out, tree, engine)),
            "stderr": say(normalise(err, tree, engine)),
        }
        if files:
            # Files are compared as written: nothing in them is the engine
            # talking about itself.
            for rel, text in snapshot(tree).items():
                outcome[engine][rel] = "<absent>" if text is None else text
    problems = []
    shaped_budget = False
    for key in sorted(set(outcome["ts"]) | set(outcome["rs"])):
        left, right = outcome["ts"].get(key, "<absent>"), outcome["rs"].get(key, "<absent>")
        if left != right:
            unreached = ("<absent>", '{"sessionId":null,"startTime":null}\n')
            if "/.focus-sessions/pids/" in key and (left in unreached or right in unreached):
                shaped_budget = True
                continue
            problems.append(f"{key}: {first_difference(left, right)}")
    if not problems:
        verdict = "same"
        detail = f"exit {outcome['ts']['exit']}"
        said = (outcome["ts"]["stderr"] or outcome["ts"]["stdout"]).strip().split("\n")[0][:110]
        if said:
            detail += f": {said}"
    elif any(re.fullmatch(pattern, name) for pattern in DEVIATIONS):
        verdict = "deviation"
        detail = next(reason for pattern, reason in DEVIATIONS.items() if re.fullmatch(pattern, name))
        if EXPLAIN:
            detail += "\n           " + "\n           ".join(problems)
    else:
        verdict, detail = "DIFFERENT", "\n           ".join(problems)
    if started_from is not None and not name.startswith("fuzz/"):
        deviation = detail if verdict == "deviation" else None
        RECORDED_CHECKS[name] = {
            "name": name, "tree": started_from, "args": [to_focus(arg) for arg in args],
            "env": {key: to_focus(value) for key, value in (env or {}).items()},
            "stdin": stdin if stdin is None else to_focus(stdin),
            "writes": files and "--check" not in args,
            "deviation": deviation,
            "expect": {key: compact(value) for key, value in outcome["ts"].items()},
        }
    return verdict, name, detail


# --------------------------------------------------------------------------
# Compile, in place.
# --------------------------------------------------------------------------


def compile_check(name, *edits, args=("compile",)):
    check(f"compile/{name}", pair(f"compile/{name}", *edits), list(args), files=True)


def plugin_stages(tree):
    tree.stage("inception", "acme-lint", "plugin: acme", "requires_stage:", "  - requirements-analysis",
               "scopes:", "  - feature", "  - acme-quick", "produces:", "  - lint-report")
    tree.stage("construction", "acme-ship", "plugin: acme", "requires_stage:", "  - acme-lint", "scopes:",
               "  - acme-quick", "consumes:", "  - artifact: lint-report", "    required: true")
    tree.scope_file("acme-quick.md", "acme-quick", "plugin: acme", "keywords:", "  - quick")


def duplicate_slug(tree):
    tree.stage("inception", "twin")
    tree.write(".claude/aidlc-common/stages/operation/twin.md",
               tree.read(".claude/aidlc-common/stages/inception/twin.md").replace("phase: inception",
                                                                                  "phase: operation"))


def unknown_phase_dir(tree):
    tree.stage("retirement", "farewell")
    tree.sub(".claude/aidlc-common/stages/retirement/farewell.md", r"^phase: retirement", "phase: operation")


def grid_only(tree, name="legacy", extra=""):
    tree.write(f".claude/scopes/aidlc-{name}.md", identity(name, extra))
    grid = tree.json(".claude/tools/data/scope-grid.json")
    grid[name] = {"stages": some_stages(tree, {"code-generation", "build-and-test"})}
    tree.write_json(".claude/tools/data/scope-grid.json", grid)


def with_record(name, stages=("code-generation",), extra=""):
    return lambda tree: tree.write(f"aidlc/scopes/{name}.md", record(name, some_stages(tree, set(stages)), extra))


COMPILE = {
    # Authoring and numbering (the M0 set, now compiled in place).
    "untouched": [],
    "new_stage": [lambda t: t.stage("inception", "zz-extra", "requires_stage:", "  - delivery-planning", "scopes:",
                                    "  - feature")],
    "new_stages_against_file_order": [
        lambda t: t.stage("operation", "aa-last", "requires_stage:", "  - zz-first"),
        lambda t: t.stage("operation", "zz-first"),
        lambda t: t.stage("operation", "mm-hinted", "number: 4.2"),
        lambda t: t.stage("operation", "bb-hinted", "number: 4.9")],
    "new_scope_column": [lambda t: t.stage("ideation", "extra-scope", "scopes:", "  - 2024", "  - brand-new",
                                           "  - 7")],
    "duplicate_edge": [lambda t: t.stage("inception", "dup-edge", "requires_stage:", "  - delivery-planning",
                                         "  - delivery-planning")],
    "duplicate_slug": [duplicate_slug],
    "filename_mismatch": [lambda t: t.stage("inception", "right-name"),
                          lambda t: os.rename(t.path(".claude/aidlc-common/stages/inception/right-name.md"),
                                              t.path(".claude/aidlc-common/stages/inception/wrong-name.md"))],
    "unknown_sensor": [lambda t: t.stage("inception", "needs-sensor", "sensors:", "  - no-such-sensor")],
    "unknown_agent": [lambda t: t.stage("inception", "needs-agent", "reviewer: aidlc-nobody-agent",
                                        "review_artifact: thing", "produces:", "  - thing")],
    "unknown_dependency": [lambda t: t.stage("inception", "dangling", "requires_stage:", "  - no-such-stage")],
    "forward_edge": [lambda t: t.sub(".claude/aidlc-common/stages/ideation/intent-capture.md",
                                     r"^requires_stage:.*$", "requires_stage:\n  - feedback-optimization")],
    "new_stage_cycle": [lambda t: t.stage("operation", "loop-a", "requires_stage:", "  - loop-b"),
                        lambda t: t.stage("operation", "loop-b", "requires_stage:", "  - loop-a"),
                        lambda t: t.stage("operation", "loop-c", "requires_stage:", "  - loop-a")],
    "duplicate_producer": [lambda t: t.stage("inception", "second-producer", "produces:", "  - requirements")],
    "reserved_key": [lambda t: t.stage("inception", "reserved", "timeout: 30", "bundle: x", "mystery: y")],
    "plugin_named_like_core": [lambda t: t.stage("inception", "aidlc-x-stage", "plugin: aidlc-x")],
    "plugin_slug_prefix": [lambda t: t.stage("inception", "stray", "plugin: acme")],
    "unknown_phase_dir": [unknown_phase_dir],
    "bad_scope_file": [lambda t: t.sub(".claude/scopes/aidlc-bugfix.md", r"^skeleton: .*", "skeleton: maybe")],
    "bad_scope_guard_policy": [lambda t: t.sub(".claude/scopes/aidlc-bugfix.md", r"^guard_policy: .*",
                                               "guard_policy: lenient")],
    "two_freeform_defaults": [
        lambda t: t.sub(".claude/scopes/aidlc-bugfix.md", r"^name: bugfix$", "name: bugfix\nfreeform_default: true"),
        lambda t: t.sub(".claude/scopes/aidlc-feature.md", r"^name: feature$",
                        "name: feature\nfreeform_default: true")],
    "bad_rule_status": [lambda t: t.edit("aidlc/spaces/default/memory/team.md",
                                         lambda text: "---\nstatus: retired\n---\n" + text)],
    "bad_rule_date": [lambda t: t.write("aidlc/spaces/default/memory/project.md",
                                        "---\nstale_after: 2026-02-30\n---\nbody\n")],
    "sensor_id_mismatch": [lambda t: t.sub(".claude/sensors/aidlc-linter.md", r"^id: linter$", "id: lint")],
    "sensor_bad_severity": [lambda t: t.sub(".claude/sensors/aidlc-linter.md", r"^default_severity: .*",
                                            "default_severity: loud")],
    "agent_without_name": [lambda t: t.sub(".claude/agents/aidlc-product-agent.md", r"^name: .*\n", "")],
    "per_unit_not_subagent": [lambda t: t.sub(".claude/aidlc-common/stages/construction/code-generation.md",
                                              r"^mode: subagent$", "mode: inline")],
    "orphan_grid_column": [lambda t: t.edit(".claude/tools/data/scope-grid.json",
                                            lambda text: text.replace("{", '{\n  "ghost": { "stages": {} },', 1))],
    "missing_grid": [lambda t: t.remove(".claude/tools/data/scope-grid.json")],
    "missing_graph": [lambda t: t.remove(".claude/tools/data/stage-graph.json")],

    # Plugin selection.
    "plugins_core_only": [lambda t: t.plugins(["aidlc"])],
    "plugins_enabled": [plugin_stages, lambda t: t.plugins(["aidlc", "acme"])],
    "plugins_unselected": [plugin_stages],
    "plugins_disabled": [plugin_stages, lambda t: t.plugins(["aidlc"])],
    "plugins_only_plugin": [plugin_stages, lambda t: t.plugins(["acme"]),
                            lambda t: t.sub(".claude/aidlc-common/stages/inception/acme-lint.md",
                                            r"requires_stage:\n  - requirements-analysis", "requires_stage: []")],
    "plugins_dropped_edge": [plugin_stages, lambda t: t.plugins(["acme"])],
    "plugins_closure_fails": [
        plugin_stages, lambda t: t.plugins(["aidlc"]),
        lambda t: t.stage("operation", "needs-lint", "consumes:", "  - artifact: lint-report",
                          "    required: true")],
    "plugins_closure_optional": [
        plugin_stages, lambda t: t.plugins(["aidlc"]),
        lambda t: t.stage("operation", "wants-lint", "consumes:", "  - artifact: lint-report",
                          "    required: false")],
    "plugins_closure_two_listings": [
        plugin_stages, lambda t: t.plugins(["aidlc"]),
        lambda t: t.stage("inception", "acme-also", "plugin: acme", "produces:", "  - other", "optional_produces:",
                          "  - lint-report-two"),
        lambda t: t.stage("inception", "beta-make", "plugin: beta", "produces:", "  - lint-report-two"),
        lambda t: t.stage("operation", "needs-two", "consumes:", "  - artifact: lint-report-two",
                          "    required: true")],
    "plugins_empty_selection": [lambda t: t.plugins([])],
    "plugins_not_a_list": [lambda t: t.plugins("acme")],
    "plugins_blank_entry": [lambda t: t.plugins(["aidlc", "  "])],
    "plugins_number_entry": [lambda t: t.plugins([7])],
    "plugins_trimmed_and_repeated": [plugin_stages, lambda t: t.plugins([" aidlc ", "acme", "acme"])],
    "plugins_legacy_policy_keys": [lambda t: t.edit(
        ".claude/tools/data/harness.json",
        lambda text: json.dumps({**json.loads(text), "flags": {}, "models": {}}))],
    "plugins_manifest_not_json": [lambda t: t.write(".claude/tools/data/harness.json", "{ not json")],
    "plugins_scope_prefix_reserved": [lambda t: t.scope_file("aidlc-x-y.md", "xy", "plugin: aidlc-x")],
    "plugins_disabled_scope_default": [
        plugin_stages, lambda t: t.plugins(["aidlc"]),
        lambda t: t.sub(".claude/scopes/acme-quick.md", r"^plugin: acme$", "plugin: acme\nfreeform_default: true"),
        lambda t: t.sub(".claude/scopes/aidlc-feature.md", r"^name: feature$",
                        "name: feature\nfreeform_default: true")],
    "plugins_two_enabled_defaults": [
        plugin_stages, lambda t: t.plugins(["aidlc", "acme"]),
        lambda t: t.sub(".claude/scopes/acme-quick.md", r"^plugin: acme$", "plugin: acme\nfreeform_default: true"),
        lambda t: t.sub(".claude/scopes/aidlc-feature.md", r"^name: feature$",
                        "name: feature\nfreeform_default: true")],
    "plugins_scope_without_stages": [lambda t: t.plugins(["aidlc"]),
                                     lambda t: t.scope_file("aidlc-empty.md", "empty")],
    "scope_without_stages_unselected": [lambda t: t.scope_file("aidlc-empty.md", "empty")],

    # Composed scopes.
    "composed_record_restores_identity": [with_record("spike")],
    "composed_record_and_identity": [with_record("spike"),
                                     lambda t: t.write(".claude/scopes/aidlc-spike.md",
                                                       identity("spike").replace("composed", "hand edited"))],
    "composed_grid_only_backfills": [grid_only],
    "composed_grid_only_renamed_file": [grid_only, lambda t: os.rename(t.path(".claude/scopes/aidlc-legacy.md"),
                                                                       t.path(".claude/scopes/zz-renamed.md"))],
    "composed_record_beats_grid": [grid_only, with_record("legacy", ("build-and-test",))],
    "composed_named_like_stock": [with_record("feature")],
    "composed_integer_name": [with_record("2024")],
    "composed_two_records": [with_record("spike"), with_record("alpha", ("build-and-test", "code-generation"))],
    "composed_under_selection": [with_record("spike"), lambda t: t.plugins(["aidlc"])],
    "composed_grid_only_under_selection": [grid_only, lambda t: t.plugins(["aidlc"])],
    "composed_grid_entry_extra_keys": [grid_only, lambda t: t.edit(
        ".claude/tools/data/scope-grid.json",
        lambda text: json.dumps({**json.loads(text), "legacy": {"stages": {"x": "EXECUTE"}, "note": 1}}))],
    "composed_grid_entry_null_stages": [grid_only, lambda t: t.edit(
        ".claude/tools/data/scope-grid.json",
        lambda text: json.dumps({**json.loads(text), "legacy": {"stages": None}}))],
    "composed_grid_entry_array_stages": [grid_only, lambda t: t.edit(
        ".claude/tools/data/scope-grid.json",
        lambda text: json.dumps({**json.loads(text), "legacy": {"stages": ["state-init"]}}))],
    "composed_grid_entry_no_stages": [grid_only, lambda t: t.edit(
        ".claude/tools/data/scope-grid.json",
        lambda text: json.dumps({**json.loads(text), "legacy": {"cells": {}}}))],
    "composed_grid_not_json": [grid_only, lambda t: t.write(".claude/tools/data/scope-grid.json", "{ nope"),
                               with_record("spike")],
    "composed_duplicate_name": [with_record("spike"),
                                lambda t: t.write("aidlc/scopes/zz-copy.md", t.read("aidlc/scopes/spike.md"))],
    "composed_no_region": [lambda t: t.write("aidlc/scopes/spike.md", identity("spike"))],
    "composed_two_regions": [with_record("spike"),
                             lambda t: t.edit("aidlc/scopes/spike.md", lambda text: text + f"\n{BEGIN}\n{END}\n")],
    "composed_end_before_begin": [lambda t: t.write("aidlc/scopes/spike.md",
                                                    f"{identity('spike')}\n{END}\n\n{BEGIN}\n")],
    "composed_no_fence": [lambda t: t.write("aidlc/scopes/spike.md", f"{identity('spike')}\n{BEGIN}\n\n{END}\n")],
    "composed_bad_fence_json": [lambda t: t.write(
        "aidlc/scopes/spike.md", f"{identity('spike')}\n{BEGIN}\n\n```json\n{{ nope\n```\n\n{END}\n")],
    "composed_stages_not_object": [lambda t: t.write(
        "aidlc/scopes/spike.md", f"{identity('spike')}\n{BEGIN}\n\n```json\n{{\"stages\": []}}\n```\n\n{END}\n")],
    "composed_bad_action": [lambda t: t.write(
        "aidlc/scopes/spike.md",
        f"{identity('spike')}\n{BEGIN}\n\n```json\n{{\"stages\": {{\"a\": \"MAYBE\", \"b\": 1}}}}\n```\n\n{END}\n")],
    "composed_no_name": [with_record("spike"),
                         lambda t: t.sub("aidlc/scopes/spike.md", r"^name: spike\n", "")],
    "composed_bad_name": [with_record("spike"),
                          lambda t: t.sub("aidlc/scopes/spike.md", r"^name: spike$", "name: not a name")],
    "composed_no_frontmatter": [lambda t: t.write("aidlc/scopes/spike.md", "just prose\n")],
    "composed_identity_has_sentinel": [grid_only, lambda t: t.edit(".claude/scopes/aidlc-legacy.md",
                                                                  lambda text: text + f"\n{END}\n")],
    "composed_fence_in_prose": [lambda t: t.write(
        "aidlc/scopes/spike.md",
        record("spike", some_stages(t, {"code-generation"})).replace(
            "A tailored plan.", "A tailored plan.\n\n```json\n{\"stages\": {\"x\": \"EXECUTE\"}}\n```"))],
    "composed_trailing_blank_lines": [lambda t: t.write(
        "aidlc/scopes/spike.md",
        record("spike", some_stages(t, {"code-generation"})).replace("A tailored plan.\n", "A tailored plan.\n\n\n\n"))],
    "composed_stale_stage_names": [lambda t: t.write("aidlc/scopes/spike.md",
                                                     record("spike", {"gone-stage": "EXECUTE", "10": "SKIP"}))],
    "composed_named_constructor": [with_record("constructor")],
    "composed_record_then_compile_error": [
        with_record("spike"), lambda t: t.stage("inception", "dangling", "requires_stage:", "  - no-such-stage")],
    "grid_file_is_link": [
        lambda t: t.sub(".claude/aidlc-common/stages/ideation/intent-capture.md", r"^condition: .*",
                        "condition: changed"),
        lambda t: (shutil.move(t.path(".claude/tools/data/scope-grid.json"), t.path("moved-grid.json")),
                   os.symlink(t.path("moved-grid.json"), t.path(".claude/tools/data/scope-grid.json")))],
    "composed_data_dir_is_link": [with_record("spike"), lambda t: (
        shutil.move(t.path(".claude/tools/data"), t.path("moved-data")),
        os.symlink(t.path("moved-data"), t.path(".claude/tools/data")))],
    "composed_scopes_dir_is_link": [with_record("spike"), lambda t: (
        shutil.move(t.path(".claude/scopes"), t.path("moved-scopes")),
        os.symlink(t.path("moved-scopes"), t.path(".claude/scopes")))],
    # The directory a scope is restored into is made when it is not there.
    "composed_scopes_dir_missing": [with_record("spike"), lambda t: shutil.rmtree(t.path(".claude/scopes"))],
}

COMPILE_CHECKS = {
    "check_fresh": [],
    "check_stale_stage": [lambda t: t.sub(".claude/aidlc-common/stages/ideation/intent-capture.md", r"^condition: .*",
                                          "condition: changed")],
    "check_stale_grid": [lambda t: t.sub(".claude/aidlc-common/stages/ideation/intent-capture.md",
                                         r"^scopes:\n", "scopes:\n  - extra-scope\n")],
    "check_grid_reordered": [lambda t: t.edit(
        ".claude/tools/data/scope-grid.json",
        lambda text: json.dumps(dict(reversed(list(json.loads(text).items())))))],
    "check_missing_grid": [lambda t: t.remove(".claude/tools/data/scope-grid.json")],
    "check_missing_graph": [lambda t: t.remove(".claude/tools/data/stage-graph.json")],
    "check_composed_column": [grid_only],
}

# --------------------------------------------------------------------------
# The read-only verbs.
# --------------------------------------------------------------------------


def compiled(*edits):
    """Edits, then an in-place compile by each engine on its own copy."""
    def build(name):
        trees = pair(name, *edits)
        for engine, tree in zip(("ts", "rs"), trees):
            code, _, err = run(engine, tree, ["compile"])
            if code != 0:
                raise SystemExit(f"setup compile failed for {name} ({engine}): {err}")
        return trees
    return build


def raw(*edits):
    return lambda name: pair(name, *edits)


PER_UNIT_STAGES = ("nfr-requirements", "nfr-design", "functional-design", "infrastructure-design", "code-generation")


def no_marker(tree, slugs):
    """The compiled graph without the per-unit marker on `slugs`."""
    graph = tree.json(".claude/tools/data/stage-graph.json")
    for stage in graph:
        if stage["slug"] in slugs:
            stage.pop("for_each", None)
    tree.write_json(".claude/tools/data/stage-graph.json", graph)


def add_cycle(tree):
    graph = tree.json(".claude/tools/data/stage-graph.json")
    by_slug = {s["slug"]: s for s in graph}
    by_slug["requirements-analysis"]["requires_stage"].append("code-generation")
    by_slug["build-and-test"]["requires_stage"].append("build-and-test")
    tree.write_json(".claude/tools/data/stage-graph.json", graph)


def stage_prior(tree):
    plugin_stages(tree)
    tree.sub(".claude/aidlc-common/stages/inception/acme-lint.md", r"^inputs: i$",
             "ars:\n  targets: [ve, r]\n  cost: 2\n  project_types: [greenfield]\ninputs: i")
    tree.sub(".claude/aidlc-common/stages/construction/acme-ship.md", r"^inputs: i$",
             "ars:\n  targets: []\n  cost: 3\ninputs: i")
    tree.stage("operation", "acme-watch", "plugin: acme", "ars:", "  targets: [ua]", "  cost: null")
    tree.stage("operation", "acme-gate", "plugin: acme", "ars:", "  targets: []", "  cost: 1", "  role: phase-gate")


def memory(layer, text):
    return lambda tree: tree.edit(f"aidlc/spaces/default/memory/{layer}.md",
                                  lambda body: body.replace("## Guard Policy\n", f"## Guard Policy\n\n{text}\n", 1))


SETTINGS = {"sensors": "on", "learnings": "off", "summary_confirmation": "off", "plan_approval": "on",
            "collaborators": "off", "review_cap": "advisory"}


def proposals(tree):
    """Proposal files shared by the validate-grid checks, written into the tree."""
    grid = tree.json(".claude/tools/data/scope-grid.json")
    bugfix = grid["bugfix"]["stages"]
    feature = grid["feature"]["stages"]
    tree.write_json("p/bugfix.json", {"stages": bugfix})
    tree.write_json("p/feature_bare.json", feature)
    flipped = dict(bugfix)
    flipped["code-review"] = "SKIP" if flipped.get("code-review") == "EXECUTE" else "EXECUTE"
    tree.write_json("p/near_bugfix.json", {"stages": flipped})
    broken = dict(bugfix, nope="EXECUTE")
    broken["requirements-analysis"] = "MAYBE"
    broken["code-generation"] = 1
    broken["build-and-test"] = None
    del broken["state-init"]
    tree.write_json("p/broken.json", {"stages": broken})
    starved = {slug: "SKIP" for slug in bugfix}
    starved.update({"workspace-scaffold": "EXECUTE", "workspace-detection": "EXECUTE", "state-init": "EXECUTE",
                    "code-generation": "EXECUTE", "build-and-test": "EXECUTE"})
    tree.write_json("p/starved.json", {"stages": starved})
    no_init = dict(feature)
    no_init["state-init"] = "SKIP"
    tree.write_json("p/no_init.json", {"stages": no_init, "guardPolicy": "off", "scopeSettings": SETTINGS,
                                       "depth": "standard"})
    tree.write_json("p/full_bugfix.json", {"stages": bugfix, "guardPolicy": "off", "scopeSettings": SETTINGS,
                                           "depth": "minimal"})
    tree.write_json("p/full_bugfix_strict.json", {"stages": bugfix, "guardPolicy": "strict",
                                                  "scopeSettings": dict(SETTINGS, review_cap="none"),
                                                  "depth": "Comprehensive"})
    tree.write_json("p/full_near.json", {"stages": flipped, "changeControl": "relaxed",
                                         "scopeSettings": dict(SETTINGS, plan_approval="off", sensors="off"),
                                         "depth": " Standard "})
    tree.write_json("p/full_feature.json", {"stages": feature, "guardPolicy": "relaxed",
                                            "scopeSettings": dict(SETTINGS, learnings="on", collaborators="on",
                                                                  review_cap="adversarial"),
                                            "depth": "comprehensive"})
    tree.write_json("p/bad_settings.json", {"stages": bugfix, "guardPolicy": "sometimes",
                                            "scopeSettings": {"sensors": "yes", "learnings": 1, "extra": "x",
                                                              "review_cap": "advisory"}})
    tree.write_json("p/settings_not_object.json", {"stages": bugfix, "scopeSettings": ["on"]})
    tree.write_json("p/no_depth.json", {"stages": flipped, "guardPolicy": "off", "scopeSettings": SETTINGS})
    tree.write_json("p/stages_string.json", {"stages": "x", "depth": "minimal"})
    tree.write("p/array.json", "[1, 2]\n")
    tree.write("p/null.json", "null\n")
    tree.write("p/stages_array.json", '{"stages": ["EXECUTE"]}\n')
    tree.write("p/not_json.json", "{ nope\n")
    tree.write("aidlc/spaces/default/intents/.aidlc-engine/composer-proposal.json",
               json.dumps({"stages": feature}))


RESOLVE = {"GRAPH_RESOLVE": "1"}

VERB_TREES = {
    "stock": raw(proposals),
    "plugins_disabled": compiled(plugin_stages, lambda t: t.plugins(["aidlc"]), proposals),
    "plugins_enabled": compiled(stage_prior, lambda t: t.plugins(["aidlc", "acme"]), proposals),
    "plugins_only": compiled(stage_prior, lambda t: t.plugins(["acme"]),
                             lambda t: t.sub(".claude/aidlc-common/stages/inception/acme-lint.md",
                                             r"requires_stage:\n  - requirements-analysis", "requires_stage: []"),
                             proposals),
    "composed": compiled(with_record("spike", ("code-generation", "build-and-test")), grid_only,
                         with_record("Zeta+v2", ("requirements-analysis",)), proposals),
    "no_proposal": raw(),
    "cyclic": raw(add_cycle),
    "no_grid": raw(proposals, lambda t: t.remove(".claude/tools/data/scope-grid.json")),
    "broken_graph": raw(lambda t: t.write(".claude/tools/data/stage-graph.json", "{ nope")),
    "missing_graph": raw(lambda t: t.remove(".claude/tools/data/stage-graph.json")),
    "bad_scope": raw(lambda t: t.sub(".claude/scopes/aidlc-bugfix.md", r"^skeleton: .*", "skeleton: maybe"), proposals),
    "extra_scope_file": raw(lambda t: t.scope_file("aidlc-empty.md", "empty", "keywords:", "  - Bug", "  - nothing"),
                            proposals),
    "memory_strict": raw(memory("team", "- **Mode**: strict"), proposals),
    "memory_relaxed_then_strict": raw(memory("org", "Mode: relaxed"), memory("project", "mode :  STRICT"), proposals),
    "memory_invalid": raw(memory("org", "Mode: lenient"), proposals),
    "memory_retired_heading": raw(lambda t: t.edit("aidlc/spaces/default/memory/team.md",
                                                   lambda body: body + "\n## Change Control\n\nMode: strict\n"),
                                  proposals),
    "memory_wrapped": raw(memory("team", "- Mode:\n  strict\n- Other: x"), proposals),
    # A `Mode:` line a reader of the rendered file does not see declares nothing.
    "memory_in_fence": raw(memory("team", "```\nMode: strict\n```"), proposals),
    "memory_in_comment": raw(memory("team", "see <!-- the old rule:\nMode: strict\n--> above"), proposals),
    "memory_in_html_block": raw(memory("team", "<div>\nMode: strict\n</div>"), proposals),
    "memory_in_indented_code": raw(memory("team", "Example:\n\n    Mode: strict"), proposals),
    "memory_in_code_span": raw(memory("team", "a `span that\nMode: strict\nends` here"), proposals),
    "memory_in_quoted_fence": raw(memory("team", "> ```\n> Mode: strict\n> ```"), proposals),
    "memory_heading_in_fence": raw(lambda t: t.edit("aidlc/spaces/default/memory/team.md", lambda body: body.replace(
        "## Guard Policy\n", "```md\n## Guard Policy\nMode: strict\n```\n\n## Guard Policy\n", 1)), proposals),
    # And one that only looks hidden is read.
    "memory_after_comment": raw(memory("team", "<!-- one\ntwo --> Mode: strict"), proposals),
    "memory_in_details": raw(memory("team", "<details>\n\nMode: strict\n\n</details>"), proposals),
    "memory_escaped_backticks": raw(memory("team", "a \\` b\nMode: strict\n\\` c"), proposals),
    "memory_span_after_escape": raw(memory("team", "a \\` b `\nMode: strict\n` c"), proposals),
    "memory_lowercase_declaration": raw(memory("team", "<!x\nMode: strict"), proposals),
    # Sixty small tables use up upstream's render budget; what follows them it reads as text.
    "memory_past_budget": raw(lambda t: t.edit("aidlc/spaces/default/memory/team.md", lambda body: body.replace(
        "## Guard Policy\n", "".join(f"| a{n} | b |\n| --- | --- |\n| `x{n}` | <!-- c --> |\n\n" for n in range(60))
        + "## Guard Policy\n\n<!--\nMode: strict\n-->\n", 1)), proposals),
    "other_space": raw(lambda t: t.write("aidlc/spaces/payments/memory/team.md", "## Guard Policy\n\nMode: strict\n"),
                       proposals),
    "active_space": raw(lambda t: t.write("aidlc/spaces/payments/memory/team.md",
                                          "## Guard Policy\n\nMode: strict\n"),
                        lambda t: t.write("aidlc/active-space", "payments\n"), proposals),
    "bad_priors": raw(lambda t: t.edit(".claude/tools/data/ars-priors.json",
                                       lambda text: text.replace('"iae": 0.2', '"iae": 0.25'))),
    "stale_priors": raw(lambda t: t.edit(".claude/tools/data/ars-priors.json",
                                         lambda text: text.replace('"intent-capture":', '"gone-stage":'))),
    "priors_cost_string": raw(lambda t: t.edit(".claude/tools/data/ars-priors.json",
                                               lambda text: text.replace('"targets": ["iae"], "cost": 2',
                                                                         '"targets": ["iae"], "cost": "2"'))),
    "priors_no_threshold": raw(lambda t: t.edit(".claude/tools/data/ars-priors.json",
                                                lambda text: text.replace('"5": 0.5', '"6": 0.5'))),
    "priors_missing": raw(lambda t: t.remove(".claude/tools/data/ars-priors.json")),
    "stage_cost_no_threshold": compiled(stage_prior, lambda t: t.edit(
        ".claude/tools/data/ars-priors.json", lambda text: text.replace('"2": 0.2,', ''))),
}

ARS = ["--iae", "0.3", "--csu", "0.75", "--ve", "0.2", "--r", "0.55", "--ua", "0.4"]

BASIC_VERBS = [
    ("topo", ["topo"]), ("artifacts", ["artifacts"]), ("cycles", ["cycles"]),
    ("cycles_scope", ["cycles", "--scope", "feature"]), ("cycles_scope_unknown", ["cycles", "--scope", "nope"]),
    ("cycles_scope_trailing", ["cycles", "--scope"]),
    ("producers", ["producers", "requirements"]), ("producers_none", ["producers", "no-such-artifact"]),
    ("producers_missing_arg", ["producers"]), ("consumers", ["consumers", "requirements"]),
    ("scope", ["scope", "bugfix"]), ("scope_feature", ["scope", "feature"]), ("scope_unknown", ["scope", "nope"]),
    ("scope_missing_arg", ["scope"]), ("validate_scope_unknown", ["validate-scope", "nope"]),
    ("export", ["export"]), ("resolve_ungated", ["resolve", "bugfix", "--stdout"]),
    ("ars", ["ars", *ARS]),
]
SCOPES = ["bugfix", "classic", "enterprise", "express", "feature", "infra", "mvp", "poc", "refactor",
          "security-patch", "workshop"]


def verbs():
    built = {}

    def trees(name):
        if name not in built:
            built[name] = VERB_TREES[name](f"verbs/{name}")
        return built[name]

    def on(tree, name, args, env=None, stdin=None, files=False):
        label = f"verbs/{tree}/{name}"
        if ONLY and not re.search(ONLY, label):
            return
        check(label, trees(tree), args, env, stdin, files)

    def alone(name, args, env, *edits):
        """A check that writes, on a pair of trees of its own."""
        label = f"verbs/{name}"
        if not ONLY or re.search(ONLY, label):
            check(label, pair(label, *edits), args, env, files=True)

    # `resolve` writes the plan into the engine directory of the record it finds: the active intent's, the
    # lone record's, the one a session is bound to, or the space's own while there is no intent.
    state = lambda record, space="default": lambda t: t.write(
        f"aidlc/spaces/{space}/intents/{record}/aidlc-state.md", "# State\n")
    cursor = lambda record, space="default": lambda t: t.write(
        f"aidlc/spaces/{space}/intents/active-intent", record + "\n")
    one, two = "add-login-1a2b3c4d", "fix-logout-9f8e7d6c"
    alone("resolve_writes/no_intent", ["resolve", "bugfix"], RESOLVE)
    alone("resolve_writes/lone_record", ["resolve", "bugfix"], RESOLVE, state(one))
    alone("resolve_writes/active_intent", ["resolve", "feature"], RESOLVE, state(one), state(two), cursor(two))
    alone("resolve_writes/two_records_no_cursor", ["resolve", "bugfix"], RESOLVE, state(one), state(two))
    alone("resolve_writes/other_space", ["resolve", "bugfix"], RESOLVE, state(one, "payments"),
          lambda t: t.write("aidlc/active-space", "payments\n"))
    alone("resolve_writes/twice", ["resolve", "bugfix"], RESOLVE, state(one),
          lambda t: t.write(f"aidlc/spaces/default/intents/{one}/.aidlc-engine/plan.json", "[]\n"))
    alone("resolve_writes/unknown_scope", ["resolve", "nope"], RESOLVE, state(one))
    # A file the environment names is written where it is said to go, and its directory is not made.
    alone("resolve_writes/plan_path", ["resolve", "bugfix"], {**RESOLVE, "PLAN_PATH": "plan.json"}, state(one))
    alone("resolve_writes/plan_path_missing_dir", ["resolve", "bugfix"], {**RESOLVE, "PLAN_PATH": "nowhere/plan.json"})
    alone("resolve_writes/plan_path_empty", ["resolve", "bugfix"], {**RESOLVE, "PLAN_PATH": ""}, state(one))
    alone("resolve_writes/stdout_writes_nothing", ["resolve", "bugfix", "--stdout"], RESOLVE, state(one))
    alone("resolve_writes/registry_unreadable_row", ["resolve", "bugfix", "--stdout"], RESOLVE, state(one), state(two),
          lambda t: t.write("aidlc/spaces/default/intents/intents.json", '[{"slug": "add-login"}]\n'))
    # The process map: an entry naming no ancestor is a miss; one naming this process (the engine's parent) is
    # the conversation, whose binding then names the record. The entry carries this run's pid: compared live.
    alone("resolve_writes/sessions_by_process_unmapped", ["resolve", "bugfix"], RESOLVE, state(one),
          lambda t: t.write("aidlc/.aidlc-sessions/pids/1.json", "{}\n"))
    mapped = lambda t: [t.write(rel, text) for rel, text in my_pid_entry().items()]
    bound = lambda t: t.write("aidlc/.aidlc-sessions/s1.binding.json",
                              '{"space":"default","intent":"%s","boundAt":"2026-01-05T10:00:00Z","source":"create"}' % one)
    alone("resolve_writes/sessions_by_process", ["resolve", "bugfix"], RESOLVE, state(one), state(two), cursor(two), mapped, bound)
    alone("resolve_writes/sessions_by_process_stdout", ["resolve", "bugfix", "--stdout"], RESOLVE, state(one), state(two), cursor(two), mapped, bound)
    alone("resolve_writes/sessions_by_process_unbound", ["resolve", "bugfix"], RESOLVE, state(one), state(two), cursor(two), mapped)
    alone("resolve_writes/sessions_by_process_stale_override", ["resolve", "bugfix"], {**RESOLVE, "SESSION_OVERRIDE": "s2"}, state(one), mapped, bound)
    alone("resolve_writes/sessions_by_process_payload_override", ["resolve", "bugfix"], {**RESOLVE, "SESSION_OVERRIDE": "s2", "SESSION_OVERRIDE_SOURCE": "payload"}, state(one), mapped, bound)
    alone("resolve_writes/sessions_by_process_same_override", ["resolve", "bugfix"], {**RESOLVE, "SESSION_OVERRIDE": "s1"}, state(one), state(two), cursor(two), mapped, bound)
    UNRECORDED.update(f"verbs/resolve_writes/sessions_by_process{suffix}" for suffix in ("", "_stdout", "_unbound", "_stale_override", "_payload_override", "_same_override"))
    # An observer changes nothing: each durable write refuses.
    for marker in ("STOP_HOOK_PROBE", "ROUTE_CHECK"):
        probe = {marker: "1"}
        alone(f"observer/{marker.lower()}/resolve", ["resolve", "bugfix"], {**RESOLVE, **probe}, state(one))
        alone(f"observer/{marker.lower()}/resolve_stdout", ["resolve", "bugfix", "--stdout"], {**RESOLVE, **probe},
              state(one))
        alone(f"observer/{marker.lower()}/resolve_plan_path", ["resolve", "bugfix"],
              {**RESOLVE, **probe, "PLAN_PATH": "p/plan.json"})
        alone(f"observer/{marker.lower()}/compile", ["compile"], probe)
        alone(f"observer/{marker.lower()}/compile_check", ["compile", "--check"], probe)
        alone(f"observer/{marker.lower()}/compile_restores_scope", ["compile"], probe, with_record("spike"))
        alone(f"observer/{marker.lower()}/compile_backfills_record", ["compile"], probe, grid_only)
        alone(f"observer/{marker.lower()}/topo", ["topo"], probe)
    alone("observer/not_one/compile", ["compile"], {"STOP_HOOK_PROBE": "true"})
    alone("observer/not_one/resolve", ["resolve", "bugfix"], {**RESOLVE, "ROUTE_CHECK": "0"}, state(one))

    for tree in ("stock", "plugins_disabled", "plugins_enabled", "plugins_only", "composed", "cyclic", "no_grid",
                 "broken_graph", "missing_graph", "bad_scope", "extra_scope_file"):
        for name, args in BASIC_VERBS:
            on(tree, name, args)
        on(tree, "resolve_stdout", ["resolve", "bugfix", "--stdout"], RESOLVE)
        on(tree, "resolve_unknown", ["resolve", "nope", "--stdout"], RESOLVE)
    for scope in SCOPES:
        on("stock", f"validate_scope_{scope}", ["validate-scope", scope])
        on("stock", f"resolve_{scope}", ["resolve", scope, "--stdout"], RESOLVE)
        on("plugins_disabled", f"validate_scope_{scope}", ["validate-scope", scope])
    for tree, scope in (("plugins_enabled", "acme-quick"), ("plugins_only", "acme-quick"), ("composed", "spike"),
                        ("composed", "legacy"), ("composed", "Zeta+v2"), ("extra_scope_file", "empty")):
        on(tree, f"scope_{scope}", ["scope", scope])
        on(tree, f"validate_scope_{scope}", ["validate-scope", scope])
        on(tree, f"resolve_{scope}", ["resolve", scope, "--stdout"], RESOLVE)
        on(tree, f"cycles_scope_{scope}", ["cycles", "--scope", scope])

    # ARS.
    ars_cases = {
        "greenfield_completed": [*ARS, "--project-type", "greenfield", "--completed",
                                 "intent-capture, code-generation,,"],
        "brownfield_caps": [*ARS, "--project-type", "Brownfield"],
        "all_zero": ["--iae", "0", "--csu", "0", "--ve", "0", "--r", "0", "--ua", "0"],
        "all_one": ["--iae", "1", "--csu", "1.0", "--ve", "1.00", "--r", "1e0", "--ua", "0x1"],
        "half_point": ["--iae", "0.05", "--csu", "0.83", "--ve", "0.73", "--r", "0.15", "--ua", "1"],
        "band_edges": ["--iae", "0.3", "--csu", "0.7", "--ve", "0.29", "--r", "0.69", "--ua", "0.2"],
        "three_decimals": ["--iae", "0.299", *ARS[2:]],
        "above_one": ["--iae", "1.5", *ARS[2:]],
        "negative": ["--iae", "-0.1", *ARS[2:]],
        "not_a_number": ["--iae", "abc", *ARS[2:]],
        "blank": ["--iae", " ", *ARS[2:]],
        "missing_flag": ARS[:8],
        "flag_without_value": ["--iae", "--csu", "0.1", "--ve", "0.1", "--r", "0.1", "--ua", "0.1"],
        "completed_unknown": [*ARS, "--completed", "nope"],
        "completed_no_value": [*ARS, "--completed"],
        "project_type_no_value": [*ARS, "--project-type"],
        "project_type_wrong": [*ARS, "--project-type", "bluefield"],
        "project_type_flag_as_value": [*ARS, "--project-type", "--completed"],
    }
    for tree in ("stock", "plugins_disabled", "plugins_enabled", "plugins_only", "composed"):
        for name, args in ars_cases.items():
            on(tree, f"ars_{name}", ["ars", *args])
    for tree in ("bad_priors", "stale_priors", "priors_cost_string", "priors_no_threshold", "priors_missing",
                 "stage_cost_no_threshold", "no_grid"):
        on(tree, "ars", ["ars", *ARS])

    # validate-grid.
    grid_cases = {
        "bugfix": ["--proposal", "p/bugfix.json"],
        "bare_strict_greenfield": ["--proposal", "p/feature_bare.json", "--strict", "--project-type", "greenfield"],
        "bare_brownfield": ["--proposal", "p/feature_bare.json", "--project-type", "BROWNFIELD"],
        "near": ["--proposal", "p/near_bugfix.json"],
        "broken": ["--proposal", "p/broken.json"],
        "starved": ["--proposal", "p/starved.json"],
        "starved_strict": ["--proposal", "p/starved.json", "--strict"],
        "matched_ok": ["--proposal", "p/full_bugfix.json", "--matched", "bugfix"],
        "matched_strict_proposal": ["--proposal", "p/full_bugfix_strict.json", "--matched", "bugfix"],
        "matched_differs": ["--proposal", "p/full_near.json", "--matched", "bugfix"],
        "matched_not_stock": ["--proposal", "p/full_bugfix.json", "--matched", "nope"],
        "matched_other": ["--proposal", "p/full_bugfix.json", "--matched", "enterprise"],
        "matched_bare": ["--proposal", "p/bugfix.json", "--matched", "bugfix"],
        "matched_report": ["--proposal", "p/full_bugfix.json", "--matched", "bugfix", "--report"],
        "matched_report_wrong": ["--proposal", "p/full_feature.json", "--matched", "feature", "--report"],
        "matched_guard_lower": ["--proposal", "p/full_bugfix.json", "--matched", "enterprise", "--guard-policy",
                                "relaxed"],
        "custom_ok": ["--proposal", "p/full_bugfix.json", "--custom"],
        "custom_near": ["--proposal", "p/full_near.json", "--custom"],
        "custom_near_greenfield": ["--proposal", "p/full_near.json", "--custom", "--project-type", "greenfield"],
        "custom_feature": ["--proposal", "p/full_feature.json", "--custom"],
        "custom_feature_report": ["--proposal", "p/full_feature.json", "--custom", "--report"],
        "custom_strict_policy": ["--proposal", "p/full_bugfix_strict.json", "--custom"],
        "custom_no_depth": ["--proposal", "p/no_depth.json", "--custom"],
        "custom_no_init": ["--proposal", "p/no_init.json", "--custom"],
        "custom_bare": ["--proposal", "p/bugfix.json", "--custom"],
        "custom_flag_policy": ["--proposal", "p/bugfix.json", "--custom", "--guard-policy", "off"],
        "bad_settings": ["--proposal", "p/bad_settings.json", "--custom"],
        "settings_not_object": ["--proposal", "p/settings_not_object.json"],
        "stages_string": ["--proposal", "p/stages_string.json"],
        "keywords": ["--proposal", "p/bugfix.json", "--keywords", "Bug, hotfix,,zzz, FIX"],
        "keywords_no_value": ["--proposal", "p/bugfix.json", "--keywords"],
        "guard_flag": ["--proposal", "p/bugfix.json", "--guard-policy", "RELAXED"],
        "guard_flag_markdown": ["--proposal", "p/bugfix.json", "--guard-policy", "`off`"],
        "guard_flag_bad": ["--proposal", "p/bugfix.json", "--guard-policy", "lenient"],
        "guard_flag_no_value": ["--proposal", "p/bugfix.json", "--guard-policy"],
        "guard_flag_retired": ["--proposal", "p/bugfix.json", "--change-control", "off"],
        "guard_flag_strict": ["--proposal", "p/bugfix.json", "--guard-policy", "strict"],
        "matched_and_custom": ["--proposal", "p/bugfix.json", "--matched", "bugfix", "--custom"],
        "matched_no_value": ["--proposal", "p/bugfix.json", "--matched"],
        "project_type_wrong": ["--proposal", "p/bugfix.json", "--project-type", "bluefield"],
        "project_type_trailing": ["--proposal", "p/bugfix.json", "--project-type"],
        "proposal_no_value": ["--proposal"],
        "array": ["--proposal", "p/array.json"],
        "null": ["--proposal", "p/null.json"],
        "stages_array": ["--proposal", "p/stages_array.json"],
        "not_json": ["--proposal", "p/not_json.json"],
        "missing_file": ["--proposal", "p/none.json"],
        "default_path": [],
        "space_named": ["--proposal", "p/full_bugfix.json", "--custom", "--space", "payments"],
        "space_is_a_path": ["--proposal", "p/full_bugfix.json", "--custom", "--space", "../payments"],
    }
    for tree in ("stock", "plugins_disabled", "plugins_enabled", "plugins_only", "composed", "no_grid",
                 "extra_scope_file", "memory_strict", "memory_relaxed_then_strict", "memory_invalid",
                 "memory_retired_heading", "memory_wrapped", "other_space", "active_space", "bad_scope"):
        for name, args in grid_cases.items():
            on(tree, f"grid_{name}", ["validate-grid", *args])
    for tree in ("memory_in_fence", "memory_in_comment", "memory_in_html_block", "memory_in_indented_code",
                 "memory_in_code_span", "memory_in_quoted_fence", "memory_heading_in_fence", "memory_after_comment",
                 "memory_in_details", "memory_escaped_backticks", "memory_span_after_escape",
                 "memory_lowercase_declaration", "memory_past_budget"):
        for name in ("bugfix", "matched_ok", "matched_guard_lower", "custom_flag_policy", "guard_flag_strict"):
            on(tree, f"grid_{name}", ["validate-grid", *grid_cases[name]])
    on("stock", "grid_stdin", ["validate-grid", "--proposal", "-"],
       stdin=json.dumps({"stages": {"code-generation": "EXECUTE"}}))
    on("no_proposal", "grid_default_path_missing", ["validate-grid"])
    for switch in ("DISABLE_SENSORS", "DISABLE_PLAN_APPROVAL_GUARD", "DISABLE_COLLABORATORS"):
        on("stock", f"grid_kill_switch_{switch.lower()}",
           ["validate-grid", "--proposal", "p/full_feature.json", "--custom"], {switch: "1"})
    on("stock", "grid_kill_switch_not_one", ["validate-grid", "--proposal", "p/full_feature.json", "--custom"],
       {"DISABLE_SENSORS": "true"})
    settings_checks(on)


def recorded(*switches, **more):
    """A settings file that records `switches` (and whatever else `more` says)."""
    return json.dumps({"schemaVersion": 1, "flags": {"schemaVersion": 1, "bypasses": list(switches)}, **more})


def settings_files(**files):
    """An edit that writes settings files: project=, local=, machine= (the last under machine/, which a check
    names as the install root)."""
    where = {"project": "aidlc.settings.json", "local": "aidlc.settings.local.json",
             "machine": "machine/aidlc.settings.json"}
    return lambda tree: [tree.write(where[layer], text) for layer, text in files.items()]


VERB_TREES.update({
    "settings_project": raw(proposals, settings_files(
        project=recorded("AIDLC_DISABLE_SENSORS", "AIDLC_DISABLE_LEARNINGS"))),
    "settings_local": raw(proposals, settings_files(local=recorded("AIDLC_DISABLE_PLAN_APPROVAL_GUARD"))),
    "settings_machine": raw(proposals, settings_files(
        machine=recorded("AIDLC_DISABLE_SUMMARY_CONFIRMATION", offline=True))),
    "settings_layers": raw(proposals, settings_files(
        machine=recorded("AIDLC_DISABLE_SENSORS", **{"release-base-url": "https://releases.example.com/"}),
        project=recorded("AIDLC_DISABLE_LEARNINGS", "AIDLC_SKIP_ARTIFACT_GUARD"),
        local=recorded("AIDLC_DISABLE_SUMMARY_CONFIRMATION"))),
    "settings_no_switches": raw(proposals, settings_files(
        project=json.dumps({"schemaVersion": 1, "flags": {"schemaVersion": 1, "swarm": True}}),
        local=json.dumps({"schemaVersion": 1}))),
    "settings_unknown_key": raw(proposals, settings_files(project='{"schemaVersion": 1, "flag": {}}')),
    "settings_not_json": raw(proposals, settings_files(local="{ nope")),
    "settings_machine_key_in_project": raw(proposals, settings_files(
        project='{"schemaVersion": 1, "offline": true}')),
    "settings_unknown_switch": raw(proposals, settings_files(project=recorded("AIDLC_DISABLE_COLLABORATORS"))),
    "settings_is_directory": raw(proposals, lambda tree: tree.write("aidlc.settings.json/inside.txt", "x\n")),
    "settings_url_spelling": raw(proposals, settings_files(
        machine=json.dumps({"schemaVersion": 1, "release-base-url": "HTTPS://Releases.Example.com"}))),
})

DEVIATIONS.update({
    "verbs/settings_url_spelling/grid_ceremonies_on":
        "upstream validates release-base-url with its runtime's URL parser, which accepts many spellings of one "
        "location; focus reads one spelling (lower-case scheme, two slashes, a plain host) and refuses the rest",
})


def settings_checks(on):
    on_ = ["validate-grid", "--proposal", "p/full_feature.json", "--custom"]
    root = {"INSTALL_ROOT": "machine"}
    for tree in ("settings_project", "settings_local", "settings_machine", "settings_layers", "settings_no_switches",
                 "settings_unknown_key", "settings_not_json", "settings_machine_key_in_project",
                 "settings_unknown_switch", "settings_is_directory", "settings_url_spelling"):
        on(tree, "grid_ceremonies_on", on_, root)
        # No ceremony is on in this plan, so no switch is looked up and the settings are never read.
        on(tree, "grid_no_ceremonies", ["validate-grid", "--proposal", "p/bugfix.json"], root)
    # The environment answers first, whatever it says.
    on("settings_project", "grid_env_says_zero", on_, {"DISABLE_SENSORS": "0"})
    on("settings_project", "grid_env_says_nothing", on_, {"DISABLE_SENSORS": "", "DISABLE_LEARNINGS": ""})
    on("settings_project", "grid_env_says_one", on_, {"DISABLE_SENSORS": "1", "DISABLE_PLAN_APPROVAL_GUARD": "1"})
    on("settings_layers", "grid_env_clears_each", on_,
       {**root, "DISABLE_SENSORS": "0", "DISABLE_LEARNINGS": "no", "DISABLE_SUMMARY_CONFIRMATION": "off"})
    # Without the install root named, the machine's file is somewhere else and is not this one.
    on("settings_machine", "grid_machine_file_not_named", on_)
    on("settings_layers", "grid_machine_file_not_named", on_)
    on("settings_layers", "grid_matched", ["validate-grid", "--proposal", "p/full_bugfix.json", "--matched", "bugfix"],
       root)
    on("settings_layers", "grid_report", [*on_, "--report"], root)


def fuzz(count):
    rng = random.Random(SEED)
    trees = {name: VERB_TREES[name](f"fuzz/{name}") for name in ("stock", "plugins_enabled", "composed")}
    grids = {name: pair_[0].json(".claude/tools/data/scope-grid.json") for name, pair_ in trees.items()}
    slugs = {name: [s["slug"] for s in pair_[0].json(".claude/tools/data/stage-graph.json")]
             for name, pair_ in trees.items()}
    score = lambda: rng.choice(["0", "1", f"{rng.randint(0, 100) / 100:.2f}", f"{rng.randint(0, 100) / 100}",
                                f"{rng.randint(0, 10) / 10}"])
    for index in range(count):
        tree = rng.choice(list(trees))
        if index % 2 == 0:
            args = ["ars"]
            for component in ("iae", "csu", "ve", "r", "ua"):
                args += [f"--{component}", score()]
            if rng.random() < 0.5:
                args += ["--project-type", rng.choice(["greenfield", "brownfield"])]
            if rng.random() < 0.4:
                args += ["--completed", ",".join(rng.sample(slugs[tree], rng.randint(1, 4)))]
            check(f"fuzz/{tree}/ars_{index}", trees[tree], args)
            continue
        base = dict(rng.choice(list(grids[tree].values())).get("stages") or {})
        for slug in rng.sample(slugs[tree], rng.randint(0, 5)):
            base[slug] = rng.choice(["EXECUTE", "SKIP"])
        if rng.random() < 0.15:
            base.pop(rng.choice(list(base)), None)
        if rng.random() < 0.1:
            base["invented-stage"] = "EXECUTE"
        document = {"stages": base}
        if rng.random() < 0.7:
            document[rng.choice(["guardPolicy", "changeControl"])] = rng.choice(["strict", "relaxed", "off", "Off"])
        if rng.random() < 0.7:
            document["scopeSettings"] = {
                **{key: rng.choice(["on", "off"]) for key in ("sensors", "learnings", "summary_confirmation",
                                                                 "plan_approval", "collaborators")},
                "review_cap": rng.choice(["adversarial", "advisory", "none"])}
        if rng.random() < 0.8:
            document["depth"] = rng.choice(["minimal", "standard", "comprehensive", "Minimal", "deep"])
        args = ["validate-grid", "--proposal", "-"]
        route = rng.random()
        if route < 0.4:
            args.append("--custom")
        elif route < 0.75:
            args += ["--matched", rng.choice(list(grids[tree]))]
        if rng.random() < 0.3:
            args.append("--strict")
        if rng.random() < 0.3:
            args.append("--report")
        if rng.random() < 0.5:
            args += ["--project-type", rng.choice(["greenfield", "brownfield"])]
        check(f"fuzz/{tree}/grid_{index}", trees[tree], args, stdin=json.dumps(document))


# --------------------------------------------------------------------------
# The audit command.
#
# A check is a project built from nothing but the files the check names, and
# one or more commands run on it in order. The outcome is what each command
# printed and how it exited, and every file of the workspace the commands
# created, changed or removed. Each check gets trees of its own: a command
# that appends writes the time, so a tree an earlier check wrote to cannot be
# recorded and replayed later.
#
# Upstream's tool is run from its distribution with --project-dir, so a tree
# needs no harness installed.
# --------------------------------------------------------------------------

AUDIT_TOOL = os.path.join(DIST, ".claude", "tools", "aidlc-audit.ts")
STAMP = re.compile(r"\d{4}-\d\d-\d\dT\d\d:\d\d:\d\dZ")
RECORD_DIR = "aidlc/spaces/default/intents/add-login-1a2b3c4d"


def normalise_audit(text, tree, engine):
    text = without_tree(text, tree)
    # A handoff named after the project's own folder, which each engine's tree names its own way.
    text = text.replace(f"developer-scan-{os.path.basename(tree.root)}.md", "developer-scan-<root>.md")
    if engine == "ts":
        # Bun reads the working directory's package.json as it starts, and says on standard error what it
        # thinks of one it cannot parse; that is the runtime, not the tool.
        text = re.sub(r"^\d+ \| [^\n]*\n[ ]*\^\n(?:error|warn): [^\n]*\n +at [^\n]*package\.json:\d+:\d+\n", "", text,
                      flags=re.M)
    # The runtime command names itself at the start of what it says on standard error.
    text = re.sub(r"^%s(?=[ :])" % ("aidlc-runtime" if engine == "ts" else "focus runtime"), "runtime", text,
                  flags=re.M)
    text = text.replace("Run aidlc-runtime --help", "Run <cli> runtime --help")
    text = text.replace("Run focus engine runtime --help", "Run <cli> runtime --help")
    text = re.sub(r"^(runtime (?:read|summary): )(?:JSON Parse error: [^\n]*|[^\n]* at line \d+ column \d+)$",
                  r"\1<runtime>", text, flags=re.M)
    # A file that turns out to be a directory: each runtime's own words for it.
    text = re.sub(r"^(runtime \w+: )(?:EISDIR: [^\n]*|[^\n]*: Is a directory \(os error 21\))$", r"\1<runtime>", text,
                  flags=re.M)
    for pattern, replacement in RUNTIME_TEXT:
        text = re.sub(pattern, replacement, text, flags=re.M)
    text = text.replace("Usage: aidlc-audit ", "Usage: <cli> audit ").replace("Usage: focus engine audit ",
                                                                             "Usage: <cli> audit ")
    # A command of the log tool rendered for the agent to run next: each engine's own spelling of the same route.
    text = re.sub(r"`bun \.claude/tools/(?:aidlc|focus)-log\.ts ((?:decision|answer) [^`]*)`", r"<run log \1>", text)
    text = re.sub(r"`focus engine log ((?:decision|answer) [^`]*)`", r"<run log \1>", text)
    for tool in ("orchestrate", "log"):
        suffix = " report" if tool == "orchestrate" else ""
        text = text.replace(f"aidlc-{tool}.ts{suffix}", f"<cli> {tool}{suffix}")
        text = text.replace(f"`focus engine {tool}{suffix}`", f"<cli> {tool}{suffix}")
    # A tool's script named whole in a sentence (the `next` a refusal tells the agent to run).
    text = re.sub(r"`bun \.claude/tools/aidlc-(state|orchestrate)\.ts(?: ([^`]*))?`",
                  lambda m: f"<cli> {m.group(1)}" + (f" {m.group(2)}" if m.group(2) else ""), text)
    # The state tool names itself, its verbs and the orchestrator's in each engine's own spelling.
    # (A file's text has already been through the name map, so the tool is named either way.)
    text = re.sub(r"(?:aidlc|focus)-(state|orchestrate|log)\.ts(?: ([a-z-]+))?",
                  lambda m: f"<cli> {m.group(1)}" + (f" {m.group(2)}" if m.group(2) else ""), text)
    text = re.sub(r"`(?:aidlc|focus) engine (state|orchestrate)(?: ([^`]*))?`", lambda m: f"<cli> {m.group(1)}" + (f" {m.group(2)}" if m.group(2) else ""), text)
    # The entry script naming a tool's verb in a sentence (`<next>` in a hook-activation step).
    text = re.sub(r"`bun \.claude/tools/(?:aidlc|focus)\.ts engine (state|orchestrate|log)(?: ([^`]*))?`",
                  lambda m: f"<cli> {m.group(1)}" + (f" {m.group(2)}" if m.group(2) else ""), text)
    # A file where a folder is to be made: each runtime's own words.
    text = re.sub(r"(?:EEXIST: file already exists, mkdir '[^']*'|(?:<project-dir>/)?[^\s\"(]*: File exists \(os error 17\))", "<runtime>", text)
    # A state file that is a directory: each runtime's own words.
    text = re.sub(r"(?:EISDIR: illegal operation on a directory, read|(?:<project-dir>/)?[^\s\"]*(?:aidlc|focus)-state\.md: Is a directory \(os error 21\))",
                  "<runtime>", text)
    # The tool's own name (never the state file's) in a row's Tool or Details, or in the line it prints.
    text = re.sub(r"(?<![\w-])(?:aidlc|focus)-state(?![\w.-])(?: (get|set|checkbox|count|[a-z-]+))?(?![\w-])",
                  lambda m: "<cli> state" + (f" {m.group(1)}" if m.group(1) else ""), text)
    text = text.replace("Usage: focus engine state ", "Usage: <cli> state ")
    text = text.replace("Run focus engine state --help", "Run <cli> state --help")
    # The state tool naming one of its own verbs in a sentence (`skip takes --route or --unit`).
    text = re.sub(r"(?<![`\w])focus engine state (?=[a-z-]+ )", "<cli> state ", text)
    text = text.replace("Usage: focus engine utility ", "Usage: <cli> utility ")
    # The utility tool's own name in a row's Tool or Command.
    text = re.sub(r"(?<![\w-])(?:aidlc|focus)-utility(?![\w.-])", "<cli> utility", text)
    # The fence switch is one operation, spelled for each engine's installation.
    text = re.sub(r"`bun \.claude/tools/(?:aidlc|focus)-utility\.ts config-change --(guard\.[a-z-]+) off`", r"<cli-config \1 off>", text)
    text = re.sub(r"`focus engine config set (guard\.[a-z-]+) off`", r"<cli-config \1 off>", text)
    text = re.sub(r"`bun \.claude/tools/(?:aidlc|focus)-utility\.ts config-change --guard-policy (strict|relaxed|off)`", r"<cli-config guard-policy \1>", text)
    text = re.sub(r"`focus engine config set guard-policy (strict|relaxed|off)`", r"<cli-config guard-policy \1>", text)
    text = text.replace("how closely AI-DLC checks", "how closely <engine> checks").replace("how closely Focus checks", "how closely <engine> checks")
    # The jump tool's own name in a row's Tool or Command.
    text = re.sub(r"(?<![\w-])(?:aidlc|focus)-jump(?![\w.-])", "<cli> jump", text)
    # The runtime tool naming itself in the line it prints (`aidlc-runtime compile:` / `focus runtime compile:`).
    # Inside a file the tool's name was already translated when the file was read.
    text = re.sub(r"(?<![\w-])(?:aidlc-runtime|focus-runtime|focus runtime) (compile|read|summary):", r"<cli> runtime \1:", text)
    # A handoff receipt's instant in epoch milliseconds, written during the run.
    text = re.sub(r'"issuedAtMs":(\d{13})', lambda m: '"issuedAtMs":<now-ms>' if int(m.group(1)) >= STARTED_MS else m.group(0), text)
    # A tool named in a sentence, by its script upstream and by its command here.
    text = re.sub(r"`bun \.claude/tools/(?:aidlc|focus)-([a-z-]+)\.ts(?: ([^`]*))?`", lambda m: f"<cli> {m.group(1)}" + (f" {m.group(2)}" if m.group(2) else ""), text)
    text = re.sub(r"`(?:aidlc|focus) engine ([a-z-]+)(?: ([^`]*))?`", lambda m: f"<cli> {m.group(1)}" + (f" {m.group(2)}" if m.group(2) else ""), text)
    # The Command field of an error row carries the arguments the differential adds for Focus.
    text = text.replace(" --harness claude\n", "\n")
    text = STAMP.sub(lambda found: "<now>" if found.group(0) >= STARTED else found.group(0), text)
    text = re.sub(r"(could not be opened: [^\n(]*\()[^\n]*?\)(?=\"\}|\n|$)", r"\1<runtime>)", text)
    # A record minted during the run is named after the day.
    text = re.sub(rf"(?<![0-9a-f]){TODAY}-(?=[a-z])", "<day>-", text)
    return text


def uncaught(stderr):
    """The sentence of an error upstream let reach the runtime, or None when stderr is not one."""
    # A plain Error prints as `error: sentence`; a named one as its name, the sentence, then its own fields.
    found = re.search(r"^(?:error|[A-Z]\w*Error): (.*?)\n(?: \w+: [^\n]*\n)*\s+at ", stderr, flags=re.M | re.S)
    if not found or "\nBun v" not in stderr:
        return None
    return found.group(1)


def group_thrown(stderr):
    """A hook group's stderr with each member's stack trace read as the line a command says, once; and whether
    there was one (SHAPES: group-thrown)."""
    lines, seen = [], False
    for found in re.finditer(r"^(?:Error: ([^\n]*)\n(?:    at [^\n]*\n)+|[^\n]*\n?)", stderr, flags=re.M):
        if found.group(1) is not None:
            seen = True
            line = f"aidlc: {found.group(1)}\n"
            if line not in lines:
                lines.append(line)
        elif found.group(0):
            lines.append(found.group(0))
    return ("".join(lines), True) if seen else (stderr, False)


def workspace_files(tree, top="aidlc"):
    """Every file under `top` (the workspace), by path under Focus's names. A link is listed, not read."""
    say = (lambda text: text) if tree.mapped else to_focus
    top = tree.path(top)
    files = {}
    for folder, dirs, names in os.walk(top):
        for name in list(dirs):
            if os.path.islink(os.path.join(folder, name)):
                files[say(os.path.relpath(os.path.join(folder, name), tree.root))] = "<link>"
        for name in names:
            path = os.path.join(folder, name)
            rel = say(os.path.relpath(path, tree.root))
            # Focus's lock is a file in the workspace; upstream's is a directory elsewhere, and the mutex it takes
            # around that directory's recovery leaves an empty `<lock>.gate-mutex` file behind.
            if os.path.basename(rel).startswith(".focus-lock") or rel.endswith(".gate-mutex"):
                continue
            if os.path.islink(path):
                files[rel] = "<link>"
            elif not os.path.isfile(path):
                files[rel] = "<not a file>"
            else:
                with open(path, "rb") as handle:
                    text = handle.read().decode("utf-8", errors="replace")
                if name.endswith(".rules-held.json"):
                    text = own_hashes(tree, text)
                if BASELINE_SNAPSHOT.search(rel):
                    rel, text = own_baseline(tree, rel, path, say)
                    files[rel] = text
                    continue
                files[rel] = say(text)
    return files


# A source baseline is the hash of the project's source listing, kept beside the record under that hash. The listing
# names every source file with the hash of its content, and a tree's files carry the engine's name in their paths and
# text, so the two engines' hashes differ by construction. Each engine's are recomputed here from its own tree: a
# snapshot whose bytes hash to the name it is kept under is `baseline-<verified>.tsv`, a row whose hash is the hash
# of the tree's own file (or of where its own link points) reads `<own>`, and a row's field naming a verified snapshot
# reads `sha256:<verified>`. One that does not match stays as it is, and shows.
BASELINE_SNAPSHOT = re.compile(r"/source-review/[a-z][a-z0-9-]*/baseline-([0-9a-f]{12})\.tsv$")
BASELINE_ROW = re.compile(r"^([^\t\\]*)\t([^\t\\]+)\t(100644|100755|120000)\t([0-9a-f]{64})$")


def own_baseline(tree, rel, path, say):
    with open(path, "rb") as handle:
        raw = handle.read()
    digest = hashlib.sha256(raw).hexdigest()
    if digest[:12] != BASELINE_SNAPSHOT.search(rel).group(1):
        return rel, say(raw.decode("utf-8", errors="replace"))
    if not hasattr(tree, "baselines"):
        tree.baselines = set()
    tree.baselines.add(digest)

    def own(row):
        found = BASELINE_ROW.match(row)
        if found is None:
            return row
        repo, name, mode, sha = found.groups()
        target = os.path.join(tree.root, repo, name) if repo else os.path.join(tree.root, name)
        try:
            if mode == "120000":
                held = [os.readlink(target).encode("utf-8")]
            else:
                with open(target, "rb") as handle:
                    content = handle.read()
                held = [content, content.replace(b"\r\n", b"\n")]
        except OSError:
            return row
        # The file as it is now, or as it was when the command started: a command can change a source file after
        # it has taken the baseline (creation adds its block to `.gitignore`).
        hashes = {hashlib.sha256(one).hexdigest() for one in held} | getattr(tree, "source_before", {}).get(target, set())
        return f"{repo}\t{name}\t{mode}\t<own>" if sha in hashes else row

    rows = [say(own(row)) for row in raw.decode("utf-8", errors="replace").split("\n") if row]
    # In an order that does not depend on the engine's name.
    rows.sort(key=lambda row: re.sub("focus", "\x01", row, flags=re.I))
    return BASELINE_SNAPSHOT.sub(lambda m: m.group(0).replace(m.group(1), "<verified>"), rel), "".join(row + "\n" for row in rows)


def source_hashes(tree):
    """The hashes of every file of a tree outside the workspace and the harness, raw and with each CRLF as LF."""
    hashes = {}
    for folder, dirs, names in os.walk(tree.root):
        if folder == tree.root:
            dirs[:] = [name for name in dirs if name not in ("aidlc", "focus", "node_modules") and not name.startswith(".")]
        for name in names:
            path = os.path.join(folder, name)
            if os.path.isfile(path) and not os.path.islink(path):
                with open(path, "rb") as handle:
                    content = handle.read()
                hashes[path] = {hashlib.sha256(content).hexdigest(), hashlib.sha256(content.replace(b"\r\n", b"\n")).hexdigest()}
    return hashes


def verified_baselines(tree, text):
    known = getattr(tree, "baselines", set())
    return re.sub(r"(\*\*Source Baseline\*\*: sha256:)([0-9a-f]{64})",
                  lambda m: m.group(1) + ("<verified>" if m.group(2) in known else m.group(2)), text)


AUTHORIZATION_ID = re.compile(r"(?<![0-9a-f])[0-9a-f]{64}(?![0-9a-f])")


def authorization_ids(tree):
    """The ids of the summary authorizations a tree's records keep that are what their own fields hash to."""
    known = set()
    for folder, _, names in os.walk(tree.root):
        if "stage.json" not in names or os.path.basename(os.path.dirname(folder)) != "summary-authorization":
            continue
        try:
            with open(os.path.join(folder, "stage.json"), encoding="utf-8") as handle:
                kept = json.load(handle)
            named = json.dumps([kept[key] for key in ("attempt", "stage", "unit", "workflow", "questions_file", "questions_sha256", "choice")],
                               ensure_ascii=False, separators=(",", ":"))
        except (OSError, ValueError, KeyError, TypeError):
            continue
        if hashlib.sha256(named.encode("utf-8")).hexdigest() == kept.get("id"):
            known.add(kept["id"])
    return known


def own_hashes(tree, text):
    """A rules record hashes the memory files as each engine ships them, which differ by the engine's name in
    their text: a hash that is the tree's own file's is shown as `<sha256>`, and one that is not stays."""
    def own(found):
        path = os.path.join(tree.root, found.group(1))
        try:
            with open(path, "rb") as handle:
                digest = hashlib.sha256(handle.read()).hexdigest()
        except OSError:
            return found.group(0)
        return f'"{found.group(1)}":"<sha256>"' if digest == found.group(2) else found.group(0)
    return re.sub(r'"([^"]+\.md)":"([0-9a-f]{64})"', own, text)


def my_pid_entry(session="s1"):
    """The process map naming this process (the engines' parent) as `session`'s, with its start time as both
    engines read it on Linux (`/proc/<pid>/stat`, the 22nd field)."""
    with open("/proc/self/stat", encoding="utf-8") as handle:
        raw = handle.read()
    fields = raw[raw.rindex(")") + 1:].split()
    return {f"aidlc/.aidlc-sessions/pids/{os.getpid()}": json.dumps({"sessionId": session, "startTime": fields[19]}) + "\n"}


def dated(rel):
    """A path with today's record-day prefix written as its placeholder."""
    return rel.replace(f"/intents/{TODAY}-", "/intents/<day>-")


def minted(rel, text):
    """A path and content with what was minted during the run written as placeholders: a clone identity, a
    record's day prefix, a record's id."""
    rel = re.sub(r"/audit/[a-z0-9][a-z0-9-]*-[0-9a-f]{12}\.md$", "/audit/<host>-<token>.md", rel)
    rel = dated(rel)
    if rel.endswith("/.focus-clone-id") and text is not None:
        text = re.sub(r"\A[0-9a-f]{12}\n[a-z0-9][a-z0-9-]*\n\Z", "<token>\n<host>\n", text)
    if text is not None and (rel.endswith("/intents.json") or rel.endswith("/creation-receipt")):
        text = re.sub(UUID_V7, "<uuid>", text)
    return rel, text


MESSAGE_FILE = re.compile(r"/\.focus-sessions/messages/([0-9a-f]{8})\.json$")


def placeholder_messages(files):
    """The message records minted during a run, numbered in the order the ledger names them (the `Message Id`
    of each HUMAN_TURN row, in order; a record no row names after those, by its `at` and id), and their ids
    written as placeholders wherever they appear: the file's name, its `id` field, and the row's field."""
    minted_ids = []
    for key, text in files.items():
        found = MESSAGE_FILE.search(key)
        if found and isinstance(text, str) and text not in ("<removed>", "<absent>"):
            try:
                at = json.loads(text).get("at", "")
            except (ValueError, AttributeError):
                at = ""
            minted_ids.append((at, found.group(1)))
    if not minted_ids:
        return files
    named = [found for _, found in sorted(minted_ids)]
    in_rows = []
    for key in sorted(files):
        if "/audit/" in key and isinstance(files[key], str):
            for found in re.findall(r"\*\*Message Id\*\*: ([0-9a-f]{8})\n", files[key]):
                if found in named and found not in in_rows:
                    in_rows.append(found)
    ordered = in_rows + [found for found in named if found not in in_rows]
    names = {found: f"<message-{n}>" for n, found in enumerate(ordered, start=1)}
    out = {}
    for key, text in files.items():
        for found, name in names.items():
            key = key.replace(f"/messages/{found}.json", f"/messages/{name}.json")
            if isinstance(text, str):
                text = text.replace(f'"id":"{found}"', f'"id":"{name}"').replace(f"**Message Id**: {found}\n", f"**Message Id**: {name}\n")
        out[key] = text
    return out


def audit_pair(name, files, links=(), hard_links=()):
    """Two empty projects holding `files` ({path: text}, upstream's names), `links` and `hard_links`
    (each a (path, target))."""
    trees = []
    for engine in ("ts", "rs"):
        root = os.path.join(WORK, name.replace("/", "_"), engine)
        os.makedirs(root)
        tree = Tree(root, mapped=engine == "rs")
        for rel, text in files.items():
            tree.write(rel, text)
        for rel, target in links:
            os.makedirs(os.path.dirname(tree.path(rel)), exist_ok=True)
            os.symlink(tree.path(target), tree.path(rel))
        for rel, target in hard_links:
            os.link(tree.path(target), tree.path(rel))
        trees.append(tree)
    return trees


# The marks a coding tool leaves on the environment of the commands it starts: a refusal names the host whose
# terminal the command came from, so a check runs with none, and names one itself when it wants one.
HOST_MARKS = ("CLAUDECODE", "CLAUDE_CODE_", "CODEX_", "CURSOR_", "KIRO_", "OPENCODE", "COPILOT_", "VSCODE_")


def run_audit(engine, tree, args, env, tool="audit"):
    full = dict(os.environ)
    for key in list(full):
        if (key.startswith(("AIDLC_", "FOCUS_")) or key == "CLAUDE_PROJECT_DIR" or key.upper() == "TERM_PROGRAM"
                or key.upper().startswith(HOST_MARKS)):
            del full[key]
    # A name given as `=NAME` is set as it is, for both engines.
    raw = {k[1:]: v for k, v in (env or {}).items() if k.startswith("=")}
    env = {k: v for k, v in (env or {}).items() if not k.startswith("=")}
    full.update(raw)
    # `{P}` in an argument is the project's own location.
    # `{N}` is the name of the project's folder, the name its one repository goes by.
    args = [arg.replace("{P}", tree.root).replace("{N}", os.path.basename(tree.root)) for arg in args]
    # A hook step is the hook's name and the host's payload, which it reads on stdin.
    stdin = None
    if tool == "hook":
        args, stdin = args[:1], (args[1] if len(args) > 1 else "")
    # A step that says where the project is says it itself.
    where = [] if "--project-dir" in args else ["--project-dir", tree.root]
    if engine == "ts":
        # The one variable upstream prefixes otherwise (see names.py).
        full.update({("AWS_AIDLC_DEFAULT_SCOPE" if k == "DEFAULT_SCOPE" else f"AIDLC_{k}"): v for k, v in (env or {}).items()})
        if tool == "hook":
            # The dispatcher finds the hook beside itself, and the hook its project above it.
            command = [BUN, tree.path(".claude/tools/aidlc.ts"), "engine", "hook", *args]
        else:
            # The runtime tool reads the stage graph beside it, so it is run from the tree's own installation.
            script = AUDIT_TOOL if tool == "audit" else tree.path(f".claude/tools/aidlc-{tool}.ts")
            command = [BUN, script, *args, *where]
    else:
        full.update({f"FOCUS_{k}": to_focus(v) for k, v in (env or {}).items()})
        command = [FOCUS, "engine", tool, *map(to_focus, args), *where, "--harness", "claude"]
        stdin = stdin if stdin is None else to_focus(stdin)
    done = subprocess.run(command, cwd=tree.root, env=full, input=stdin, capture_output=True, text=True, timeout=120)
    return done.returncode, done.stdout, done.stderr


# Checks whose trees a recording cannot hold: a hard link is two names for one file.
UNRECORDED = set()


def audit_check(name, files, steps, env=None, links=(), hard_links=(), group="audit"):
    """Queue one audit check: `steps` (a list of argument lists) run in order on a project holding `files`."""
    name = f"{group}/{name}"
    if ONLY and not re.search(ONLY, name):
        return
    if hard_links:
        UNRECORDED.add(name)
    JOBS.append((name, audit_pair(name, files, links, hard_links), steps, env, None, "audit"))


# The ask after a refusal's sentence: on its own line in what is printed, and as the two characters `\n`
# inside the error row's one-line field.
ASK_LINE = re.compile(r'\n\{"kind":"ask",.*')
ASK_FIELD = re.compile(r'\\n\{"kind":"ask",[^\n]*')


def without_guard_ask(stderr):
    """Upstream's refusal without the guard-recovery ask on its last line, and whether there was one."""
    try:
        parsed = json.loads(stderr)
    except ValueError:
        return stderr, False
    if not isinstance(parsed, dict) or not isinstance(parsed.get("error"), str) or not ASK_LINE.search(parsed["error"]):
        return stderr, False
    parsed["error"] = ASK_LINE.sub("", parsed["error"])
    return json.dumps(parsed, ensure_ascii=False, separators=(",", ":")) + "\n", True


ARTIFACT_FILENAMES = {"build-test-results": "test-results.md", "load-test-results": "test-results.md", "traceability": "traceability.json"}


def committed_text(data):
    """`committedTextBytes`: CRLF read as LF when the bytes are text with CRLFs."""
    if b"\r" not in data or b"\0" in data or re.search(rb"\r(?!\n)", data):
        return data
    return data.replace(b"\r\n", b"\n")


# Printable ASCII in the order of the CLDR root collation (`localeCompare`), a letter's cases together.
ROOT_ORDER = " _-,;:!?.'\"()[]{}@*/\\&#%`^+<=>|~$0123456789abcdefghijklmnopqrstuvwxyz"


def root_key(text):
    """A sort key giving `localeCompare`'s order for printable ASCII (a NUL is ignorable)."""
    return [ROOT_ORDER.find(char.lower()) for char in text if char != "\0"]


def verify_validation_bases(tree, engine, rel, text):
    """Each receipt's hashes in `text` (a shard at `rel`, under Focus's names) recomputed from this engine's own
    tree and names, and written as <verified> where they match. Only the artifacts of ordinary stages (one file
    under the record) are recomputed; a per-unit or code-knowledge-base artifact keeps its hash."""
    if "**Validation Basis**: " not in text:
        return text, False
    graph = {stage["slug"]: stage for stage in tree.json(".claude/tools/data/stage-graph.json")}
    record = to_upstream(rel.rsplit("/audit/", 1)[0])
    say = (lambda path: path) if engine == "ts" else to_focus
    canonical = lambda value: json.dumps(value, separators=(",", ":"), sort_keys=True, ensure_ascii=False)
    sha = lambda value: "sha256:" + hashlib.sha256(value.encode("utf-8") if isinstance(value, str) else value).hexdigest()

    def repos():
        try:
            rows = tree.json("aidlc/spaces/default/intents/intents.json")
            row = next(row for row in rows if row.get("dirName") == os.path.basename(record))
        except (ValueError, OSError, StopIteration, AttributeError):
            return None
        listed = row.get("repos") or []
        if listed:
            return sorted(set(listed))
        # With none recorded, the store is named after the project's folder (each engine's own), unless no store
        # carries that name and exactly one store no row claims does: a folder that was moved keeps its store.
        name = os.path.basename(tree.root)
        root = tree.path("aidlc/spaces/default/codekb")
        if not os.path.exists(os.path.join(root, name)) and os.path.isdir(root):
            claimed = {repo for other in rows for repo in (other.get("repos") or [])}
            stores = [store for store in os.listdir(root) if os.path.isdir(os.path.join(root, store)) and store not in claimed]
            if len(stores) == 1:
                return stores
        return [name]

    def state():
        try:
            return tree.read(f"{record}/aidlc-state.md")
        except OSError:
            return ""

    def plan_action(slug):
        found = re.search(rf"^- \[[ xSR?-]\] {re.escape(slug)}\s*\u2014\s*(EXECUTE|SKIP)\b", state(), flags=re.M)
        if found:
            return found.group(1)
        return tree.json(".claude/tools/data/scope-grid.json").get(re.search(r"^- \*\*Scope\*\*: (.*)$", state(), flags=re.M).group(1), {}).get("stages", {}).get(slug)

    def stage_level():
        """`usesStageLevelPerUnitArtifacts`."""
        if plan_action("units-generation") == "EXECUTE":
            return False
        return not re.search(r"^- \[x\] units-generation ", state(), flags=re.M)

    def units(slug):
        """The DAG's units with their kinds, or the directories under construction/ holding the stage's with no
        DAG; None when the DAG is malformed (the receipt is then a warning, verified by its text)."""
        try:
            doc = tree.read(f"{record}/inception/units-generation/unit-of-work-dependency.md")
        except OSError:
            doc = None
        if doc is not None:
            block_ = re.search(r"```yaml\nunits:\n(.*?)```", doc, flags=re.S)
            if block_ is None:
                return None
            found, current = [], None
            for line in block_.group(1).split("\n"):
                name = re.match(r"\s*- name: (\S+)", line)
                kind = re.match(r"\s+kind: (\S+)", line)
                if name:
                    current = [name.group(1), None]
                    found.append(current)
                elif kind and current:
                    current[1] = kind.group(1)
            return sorted((tuple(unit) for unit in found), key=lambda unit: root_key(unit[0]))
        try:
            cached = tree.json(f"{record}/runtime-graph.json").get("bolt_dag", {})
            kinds = {unit["name"]: unit["kind"] for unit in cached.get("units", []) if isinstance(unit, dict)}
            return sorted(((name, kinds.get(name)) for batch in cached["batches"] for name in batch), key=lambda unit: root_key(unit[0]))
        except (OSError, ValueError, KeyError, TypeError, AttributeError):
            pass
        construction = tree.path(f"{record}/construction")
        if not os.path.isdir(construction):
            return []
        return sorted(((name, None) for name in os.listdir(construction) if os.path.isdir(os.path.join(construction, name, slug))),
                      key=lambda unit: root_key(unit[0]))

    def applies(stage, artifact, kind):
        kinds = (stage.get("produces_kinds") or {}).get(artifact)
        return kind is None or kinds is None or kind in kinds

    def verify(entry):
        stage = graph.get(entry.get("producer"))
        if stage is None:
            return entry
        filename = ARTIFACT_FILENAMES.get(entry["artifact"], entry["artifact"] + ".md")
        if stage["slug"] == "reverse-engineering":
            stores = repos()
            if stores is None:
                return entry
            paths = [(f"aidlc/spaces/default/codekb/{repo}/{filename}", None, None) for repo in stores]
        # `isPerUnitStage`: by the marker, or by being one of the five stages that always run per unit.
        elif (stage.get("for_each") == "unit-of-work" or stage["slug"] in PER_UNIT_STAGES) and not stage_level():
            resolved = units(stage["slug"])
            if resolved is None:
                return entry
            paths = [(f"{record}/construction/{unit}/{stage['slug']}/{filename}", unit, kind) for unit, kind in resolved
                     if applies(stage, entry["artifact"], kind)]
        else:
            paths = [(f"{record}/{stage['phase']}/{stage['slug']}/{filename}", None, None)]
        prints = []
        for path, unit, kind in paths:
            full = tree.path(path)
            if not os.path.lexists(full):
                digest, present = "missing", False
            elif not os.path.isfile(full):
                digest, present = "not-a-file", False
            else:
                with open(full, "rb") as handle:
                    digest, present = sha(committed_text(handle.read())), True
            prints.append((say(path), unit, kind, digest, present))
        prints = [print_ for print_ in prints if entry.get("required", True) or print_[4]]
        prints.sort(key=lambda print_: root_key((print_[1] or "") + (print_[2] or "") + print_[0]))
        structure = sha(canonical([{"path": path, "unit": unit, "unitKind": kind} for path, unit, kind, _, _ in prints]))
        content = sha(canonical([{"path": path, "sha256": digest, "present": present} for path, _, _, digest, present in prints]))
        if entry.get("structureHash") == structure and entry.get("contentHash") == content and entry.get("instanceCount") == len(prints):
            return {**entry, "structureHash": "<verified>", "contentHash": "<verified>"}
        return entry

    def rewrite(found):
        basis = json.loads(found.group(1))
        for side in ("inputs", "outputs"):
            basis[side] = [verify(entry) for entry in basis.get(side, [])]
        return "**Validation Basis**: " + canonical(basis)

    return re.sub(r"^\*\*Validation Basis\*\*: (\{.*\})$", rewrite, text, flags=re.M), True


def compare_audit(name, trees, steps, env, tool="audit"):
    ts, rs = trees
    outcome, shaped, shaped_ask, shaped_basis = {}, False, False, False
    shaped_authorization = False
    shaped_offsets = shaped_thrown = False
    started_from = None
    if RECORD and not has_links(ts.root) and name not in UNRECORDED and not name.startswith("fuzz/"):
        # An audit check starts from nothing but its files; a runtime check from an installed project, kept whole
        # where the verb reads the whole project (the utility's scan, and the source a baseline lists).
        started_from = workspace_files(ts, "") if tool == "audit" else overlay(ts, hidden=True, everything=tool in WHOLE_PROJECT)
        # A file whose time a check set by hand: the replay sets it again.
        times = {} if tool == "audit" else hand_set_times(ts, started_from, everything=tool in WHOLE_PROJECT)
    for engine, tree in (("ts", ts), ("rs", rs)):
        say = to_focus if engine == "ts" else (lambda text: text)
        before = workspace_files(tree)
        tree.source_before = source_hashes(tree)
        exits, outs, errs = [], [], []
        for args in steps:
            # A step that names its own tool (`@state advance …`) runs that one.
            named = bool(args) and args[0].startswith("@")
            code, out, err = run_audit(engine, tree, args[1:] if named else args, env, args[0][1:] if named else tool)
            if engine == "ts" and tool == "hook" and args[:1] == ["guard-tool-call"] and code == 1:
                err, thrown = group_thrown(err)
                shaped_thrown |= thrown
            if engine == "ts" and code != 0:
                sentence = uncaught(err)
                if sentence is not None:
                    err = json.dumps({"error": sentence}, ensure_ascii=False, separators=(",", ":")) + "\n"
                    shaped = True
                err, asked = without_guard_ask(err)
                shaped_ask |= asked
            exits.append(str(code))
            outs.append(say(normalise_audit(out, tree, engine)))
            errs.append(say(normalise_audit(err, tree, engine)))
        mark = (lambda parts: parts[0]) if len(steps) == 1 else (
            lambda parts: "".join(f"[{index}] {part}" if part.endswith("\n") or not part else f"[{index}] {part}\n"
                                  for index, part in enumerate(parts)))
        outcome[engine] = {"exit": ",".join(exits), "stdout": mark(outs), "stderr": mark(errs)}
        after = workspace_files(tree)
        if engine == "ts" and name.startswith("fuzz/runtime"):
            seen_in_graphs(after)
        for rel in sorted(set(before) | set(after)):
            if before.get(rel) == after.get(rel):
                continue
            text = after.get(rel)
            # The guard-recovery streak is the recovery machinery's (SHAPES).
            if "/.focus-engine/guard-refusals/" in rel and engine == "ts":
                shaped_ask = True
                continue
            if text is not None and "/audit/" in rel:
                text, seen = verify_validation_bases(tree, engine, rel, text)
                shaped_basis |= seen and engine == "ts"
            # The gate-words record indexes the shard by byte offset, and upstream's shard header is one byte
            # longer than Focus's (SHAPES: gate-words-offset).
            if text is not None and "/.focus-engine/gate-words/" in rel and engine == "ts":
                shaped_offsets = True
                text = re.sub(r'"(offset|dropped)":([1-9]\d*)', lambda m: f'"{m.group(1)}":{int(m.group(2)) - 1}', text)
            shown, text = minted(rel, text if text is None else normalise_audit(text, tree, engine))
            if text is not None and "**Source Baseline**: sha256:" in text:
                text = verified_baselines(tree, text)
            if text is not None and engine == "ts" and ASK_FIELD.search(text):
                shaped_ask = True
                text = ASK_FIELD.sub("", text)
            outcome[engine][shown] = "<removed>" if text is None else text
        # A summary's authorization id is taken over the questions file's name, which carries the workspace
        # directory's (SHAPES: summary-authorization).
        own_ids = authorization_ids(tree)
        if own_ids:
            shaped_authorization |= engine == "ts"
            for key, text in outcome[engine].items():
                outcome[engine][key] = AUTHORIZATION_ID.sub(lambda m: "<verified>" if m.group(0) in own_ids else m.group(0), text)
    for engine in ("ts", "rs"):
        outcome[engine] = placeholder_messages(outcome[engine])
    problems = []
    shaped_budget = False
    for key in sorted(set(outcome["ts"]) | set(outcome["rs"])):
        left, right = outcome["ts"].get(key, "<absent>"), outcome["rs"].get(key, "<absent>")
        if left != right:
            unreached = ("<absent>", '{"sessionId":null,"startTime":null}\n')
            if "/.focus-sessions/pids/" in key and (left in unreached or right in unreached):
                shaped_budget = True
                continue
            problems.append(f"{key}: {first_difference(left, right)}")
    if not problems:
        verdict = "same"
        said = (outcome["ts"]["stderr"] or outcome["ts"]["stdout"]).strip().split("\n")[0][:110]
        detail = f"exit {outcome['ts']['exit'][:40]}" + (f": {said}" if said else "")
        if SHOW:
            detail += "\n" + "\n".join(f"           | {line}" for line in outcome["ts"]["stdout"].split("\n"))
            detail += "\n           wrote: " + ", ".join(sorted(key for key in outcome["ts"] if "/" in key))
        if shaped:
            shapes["uncaught"] += 1
            detail += "  (upstream: uncaught)"
        if shaped_ask:
            shapes["guard-ask"] += 1
            detail += "  (guard ask left out)"
        if shaped_basis:
            shapes["validation-basis"] += 1
            detail += "  (receipt hashes verified)"
        if shaped_authorization:
            shapes["summary-authorization"] += 1
            detail += "  (authorization id verified)"
        if shaped_budget:
            shapes["process-map-budget"] += 1
            detail += "  (a process-map slot missed its budget)"
        if shaped_thrown:
            shapes["group-thrown"] += 1
            detail += "  (a member's stack trace read as its sentence)"
        if shaped_offsets:
            shapes["gate-words-offset"] += 1
            detail += "  (shard offsets read one byte back)"
    elif any(re.fullmatch(pattern, name) for pattern in DEVIATIONS):
        verdict = "deviation"
        detail = next(reason for pattern, reason in DEVIATIONS.items() if re.fullmatch(pattern, name))
        if EXPLAIN:
            detail += "\n           " + "\n           ".join(problems)
    else:
        verdict, detail = "DIFFERENT", "\n           ".join(problems)
    if started_from is not None:
        import hashlib
        if tool == "audit":
            started_from = {dated(rel): text for rel, text in started_from.items()}
            key = hashlib.sha256(json.dumps(started_from, sort_keys=True).encode()).hexdigest()[:16]
            RECORDED_TREES[key] = started_from
        else:
            key = started_from
        RECORDED_CHECKS[name] = {
            "name": name, "tool": tool, "base": "empty" if tool == "audit" else "installed", "tree": key,
            "steps": [[to_focus(arg) for arg in args] for args in steps], "args": [],
            "env": {key_: to_focus(value) for key_, value in (env or {}).items()},
            "stdin": None, "writes": True,
            **({"times": times} if times else {}),
            "deviation": detail if verdict == "deviation" else None,
            # The process map names the host's own processes (the engines' ancestors): the same for both engines
            # live, and nothing a replay under another process tree could reproduce.
            "expect": {key_: compact(value) for key_, value in outcome["ts"].items() if "/pids/" not in key_},
        }
    return verdict, name, detail


def block(event, at, *fields):
    return "\n".join(["", f"## {event.title()}", f"**Timestamp**: {at}", f"**Event**: {event}", *fields, ""]) + "\n---\n"


def note(heading, at, *lines):
    return "\n".join(["", f"## {heading}", f"**Timestamp**: {at}", *lines, ""]) + "\n---\n"


TITLE = "# AI-DLC Audit Log\n"
CLONE = {"aidlc/.aidlc-clone-id": "abc123\nlaptop\n"}
RECORD_FILES = {**CLONE, f"{RECORD_DIR}/aidlc-state.md": "# State\n",
                "aidlc/spaces/default/intents/active-intent": "add-login-1a2b3c4d\n"}
MINE = TITLE + block("WORKFLOW_STARTED", "2026-01-05T10:00:00Z", "**Workflow**: feature") + block(
    "STAGE_STARTED", "2026-01-05T10:00:05Z", "**Stage**: requirements-analysis", "- **Note**: bulleted",
    "**Stage**: second") + note("A decision", "2026-01-05T10:00:06Z", "We chose the", "", "second option.") + block(
    "STAGE_COMPLETED", "2026-01-05T10:01:00Z", "**Stage**: requirements-analysis", "**7**: seven", "**Empty**:")
THEIRS = TITLE + block("STAGE_STARTED", "2026-01-05T10:00:05Z", "**Stage**: code-generation") + block(
    "ERROR_LOGGED", "2026-01-05T10:00:30Z", "**Tool**: aidlc-state", "**Stage**: code-generation") + block(
    "GATE_APPROVED", "2026-01-05T10:02:00Z", "**Stage**: requirements-analysis")
HISTORY_FILES = {**RECORD_FILES, f"{RECORD_DIR}/audit/laptop-abc123.md": MINE,
                 f"{RECORD_DIR}/audit/desktop-ffffffffffff.md": THEIRS,
                 # A sync tool's conflict copy of this clone's shard, one row longer.
                 f"{RECORD_DIR}/audit/laptop-abc123 2.md": MINE + block("STAGE_SKIPPED", "2026-01-05T10:03:00Z"),
                 f"{RECORD_DIR}/audit/README.txt": "not a shard\n"}
EVENT_TYPES = re.findall(r'"([A-Z_]+)"', open(AUDIT_TOOL, encoding="utf-8").read()
                         .split("const VALID_EVENT_TYPES = new Set([")[1].split("]);")[0])
APPEND = ["append", "ERROR_LOGGED", "--field", "Stage=code-generation"]
BATCH = lambda *entries: ["append-batch", json.dumps(list(entries))]
ROW = lambda event, **fields: {"eventType": event, "fields": fields}

DEVIATIONS.update({
    r"audit/usage/(none|unknown)":
        "upstream's usage lists audit-fork and audit-merge; focus lists the verbs it has",
    r"audit/usage/audit_(fork|merge)":
        "forking and merging a bolt's ledger come with bolts and worktrees, which focus has not ported; it "
        "refuses both verbs by name",
    r"audit/append/field_(without_equals|empty_key|dangling)":
        "upstream drops a --field it cannot split into key=value and appends without it; focus refuses, because "
        "a field dropped from the ledger leaves nothing to say it was asked for",
    r"audit/append/(stray_argument|flag_as_type)":
        "upstream ignores arguments it does not know; focus refuses them",
    r"audit/append_(batch|raw)/stray_argument":
        "upstream ignores arguments it does not know; focus refuses them",
    "audit/append/audit_is_file":
        "with a file where the audit directory goes, upstream surfaces the runtime's error for the open; focus "
        "says which shard could not be opened",
    "audit/append/metrics_endpoint":
        "upstream sends a metric per entry to the configured endpoint; focus has no metrics yet and refuses the "
        "append instead of leaving the endpoint without what it expects",
    "audit/append/unit_claim":
        "upstream ignores a unit claim that does not apply and otherwise lets it name the shard; focus does not "
        "work with units yet and refuses while the claim file exists",
    "audit/append/delegated_worktree":
        "upstream resolves a delegated worktree to the main checkout's intent; focus does not work with "
        "worktrees yet and refuses while the marker exists",
    "verbs/resolve_writes/registry_unreadable_row":
        "with a registry row it cannot read, upstream fails with what its runtime says about the value; focus "
        "says which file and what a row needs. Both fail before printing, --stdout or not",
})


def audit_checks():
    check_ = audit_check
    # Usage.
    check_("usage/none", {}, [[]])
    check_("usage/unknown", {}, [["nope"]])
    check_("usage/audit_fork", RECORD_FILES, [["audit-fork", "--slug", "one"]])
    check_("usage/audit_merge", RECORD_FILES, [["audit-merge", "--slug", "one"]])
    check_("usage/append", {}, [["append"]])
    check_("usage/append_empty_type", {}, [["append", ""]])
    check_("usage/append_batch", {}, [["append-batch"]])
    check_("usage/append_raw", {}, [["append-raw"]])
    check_("usage/append_raw_no_body", {}, [["append-raw", "Heading"]])
    check_("usage/append_raw_empty_body", {}, [["append-raw", "Heading", ""]])

    # An observer reads and never appends; what it may still do is mint this clone's identity.
    for marker in ("STOP_HOOK_PROBE", "ROUTE_CHECK"):
        probe = {marker: "1"}
        check_(f"observer/{marker.lower()}/append", RECORD_FILES, [APPEND, ["history"]], probe)
        check_(f"observer/{marker.lower()}/append_batch", RECORD_FILES,
               [BATCH(ROW("ERROR_LOGGED", Stage="x")), ["history"]], probe)
        check_(f"observer/{marker.lower()}/append_raw", RECORD_FILES, [["append-raw", "Heading", "body"]], probe)
        check_(f"observer/{marker.lower()}/history", HISTORY_FILES, [["history"]], probe)
        check_(f"observer/{marker.lower()}/mints_identity", {}, [APPEND, APPEND], probe)
        check_(f"observer/{marker.lower()}/completes_identity",
               {**RECORD_FILES, "aidlc/.aidlc-clone-id": "abc123\n", f"{RECORD_DIR}/audit/old-name-abc123.md": MINE},
               [APPEND], probe)
        check_(f"observer/{marker.lower()}/refused_first", RECORD_FILES, [["append", "HUMAN_TURN"]], probe)
    check_("observer/not_one/append", RECORD_FILES, [APPEND], {"STOP_HOOK_PROBE": "yes", "ROUTE_CHECK": ""})

    # append: where an entry goes.
    check_("append/no_intent", CLONE, [APPEND + ["--field", "Note=a b"], ["history"]])
    check_("append/active_intent", RECORD_FILES, [APPEND, ["history"]])
    check_("append/mints_identity", {}, [APPEND, APPEND])
    check_("append/mints_identity_in_record", {k: v for k, v in RECORD_FILES.items() if k not in CLONE}, [APPEND])
    check_("append/completes_identity",
           {**RECORD_FILES, "aidlc/.aidlc-clone-id": "abc123\n", f"{RECORD_DIR}/audit/old-name-abc123.md": MINE},
           [APPEND])
    check_("append/existing_shard", HISTORY_FILES, [APPEND, ["history", "--limit", "2"]])
    check_("append/session_binding",
           {**RECORD_FILES, "aidlc/spaces/default/intents/fix-logout-9f8e7d6c/aidlc-state.md": "# State\n",
            "aidlc/.aidlc-sessions/s1.binding.json": json.dumps(
                {"space": "default", "intent": "fix-logout-9f8e7d6c", "boundAt": "2026-01-05T10:00:00Z",
                 "source": "switch"})},
           [APPEND], env={"SESSION_OVERRIDE": "s1"})
    check_("append/other_space",
           {**CLONE, "aidlc/active-space": "payments\n",
            "aidlc/spaces/payments/intents/x/aidlc-state.md": "# State\n"}, [APPEND])

    # append: what is refused, and what a field becomes.
    check_("append/every_type", RECORD_FILES, [["append", event, "--field", "Stage=x"] for event in EVENT_TYPES])
    check_("append/every_type_allowed", RECORD_FILES,
           [["append", event, "--field", "Stage=x"] for event in EVENT_TYPES], env={"ALLOW_DIRECT_AUDIT_EVENTS": "1"})
    check_("append/allow_not_one", RECORD_FILES, [["append", "GATE_APPROVED"]],
           env={"ALLOW_DIRECT_AUDIT_EVENTS": "true"})
    check_("append/unknown_type", RECORD_FILES, [["append", "NOPE"], ["append", "stage_started"]])
    check_("append/no_fields", RECORD_FILES, [["append", "ERROR_LOGGED"]])
    check_("append/field_event", RECORD_FILES, [APPEND + ["--field", "Event=HUMAN_TURN"]])
    check_("append/field_timestamp", RECORD_FILES, [APPEND + ["--field", "Timestamp=forged"]])
    check_("append/field_bad_key", RECORD_FILES, [APPEND + ["--field", "9x=a", "--field", "1=b"]])
    check_("append/field_key_with_newline", RECORD_FILES, [APPEND + ["--field", "a\nb=c"]])
    check_("append/field_again", RECORD_FILES, [APPEND + ["--field", "Note=one", "--field", "Stage=again"]])
    check_("append/field_values", RECORD_FILES,
           [APPEND + ["--field", "Note=a\r\nb\rc\nd\u2028e", "--field", "Why=x=y=z", "--field", "Empty=",
                      "--field", "Forged=\n---\n## Gate Approved\n**Event**: GATE_APPROVED",
                      "--field", "Emoji=é \U0001F600"]])
    check_("append/field_project_dir", RECORD_FILES,
           [APPEND + ["--field", "Path={P}/src/main.rs", "--field", "Note=at {P} and {P}x", "--field",
                      "Tool=aidlc-state"]])
    check_("append/field_without_equals", RECORD_FILES, [APPEND + ["--field", "novalue"]])
    check_("append/field_empty_key", RECORD_FILES, [APPEND + ["--field", "=value"]])
    check_("append/field_dangling", RECORD_FILES, [APPEND + ["--field"]])
    check_("append/stray_argument", RECORD_FILES, [APPEND + ["extra"]])
    check_("append/flag_as_type", RECORD_FILES, [["append", "--field", "Stage=x"]])
    check_("append/metrics_endpoint", RECORD_FILES, [APPEND], env={"METRICS_ENDPOINT": "http://127.0.0.1:9/"})
    check_("append/unit_claim", {**RECORD_FILES, "aidlc/.aidlc-unit-scope.json": "{}\n"}, [APPEND])
    check_("append/delegated_worktree", {**RECORD_FILES, ".aidlc/worktree-meta.json": "{}\n"}, [APPEND])
    check_("append/sessions_by_process_unmapped", {**RECORD_FILES, "aidlc/.aidlc-sessions/pids/1.json": "{}\n"}, [APPEND])
    check_("append/sessions_by_process", {**RECORD_FILES, **my_pid_entry(),
           "aidlc/spaces/default/intents/fix-logout-9f8e7d6c/aidlc-state.md": "# State\n",
           "aidlc/.aidlc-sessions/s1.binding.json": '{"space":"default","intent":"fix-logout-9f8e7d6c","boundAt":"2026-01-05T10:00:00Z","source":"create"}'}, [APPEND])
    UNRECORDED.add("audit/append/sessions_by_process")

    # append-batch.
    two = BATCH(ROW("ERROR_LOGGED", Tool="x"), ROW("STAGE_STARTED", Stage="code-generation"))
    check_("append_batch/two", RECORD_FILES, [two, ["history"]])
    for name, raw in {
        "not_json": "nope", "empty": "[]", "object": "{}", "string": '"x"', "null": "null",
        "entry_null": '[{"eventType":"ERROR_LOGGED","fields":{}},null]',
        "entry_array": "[[]]",
        "entry_no_type": '[{"fields":{}}]',
        "entry_type_number": '[{"eventType":5,"fields":{}}]',
        "entry_no_fields": '[{"eventType":"ERROR_LOGGED"}]',
        "entry_fields_array": '[{"eventType":"ERROR_LOGGED","fields":[]}]',
        "entry_fields_null": '[{"eventType":"ERROR_LOGGED","fields":null}]',
        "value_number": '[{"eventType":"ERROR_LOGGED","fields":{"a":"b"}},{"eventType":"ERROR_LOGGED","fields":{"a":1}}]',
        "value_null": '[{"eventType":"ERROR_LOGGED","fields":{"a":null}}]',
        "unknown_type_second": '[{"eventType":"ERROR_LOGGED","fields":{}},{"eventType":"NOPE","fields":{}}]',
        "reserved_key_second": '[{"eventType":"ERROR_LOGGED","fields":{}},{"eventType":"ERROR_LOGGED","fields":{"Event":"X"}}]',
        "reserved_type": '[{"eventType":"ERROR_LOGGED","fields":{}},{"eventType":"HUMAN_TURN","fields":{}}]',
        "reserved_type_in_malformed_batch": '[{"eventType":"ERROR_LOGGED"},{"eventType":"HUMAN_TURN"}]',
        "protected_type": '[{"eventType":"ERROR_LOGGED","fields":{}},{"eventType":"GATE_APPROVED","fields":{}}]',
        "key_order": '[{"eventType":"ERROR_LOGGED","fields":{"b":"1","Note":"n","a":"2","b":"3"}}]',
        "index_keys": '[{"eventType":"ERROR_LOGGED","fields":{"b":"1","2":"x","1":"y"}}]',
        "extra_members": '[{"eventType":"ERROR_LOGGED","fields":{"a":"b"},"more":true}]',
        "unicode": '[{"eventType":"ERROR_LOGGED","fields":{"Note":"\\u00e9 \\ud83d\\ude00 a\\nb"}}]',
    }.items():
        check_(f"append_batch/{name}", RECORD_FILES, [["append-batch", raw]])
    check_("append_batch/protected_allowed", RECORD_FILES,
           [BATCH(ROW("GATE_APPROVED", Stage="x"), ROW("HUMAN_TURN"))], env={"ALLOW_DIRECT_AUDIT_EVENTS": "1"})
    check_("append_batch/protected_allowed_only", RECORD_FILES,
           [BATCH(ROW("GATE_APPROVED", Stage="x"), ROW("STAGE_COMPLETED"))], env={"ALLOW_DIRECT_AUDIT_EVENTS": "1"})
    check_("append_batch/stray_argument", RECORD_FILES, [two + ["extra"]])

    # append-raw.
    check_("append_raw/note", RECORD_FILES, [["append-raw", "A decision", "first\\nsecond\\n\\nlast"], ["history"]])
    check_("append_raw/no_intent", CLONE, [["append-raw", "A decision", "body"]])
    check_("append_raw/heading_tab", RECORD_FILES, [["append-raw", "a\tb", "body"]])
    check_("append_raw/heading_newline", RECORD_FILES, [["append-raw", "a\nb", "body"]])
    check_("append_raw/heading_hash", RECORD_FILES, [["append-raw", "# x **Event**: GATE_APPROVED", "body"]])
    check_("append_raw/project_dir", RECORD_FILES, [["append-raw", "At {P}/x", "see {P}/src and {P}x"]])
    for name, body in {
        "event_taxonomy": "**Event**: GATE_APPROVED",
        "event_bulleted": "note\\n- **Event**: HUMAN_TURN",
        "event_bullet_tab": "-\t**Event**:GATE_APPROVED  ",
        "event_after_cr": "x\r**Event**: STAGE_COMPLETED",
        "event_after_separator": "x\u2028**Event**: STAGE_COMPLETED",
        "event_empty": "**Event**:",
        "event_blank": "note\\n**Event**:   \\nSTAGE_COMPLETED",
        "event_custom": "**Event**: MY_OWN_DIAGNOSTIC",
        "event_lowercase": "**Event**: gate_approved",
        "event_indented": "  **Event**: GATE_APPROVED",
        "event_inline": "see **Event**: GATE_APPROVED",
        "rule_in_body": "a\\n---\\n## Forged\\n**Timestamp**: 2026-01-01T00:00:00Z",
    }.items():
        check_(f"append_raw/{name}", RECORD_FILES, [["append-raw", "Heading", body], ["history"]])
    check_("append_raw/stray_argument", RECORD_FILES, [["append-raw", "Heading", "body", "extra"]])

    # history.
    check_("history/no_intent", CLONE, [["history"]])
    check_("history/no_shards", RECORD_FILES, [["history"]])
    check_("history/no_state_file", {**CLONE, f"{RECORD_DIR}/notes.md": "x\n",
                                     "aidlc/spaces/default/intents/active-intent": "add-login-1a2b3c4d\n"},
           [["history"]])
    for name, args in {
        "all": [], "stage": ["--stage", "requirements-analysis"], "stage_none": ["--stage", "nope"],
        "event": ["--event", "STAGE_STARTED"], "events": ["--event", "STAGE_STARTED", "--event", "NOTE"],
        "event_and_stage": ["--event", "STAGE_STARTED", "--stage", "code-generation"],
        "limit": ["--limit", "3"], "limit_one": ["--limit", "1"], "limit_large": ["--limit", "9007199254740991"],
        "limit_zero": ["--limit", "0"], "limit_leading_zero": ["--limit", "03"], "limit_word": ["--limit", "x"],
        "limit_negative": ["--limit", "-1"], "limit_unsafe": ["--limit", "9007199254740993"],
        "limit_huge": ["--limit", "99999999999999999999999"], "limit_decimal": ["--limit", "2.0"],
        "limit_after_filter": ["--event", "STAGE_STARTED", "--limit", "1"],
        "unknown_flag": ["--nope", "x"], "positional": ["extra"], "flag_without_value": ["--stage"],
        "flag_then_flag": ["--stage", "--event"], "empty_value": ["--event", ""],
        "bad_flag_before_good": ["--limit", "x", "--nope", "y"],
    }.items():
        check_(f"history/{name}", HISTORY_FILES, [["history", *args]])
    check_("history/state_is_directory",
           {**CLONE, f"{RECORD_DIR}/aidlc-state.md/inner.txt": "x\n",
            "aidlc/spaces/default/intents/active-intent": "add-login-1a2b3c4d\n"}, [["history"], APPEND])
    check_("history/crlf_shard", {**RECORD_FILES, f"{RECORD_DIR}/audit/laptop-abc123.md": MINE.replace("\n", "\r\n")},
           [["history"]])
    check_("history/shard_is_directory", {**HISTORY_FILES, f"{RECORD_DIR}/audit/folder.md/inner.txt": "x\n"},
           [["history"]])
    check_("history/audit_is_file", {**RECORD_FILES, f"{RECORD_DIR}/audit": "a file\n"}, [["history"]])
    check_("append/audit_is_file", {**RECORD_FILES, f"{RECORD_DIR}/audit": "a file\n"}, [APPEND])
    check_("history/space_shard_not_read",
           {**HISTORY_FILES, "aidlc/spaces/default/intents/audit/laptop-abc123.md": TITLE + block(
               "WORKSPACE_SCANNED", "2026-01-04T09:00:00Z")}, [["history"]])
    check_("history/after_writes", RECORD_FILES,
           [APPEND, ["append-raw", "Why", "because"], BATCH(ROW("STAGE_STARTED", Stage="a"), ROW("STAGE_SKIPPED")),
            ["history"], ["history", "--stage", "a"], ["history", "--event", "NOTE"]])

    # Links: compared live, never recorded.
    elsewhere = {**RECORD_FILES, "elsewhere/laptop-abc123.md": MINE, "elsewhere.md": MINE, "state.md": "# State\n"}
    check_("links/audit_dir", elsewhere, [["history"], APPEND], links=[(f"{RECORD_DIR}/audit", "elsewhere")])
    check_("links/shard", elsewhere, [["history"], APPEND],
           links=[(f"{RECORD_DIR}/audit/laptop-abc123.md", "elsewhere.md")])
    check_("links/other_shard", elsewhere, [["history"], APPEND],
           links=[(f"{RECORD_DIR}/audit/desktop-ffffffffffff.md", "elsewhere.md")])
    check_("links/state_file", {k: v for k, v in elsewhere.items() if not k.endswith("aidlc-state.md")},
           [["history"], APPEND], links=[(f"{RECORD_DIR}/aidlc-state.md", "state.md")])
    check_("links/project_dir_above", elsewhere, [APPEND, ["history"]],
           links=[("aidlc/spaces/default/intents/linked", "elsewhere")])
    check_("links/state_hard_linked", RECORD_FILES, [["history"], APPEND],
           hard_links=[("backup-of-state.md", f"{RECORD_DIR}/aidlc-state.md")])
    check_("links/shard_hard_linked", HISTORY_FILES, [["history", "--limit", "1"], APPEND],
           hard_links=[("backup-of-shard.md", f"{RECORD_DIR}/audit/laptop-abc123.md")])


# --------------------------------------------------------------------------
# The runtime command.
#
# A check is an installed project (the stage graph is the harness's own) with
# the files the check names laid over it, and one or more commands run on it
# in order, compared like an audit check: what each command said, and every
# file of the workspace it created, changed or removed.
# --------------------------------------------------------------------------

STATE = "# AI-DLC State\n\n- **Scope**: feature\n- **Status**: In Progress\n"
T = lambda minute, second=0: f"2026-01-05T10:{minute:02d}:{second:02d}Z"
STARTED_ROW = lambda minute, stage, *fields: block("STAGE_STARTED", T(minute), f"**Stage**: {stage}", *fields)
COMPLETED_ROW = lambda minute, stage, *fields: block("STAGE_COMPLETED", T(minute), f"**Stage**: {stage}", *fields)
WORKFLOW = block("WORKFLOW_STARTED", T(0), "**Scope**: bugfix")
SHARD = f"{RECORD_DIR}/audit/laptop-abc123.md"
DIARY = lambda phase, stage: f"{RECORD_DIR}/{phase}/{stage}/memory.md"
A_DIARY = ("# Memory\n\n## Interpretations\n- 2026-01-05T10:01:00Z — read it as X; because Y\n\n## Deviations\n\n"
           "## Tradeoffs\n- one\n- two\n\n## Open questions\n> none yet\n")
UNITS = f"{RECORD_DIR}/inception/units-generation/unit-of-work-dependency.md"
A_UNITS = ("# Dependencies\n\n```yaml\nunits:\n  - name: api\n    depends_on: [auth]\n  - name: auth\n    kind: service\n"
           "    depends_on: []\n  - name: web\n    depends_on:\n      - api\n```\n")
FIRED = lambda minute, second, stage, fire, sensor="linter", *fields: block(
    "SENSOR_FIRED", T(minute, second), f"**Stage slug**: {stage}", f"**Sensor ID**: {sensor}", f"**Fire id**: {fire}",
    *fields)
ENDED = lambda kind, minute, second, fire, *fields: block(f"SENSOR_{kind}", T(minute, second), f"**Fire id**: {fire}",
                                                          *fields)
FORKED = lambda minute, bolt, *fields: block("STATE_FORKED", T(minute), f"**Bolt slug**: {bolt}", *fields)
MERGED = lambda minute, bolt: block("STATE_MERGED", T(minute), f"**Bolt slug**: {bolt}")
A_WORKFLOW = (TITLE + WORKFLOW + STARTED_ROW(1, "intent-capture", "**Agent**: someone-else") +
              COMPLETED_ROW(5, "intent-capture") + STARTED_ROW(6, "requirements-analysis") +
              COMPLETED_ROW(9, "requirements-analysis") + STARTED_ROW(10, "code-generation"))


def runtime_check(name, files, steps, env=None, edits=(), group="runtime", tool=None):
    """Queue one runtime check: `steps` run in order on an installed project holding `files`. The group names the
    tool, unless `tool` does."""
    name = f"{group}/{name}"
    if ONLY and not re.search(ONLY, name):
        return
    def lay(tree):
        for rel, text in files.items():
            if text is None:
                os.remove(tree.path(rel)) if os.path.exists(tree.path(rel)) else None
            else:
                tree.write(rel, text)
    # The tool is run from the tree's own installation.
    JOBS.append((name, pair(name, lay, *edits), steps, env, None, tool or group))


DEVIATIONS.update({
    r"state/usage/[a-z-]+_not_ported":
        "upstream passes the verb through the state-transition fence (standing aside, with a GUARD_STOOD_ASIDE row) "
        "and then runs it or prints its usage; focus refuses the verb by name, with no fence consulted, since it "
        "does not have it yet",
    r"state/usage/(none|unknown)":
        "upstream's usage lists its 33 verbs, Focus's the thirteen it has",
    r"state/usage/flag_without_its_value":
        "upstream takes a trailing --project-dir with no value as part of the field; focus refuses the flag",
    r"state/usage/metrics_endpoint":
        "upstream sends a metric for the ERROR_LOGGED row to the configured endpoint; focus has no metrics yet "
        "and writes no row rather than leave the endpoint without what it expects (the refusal is printed either way)",
    r"state/marker/active_directive.*":
        "a record carrying an active-directive marker: upstream reads the engine's instruction and the turn "
        "sequence off it and keeps the state-write record beside it; the orchestrator that writes it is M5, so "
        "focus refuses to decide a fence with the marker there, and refuses to write the state beside it",
    r"state/record/delegated_worktree":
        "upstream walks the process tree to find which conversation is calling, and resolves a delegated worktree to "
        "the main checkout's intent; focus refuses and asks for the session",
    r"state/advance/(summary_owed|review_owed|review_owed_advisory_cap|pipeline_owed)":
        "the stage owes evidence that is an aidlc-log record (a summary confirmation, a reviewer's receipt, a "
        "pipeline's handoffs) compared with the stage's outputs and the source the review saw (the attestation, "
        "M5): upstream weighs it and refuses with what is missing; focus refuses by name until it reads those "
        "records. With the ceremony off, the review class none, or the kill switch set, neither owes it and the "
        "transition runs whole (the checks beside this one)",
    "state/advance/review_rows_in_ledger":
        "a closed stage's recovery reads the review rows of its attempt (which the stage's STAGE_STARTED floors); "
        "upstream finds the one here stale and asks for a fresh review; focus refuses to weigh review rows at all",
    "state/advance/construction":
        "a Construction stage runs per unit of work with checkpoints, unit gates and swarms (M4): upstream admits "
        "it with no unit DAG as a stage-level one; focus refuses the phase by name",
    r"state/(advance/usage_ledger|skip/route_usage_ledger)":
        "a completion row carries the token usage the harness hooks folded into the usage ledger; upstream reads "
        "the ledger and adds the rollup fields; focus does not read it yet and refuses the completion while the "
        "ledger holds usage for it, rather than write the row without",
    "state/skip/unit":
        "skip --unit skips one unit of work's step, which needs the unit DAG and the units' lifecycle (M4); "
        "upstream checks the Construction policy first and refuses on that; focus refuses the flag by name",
    "state/park/team_units":
        "with Unit Ownership team, park and unpark work on the unit this checkout claimed, read from its unit "
        "scope stamp and checked against the claim registry and the fan-out (M4): focus refuses both by name",
    "jump/usage/flag_without_its_value":
        "upstream takes a trailing --project-dir with no value as an argument of the verb; focus refuses the flag",
    "jump/execute/units":
        "execute --units moves units of work forward one at a time to a per-unit stage, with the state tool's "
        "one-unit skip (M4): upstream runs it or refuses on its own shape; focus refuses the flag by name",
    "jump/execute/directive_marker":
        "a record carrying an active-directive marker: a jump reads the step the person was on off it (M5); "
        "focus refuses to jump with the marker there",
    "jump/execute/backward_session":
        "with a session bound, a backward jump keeps its way back for the chat's next turn (the pending person "
        "lines, the human-turn hook's record, M4) and leaves `notice` out of its output; focus refuses by name",
    "jump/execute/backward_over_reviewed_stage_rows":
        "a backward jump names the reviews it invalidates, read from the review brief's records (M5): with review "
        "rows on the ledger for a reviewed stage behind the jump, upstream reads them and focus refuses by name; "
        "an embedded `## Review` section is named by both",
    r"(jump/execute|utility/create)/delegated_worktree":
        "upstream runs inside a worktree an engine created for one unit of work, resolving it to the main checkout's "
        "piece of work; focus refuses the checkout by name (worktrees come with parallel Construction, deferred)",
    "jump/execute/source_repos_delegated_worktree":
        "a member repository that is a delegated worktree is read through that worktree's own boundary (the code "
        "paths of its unit of work); focus does not read that boundary yet and refuses the row whose baseline it "
        "would be",
    r"(state/(advance/repos|gates/revision_backstop_repos)|log/link/repos)_(not_a_name|null_name|a_list_inside|not_a_list)":
        "a registry row whose `repos` is not a list of text: upstream takes what is there, so a reader that joins a "
        "path with a name falls over with the runtime's TypeError (the stores of the code knowledge base), and one "
        "that only looks names up in the list goes on (the revision backstop) or falls over further on (a link of a "
        "pipeline, where it filters the list); focus refuses the row by name, "
        "whoever reads it",
    r"jump/execute/source_repos_refused_\w+":
        "a registry row whose `repos` is not a list of names: upstream takes what is there and falls over where it "
        "first joins a path, with the runtime's TypeError (or, with an empty name, lists the whole project, engine "
        "and harness included, as that member); focus refuses the row by name (exit 1 either way where upstream "
        "falls over, and the jump's error row written by both)",
    r"log/answer/\w+_not_ported":
        "what an answer may carry that focus has not ported: the engine's own questions (the recovery question, "
        "slice 4; a summary confirmation, 2c; Construction's checkpoints and a unit of work, 5; Plan Approval, 11). "
        "Upstream reads what each needs and records or refuses on its own terms; focus refuses each by name",
    r"log/usage/review_not_ported":
        "the verbs of the log tool that focus has not ported (M5, slices 2b to 2e): upstream runs them; focus "
        "refuses each by name",
    r"log/decision/checkpoint_\w+_not_ported":
        "a question the engine pairs with a record of its own: the verification command and the policy of "
        "Construction (slice 5), Plan Approval (slice 11). Upstream reads "
        "the flags each needs and records it; focus refuses the checkpoint by name",
    r"log/decision/(unit|batch_file)_not_ported":
        "a question about one unit of work, and the Plan Approval questions of several at once: upstream records "
        "the unit with its claim (or refuses the batch on its own shape); focus refuses both flags by name, since "
        "it runs no units yet",
    r"log/decision/summary_checkpoint_(a_link|a_dangling_link|behind_a_link|second_name|a_folder)":
        "the questions file of a summary is hashed into the receipt a gate weighs. Upstream opens it by name, so "
        "it reads through a link, a linked directory or a second hard link, says of a link to nothing that the "
        "file does not exist, and falls over on a directory with the runtime's EISDIR; focus reads it as every "
        "file of the record that decides something, a regular "
        "file with one name reached through no link, and refuses the rest by name",
    "jump/reopen/by_name":
        "reopen starts a per-unit stage again for named units with their own GATE_REJECTED rows (M4); focus "
        "refuses by name once the usage is satisfied",
    r"utility/status/directive_marker(_by_flags)?":
        "a record carrying an active-directive marker: the status reads the step the engine last handed out off "
        "it, and the guards read the authority behind the command off it (M5); focus refuses the status by name "
        "while the marker is there, named by flags or not",
    "utility/status/team":
        "with Unit Ownership team the status ends with the team board, rendered by the orchestrator from the "
        "units' claims (M4): upstream runs it and fails on the DAG it cannot read; focus refuses by name",
    "state/gates/sensors_on":
        "with the Sensors ceremony on and the declared artifacts on disk, opening a gate fires the stage's gate "
        "sensors through the sensor tool (M5): upstream runs them, records their findings and opens the gate; focus "
        "refuses the opening by name and says how to open it without them (`--sensors off`, FOCUS_DISABLE_SENSORS)",
    r"state/gates/(summary_owed|review_owed|review_guard_off|revision_backstop_with_reviewer)":
        "the gate owes evidence that is an aidlc-log record (a summary confirmation, a reviewer's receipt, M5): "
        "upstream weighs it and refuses with what is missing; focus refuses by name until it reads those records "
        "(the kill switch opens the gate in both; upstream still weighs the receipt at the approval)",
    r"state/gates/review_findings_(rows|section)":
        "a review's findings follow the gate's decision (the review brief, M5): upstream reads them off the review "
        "artifact's `## Review` section (and its review rows) and records each open one on the decision as an "
        "accepted risk; focus refuses the decision by name while there are review rows on the ledger or the "
        "section is there, since it does not read findings yet",
    r"state/gates/reject_findings(_with_reviewer)?":
        "--reject-finding and --reopen-finding name a review's finding: upstream looks it up in the review brief "
        "and refuses one that is not current; focus refuses the flags by name",
    "state/gates/unit_team":
        "with Unit Ownership team, a per-unit Construction stage's gate is a unit's gate, which needs the unit DAG "
        "and the units' lifecycle (M4): upstream looks the unit up in the DAG; focus refuses by name",
    "state/gates/approve_usage_ledger":
        "a completion row carries the token usage the harness hooks folded into the usage ledger; upstream reads "
        "the ledger and adds the rollup fields; focus does not read it yet and refuses the approval while the "
        "ledger holds usage for it, rather than write the row without",
    "state/gates/reject_active_directive":
        "a record carrying an active-directive marker: a gate decision reads the engine's instruction off it for "
        "a guard-recovery answer (M5); focus refuses to decide the gate with the marker there",
    r"utility/usage/(none|unknown)":
        "upstream's refusal lists its 34 verbs (and a missing verb is \"undefined\"), Focus's the ones it has",
    r"utility/usage/[a-z-]+_not_ported":
        "upstream runs the verb; focus refuses it by name, since it does not have it yet",
    r"utility/usage/error_(logged|without_record)":
        "the refusal of an unknown verb lists each engine's own verbs; the row that records it is compared up to "
        "that text",
    "utility/create/help":
        "upstream's usage offers --request; focus's lists the flags it has",
    "utility/create/request_flag":
        "an engine question's answer names its request by id; the question store is M5's, so focus refuses the "
        "flag by name where upstream looks the question up",
    "utility/create/flat_layout":
        "the first creation on a project at the flat focus-docs/ layout moves that layout into its own record; "
        "focus does not migrate yet and refuses",
    "utility/create/pending_person_lines":
        "the lines a chat is still owed (the human-turn hook's record, M4) follow the work it creates: upstream "
        "rewrites the file for the new record, focus leaves it where it is",
    r"utility/create/(creation_grant|presence_bypass)_on_disk":
        "what the person said before the work existed, and a recorded presence bypass, are the human-turn hook's "
        "records (M4); upstream spends them, focus refuses when one is there for the session",
    "utility/detect/project_dir_without_install":
        "upstream scans the directory named and reports the scopes of the install its own script runs from; focus "
        "runs from nowhere, and refuses a project that does not hold the install",
    "runtime/usage/none":
        "upstream's usage lists fragment-fork and fragment-merge; focus lists the verbs it has",
    r"runtime/usage/fragment_(fork|merge)":
        "copying the runtime graph into a bolt's worktree comes with bolts and worktrees, which focus has not "
        "ported; it refuses both verbs by name",
    r"runtime/usage/(stray_argument|flag_twice|flag_without_its_value)":
        "upstream ignores what it does not use (an argument it does not know, a flag given twice, a flag with no "
        "value); focus refuses it",
    r"runtime/(compile|summary)/stage_graph_(phase_missing|phase_not_text|null_row|not_a_list)":
        "on a stage-graph.json that is not shaped as the compile writes it, upstream fails with what its runtime "
        "says of the value, or (summary, a phase that is not text) counts under the value's name; focus says "
        "the stage graph is not one the compile wrote",
    "runtime/summary/phase_named_like_an_object_member":
        "upstream keeps the phases of a summary in a plain object, so a phase named after something every object "
        "inherits (`constructor`) is not listed; focus lists it like any other",
    r"runtime/read/json_(number_out_of_range|half_pair_escape|nested_deeper_than_127)":
        "three things JSON.parse reads and focus's JSON reader refuses: a number no double holds (1e400), an "
        "escaped half of a surrogate pair, and nesting deeper than 127 levels. focus says the file is not JSON it "
        "reads",
    "runtime/compile/workspace_is_a_link":
        "with the workspace directory a link, upstream leaves out the shards behind it and writes an empty graph "
        "through the link; focus takes no lock and writes nothing through a link",
    "runtime/compile/metrics_endpoint":
        "upstream sends a metric for the MEMORY_EMPTY event to the configured endpoint; focus has no metrics yet "
        "and refuses the append, and with it the compile, instead of leaving the endpoint without what it expects",
    "runtime/compile/delegated_worktree":
        "upstream resolves a delegated worktree to the main checkout's intent; focus does not work with "
        "worktrees yet and refuses while the marker exists",
    "runtime/compile/unit_claim":
        "upstream ignores a unit claim that does not apply and otherwise lets it name the shard; focus does not "
        "work with units yet and refuses while the claim file exists",
    r"runtime/compile/orphan_(no_zone|junk_after|latest_no_zone|leap_day|long_fraction|date_only)":
        "whether a sensor that never ended counts as incomplete in an open stage depends on how long ago it fired, "
        "and upstream asks JavaScript's Date.parse, which reads many forms by the runtime's own rules and in the "
        "machine's time zone. focus reads an instant written with its zone on a real date, knows that a text "
        "with no digit is no date, and refuses the compile when the timestamp it needs is anything else",
    r"runtime/(read|summary)/not_a_graph_.*":
        "on a runtime-graph.json that is not shaped as the engine writes it, upstream fails with what its runtime "
        "says of the value, counts under names that are no outcome, or reads it without a word (a count that is "
        "text, learnings or a breakdown that is no object, counts whose sum no number holds exactly); focus says "
        "the file is not a runtime graph and how to write it again",
    r"runtime/summary/duration_(no_zone|date_only)":
        "the duration is a difference of two Date.parse answers upstream; focus refuses a timestamp it does not "
        "read (see the orphan checks)",
})


STATE_FULL = ("# AI-DLC State\n\n## Workflow\n- **Scope**: feature\n- **Status**: In Progress\n- **Current Stage**: code-generation\n"
              "- **Completed**: 2\n- **Guard Policy**: off (from scope feature)\n\n## Stage Progress\n"
              "- [x] intent-capture \u2014 EXECUTE\n- [x] requirements-analysis \u2014 EXECUTE\n- [-] code-generation \u2014 EXECUTE\n"
              "- [ ] build-and-test \u2014 EXECUTE\n- [S] nfr-design \u2014 SKIP: not needed\n\n## Runtime State\n- **Parked**: no\n")


def state_checks():
    """`aidlc-state.ts` on the verbs Focus has: get, set, checkbox, count, and what stands before a write."""
    check_ = lambda name, files, steps, env=None, edits=(): runtime_check(name, files, steps, env, edits, group="state")
    record = {**RECORD_FILES, f"{RECORD_DIR}/aidlc-state.md": STATE_FULL}
    direct = {"ALLOW_DIRECT_STATE_TRANSITIONS": "1"}
    shard = {SHARD: TITLE + WORKFLOW + STARTED_ROW(1, "intent-capture")}

    # Usage.
    check_("usage/none", record, [[]])
    check_("usage/unknown", record, [["nope"]])
    check_("usage/get_no_field", record, [["get"]])
    check_("usage/set_no_pair", record, [["set"]], direct)
    check_("usage/checkbox_no_pair", record, [["checkbox"]], direct)
    check_("usage/count_no_state", record, [["count"]])
    for verb in ("gate-start", "approve", "park", "unit", "fork", "practices-event", "set-construction-checkpoints"):
        check_(f"usage/{verb}_not_ported", {**record, **shard}, [[verb]])

    # get: a field, a field with spaces, an empty one, one that is not there, and with no state file.
    check_("get/fields", record, [["get", "Scope"], ["get", "Current", "Stage"], ["get", "Parked"], ["get", "Nope"],
                                  ["get", "Completed"], ["get", "Stage Progress"]])
    check_("get/empty_value", {**record, f"{RECORD_DIR}/aidlc-state.md": "- **Scope**: \n- **Next**: x\n"},
           [["get", "Scope"], ["get", "Next"]])
    check_("get/no_state_file", RECORD_FILES | {f"{RECORD_DIR}/aidlc-state.md": None}, [["get", "Scope"]])
    check_("get/no_record", CLONE, [["get", "Scope"]])
    check_("get/crlf", {**record, f"{RECORD_DIR}/aidlc-state.md": STATE_FULL.replace("\n", "\r\n")},
           [["get", "Scope"], ["get", "Current Stage"], ["count", "completed"]])
    # The error row names the tool, the command and the error, with the project's path redacted.
    check_("get/error_logged", {**record, **shard}, [["get", "Nope"], ["get", "{P}/x"], ["count", "nope"], ["get"]])
    check_("get/error_without_shard", record, [["get", "Nope"]])
    check_("get/error_without_intent", {**CLONE, "aidlc/spaces/default/intents/aidlc-state.md": STATE_FULL}, [["get", "Nope"]])

    # set: fields, special values, a batch that fails before any write, typed setters, dollar patterns.
    check_("set/fields", {**record, **shard},
           [["set", "Scope=bugfix", "Current Stage=build-and-test"], ["get", "Scope"], ["get", "Current Stage"]], direct)
    check_("set/now_and_counts", {**record, **shard},
           [["set", "Completed=+1"], ["set", "Completed=-1", "Completed=-1", "Completed=-1"], ["set", "Parked=+1"],
            ["set", "Nope=+1", "Scope=-1"], ["get", "Scope"], ["set", "Status=NOW"]], direct)
    check_("set/absent_field_is_no_op", {**record, **shard}, [["set", "Nope=1"], ["set", "Scope=x", "Nope=2"], ["get", "Scope"]], direct)
    check_("set/invalid_pair", {**record, **shard}, [["set", "Scope=x", "nopair"], ["set", "=x"], ["get", "Scope"]], direct)
    check_("set/typed_setters", {**record, **shard},
           [["set", "Construction Checkpoints=enabled"], ["set", "Scope=x", "Construction Execution=swarm"],
            ["set", "Construction Iteration=unit-major"], ["set", "Construction Verification Command=make"]], direct)
    check_("set/dollar_patterns", {**record, **shard},
           [["set", "Scope=$1 $& $$ $' $9 $<n>"], ["get", "Scope"], ["set", "Scope=$2"], ["get", "Scope"]], direct)
    check_("set/empty_and_odd_values", {**record, **shard},
           [["set", "Scope="], ["get", "Scope"], ["set", "Scope=a=b=c"], ["get", "Scope"], ["set", "Scope=  spaced  "],
            ["get", "Scope"], ["set", "Scope=\u00e9 \U0001F600"], ["get", "Scope"]], direct)
    check_("set/no_state_file", {**RECORD_FILES, f"{RECORD_DIR}/aidlc-state.md": None, **shard}, [["set", "Scope=x"]], direct)
    check_("set/archived", {**record, f"{RECORD_DIR}/aidlc-state.md": STATE_FULL.replace("In Progress", "Archived"), **shard},
           [["set", "Scope=x"], ["checkbox", "build-and-test=completed"], ["get", "Scope"], ["count", "completed"]], direct)
    check_("set/crlf", {**record, f"{RECORD_DIR}/aidlc-state.md": STATE_FULL.replace("\n", "\r\n"), **shard},
           [["set", "Scope=x", "Completed=+1"], ["checkbox", "build-and-test=completed"]], direct)

    # checkbox and count.
    check_("checkbox/states", {**record, **shard},
           [["checkbox", "build-and-test=in-progress", "code-generation=completed"], ["count", "completed"],
            ["count", "in-progress"], ["checkbox", "nfr-design=pending"], ["count", "skipped"], ["count", "pending"],
            ["checkbox", "intent-capture=awaiting-approval", "intent-capture=revising"], ["count", "revising"]], direct)
    check_("checkbox/unknown_slug_and_state", {**record, **shard},
           [["checkbox", "nope=completed"], ["checkbox", "build-and-test=done"], ["checkbox", "=completed"],
            ["checkbox", "build-and-test=completed", "x=nope"], ["count", "completed"]], direct)
    check_("checkbox/completed_counter_follows", {**record, f"{RECORD_DIR}/aidlc-state.md": STATE_FULL.replace("**Completed**: 2", "**Completed**: 9"), **shard},
           [["get", "Completed"], ["checkbox", "build-and-test=completed"], ["get", "Completed"]], direct)
    check_("checkbox/no_completed_field", {**record, f"{RECORD_DIR}/aidlc-state.md": STATE_FULL.replace("- **Completed**: 2\n", ""), **shard},
           [["checkbox", "build-and-test=completed"], ["get", "Completed"]], direct)
    check_("count/states", record, [["count", "completed"], ["count", "pending"], ["count", "in-progress"], ["count", "skipped"],
                                    ["count", "awaiting-approval"], ["count", "revising"], ["count", "done"]])

    # The engine-owned gate: by the fence, by the owner, by the switch.
    lowered = {**record, **shard}
    check_("gate/fence_down_by_scope_default", {**record, f"{RECORD_DIR}/aidlc-state.md": STATE_FULL.replace("- **Guard Policy**: off (from scope feature)\n", ""), **shard},
           [["set", "Scope=x"], ["checkbox", "build-and-test=completed"]])
    check_("gate/fence_down_by_policy_line", lowered, [["set", "Scope=x"]])
    check_("gate/fence_down_relaxed_speaks", {**record, f"{RECORD_DIR}/aidlc-state.md": STATE_FULL.replace("off (from scope feature)", "relaxed (set by you)") + "- **Guards Off**: state-transition (set by you)\n", **shard},
           [["set", "Scope=x"], ["checkbox", "build-and-test=completed"]])
    check_("gate/fence_down_without_shard", record, [["set", "Scope=x"], ["get", "Scope"]])
    check_("gate/fence_up_strict", {**record, f"{RECORD_DIR}/aidlc-state.md": STATE_FULL.replace("off (from scope feature)", "strict (set by you)"), **shard},
           [["set", "Scope=x"], ["checkbox", "build-and-test=completed"], ["get", "Scope"]])
    check_("gate/fence_up_by_guards_on", {**record, f"{RECORD_DIR}/aidlc-state.md": STATE_FULL + "- **Guards On**: state-transition (set by you)\n", **shard},
           [["set", "Scope=x"]])
    check_("gate/fence_up_by_memory", {**record, "aidlc/spaces/default/memory/team.md": "# Team\n\n## Guard Policy\n\nMode: strict\n", **shard},
           [["set", "Scope=x"]])
    check_("gate/fence_up_enterprise", {**record, f"{RECORD_DIR}/aidlc-state.md": STATE_FULL.replace("- **Guard Policy**: off (from scope feature)\n", "").replace("feature", "enterprise"), **shard},
           [["set", "Scope=x"]])
    check_("gate/policy_unreadable_state", {**record, f"{RECORD_DIR}/aidlc-state.md": STATE_FULL.replace("off (from scope feature)", "sometimes"), **shard},
           [["set", "Scope=x"]])
    check_("gate/policy_unreadable_memory", {**record, "aidlc/spaces/default/memory/org.md": "## Guard Policy\n\nMode: maybe\n", **shard},
           [["set", "Scope=x"]])
    check_("gate/no_state_file", {**RECORD_FILES, f"{RECORD_DIR}/aidlc-state.md": None, **shard}, [["set", "Scope=x"]])
    # The owner is this process, which a replay is not run from.
    UNRECORDED.add("state/gate/owner_is_parent")
    check_("gate/owner_is_parent", {**record, f"{RECORD_DIR}/aidlc-state.md": STATE_FULL.replace("off (from scope feature)", "strict (set by you)"), **shard},
           [["set", "Scope=x"], ["get", "Scope"]], {"STATE_TRANSITION_OWNER": f"orchestrate:{os.getpid()}"})
    check_("gate/owner_is_another", {**record, f"{RECORD_DIR}/aidlc-state.md": STATE_FULL.replace("off (from scope feature)", "strict (set by you)"), **shard},
           [["set", "Scope=x"]], {"STATE_TRANSITION_OWNER": "orchestrate:1"})
    check_("gate/switch_off_is_not_on", {**record, f"{RECORD_DIR}/aidlc-state.md": STATE_FULL.replace("off (from scope feature)", "strict (set by you)"), **shard},
           [["set", "Scope=x"]], {"ALLOW_DIRECT_STATE_TRANSITIONS": "0"})
    check_("gate/reads_need_no_gate", {**record, f"{RECORD_DIR}/aidlc-state.md": STATE_FULL.replace("off (from scope feature)", "strict (set by you)"), **shard},
           [["get", "Scope"], ["count", "completed"]])

    # Observers, and a record with a marker.
    for marker in ("STOP_HOOK_PROBE", "ROUTE_CHECK"):
        check_(f"observer/{marker.lower()}", {**record, **shard},
               [["set", "Scope=x"], ["checkbox", "build-and-test=completed"], ["get", "Scope"], ["count", "completed"]],
               {marker: "1", **direct})
        check_(f"observer/{marker.lower()}_at_the_gate", {**record, **shard}, [["set", "Scope=x"]], {marker: "1"})
    check_("marker/active_directive", {**record, f"{RECORD_DIR}/.aidlc-engine/active-directive.json": "{}\n", **shard},
           [["set", "Scope=x"], ["get", "Scope"]], direct)
    check_("marker/active_directive_at_the_gate", {**record, f"{RECORD_DIR}/.aidlc-engine/active-directive.json": "{}\n", **shard},
           [["set", "Scope=x"]])
    # The markers' times decide, and a recording holds no times: these are compared live only.
    UNRECORDED.update({"state/marker/turn_markers_at_the_gate", "state/marker/turn_after_touch",
                       "state/marker/touch_after_turn", "state/marker/turn_marker_is_a_directory"})
    check_("marker/turn_markers_at_the_gate", {**record, f"{RECORD_DIR}/.aidlc-engine/human-turn": "", f"{RECORD_DIR}/.aidlc-engine/engine-touch": "", **shard},
           [["set", "Scope=x"]])
    # The person's turn after the engine's touch is a grant; the engine's after the person's covers nothing.
    def turn_after_touch(tree):
        os.utime(tree.path(f"{RECORD_DIR}/.aidlc-engine/engine-touch"), (1_700_000_000, 1_700_000_000))
        os.utime(tree.path(f"{RECORD_DIR}/.aidlc-engine/human-turn"), (1_700_000_100, 1_700_000_100))
    def touch_after_turn(tree):
        os.utime(tree.path(f"{RECORD_DIR}/.aidlc-engine/human-turn"), (1_700_000_000, 1_700_000_000))
        os.utime(tree.path(f"{RECORD_DIR}/.aidlc-engine/engine-touch"), (1_700_000_100, 1_700_000_100))
    markers = {**record, f"{RECORD_DIR}/.aidlc-engine/human-turn": "", f"{RECORD_DIR}/.aidlc-engine/engine-touch": "", **shard}
    check_("marker/turn_after_touch", markers, [["set", "Scope=x"]], edits=(turn_after_touch,))
    check_("marker/turn_after_touch_unattended", markers, [["set", "Scope=x"]], {"UNATTENDED": "1"}, edits=(turn_after_touch,))
    check_("marker/touch_after_turn", markers, [["set", "Scope=x"]], edits=(touch_after_turn,))
    check_("marker/turn_marker_is_a_directory", {**record, f"{RECORD_DIR}/.aidlc-engine/human-turn/x": "", f"{RECORD_DIR}/.aidlc-engine/engine-touch": "", **shard},
           [["set", "Scope=x"]], edits=(lambda tree: os.utime(tree.path(f"{RECORD_DIR}/.aidlc-engine/human-turn"), (1_800_000_000, 1_800_000_000)),))

    # Which record, and what Focus does not resolve.
    other = "aidlc/spaces/payments/intents"
    check_("record/other_space", {**CLONE, "aidlc/active-space": "payments\n", f"{other}/aidlc-state.md": STATE_FULL}, [["get", "Scope"], ["set", "Scope=x"], ["get", "Scope"]], direct)
    check_("record/two_records_no_cursor", {**CLONE, f"{RECORD_DIR}/aidlc-state.md": STATE_FULL, "aidlc/spaces/default/intents/fix-logout-9f8e7d6c/aidlc-state.md": STATE_FULL}, [["get", "Scope"]])
    check_("record/sessions_by_process_unmapped", {**record, "aidlc/.aidlc-sessions/pids/1.json": "{}\n"}, [["get", "Scope"]])
    check_("record/sessions_by_process", {**record, **my_pid_entry(), "aidlc/spaces/default/intents/fix-logout-9f8e7d6c/aidlc-state.md": "# State\n- **Scope**: theirs\n",
           "aidlc/.aidlc-sessions/s1.binding.json": '{"space":"default","intent":"fix-logout-9f8e7d6c","boundAt":"2026-01-05T10:00:00Z","source":"create"}'}, [["get", "Scope"]])
    UNRECORDED.add("state/record/sessions_by_process")
    check_("record/delegated_worktree", {**record, ".aidlc/worktree-meta.json": "{}\n"}, [["get", "Scope"]])
    check_("record/unit_claim", {**record, "aidlc/.aidlc-unit-scope.json": "{}\n", **shard}, [["get", "Scope"], ["set", "Scope=x"]], direct)
    check_("record/state_is_a_directory", {**RECORD_FILES, f"{RECORD_DIR}/aidlc-state.md": None, f"{RECORD_DIR}/aidlc-state.md/x": "x"},
           [["get", "Scope"], ["set", "Scope=x"]], direct)
    check_("usage/project_dir_before_the_verb", record, [["--project-dir", "{P}", "get", "Scope"]])
    check_("usage/stray_argument", record, [["get", "Scope", "--x"], ["count", "completed", "extra"]])
    check_("usage/flag_without_its_value", record, [["get", "Scope", "--project-dir"]])
    check_("usage/metrics_endpoint", {**record, **shard}, [["get", "Nope"]], {"METRICS_ENDPOINT": "http://127.0.0.1:9/x"})


def utility_checks():
    """`aidlc-utility.ts` on the verbs Focus has: detect and now, and how a refusal is recorded."""
    check_ = lambda name, files, steps, env=None, edits=(): runtime_check(name, files, steps, env, edits, group="utility")
    record = {**RECORD_FILES, f"{RECORD_DIR}/aidlc-state.md": STATE_FULL}
    shard = {SHARD: TITLE + WORKFLOW + STARTED_ROW(1, "intent-capture")}
    both = [["detect"], ["detect", "--json"]]
    empty = lambda *names: {name: "" for name in names}

    # Usage, and what a refusal leaves.
    check_("usage/none", record, [[]])
    check_("usage/unknown", record, [["nope"]])
    for verb in ("help", "version", "status", "doctor", "intent", "space", "config-get", "scope-table"):
        check_(f"usage/{verb}_not_ported", {**record, **shard}, [[verb]])
    check_("usage/error_logged", {**record, **shard},
           [["nope"], ["nope", "--intent", "add-login-1a2b3c4d"], ["nope", "--space", "default"], ["nope", "--space"],
            ["nope", "--intent", "fix-logout-9f8e7d6c"], ["nope", "--json", "--x=y"]])
    check_("usage/error_without_record", CLONE, [["nope"]])
    check_("now/prints_the_time", {}, [["now"], ["now", "--json"]])

    # detect: the root.
    check_("detect/empty", {}, both)
    check_("detect/empty_record", record, both)
    # Counts kept distinct: languages with the same count are listed in the order the OS lists the files.
    check_("detect/root_files", empty("a.ts", "b.TS", "c.tsx", "h.ts", "d.js", "e.mjs", "f.cjs", "README.md", ".env.py", ".py",
                                      "noext", "x."), both)
    # One extension at a time: languages with the same count are listed in the order the OS lists the files,
    # which a replay on another file system would not reproduce.
    for ext in [".ts", ".tsx", ".js", ".jsx", ".mjs", ".cjs", ".py", ".java", ".kt", ".go", ".rs", ".rb", ".cs", ".cpp",
                ".c", ".h", ".hpp", ".swift", ".php", ".md", ".TS", ".Py"]:
        check_(f"detect/extension_{ext[1:]}", empty(f"src/f{ext}", f"src/g{ext}", "src/h.sql"), both)
    check_("detect/secondary_threshold", {**empty(*(f"src/{n}.go" for n in range(10))), **empty("src/a.rs", "src/b.rs", "src/c.py")}, both)
    check_("detect/secondary_threshold_is_a_fifth", {**empty(*(f"src/{n}.go" for n in range(8))), "src/a.rs": ""}, both)
    # A tie between languages follows the order the OS lists the files, so it is compared live and not replayed.
    UNRECORDED.add("utility/detect/one_of_each_is_a_tie")
    check_("detect/one_of_each_is_a_tie", empty("src/a.rs", "src/b.py", "lib/c.go"), both)
    check_("detect/source_dirs_to_depth_six",
           {**empty("src/a/b/c/d/e/f/g.py", "src/a/b/c/d/e/f/g/h.py", "other/deep.py", "tests/t.rb", "pages/p.php",
                    "components/c.swift", "app/a.kt", "lib/l.cs")}, both)
    check_("detect/excluded_dirs",
           empty("node_modules/x.js", "dist/y.js", "build/z.js", "target/t.rs", "vendor/v.go", ".next/n.js", ".git/g.py",
                 "aidlc-docs/d.py", "src/node_modules/m.js", "src/dist/d.ts", ".claude/c.ts", ".kiro/k.ts", ".aidlc/a.ts"), both)
    check_("detect/source_dir_is_a_file", {"src": "not a dir\n"}, both)
    check_("detect/source_dir_alone", {"src/.keep": ""}, both)

    # detect: frameworks and build systems.
    check_("detect/react_and_vite", {"package.json": '{"dependencies":{"react":"18"}}', "vite.config.ts": ""}, both)
    check_("detect/react_peer_last_word",
           {"package.json": '{"dependencies":{"react":"18"},"peerDependencies":{"react":""}}'}, both)
    check_("detect/react_peer_only", {"package.json": '{"peerDependencies":{"react":"18"}}'}, both)
    check_("detect/react_odd_values",
           {"package.json": '{"dependencies":{"react":0},"peerDependencies":"react"}'}, both)
    check_("detect/dev_deps_only", {"package.json": '{"devDependencies":{"x":"1"},"peerDependencies":{"y":"1"}}'}, both)
    check_("detect/deps_as_string", {"package.json": '{"dependencies":"abc"}'}, both)
    check_("detect/deps_as_empty_string", {"package.json": '{"dependencies":""}'}, both)
    check_("detect/deps_as_list", {"package.json": '{"dependencies":[1]}'}, both)
    check_("detect/deps_as_number", {"package.json": '{"dependencies":7}'}, both)
    check_("detect/package_json_is_a_list", {"package.json": '[{"dependencies":{"x":"1"}}]'}, both)
    check_("detect/package_json_invalid", {"package.json": '{"dependencies":'}, both)
    check_("detect/package_json_is_a_directory", {"package.json/x": ""}, both)
    check_("detect/package_json_with_bom", {"package.json": '\ufeff{"dependencies":{"x":"1"}}'}, both)
    check_("detect/package_json_duplicate_key",
           {"package.json": '{"dependencies":{"x":"1"},"dependencies":{}}'}, both)
    for name, files in [("next", {"next.config.mjs": ""}), ("angular", {"angular.json": ""}), ("nuxt", {"nuxt.config.ts": ""}),
                        ("remix", {"remix.config.js": ""}), ("gatsby", {"gatsby-config.js": ""}), ("astro", {"astro.config.mjs": ""}),
                        ("svelte", {"svelte.config.js": ""}), ("nest", {"nest-cli.json": ""}), ("django", {"manage.py": ""})]:
        check_(f"detect/framework_{name}", files, both)
    check_("detect/all_js_frameworks",
           {**empty("next.config.js", "vite.config.js", "angular.json", "nuxt.config.js", "remix.config.js", "gatsby-config.js",
                    "astro.config.ts", "svelte.config.js", "nest-cli.json", "manage.py"), "package.json": '{"dependencies":{"react":"1"}}',
            "Gemfile": "gem 'rails'\n", "pom.xml": "spring-boot", **empty("src/a.py", "src/b.py", "src/c.py")}, both)
    check_("detect/rails_gemfile", {"Gemfile": "source 'x'\ngem 'rails', '7'\n"}, both)
    check_("detect/rails_commented", {"Gemfile": "# gem 'rails'\ngem 'x' # rails\n"}, both)
    check_("detect/rails_word_boundary", {"Gemfile": "gem 'railsy'\ngem 'trails'\n"}, both)
    check_("detect/rails_after_a_comment_line", {"Gemfile": "# a\ngem 'rails'\n"}, both)
    check_("detect/rails_unicode_space", {"Gemfile": "gem\u00a0'rails'\n"}, both)
    check_("detect/spring_boot", {"pom.xml": "<artifactId>spring-boot-starter</artifactId>"}, both)
    check_("detect/maven_without_spring", {"pom.xml": "<project/>"}, both)
    for name, files in [("pnpm", {"package.json": "{}", "pnpm-lock.yaml": ""}), ("yarn", {"package.json": "{}", "yarn.lock": ""}),
                        ("bun", {"package.json": "{}", "bun.lockb": ""}), ("bun_text", {"package.json": "{}", "bun.lock": ""}),
                        ("npm", {"package.json": "{}"}), ("lock_order", {"package.json": "{}", "yarn.lock": "", "pnpm-lock.yaml": "", "bun.lock": ""}),
                        ("poetry", {"pyproject.toml": "[tool.poetry]\n"}), ("uv", {"pyproject.toml": "[tool.uv]\n"}),
                        ("hatch", {"pyproject.toml": "[tool.hatch]\n"}), ("hatch_then_poetry", {"pyproject.toml": "[tool.hatch]\n[tool.poetry]\n"}),
                        ("python", {"pyproject.toml": "[project]\n"}), ("pip", {"requirements.txt": ""}), ("setuptools", {"setup.py": ""}),
                        ("cargo", {"Cargo.toml": ""}), ("go", {"go.mod": ""}), ("gradle", {"build.gradle": ""}), ("gradle_kts", {"build.gradle.kts": ""}),
                        ("composer", {"composer.json": ""}), ("bundler", {"Gemfile": ""}),
                        ("precedence", {"Cargo.toml": "", "go.mod": "", "requirements.txt": "", "pyproject.toml": "x", "package.json": "{}"}),
                        ("manifest_is_a_directory", {"Cargo.toml/x": ""})]:
        check_(f"detect/build_{name}", files, both)

    # detect: nested projects and git repositories.
    nested = empty("README.md", "services/api/go.mod", "services/api/main.go", "services/api/handlers/h.go",
                   "services/web/index.html", "services/web/.git/HEAD", "examples/x.py", "a/b/c/d/deep.py", "Docs/x.py")
    check_("detect/nested", nested, both)
    check_("detect/nested_three_levels", empty("a/b/c/main.rs", "a/b/c/d/deep.py"), both)
    check_("detect/nested_hit_not_entered", empty("a/x.py", "a/b/y.rs", "a/b/c/z.go"), both)
    check_("detect/nested_order", empty("b/x.py", "a/y.rs", "Zed/z.go", "_/w.kt", "src-x/v.c"), both)
    check_("detect/nested_order_is_utf16", empty("\uff5e/a.py", "\U0001f600/b.py", "z/c.py"), both)
    check_("detect/nested_hit_below_a_repo", empty("a/.git/HEAD", "a/b/main.go"), both)
    check_("detect/nested_repo_only", {"web/.git/HEAD": ""}, both)
    check_("detect/nested_repo_and_hit", empty("web/.git/HEAD", "api/main.go", "api/.git/HEAD"), both)
    check_("detect/nested_repo_below_a_hit", empty("api/main.go", "api/inner/.git/HEAD"), both)
    check_("detect/nested_repo_at_depth_three", {"a/b/c/.git/HEAD": ""}, both)
    check_("detect/nested_repo_at_depth_four", {"a/b/c/d/.git/HEAD": ""}, both)
    check_("detect/nested_excluded_names",
           empty("docs/x.py", "Examples/y.py", "SAMPLE/z.py", "testdata/t.py", "fixtures/f.py", "templates/t.py", "scripts/s.py",
                 "reference/r.py", "demo/d.py", "aidlc/a.py", ".hidden/h.py", "Node_Modules/n.py", "Dist/d.py"), both)
    check_("detect/nested_excluded_names_below", empty("a/docs/x.py", "a/.hidden/h.py", "a/b/examples/e.py"), both)
    check_("detect/nested_source_dir_name_is_case_sensitive", empty("SRC/x.py", "Lib/y.py"), both)
    check_("detect/nested_frameworks_and_build",
           {"a/package.json": '{"dependencies":{"react":"1"}}', "a/vite.config.js": "", "b/Cargo.toml": "", "b/x.rs": "",
            "b/y.rs": "", "b/vite.config.ts": ""}, both)
    check_("detect/nested_build_system_first_wins", {"a/go.mod": "", "b/Cargo.toml": ""}, both)
    check_("detect/nested_languages_add_up", empty("a/x.py", "a/y.py", "b/z.py", "b/w.rs", "c/v.rs"), both)
    check_("detect/root_hit_skips_nesting", empty("x.py", "services/api/main.go"), both)
    check_("detect/root_manifest_skips_nesting", {"Cargo.toml": "", "services/api/main.go": ""}, both)

    # detect: submodules.
    modules = ("[submodule \"libs/one\"]\n\tpath = libs/one\n\turl = https://x/one.git\n; path = commented\n# url = commented\n"
               "[submodule \"two\"]\n  path=/abs\n"
               "[submodule \"three\"]\n path = a/../b\n[other]\npath = x\n[submodule \"four\"]\nurl = u\n[submodule \"five\"]\npath = five\n"
               "[submodule \"six\"]\r\npath = c:\\win\r\n[submodule \"seven\"]\npath = sub\\dir\n[submodule \"eight\"]\npath = up\\..\\x\n")
    check_("detect/gitmodules", {".gitmodules": modules, "libs/one/.git": "gitdir: ../../.git/modules/one\n", "five/.keep": ""}, both)
    check_("detect/gitmodules_garbage", {".gitmodules": "garbage\n= x\n[submodule]\npath=y\n[submodule \"\"]\npath=z\n"}, both)
    check_("detect/gitmodules_many", {".gitmodules": "".join(f"[submodule \"m{n}\"]\npath = m{n}\n" for n in range(7))}, both)
    check_("detect/gitmodules_is_a_directory", {".gitmodules/x": ""}, both)
    check_("detect/gitmodules_with_nested_hit", {".gitmodules": "[submodule \"s\"]\npath = s\n", "a/x.py": ""}, both)
    check_("detect/gitmodules_odd_header", {".gitmodules": "[submodule \"a\u2028b\"]\npath = p\n[submodule\t\"c\"]\npath = q\n"}, both)

    # detect: where the scopes live, and the files the engine wrote whole.
    check_("detect/other_space", {"aidlc/active-space": "payments\n", "aidlc/spaces/payments/.keep": ""}, both)
    check_("detect/space_cursor_without_a_dir", {"aidlc/active-space": "payments\n"}, both)
    check_("detect/project_dir_explicit", empty("x.py"), [["detect", "--project-dir", "{P}"]])
    check_("detect/project_dir_without_install", empty("x.py", "nope/y.py"), [["detect", "--project-dir", "{P}/nope"]])
    check_("detect/json_flag_with_a_value", empty("x.py"), [["detect", "--json", "yes"], ["detect", "--json=true"], ["detect", "extra"]])

    def whole_file(tree):
        descriptor = tree.json(".claude/tools/data/aidlc-projection.json")
        descriptor["rootIntegrations"].append({"path": "whole.ts", "policy": "whole-file"})
        descriptor["rootIntegrations"].append({"path": "ext.ts", "policy": "whole-file", "extendedPolicy": "json-entries"})
        descriptor["rootIntegrations"].append({"path": "./sub/../up.ts", "policy": "whole-file"})
        tree.write_json(".claude/tools/data/aidlc-projection.json", descriptor)
    check_("detect/engine_whole_files", empty("whole.ts", "ext.ts", "up.ts"), both, edits=(whole_file,))
    check_("detect/engine_whole_file_alone", empty("whole.ts"), both, edits=(whole_file,))
    def broken_descriptor(tree):
        tree.write(".claude/tools/data/aidlc-projection.json", "{")
    check_("detect/engine_descriptor_unreadable", empty("x.ts"), both, edits=(broken_descriptor,))

    def links(tree):
        tree.write("real/x.py", "")
        os.symlink(tree.path("real/x.py"), tree.path("y.py"))
        os.symlink(tree.path("real"), tree.path("linked"))
    check_("detect/links_are_not_followed", {}, both, edits=(links,))
    def linked_source_dir(tree):
        tree.write("real/x.py", "")
        os.symlink(tree.path("real"), tree.path("src"))
    check_("detect/linked_source_dir_is_read_through", {}, both, edits=(linked_source_dir,))
    def linked_manifest(tree):
        tree.write("real/package.json", '{"dependencies":{"react":"1"}}')
        os.symlink(tree.path("real/package.json"), tree.path("package.json"))
    check_("detect/linked_manifest_is_read", {}, both, edits=(linked_manifest,))


def intent_create_checks():
    """`aidlc-utility.ts intent-create`: what is refused before anything is written, and what a creation writes."""
    check_ = lambda name, files, steps, env=None, edits=(): runtime_check(name, files, steps, env, edits, group="utility")
    create = ["intent-create"]
    feature = create + ["--scope", "feature", "--arguments", "Add login to the app", "--label", "add login"]
    strict_team = {"aidlc/spaces/default/memory/team.md": "# Team\n\n## Guard Policy\n\nMode: strict\n"}

    # Usage and the flags.
    check_("create/help", {}, [create + ["--help"], create + ["-h"], create + ["--scope", "feature", "-h"]])
    check_("create/bare", {}, [create])
    check_("create/positional", {}, [create + ["--scope", "feature", "extra", "words"],
                                     create + ["--arguments=deploy", "this", "and", "that", "--scope", "feature"]])
    check_("create/intent_flag", {}, [create + ["--scope", "feature", "--intent", "x"], create + ["--scope", "feature", "--intent"]])
    check_("create/blank_values", {}, [create + ["--scope", ""], create + ["--scope", "feature", "--label", " "],
                                       create + ["--scope", "feature", "--depth"], create + ["--scope=feature", "--repos=", "--add", " "],
                                       create + ["--scope", "feature", "--project-dir"]])
    check_("create/request_flag", {}, [create + ["--request", "q-1"]])
    check_("create/unknown_values", {}, [create + ["--scope", "nope"], create + ["--scope", "feature", "--depth", "deep"],
                                         create + ["--scope", "feature", "--test-strategy", "lots"],
                                         create + ["--scope", "feature", "--project-type", "green"],
                                         create + ["--scope", "feature", "--review", "harsh"],
                                         create + ["--scope", "feature", "--guard-policy", "loose"],
                                         create + ["--scope", "feature", "--sensors", "maybe"],
                                         create + ["--scope", "feature", "--plan-approval"],
                                         create + ["--scope", "feature", "--plan-name", "My Plan"],
                                         create + ["--scope", "feature", "--space", "nope"]])
    check_("create/change_control_both", {}, [create + ["--scope", "feature", "--guard-policy", "relaxed", "--change-control", "off"]])
    check_("create/plan_changes_refused", {}, [create + ["--scope", "feature", "--skip", "nope"],
                                               create + ["--scope", "feature", "--skip", "workspace-scaffold"],
                                               create + ["--scope", "feature", "--skip", "nfr-design", "--add", "nfr-design"],
                                               create + ["--scope", "feature", "--skip", "reverse-engineering,nope", "--add", "market-research"],
                                               create + ["--scope", "bugfix", "--skip", "market-research"]])
    check_("create/reserved_label", {}, [create + ["--scope", "feature", "--label", "Help"], create + ["--scope", "feature", "--label", "Switch!"]])
    check_("create/default_scope_configured", {}, [create + ["--arguments", "x"]], {"DEFAULT_SCOPE": "bugfix"})
    check_("create/default_scope_unknown", {}, [create + ["--arguments", "x"]], {"DEFAULT_SCOPE": "nope"})
    check_("create/default_scope_classic", {}, [create + ["--label", "just a label"]])

    # What a creation writes.
    check_("create/feature", {}, [feature])
    check_("create/twice_same_label", {}, [feature, feature, create + ["--scope", "feature", "--label", "add login"]])
    check_("create/label_from_description", {}, [create + ["--scope", "feature", "--arguments", "Add a login page to the app, with sessions"]])
    check_("create/label_from_scope", {}, [create + ["--scope", "bugfix"]])
    check_("create/label_odd", {}, [create + ["--scope", "feature", "--label", "  Ümlaut & 9 lives --  "], create + ["--scope", "feature", "--label", "***"],
                                    create + ["--scope", "feature", "--label", "9 to 5"]])
    check_("create/description_odd", {}, [create + ["--scope", "feature", "--arguments", "line one\nline two\ttab  spaced \u2028 sep"],
                                          create + ["--scope", "feature", "--arguments", "\u0001\u0002"],
                                          create + ["--scope", "feature", "--arguments", "quote \" and \\ backslash \u00e9 \U0001F600"]])
    check_("create/pasted_document", {}, [create + ["--scope", "feature", "--arguments", "Build this <document>a spec\nmore</document> please"],
                                          create + ["--scope", "feature", "--arguments", "<document>only a document</document>"],
                                          create + ["--scope", "feature", "--arguments", "<document>no closing"],
                                          create + ["--scope", "feature", "--arguments", "after </document> only"],
                                          create + ["--scope", "feature", "--arguments", "</document> before <document>"]])
    for scope in ("bugfix", "classic", "enterprise", "express", "infra", "mvp", "poc", "refactor", "security-patch", "workshop"):
        check_(f"create/scope_{scope}", {}, [create + ["--scope", scope, "--label", scope]])
    check_("create/levels", {}, [feature + ["--depth", "MINIMAL", "--test-strategy", "comprehensive"]])
    check_("create/review_override", {}, [feature + ["--review", "advisory"], create + ["--scope", "feature", "--label", "two", "--review", "ADVERSARIAL"],
                                          create + ["--scope", "feature", "--label", "three", "--review", "none"]])
    check_("create/composed_plan", {}, [feature + ["--skip", "market-research, feasibility", "--add", "reverse-engineering", "--plan-name", "lean-login"]])
    check_("create/composed_plan_unnamed", {}, [feature + ["--skip", "team-formation"], create + ["--scope", "bugfix", "--label", "x", "--add", "market-research", "--plan-name", "Bad Name"]])
    check_("create/composed_plan_empties_a_phase", {}, [create + ["--scope", "bugfix", "--label", "x", "--skip", "deployment-execution,observability-setup,incident-response"]])
    check_("create/composed_plan_skips_reverse_engineering", {}, [feature + ["--skip", "reverse-engineering"]])
    check_("create/composed_plan_without_code_generation", {}, [feature + ["--skip", "code-generation"]])
    # A grid that skips an initialization stage, and a graph with no operation stage at all.
    def grid_skips_init(tree):
        grid = tree.json(".claude/tools/data/scope-grid.json")
        grid["feature"]["stages"]["workspace-scaffold"] = "SKIP"
        tree.write_json(".claude/tools/data/scope-grid.json", grid)
    check_("create/grid_skips_an_init_stage", {}, [feature], edits=(grid_skips_init,))
    def no_operation(tree):
        graph = tree.json(".claude/tools/data/stage-graph.json")
        tree.write_json(".claude/tools/data/stage-graph.json", [stage for stage in graph if stage["phase"] != "operation"])
    check_("create/graph_without_operation", {}, [feature], edits=(no_operation,))
    check_("create/ceremonies", {}, [feature + ["--sensors", "off", "--learnings", "OFF", "--summary-confirmation", "on", "--collaborators", "on"]])
    check_("create/guard_policy_scope_default", {}, [feature + ["--guard-policy", "off"]])
    check_("create/guard_policy_raised", {}, [feature + ["--guard-policy", "relaxed"], create + ["--scope", "feature", "--label", "two", "--guard-policy", "STRICT"]])
    check_("create/guard_policy_strict_on_strict_scope", {}, [create + ["--scope", "enterprise", "--label", "x", "--guard-policy", "strict"]])
    check_("create/guard_policy_lowered", {}, [create + ["--scope", "enterprise", "--label", "x", "--guard-policy", "off"],
                                               create + ["--scope", "enterprise", "--label", "x", "--guard-policy", "relaxed"]])
    check_("create/guard_policy_lowered_unattended", {}, [create + ["--scope", "enterprise", "--label", "x", "--guard-policy", "off"]], {"UNATTENDED": "1"})
    check_("create/guard_policy_lowered_bypassed", {}, [create + ["--scope", "enterprise", "--label", "x", "--guard-policy", "off"]],
           {"SKIP_HUMAN_PRESENCE_GUARD": "1"})
    check_("create/guard_policy_lowered_at_a_terminal", {}, [create + ["--scope", "enterprise", "--label", "x", "--guard-policy", "off"]],
           {"TEST_CONFIG_TTY": "1"})
    check_("create/guard_policy_lowered_at_a_terminal_bypassed", {}, [create + ["--scope", "enterprise", "--label", "x", "--guard-policy", "off"]],
           {"TEST_CONFIG_TTY": "1", "SKIP_HUMAN_PRESENCE_GUARD": "1"})
    check_("create/guard_policy_lowered_in_a_hosts_terminal", {}, [create + ["--scope", "enterprise", "--label", "x", "--guard-policy", "off"]],
           {"TEST_CONFIG_TTY": "1", "=TERM_PROGRAM": "cursor"})
    check_("create/guard_policy_lowered_under_a_hosts_mark", {}, [create + ["--scope", "enterprise", "--label", "x", "--guard-policy", "off"],
                                                                 feature + ["--plan-approval", "off"]],
           {"TEST_CONFIG_TTY": "1", "=CLAUDE_CODE_ENTRYPOINT": "cli"})
    check_("create/guard_policy_lowered_with_a_tools_config_only", {}, [create + ["--scope", "enterprise", "--label", "x", "--guard-policy", "off"]],
           {"TEST_CONFIG_TTY": "1", "=CODEX_HOME": "/x"})
    check_("create/change_control_retired", {}, [feature + ["--change-control", "relaxed"]])
    check_("create/plan_approval_off", {}, [feature + ["--plan-approval", "off"]])
    check_("create/plan_approval_off_unattended", {}, [feature + ["--plan-approval", "off"]], {"UNATTENDED": "1"})
    check_("create/plan_approval_off_bypassed", {}, [feature + ["--plan-approval", "off"]], {"SKIP_HUMAN_PRESENCE_GUARD": "1"})
    check_("create/plan_approval_off_by_scope", {}, [create + ["--scope", "express", "--label", "x", "--plan-approval", "off"],
                                                     create + ["--scope", "bugfix", "--label", "y", "--plan-approval", "off"]])
    check_("create/memory_strict", strict_team, [feature, create + ["--scope", "feature", "--label", "two", "--guard-policy", "relaxed"],
                                                 create + ["--scope", "feature", "--label", "three", "--guard-policy", "strict"],
                                                 create + ["--scope", "feature", "--label", "four", "--plan-approval", "off"]])
    check_("create/memory_unreadable", {"aidlc/spaces/default/memory/org.md": "## Guard Policy\n\nMode: maybe\n"}, [feature])
    check_("create/brownfield", {"src/app.py": "", "requirements.txt": ""}, [feature, create + ["--scope", "refactor", "--label", "x"]])
    check_("create/greenfield_scan_on_incremental_scope", {}, [create + ["--scope", "refactor", "--label", "x"], create + ["--scope", "bugfix", "--label", "y"]])
    check_("create/project_type_declared", {"src/app.py": ""}, [feature + ["--project-type", "Greenfield"], create + ["--scope", "refactor", "--label", "x", "--project-type", "greenfield"]])
    check_("create/project_type_brownfield_without_code", {}, [feature + ["--project-type", "brownfield"], create + ["--scope", "refactor", "--label", "x", "--project-type", "BROWNFIELD"]])
    check_("create/submodules", {".gitmodules": "[submodule \"one\"]\npath = libs/one\n[submodule \"two\"]\npath = two\n", "libs/one/.git": "gitdir: x\n"}, [feature])
    check_("create/submodules_many", {".gitmodules": "".join(f"[submodule \"m{n}\"]\npath = m{n}\n" for n in range(7))}, [feature])
    check_("create/repos_explicit", {"api/.git/HEAD": "", "web/.git/HEAD": ""}, [feature + ["--repos", "web, api,web"]])
    check_("create/repos_invalid", {}, [feature + ["--repos", "a/b"], feature + ["--repos", ".."], feature + ["--repos", "ok,-bad"]])
    check_("create/repos_discovered", {"api/.git/HEAD": "", "web/.git/HEAD": "", "docs/README.md": "", ".hidden/.git/HEAD": "", "node_modules/.git/HEAD": ""}, [feature])
    check_("create/space_explicit", {"aidlc/spaces/payments/memory/org.md": "# Org\n"}, [feature + ["--space", "payments"], create + ["--scope", "feature", "--label", "two", "--space", "default"]])
    check_("create/space_active", {"aidlc/active-space": "payments\n", "aidlc/spaces/payments/memory/org.md": "# Org\n"}, [feature])
    check_("create/space_cursor_without_a_dir", {"aidlc/active-space": "payments\n"}, [feature])
    check_("create/space_strict", {"aidlc/spaces/payments/memory/team.md": "# Team\n\n## Guard Policy\n\nMode: strict\n"},
           [feature + ["--space", "payments", "--guard-policy", "relaxed"], feature + ["--space", "payments"]])
    check_("create/then_state", {}, [feature, ["detect", "--json"], create + ["--scope", "bugfix", "--label", "fix it"]])
    check_("create/session_bound", {}, [feature], {"SESSION_OVERRIDE": "sess-0001"})
    check_("create/active_memory_current", {"aidlc/active-memory/org.md": "<!-- AI-DLC keeps this copy in step with aidlc/spaces/default/memory/org.md. Edit that file; this copy is replaced. -->\n# Org-Level Rules\n", "aidlc/active-memory/stale.md": "x\n"}, [feature])
    def binary_memory(tree):
        with open(tree.path("aidlc/spaces/default/memory/notes.md"), "wb") as handle:
            handle.write(b"\xff\xfe not text")
    # A recording holds text; the file that is not text is compared live only.
    UNRECORDED.add("utility/create/memory_binary")
    check_("create/memory_binary", {}, [feature], edits=(binary_memory,))
    check_("create/registry_malformed", {"aidlc/spaces/default/intents/intents.json": "{not a list"}, [feature])
    # The source baseline of the first row (see the jump's checks for each case).
    source = {"src/main.ts": "export const a = 1;\n", "src/crlf.txt": "a\r\nb\r\n", "build/out.js": "o\n", ".DS_Store": "d\n", "docs/guide.md": "# g\n"}
    check_("create/source_files", source, [feature])
    check_("create/source_registry", {**source, "build/keep.bin": "k\n", ".aidlc-source-paths.json": '{"version":1,"paths":["build/keep.bin"],"exclude":["docs"]}\n'}, [feature])
    check_("create/source_unreadable", {**source, ".aidlc-source-paths.json": "{"}, [feature])
    check_("create/source_unbindable", {**source, ".aidlc-source-paths.json": "{", ".git/HEAD": "ref: refs/heads/main\n"}, [feature])
    members = {"api/.git/HEAD": "", "web/.git/HEAD": "", "api/a.ts": "a\n", "web/b.ts": "b\n", "top.txt": "t\n"}
    check_("create/source_repos", members, [feature + ["--repos", "web,api"]])
    check_("create/source_repos_one", members, [feature + ["--repos", "web"]])
    check_("create/source_repos_unbindable", {**members, "api/.aidlc-source-paths.json": "{"}, [feature + ["--repos", "web,api"]])
    check_("create/source_dotnet_outputs", {"App.csproj": "<Project />\n", "Program.cs": "class P {}\n", "bin/Debug/app.dll": "b\n", "obj/project.assets.json": "{}\n"}, [feature])
    check_("create/delegated_worktree", {**source, ".aidlc/worktree-meta.json": "{}\n"}, [feature])
    check_("create/source_embedded_unresolved", {"vendor/lib/.git/HEAD": "ref: refs/heads/gone\n", "vendor/lib/.git/config": "[core]\n", "vendor/lib/x.c": "x\n"}, [feature])
    check_("create/registry_existing", {"aidlc/spaces/default/intents/intents.json": '[{"uuid":"u","slug":"old","dirName":"250101-old","status":"done"}]\n'}, [feature])
    for marker in ("STOP_HOOK_PROBE", "ROUTE_CHECK"):
        check_(f"create/observer_{marker.lower()}", {}, [feature], {marker: "1"})
    check_("create/flat_layout", {"aidlc-docs/aidlc-state.md": "# State\n- **Scope**: feature\n"}, [feature])
    check_("create/creation_grant_on_disk", {"aidlc/.aidlc-sessions/plan-approval/plan-approval-off-at-creation-sess-0001.json": "{}\n"},
           [feature], {"SESSION_OVERRIDE": "sess-0001"})
    check_("create/presence_bypass_on_disk", {"aidlc/.aidlc-sessions/plan-approval/presence-bypass-sess-0001": "2026-01-01T00:00:00Z\n"},
           [create + ["--scope", "enterprise", "--label", "x", "--guard-policy", "off"]], {"SESSION_OVERRIDE": "sess-0001", "SKIP_HUMAN_PRESENCE_GUARD": "1"})
    check_("create/record_dir_taken", {f"aidlc/spaces/default/intents/{TODAY}-add-login/.keep": ""}, [feature])
    check_("create/pending_person_lines", {"aidlc/.aidlc-sessions/sess-0001.person-lines": json.dumps({"work": None, "lines": [{"line": "Sensors are off for this piece of work.", "at": int(time.time() * 1000)}], "said": []}) + "\n"},
           [feature], {"SESSION_OVERRIDE": "sess-0001"})
    check_("create/memory_seed_absent", {"aidlc/spaces/default/memory/org.md": None, "aidlc/spaces/default/memory/team.md": None,
                                         "aidlc/spaces/default/memory/project.md": None}, [feature])


# --------------------------------------------------------------------------
# The lifecycle transitions.
#
# A check starts from a record upstream created (`intent-create --scope
# feature --review none --summary-confirmation off`), written here as it
# was created so a transition's reading of it is what is compared: the
# summary-confirmation ceremony off and the review class `none` are what
# let a transition run whole on both engines (admission, M3 slice 4).
# --------------------------------------------------------------------------

CREATED_DIR = "aidlc/spaces/default/intents/260105-add-login"
CREATED_SHARD = f"{CREATED_DIR}/audit/laptop-abc123.md"
CREATED_STATE = '# AI-DLC State Tracking\n\n## Project Information\n- **Project**: Add login to the app\n- **Project Description Source**: project-description.json\n- **Project Type**: Greenfield\n- **Project Type Source**: workspace scan\n- **Scope**: feature\n- **Start Date**: 2026-01-05T10:00:00Z\n- **State Version**: 8\n- **Active Agent**: aidlc-product-agent\n- **Worktree Path**:\n- **Bolt Refs**:\n- **Practices Affirmed Timestamp**:\n\n## Scope Configuration\n- **Stages to Execute**: 0.1, 0.2, 0.3, 1.1, 1.2, 1.3, 1.4, 1.5, 1.6, 1.7, 2.2, 2.3, 2.4, 2.5, 2.6, 2.7, 2.8, 2.9, 3.1, 3.2, 3.3, 3.4, 3.5, 3.6, 3.7, 4.1, 4.2, 4.3, 4.4, 4.5, 4.6, 4.7\n- **Stages to Skip**: 2.1 (reverse-engineering — greenfield)\n- **Depth**: Standard\n- **Test Strategy**: Standard\n- **Review Override**: none\n- **Guard Policy**: off (from scope feature)\n- **Sensors**: on (from scope feature)\n- **Learnings**: on (from scope feature)\n- **Summary Confirmation**: off (set by a command)\n- **Plan Approval**: on (from scope feature)\n- **Collaborators**: off (from scope feature)\n\n## Workspace State\n- **Project Root**: .\n- **Languages**: Unknown\n- **Frameworks**: Unknown\n- **Build System**: Unknown\n\n## Execution Plan Summary\n- **Total Stages**: 32\n- **Completed**: 3\n- **In Progress**: intent-capture\n\n## Runtime State\n- **Revision Count**: 0\n- **Construction Checkpoints**: enabled\n- **Construction Iteration**: unit-major\n- **Construction Execution**: serial\n\n## Phase Progress\n<!-- Status values: Pending, Active, Verified, Skipped -->\n\n- **Initialization**: Verified\n- **Ideation**: Active\n- **Inception**: Pending\n- **Construction**: Pending\n- **Operation**: Pending\n\n## Stage Progress\n<!-- Checkbox states: [ ] not started, [-] in progress, [?] awaiting approval (gate open), [R] revising (user rejected gate), [x] completed, [S] skipped via --stage/--phase jump -->\n\n### INITIALIZATION PHASE\n- [x] workspace-scaffold — EXECUTE\n- [x] workspace-detection — EXECUTE\n- [x] state-init — EXECUTE\n\n### IDEATION PHASE\n- [-] intent-capture — EXECUTE\n- [ ] market-research — EXECUTE\n- [ ] feasibility — EXECUTE\n- [ ] scope-definition — EXECUTE\n- [ ] team-formation — EXECUTE\n- [ ] rough-mockups — EXECUTE\n- [ ] approval-handoff — EXECUTE\n\n### INCEPTION PHASE\n- [ ] reverse-engineering — SKIP\n- [ ] practices-discovery — EXECUTE\n- [ ] requirements-analysis — EXECUTE\n- [ ] user-stories — EXECUTE\n- [ ] refined-mockups — EXECUTE\n- [ ] domain-design — EXECUTE\n- [ ] units-generation — EXECUTE\n- [ ] contract-design — EXECUTE\n- [ ] delivery-planning — EXECUTE\n\n### CONSTRUCTION PHASE\nPer unit: [TBD]\n- [ ] functional-design — EXECUTE\n- [ ] nfr-requirements — EXECUTE\n- [ ] nfr-design — EXECUTE\n- [ ] infrastructure-design — EXECUTE\n- [ ] code-generation — EXECUTE\n- [ ] build-and-test — EXECUTE\n- [ ] ci-pipeline — EXECUTE\n\n### OPERATION PHASE\n- [ ] deployment-pipeline — EXECUTE\n- [ ] environment-provisioning — EXECUTE\n- [ ] deployment-execution — EXECUTE\n- [ ] observability-setup — EXECUTE\n- [ ] incident-response — EXECUTE\n- [ ] performance-validation — EXECUTE\n- [ ] feedback-optimization — EXECUTE\n\n## Current Status\n- **Lifecycle Phase**: IDEATION\n- **Current Stage**: intent-capture\n- **Next Stage**: market-research\n- **Status**: Running\n- **Last Updated**: 2026-01-05T10:00:00Z\n\n## Session Resume Point\n- **Last Completed Stage**: state-init\n- **Next Action**: Execute intent-capture\n- **Pending Artifacts**: none\n'
CREATED_AUDIT = '# AI-DLC Audit Log\n\n## Workflow Start\n**Timestamp**: 2026-01-05T10:00:00Z\n**Event**: WORKFLOW_STARTED\n**Scope**: feature\n**Request**: /aidlc Add login to the app\n**Review Override**: none\n\n---\n\n## Phase Start\n**Timestamp**: 2026-01-05T10:00:00Z\n**Event**: PHASE_STARTED\n**Phase**: initialization\n**Stage count**: 3\n**Scope**: feature\n\n---\n\n## Stage Start\n**Timestamp**: 2026-01-05T10:00:00Z\n**Event**: STAGE_STARTED\n**Stage**: workspace-scaffold\n**Agent**: orchestrator\n\n---\n\n## Workspace Scaffolded\n**Timestamp**: 2026-01-05T10:00:00Z\n**Event**: WORKSPACE_SCAFFOLDED\n**Request**: /aidlc Add login to the app\n**Details**: 5 in-scope phase dirs + verification/ + space-level knowledge/ ensured (shell shipped by SEED)\n\n---\n\n## Stage Completion\n**Timestamp**: 2026-01-05T10:00:00Z\n**Event**: STAGE_COMPLETED\n**Stage**: workspace-scaffold\n**Details**: 5 in-scope phase dirs + verification/ + space-level knowledge/ ensured\n\n---\n\n## Stage Start\n**Timestamp**: 2026-01-05T10:00:00Z\n**Event**: STAGE_STARTED\n**Stage**: workspace-detection\n**Agent**: orchestrator\n\n---\n\n## Workspace Scanned\n**Timestamp**: 2026-01-05T10:00:00Z\n**Event**: WORKSPACE_SCANNED\n**Project Type**: Greenfield\n**Languages**: Unknown\n**Frameworks**: Unknown\n**Build System**: Unknown\n**Details**: Deterministic rule-based scan\n\n---\n\n## Stage Completion\n**Timestamp**: 2026-01-05T10:00:00Z\n**Event**: STAGE_COMPLETED\n**Stage**: workspace-detection\n**Details**: Classified Greenfield; languages=Unknown; frameworks=Unknown\n\n---\n\n## Stage Start\n**Timestamp**: 2026-01-05T10:00:00Z\n**Event**: STAGE_STARTED\n**Stage**: state-init\n**Agent**: orchestrator\n\n---\n\n## Workspace Initialised\n**Timestamp**: 2026-01-05T10:00:00Z\n**Event**: WORKSPACE_INITIALISED\n**Request**: /aidlc Add login to the app\n**Project Type**: Greenfield\n**Project Type Source**: workspace scan\n**Scope**: feature\n**Languages**: Unknown\n**Frameworks**: Unknown\n**Build System**: Unknown\n**Details**: 32 stages in scope, routing to intent-capture\n\n---\n\n## Stage Completion\n**Timestamp**: 2026-01-05T10:00:00Z\n**Event**: STAGE_COMPLETED\n**Stage**: state-init\n**Details**: State initialized: feature scope, 32 stages, routing to intent-capture\n\n---\n\n## Phase Completion\n**Timestamp**: 2026-01-05T10:00:00Z\n**Event**: PHASE_COMPLETED\n**From phase**: initialization\n**To phase**: ideation\n**Stages completed**: 3\n\n---\n\n## Phase Verification\n**Timestamp**: 2026-01-05T10:00:00Z\n**Event**: PHASE_VERIFIED\n**Phase boundary**: initialization → ideation\n\n---\n\n## Phase Start\n**Timestamp**: 2026-01-05T10:00:00Z\n**Event**: PHASE_STARTED\n**Phase**: ideation\n**Scope**: feature\n\n---\n\n## Stage Start\n**Timestamp**: 2026-01-05T10:00:00Z\n**Event**: STAGE_STARTED\n**Stage**: intent-capture\n**Agent**: aidlc-product-agent\n\n---\n'
CREATED_REGISTRY = ('[\n  {\n    "uuid": "01a138fa-d37d-787d-af4a-966efbb0b6fd",\n    "slug": "add-login",\n'
                    '    "dirName": "260105-add-login",\n    "scope": "feature",\n    "status": "in-flight"\n  }\n]\n')


def created(state=CREATED_STATE, audit=CREATED_AUDIT, registry=CREATED_REGISTRY, **more):
    """A created record's files: its state, its shard, its registry row, the cursor and the clone identity."""
    files = {**CLONE, "aidlc/spaces/default/intents/active-intent": "260105-add-login\n",
             "aidlc/spaces/default/intents/intents.json": registry, f"{CREATED_DIR}/aidlc-state.md": state,
             CREATED_SHARD: audit, f"{CREATED_DIR}/project-description.json": '"Add login to the app"\n'}
    files.update(more)
    return files


# What a registry row may hold as `repos` that is not a list of text, and an empty name, which is text.
NOT_NAMES = (("not_a_name", '["app", 7]'), ("null_name", '[null]'), ("a_list_inside", '[["app"]]'), ("not_a_list", '"app"'), ("empty_name", '["app", ""]'))


def artifact(stage, name, text="# Written\n", phase="ideation"):
    return {f"{CREATED_DIR}/{phase}/{stage}/{name}.md": text}


def at_stage(state, current, done=(), skipped=(), phase="IDEATION", next_stage=None):
    """`state` with `current` in progress, `done` ticked and `skipped` passed, the cursor fields following."""
    for slug in done:
        state = state.replace(f"- [ ] {slug} ", f"- [x] {slug} ").replace(f"- [-] {slug} ", f"- [x] {slug} ")
    for slug in skipped:
        state = state.replace(f"- [ ] {slug} ", f"- [S] {slug} ").replace(f"- [-] {slug} ", f"- [S] {slug} ")
    state = state.replace("- [-] intent-capture ", "- [ ] intent-capture ").replace(f"- [ ] {current} ", f"- [-] {current} ")
    state = re.sub(r"- \*\*Current Stage\*\*: [^\n]*", f"- **Current Stage**: {current}", state)
    state = re.sub(r"- \*\*In Progress\*\*: [^\n]*", f"- **In Progress**: {current}", state)
    state = re.sub(r"- \*\*Lifecycle Phase\*\*: [^\n]*", f"- **Lifecycle Phase**: {phase}", state)
    if next_stage is not None:
        state = re.sub(r"- \*\*Next Stage\*\*: [^\n]*", f"- **Next Stage**: {next_stage}", state)
    return state


def status_checks():
    """`aidlc-utility.ts status`: the workflow as a person reads it."""
    check_ = lambda name, files, steps, env=None, edits=(): runtime_check(name, files, steps, env, edits, group="utility")
    ic = artifact("intent-capture", "intent-statement")
    ideation = ["intent-capture", "market-research", "feasibility", "scope-definition", "team-formation", "rough-mockups"]
    status = [["status"]]
    direct = {"ALLOW_DIRECT_STATE_TRANSITIONS": "1"}
    check_("status/no_workflow", created(**{f"{CREATED_DIR}/aidlc-state.md": None}), status)
    check_("status/no_record", {**CLONE}, status)
    check_("status/fresh", created(), status)
    check_("status/by_flags", created(), [["status", "--intent", "260105-add-login"], ["status", "--space", "default"], ["status", "--intent", "nope"], ["status", "--space", "nope"]])
    check_("status/after_advance", created(**ic), [["@state", "advance", "intent-capture"], ["status"]], direct)
    check_("status/sparse_state", created("# State\n- **Scope**: feature\n"), status)
    check_("status/unknown_everything", created("# State\n- **Project**: \n- **Current Stage**: nope\n- **Active Agent**: Not A Slug\n- **Last Completed Stage**: nope\n- **Next Stage**: nope\n"), status)
    # The status line: the gate open (and since when), revising, approved and ready, parked.
    awaiting = CREATED_STATE.replace("- [-] intent-capture — EXECUTE", "- [?] intent-capture — EXECUTE")
    opened = CREATED_AUDIT + block("STAGE_AWAITING_APPROVAL", T(1), "**Stage**: intent-capture")
    # The wait is measured against the clock: the replay runs later.
    for name in ("awaiting", "awaiting_cleared", "awaiting_tied", "awaiting_recovered", "awaiting_reopened", "awaiting_after_jump", "awaiting_other_stage_rows"):
        UNRECORDED.add(f"utility/status/{name}")
    check_("status/awaiting", created(awaiting, opened), status)
    check_("status/awaiting_no_row", created(awaiting), status)
    check_("status/awaiting_cleared", created(awaiting, opened + block("GATE_REJECTED", T(2), "**Stage**: intent-capture")), status)
    check_("status/awaiting_tied", created(awaiting, opened, **{f"{CREATED_DIR}/audit/desktop-def456.md": TITLE + block("GATE_APPROVED", T(1), "**Stage**: intent-capture")}), status)
    check_("status/awaiting_recovered", created(awaiting, CREATED_AUDIT + block("STAGE_AWAITING_APPROVAL", T(1), "**Stage**: intent-capture", "**Recovered**: true")), status)
    check_("status/awaiting_reopened", created(awaiting, opened + block("GATE_REJECTED", T(2), "**Stage**: intent-capture") + block("STAGE_AWAITING_APPROVAL", T(3), "**Stage**: intent-capture")), status)
    check_("status/awaiting_after_jump", created(awaiting, opened + block("STAGE_JUMPED", T(2), "**Target**: intent-capture")), status)
    check_("status/awaiting_other_stage_rows", created(awaiting, opened + block("GATE_APPROVED", T(2), "**Stage**: market-research") + block("STAGE_STARTED", T(2, 1), "**Stage**: intent-capture", "**Workflow**: single-stage:x")), status)
    check_("status/awaiting_bad_timestamp", created(awaiting, CREATED_AUDIT + block("STAGE_AWAITING_APPROVAL", "yesterday", "**Stage**: intent-capture")), status)
    check_("status/revising", created(CREATED_STATE.replace("- [-] intent-capture — EXECUTE", "- [R] intent-capture — EXECUTE").replace("- **Revision Count**: 0", "- **Revision Count**: 2")), status)
    check_("status/revising_no_count", created(CREATED_STATE.replace("- [-] intent-capture — EXECUTE", "- [R] intent-capture — EXECUTE").replace("- **Revision Count**: 0\n", "")), status)
    check_("status/approved_ready", created(CREATED_STATE.replace("- [-] intent-capture — EXECUTE", "- [x] intent-capture — EXECUTE")), status)
    check_("status/completed_not_running", created(CREATED_STATE.replace("- [-] intent-capture — EXECUTE", "- [x] intent-capture — EXECUTE").replace("- **Status**: Running", "- **Status**: Completed")), status)
    check_("status/parked", created(CREATED_STATE.replace("- **Construction Execution**: serial\n", "- **Construction Execution**: serial\n- **Parked**: 2026-01-05T10:05:00Z\n- **Parked At Stage**: intent-capture\n")), status)
    # The plan, the project type, the depth.
    check_("status/plan", created(CREATED_STATE.replace("- **Scope**: feature\n", "- **Scope**: feature\n- **Plan**: lean-login\n")), status)
    check_("status/project_type", created(CREATED_STATE.replace("- **Project Type**: Greenfield", "- **Project Type**: Brownfield").replace("- **Project Type Source**: workspace scan", "- **Project Type Source**: you")), status)
    check_("status/project_type_scanned", created(CREATED_STATE.replace("- **Project Type**: Greenfield", "- **Project Type**: brownfield"),
                                                  CREATED_AUDIT + block("STAGE_COMPLETED", T(1), "**Stage**: reverse-engineering") + block("STAGE_COMPLETED", T(3), "**Stage**: reverse-engineering") + block("STAGE_COMPLETED", "soon", "**Stage**: reverse-engineering")), status)
    check_("status/project_type_unknown", created(CREATED_STATE.replace("- **Project Type**: Greenfield", "- **Project Type**: Mixed")), status)
    check_("status/depth", created(CREATED_STATE.replace("- **Depth**: Standard", "- **Depth**: Deep").replace("- **Test Strategy**: Standard", "- **Test Strategy**: Light")), status)
    check_("status/depth_from_plan", created(CREATED_STATE.replace("- **Scope**: feature\n", "- **Scope**: feature\n- **Plan**: lean-login\n").replace("- **Test Strategy**: Standard", "- **Test Strategy**: standard")), status)
    check_("status/no_depth", created(CREATED_STATE.replace("- **Depth**: Standard\n", "").replace("- **Test Strategy**: Standard\n", "")), status)
    # The guard policy and the checks switched off, the ceremonies.
    check_("status/guard_policy_strict", created(CREATED_STATE.replace("- **Guard Policy**: off (from scope feature)", "- **Guard Policy**: strict (set by you)")), status)
    check_("status/guard_policy_memory", created(**{"aidlc/spaces/default/memory/project.md": "## Guard Policy\n\nMode: strict\n"}), status)
    check_("status/guard_policy_unreadable", created(**{"aidlc/spaces/default/memory/project.md": "## Guard Policy\n\nMode: maybe\n"}), status)
    check_("status/guard_policy_invalid_line", created(CREATED_STATE.replace("- **Guard Policy**: off (from scope feature)", "- **Guard Policy**: sometimes")), status)
    check_("status/guard_policy_plan", created(CREATED_STATE.replace("- **Scope**: feature\n", "- **Scope**: feature\n- **Plan**: lean-login\n")), status)
    strict = CREATED_STATE.replace("- **Guard Policy**: off (from scope feature)", "- **Guard Policy**: strict (set by you)")
    check_("status/checks_off_by_you", created(strict.replace("- **Collaborators**: off (from scope feature)\n", "- **Collaborators**: off (from scope feature)\n- **Guards Off**: review-freeze, state-transition (set by you)\n")), status)
    check_("status/checks_off_by_env", created(strict), status, {"DISABLE_REVIEW_FREEZE_HOOK": "1", "SKIP_HUMAN_PRESENCE_GUARD": "1"})
    check_("status/checks_off_mixed", created(strict.replace("- **Collaborators**: off (from scope feature)\n", "- **Collaborators**: off (from scope feature)\n- **Guards Off**: reviewer-scope (set by you)\n")), status, {"DISABLE_PLAN_APPROVAL_GUARD": "1"})
    check_("status/checks_off_by_policy_not_listed", created(), status)
    check_("status/ceremony_lines", created(CREATED_STATE.replace("- **Sensors**: on (from scope feature)", "- **Sensors**: off (set by you)").replace("- **Learnings**: on (from scope feature)", "- **Learnings**: on").replace("- **Plan Approval**: on (from scope feature)", "- **Plan Approval**: off (from scope feature)")), status)
    check_("status/ceremony_kill_switches", created(), status, {"DISABLE_SENSORS": "1", "DISABLE_LEARNINGS": "1", "DISABLE_COLLABORATORS": "1"})
    check_("status/ceremony_malformed_lines", created(CREATED_STATE.replace("- **Sensors**: on (from scope feature)", "- **Sensors**: maybe").replace("- **Learnings**: on (from scope feature)", "- **Learnings**: ")), status)
    check_("status/ceremony_no_lines", created(CREATED_STATE.replace("- **Sensors**: on (from scope feature)\n", "").replace("- **Plan Approval**: on (from scope feature)\n", "")), status)
    check_("status/ceremony_scope_unknown", created(CREATED_STATE.replace("- **Scope**: feature\n", "- **Scope**: nope\n").replace("- **Sensors**: on (from scope feature)\n", "")), status)
    check_("status/plan_approval_off_by_you", created(CREATED_STATE.replace("- **Plan Approval**: on (from scope feature)", "- **Plan Approval**: off (set by you)")), status)
    check_("status/plan_approval_off_locked_by_memory", created(CREATED_STATE.replace("- **Plan Approval**: on (from scope feature)", "- **Plan Approval**: off (set by you)"), **{"aidlc/spaces/default/memory/team.md": "## Guard Policy\n\nMode: strict\n"}), status)
    check_("status/plan_approval_off_memory_unreadable", created(CREATED_STATE.replace("- **Plan Approval**: on (from scope feature)", "- **Plan Approval**: off (set by you)"), **{"aidlc/spaces/default/memory/team.md": "## Guard Policy\n\nMode: nope\n"}), status)
    check_("status/plan_approval_odd_source", created(CREATED_STATE.replace("- **Plan Approval**: on (from scope feature)", "- **Plan Approval**: on (from somewhere else)")), status)
    check_("status/plan_approval_switch_untrusted", created(), status, {"DISABLE_PLAN_APPROVAL_GUARD": "1", "SESSION_OVERRIDE": "s1"})
    check_("status/plan_approval_switch_with_session_recorded", created(**{"aidlc/.aidlc-sessions/.current-session": "s1\n"}), status, {"DISABLE_PLAN_APPROVAL_GUARD": "1"})
    check_("status/plan_approval_switch_bypass_recorded", created(**{"aidlc/.aidlc-sessions/plan-approval/plan-approval-bypass-s1": "2026-01-05T10:00:00Z\n"}), status, {"DISABLE_PLAN_APPROVAL_GUARD": "1", "SESSION_OVERRIDE": "s1"})
    check_("status/plan_approval_switch_bypass_malformed", created(**{"aidlc/.aidlc-sessions/plan-approval/plan-approval-bypass-s1": "soon\n"}), status, {"DISABLE_PLAN_APPROVAL_GUARD": "1", "SESSION_OVERRIDE": "s1"})
    # Completion and the phase bars: skipped stages, suffixes, every box state, a unit walk.
    bars = at_stage(CREATED_STATE, "feasibility", done=["intent-capture"], skipped=["market-research"]).replace("- [ ] scope-definition — EXECUTE", "- [?] scope-definition — EXECUTE").replace("- [ ] team-formation — EXECUTE", "- [R] team-formation — EXECUTE")
    check_("status/bars", created(bars), status)
    check_("status/bars_suffixes", created(bars.replace("- [ ] rough-mockups — EXECUTE", "- [ ] rough-mockups — SKIP").replace("- [ ] reverse-engineering — SKIP", "- [ ] reverse-engineering — EXECUTE")), status)
    check_("status/bars_unknown_scope", created(CREATED_STATE.replace("- **Scope**: feature\n", "")), status)
    check_("status/bars_scope_without_stages", created(CREATED_STATE.replace("- **Scope**: feature\n", "- **Scope**: nope\n")), status)
    construction = at_stage(CREATED_STATE, "functional-design", done=ideation + ["approval-handoff", "practices-discovery", "requirements-analysis", "user-stories", "refined-mockups", "domain-design", "units-generation", "contract-design", "delivery-planning"], phase="CONSTRUCTION")
    edge_block = "# Dependencies\n\n## Machine-Readable Edge Block\n\n```yaml\nunits:\n  - name: api\n    depends_on: []\n  - name: web\n    depends_on: [api]\n```\n"
    unit_gate = lambda minute, event, unit: block(event, T(minute), "**Stage**: code-generation", "**Checkpoint**: construction-unit", f"**Unit**: {unit}")
    check_("status/units_walk", created(construction, CREATED_AUDIT + unit_gate(1, "GATE_APPROVED", "api"), **{UNITS.replace(RECORD_DIR, CREATED_DIR): edge_block}), status)
    check_("status/units_walk_none_done", created(construction, **{UNITS.replace(RECORD_DIR, CREATED_DIR): edge_block}), status)
    check_("status/units_walk_all_done", created(construction, CREATED_AUDIT + unit_gate(1, "GATE_APPROVED", "api") + unit_gate(2, "GATE_APPROVED", "web"), **{UNITS.replace(RECORD_DIR, CREATED_DIR): edge_block}), status)
    check_("status/units_walk_rejected", created(construction, CREATED_AUDIT + unit_gate(1, "GATE_APPROVED", "api") + unit_gate(2, "GATE_REJECTED", "api") + unit_gate(3, "GATE_APPROVED", "nope"), **{UNITS.replace(RECORD_DIR, CREATED_DIR): edge_block}), status)
    check_("status/units_walk_from_runtime_graph", created(construction, **{f"{CREATED_DIR}/runtime-graph.json": json.dumps({"bolt_dag": {"units": [{"name": "api"}, {"name": "bad name"}, {"name": 3}]}})}), status)
    check_("status/units_walk_stage_major", created(construction.replace("- **Construction Iteration**: unit-major", "- **Construction Iteration**: stage-major"), **{UNITS.replace(RECORD_DIR, CREATED_DIR): edge_block}), status)
    check_("status/units_walk_no_dag", created(construction), status)
    check_("status/current_step", created(construction + "- **Active Unit**: api\n- **Unit Stage**: code-generation\n"), status)
    check_("status/current_step_same_stage", created(construction + "- **Active Unit**: api\n- **Unit Stage**: functional-design\n"), status)
    check_("status/current_step_bad_unit", created(construction + "- **Active Unit**: bad name\n- **Unit Stage**: code-generation\n"), status)
    check_("status/current_step_not_per_unit", created(CREATED_STATE + "- **Active Unit**: api\n- **Unit Stage**: code-generation\n"), status)
    # A stage is per unit by its marker, or by being one of the five that always are: a compiled graph that lost
    # the marker on one of them still names the step. A stage that is none of the five is per unit by its marker alone.
    for label, slugs, unit_stage in (("unit_stage", ["code-generation"], "code-generation"), ("current_stage", ["functional-design"], "code-generation"),
                                     ("every", ["functional-design", "nfr-requirements", "nfr-design", "infrastructure-design", "code-generation"], "nfr-design"),
                                     ("not_one_of_the_five", ["code-generation"], "build-and-test")):
        check_(f"status/current_step_no_marker_{label}", created(construction + f"- **Active Unit**: api\n- **Unit Stage**: {unit_stage}\n"), status,
               edits=[lambda tree, slugs=slugs: no_marker(tree, slugs)])
    check_("status/directive_marker", created(**{f"{GATE_DIR}/active-directive.json": json.dumps({"stage": "market-research", "unit": "api"})}), status)
    check_("status/directive_marker_by_flags", created(**{f"{GATE_DIR}/active-directive.json": json.dumps({"stage": "market-research"})}), [["status", "--space", "default"]])
    check_("status/team", created(CREATED_STATE + "- **Unit Ownership**: team\n"), status)
    # Other work open in the space.
    other_registry = CREATED_REGISTRY.replace("]", ',\n  {\n    "uuid": "01a138fa-d37d-787d-af4a-966efbb0b6fe",\n    "slug": "add-logout",\n    "dirName": "260105-add-logout",\n    "scope": "feature",\n    "status": "in-flight"\n  },\n  {\n    "uuid": "01a138fa-d37d-787d-af4a-966efbb0b6ff",\n    "slug": "done",\n    "dirName": "260105-done",\n    "scope": "feature",\n    "status": "complete"\n  }\n]')
    others = {"aidlc/spaces/default/intents/260105-add-logout/aidlc-state.md": "# State\n- **Status**: Running\n",
              "aidlc/spaces/default/intents/260105-done/aidlc-state.md": "# State\n- **Status**: Running\n",
              "aidlc/spaces/default/intents/260105-orphan/aidlc-state.md": "# State\n- **Status**: Running\n",
              "aidlc/spaces/default/intents/260105-finished/aidlc-state.md": "# State\n- **Status**: Completed\n",
              "aidlc/spaces/other/intents/260105-elsewhere/aidlc-state.md": "# State\n- **Status**: Running\n"}
    check_("status/also_open", created(registry=other_registry, **others), status)
    check_("status/also_open_one", created(registry=other_registry, **{k: v for k, v in others.items() if "logout" in k}), status)
    check_("status/also_open_registry_malformed", created(registry="{nope", **others), status)
    # Validity: a finished stage whose receipt no longer matches, and what depends on it.
    receipt = lambda inputs, outputs: json.dumps({"schema": 3, "graphContract": "sha256:" + "0" * 64, "projectType": "greenfield", "inputs": inputs, "outputs": outputs}, separators=(",", ":"))
    basis = lambda artifact, producer: {"artifact": artifact, "producer": producer, "required": True, "instanceCount": 1, "presentCount": 1, "structureHash": "sha256:" + "1" * 64, "contentHash": "sha256:" + "2" * 64}
    at_feasibility = at_stage(CREATED_STATE, "feasibility", done=["intent-capture", "market-research"])
    completed_with = lambda minute, stage, text: block("STAGE_COMPLETED", T(minute), f"**Stage**: {stage}", f"**Validation Basis**: {text}")
    check_("status/validity_stale", created(at_feasibility, CREATED_AUDIT + completed_with(1, "intent-capture", receipt([], [basis("intent-statement", "intent-capture")])), **ic), status)
    check_("status/validity_stale_propagates", created(at_feasibility, CREATED_AUDIT + completed_with(1, "intent-capture", receipt([], [basis("intent-statement", "intent-capture")])) +
                                                        completed_with(2, "market-research", receipt([basis("intent-statement", "intent-capture")], [])), **ic, **artifact("market-research", "competitive-analysis")), status)
    check_("status/validity_only_dependent", created(at_feasibility, CREATED_AUDIT + completed_with(2, "market-research", receipt([basis("intent-statement", "intent-capture")], [])), **ic), status)
    check_("status/validity_older_schema", created(at_feasibility, CREATED_AUDIT + completed_with(1, "intent-capture", json.dumps({"schema": 1, "x": 1}))), status)
    check_("status/validity_malformed", created(at_feasibility, CREATED_AUDIT + completed_with(1, "intent-capture", "{nope") + completed_with(2, "market-research", receipt([{"artifact": "a", "producer": "b"}], []))), status)
    check_("status/validity_superseded_by_legacy", created(at_feasibility, CREATED_AUDIT + completed_with(1, "intent-capture", receipt([], [basis("intent-statement", "intent-capture")])) +
                                                       completed_with(2, "intent-capture", json.dumps({"schema": 1})), **ic), status)
    check_("status/validity_restarted_attempt", created(at_feasibility, CREATED_AUDIT + completed_with(1, "intent-capture", receipt([], [basis("intent-statement", "intent-capture")])) + STARTED_ROW(2, "intent-capture")), status)
    check_("status/validity_project_type", created(at_feasibility.replace("- **Project Type**: Greenfield", "- **Project Type**: Brownfield"), CREATED_AUDIT + completed_with(1, "intent-capture", receipt([], [basis("intent-statement", "intent-capture")])), **ic), status)
    check_("status/validity_project_type_only", created(at_feasibility.replace("- **Project Type**: Greenfield", "- **Project Type**: Brownfield"),
                                                        CREATED_AUDIT + completed_with(1, "intent-capture", json.dumps({"schema": 3, "graphContract": "x", "projectType": "greenfield", "inputs": [], "outputs": []}))), status)
    check_("status/validity_fresh_receipt", created(**ic), [["@state", "advance", "intent-capture"], ["status"]], direct)
    check_("status/validity_unavailable", created(at_stage(CREATED_STATE, "requirements-analysis", done=ideation + ["approval-handoff"], phase="INCEPTION").replace("- **Scope**: feature\n", ""),
                                                   CREATED_AUDIT + completed_with(1, "practices-discovery", receipt([], []))), status)
    for marker in ("STOP_HOOK_PROBE", "ROUTE_CHECK"):
        check_(f"observer/{marker.lower()}_status", created(), status, {marker: "1"})


def transition_checks():
    """`aidlc-state.ts advance`, `finalize`, `complete-workflow`, `skip` and `resume`: the transitions, what admits
    them, and what a replay writes once."""
    check_ = lambda name, files, steps, env=None, edits=(): runtime_check(name, files, steps, env, edits, group="state")
    direct = {"ALLOW_DIRECT_STATE_TRANSITIONS": "1"}
    ic = artifact("intent-capture", "intent-statement")
    ideation = ["intent-capture", "market-research", "feasibility", "scope-definition", "team-formation", "rough-mockups"]

    # advance: usage, the slugs, the scope, and the whole transition.
    check_("advance/usage", created(), [["advance"], ["advance", "--flag"]], direct)
    check_("advance/unknown_stage", created(), [["advance", "nope"], ["advance", "intent-capture", "nope"]], direct)
    check_("advance/no_scope", created(CREATED_STATE.replace("- **Scope**: feature\n", "")), [["advance", "intent-capture"]], direct)
    check_("advance/empty_scope", created(CREATED_STATE.replace("- **Scope**: feature\n", "- **Scope**: \n")), [["advance", "intent-capture"]], direct)
    check_("advance/invalid_scope", created(CREATED_STATE.replace("- **Scope**: feature\n", "- **Scope**: nope\n")), [["advance", "intent-capture"]], direct)
    check_("advance/not_current", created(**ic), [["advance", "market-research"], ["advance", "feasibility", "scope-definition"]], direct)
    check_("advance/no_artifacts", created(), [["advance", "intent-capture"], ["resume"]], direct)
    check_("advance/whole", created(**ic), [["advance", "intent-capture"], ["resume"], ["advance", "intent-capture"], ["get", "Completed"]], direct)
    check_("advance/explicit_next", created(**ic), [["advance", "intent-capture", "feasibility"], ["resume"]], direct)
    check_("advance/explicit_next_skipped", created(**ic), [["advance", "intent-capture", "reverse-engineering"]], direct)
    check_("advance/explicit_next_plan_skip", created(CREATED_STATE.replace("- [ ] market-research \u2014 EXECUTE", "- [ ] market-research \u2014 SKIP"), **ic),
           [["advance", "intent-capture", "market-research"], ["advance", "intent-capture"]], direct)
    check_("advance/no_next", created(at_stage(CREATED_STATE, "feedback-optimization", done=ideation), **artifact("feedback-optimization", "slo-report", phase="operation")),
           [["advance", "feedback-optimization"]], direct)
    check_("advance/phase_boundary", created(at_stage(CREATED_STATE, "approval-handoff", done=ideation), **artifact("approval-handoff", "initiative-brief")),
           [["advance", "approval-handoff"], ["resume"], ["get", "Ideation"], ["get", "Inception"]], direct)
    check_("advance/already_ticked", created(at_stage(CREATED_STATE, "intent-capture", done=["intent-capture"]), **ic),
           [["advance", "intent-capture"], ["resume"]], direct)
    check_("advance/already_ticked_and_audited", created(at_stage(CREATED_STATE, "intent-capture", done=["intent-capture"]),
                                                          CREATED_AUDIT + COMPLETED_ROW(1, "intent-capture"), **ic),
           [["advance", "intent-capture"], ["advance", "intent-capture"]], direct)
    # Ticked with the next stage open but no completion row: not a replay, and the row is written now.
    check_("advance/already_ticked_next_started", created(at_stage(CREATED_STATE, "market-research", done=["intent-capture"]), **ic),
           [["advance", "intent-capture"], ["resume"]], direct)
    # A completion row at the workflow's own instant still counts for it.
    check_("advance/already_ticked_audited_same_instant", created(at_stage(CREATED_STATE, "market-research", done=["intent-capture"]),
                                                                   CREATED_AUDIT + COMPLETED_ROW(0, "intent-capture"), **ic),
           [["advance", "intent-capture"]], direct)
    check_("advance/subagent_stage", created(at_stage(CREATED_STATE, "practices-discovery", done=ideation + ["approval-handoff"], phase="INCEPTION"),
                                             **artifact("practices-discovery", "team-practices", phase="inception")),
           [["advance", "practices-discovery"], ["resume"]], direct)
    check_("advance/archived", created(CREATED_STATE.replace("- **Status**: Running", "- **Status**: Archived"), **ic),
           [["advance", "intent-capture"], ["finalize", "intent-capture"], ["complete-workflow", "intent-capture"],
            ["skip", "market-research"], ["resume"], ["get", "Scope"]], direct)
    check_("advance/artifact_guard_off", created(), [["advance", "intent-capture"]], {**direct, "SKIP_ARTIFACT_GUARD": "1"})
    check_("advance/artifact_is_a_directory", created(**{f"{CREATED_DIR}/ideation/intent-capture/intent-statement.md/x": ""}),
           [["advance", "intent-capture"]], direct)
    check_("advance/crlf_artifact", created(**artifact("intent-capture", "intent-statement", "# Written\r\nline\r\n")),
           [["advance", "intent-capture"]], direct)
    check_("advance/optional_output_present", created(at_stage(CREATED_STATE, "requirements-analysis", done=ideation + ["approval-handoff"], phase="INCEPTION"),
                                                      **artifact("requirements-analysis", "requirements", phase="inception")),
           [["advance", "requirements-analysis"]], direct)
    # The evidence a transition owes to `aidlc-log` records (M5): refused by name where upstream weighs it.
    check_("advance/summary_owed", created(CREATED_STATE.replace("- **Summary Confirmation**: off (set by a command)", "- **Summary Confirmation**: on (from scope feature)"), **ic),
           [["advance", "intent-capture"]], direct)
    check_("advance/summary_kill_switch", created(CREATED_STATE.replace("- **Summary Confirmation**: off (set by a command)", "- **Summary Confirmation**: on (from scope feature)"), **ic),
           [["advance", "intent-capture"]], {**direct, "SKIP_SUMMARY_CONFIRMATION_GUARD": "1"})
    check_("advance/summary_ceremony_off_by_switch", created(CREATED_STATE.replace("- **Summary Confirmation**: off (set by a command)", "- **Summary Confirmation**: on (from scope feature)"), **ic),
           [["advance", "intent-capture"]], {**direct, "DISABLE_SUMMARY_CONFIRMATION": "1"})
    check_("advance/review_owed", created(CREATED_STATE.replace("- **Review Override**: none", "- **Review Override**: "), **ic),
           [["advance", "intent-capture"]], direct)
    check_("advance/review_owed_advisory_cap", created(CREATED_STATE.replace("- **Review Override**: none", "- **Review Override**: advisory"), **ic),
           [["advance", "intent-capture"]], direct)
    check_("advance/review_rows_in_ledger", created(CREATED_STATE.replace("- **Review Override**: none", "- **Review Override**: "),
                                                     CREATED_AUDIT + block("REVIEW_COMPLETED", T(2), "**Stage**: intent-capture", "**Reviewer**: aidlc-product-lead-agent", "**Verdict**: READY"), **ic),
           [["advance", "intent-capture"]], direct)
    check_("advance/no_reviewer_without_override", created(CREATED_STATE.replace("- **Review Override**: none", "- **Review Override**: "), **artifact("market-research", "competitive-analysis")),
           [["skip", "intent-capture", "--route", "--reason", "no"], ["advance", "market-research"]], direct)
    pipeline = at_stage(CREATED_STATE.replace("- [ ] reverse-engineering \u2014 SKIP", "- [ ] reverse-engineering \u2014 EXECUTE"),
                        "reverse-engineering", done=ideation + ["approval-handoff"], phase="INCEPTION")
    codekb = {f"aidlc/spaces/default/codekb/app/{name}.md": "# Known\n" for name in
              ("business-overview", "architecture", "code-structure", "api-documentation", "component-inventory", "technology-stack",
               "dependencies", "code-quality-assessment", "reverse-engineering-timestamp")}
    with_repo = CREATED_REGISTRY.replace('"status": "in-flight"', '"status": "in-flight",\n    "repos": ["app"]')
    check_("advance/pipeline_owed", created(pipeline, registry=with_repo, **codekb), [["advance", "reverse-engineering"]], direct)
    check_("advance/pipeline_evidence_off", created(pipeline, registry=with_repo, **codekb), [["advance", "reverse-engineering"]],
           {**direct, "DISABLE_ENSEMBLE_EVIDENCE": "1"})
    check_("advance/codekb_incomplete", created(pipeline, registry=with_repo, **{k: v for k, v in codekb.items() if "architecture" not in k}),
           [["advance", "reverse-engineering"]], {**direct, "DISABLE_ENSEMBLE_EVIDENCE": "1"})
    # A row whose `repos` is not a list of text, read for the stores of the code knowledge base.
    for label, listed in NOT_NAMES:
        check_(f"advance/repos_{label}", created(pipeline, registry=CREATED_REGISTRY.replace('"status": "in-flight"', '"status": "in-flight",\n    "repos": ' + listed), **codekb), [["advance", "reverse-engineering"]],
               {**direct, "DISABLE_ENSEMBLE_EVIDENCE": "1"})
    check_("advance/construction", created(at_stage(CREATED_STATE, "functional-design", done=ideation + ["approval-handoff"], phase="CONSTRUCTION"),
                                           **{f"{CREATED_DIR}/construction/api/functional-design/entities.md": "# E\n"}),
           [["advance", "functional-design"]], direct)
    # The artifact of one of the five stages that always run per unit, consumed by another stage: it is resolved
    # under each unit, whether the compiled graph kept the stage's marker or lost it.
    def consumes_entities(tree, marker):
        graph = tree.json(".claude/tools/data/stage-graph.json")
        for stage in graph:
            if stage["slug"] == "intent-capture":
                stage["consumes"] = [{"artifact": "entities", "required": True}]
            if stage["slug"] == "functional-design" and not marker:
                stage.pop("for_each", None)
        tree.write_json(".claude/tools/data/stage-graph.json", graph)
    edge_block = "# Dependencies\n\n## Machine-Readable Edge Block\n\n```yaml\nunits:\n  - name: api\n    depends_on: []\n  - name: web\n    depends_on: [api]\n```\n"
    for label, files in (("under_units", {f"{CREATED_DIR}/construction/api/functional-design/entities.md": "# E\n", f"{CREATED_DIR}/construction/web/functional-design/entities.md": "# W\n",
                                          UNITS.replace(RECORD_DIR, CREATED_DIR): edge_block}),
                         ("at_the_stage", {f"{CREATED_DIR}/construction/functional-design/entities.md": "# E\n"}), ("nowhere", {})):
        for marker in (True, False):
            check_(f"advance/consumes_per_unit_{label}" + ("" if marker else "_no_marker"), created(**ic, **files), [["advance", "intent-capture"]], direct,
                   edits=[lambda tree, marker=marker: consumes_entities(tree, marker)])
    usage = {"aidlc/.aidlc-sessions/usage-ledger.json": json.dumps({"schemaVersion": 3, "cursors": {}, "workflows": {"w": {
        "totals": {"tokens": {"input": 10, "output": 5, "cacheRead": 0, "cacheCreate5m": 0, "cacheCreate1h": 0}}, "byStage": {"intent-capture": {}}}}})}
    check_("advance/usage_ledger", created(**ic, **usage), [["advance", "intent-capture"]], direct)
    check_("advance/usage_ledger_off", created(**ic, **usage), [["advance", "intent-capture"]], {**direct, "DISABLE_USAGE_TRACKING": "1"})
    check_("advance/answer_mode_reused", created(audit=CREATED_AUDIT + block("DECISION_RECORDED", T(1), "**Stage**: intent-capture",
                                                 "**Decision**: How would you like to answer them?", "**Options**: Guide me, I'll edit the file, Chat") +
                                                 block("QUESTION_ANSWERED", T(2), "**Stage**: intent-capture", "**Details**: Chat"), **ic),
           [["advance", "intent-capture"]], direct)
    # The receipt's inputs: optional ones present, brownfield-only ones, a project type that is neither, and a
    # producer that cannot be told (a warning instead of a receipt).
    check_("advance/optional_input_present", created(at_stage(CREATED_STATE, "feasibility", done=["intent-capture", "market-research"]),
                                                     **artifact("feasibility", "raid-log"), **artifact("market-research", "competitive-analysis")),
           [["advance", "feasibility"]], direct)
    inception = at_stage(CREATED_STATE, "requirements-analysis", done=ideation + ["approval-handoff"], phase="INCEPTION")
    brownfield = {**codekb, **artifact("requirements-analysis", "requirements", phase="inception"), **artifact("intent-capture", "intent-statement")}
    check_("advance/brownfield_inputs", created(inception.replace("- **Project Type**: Greenfield", "- **Project Type**: Brownfield"), registry=with_repo, **brownfield),
           [["advance", "requirements-analysis"]], direct)
    check_("advance/greenfield_inputs", created(inception, registry=with_repo, **brownfield), [["advance", "requirements-analysis"]], direct)
    check_("advance/project_type_unknown", created(inception.replace("- **Project Type**: Greenfield", "- **Project Type**: Mixed"), registry=with_repo, **brownfield),
           [["advance", "requirements-analysis"]], direct)
    check_("advance/codekb_repo_unrecorded", created(inception.replace("- **Project Type**: Greenfield", "- **Project Type**: Brownfield"), **brownfield),
           [["advance", "requirements-analysis"]], direct)
    check_("advance/codekb_two_unclaimed_stores", created(inception.replace("- **Project Type**: Greenfield", "- **Project Type**: Brownfield"), **brownfield,
                                                          **{"aidlc/spaces/default/codekb/other/architecture.md": "# Other\n"}),
           [["advance", "requirements-analysis"]], direct)
    def two_producers(tree):
        graph = tree.json(".claude/tools/data/stage-graph.json")
        next(stage for stage in graph if stage["slug"] == "market-research")["produces"].append("intent-statement")
        tree.write_json(".claude/tools/data/stage-graph.json", graph)
    check_("advance/validation_warning", created(at_stage(CREATED_STATE, "scope-definition", done=["intent-capture", "market-research", "feasibility"]),
                                                 **artifact("scope-definition", "scope-document")),
           [["advance", "scope-definition"]], direct, edits=(two_producers,))
    # Inputs a per-unit stage produced: from the unit DAG, from the directories when there is none, at stage level
    # when Units Generation was not planned, and a warning when the DAG cannot be read.
    operation = at_stage(CREATED_STATE, "deployment-pipeline", done=ideation + ["approval-handoff"], phase="OPERATION")
    per_unit = {f"{CREATED_DIR}/construction/api/infrastructure-design/cicd-pipeline.md": "# api\n",
                f"{CREATED_DIR}/construction/web/infrastructure-design/cicd-pipeline.md": "# web\n",
                f"{CREATED_DIR}/construction/web/infrastructure-design/infrastructure-specification.md": "# web\n",
                f"{CREATED_DIR}/construction/docs/README.md": "no stage dir\n",
                **artifact("deployment-pipeline", "cd-config", phase="operation")}
    check_("advance/per_unit_inputs_from_dirs", created(operation, **per_unit), [["advance", "deployment-pipeline"]], direct)
    check_("advance/per_unit_inputs_from_dag", created(operation, **per_unit, **{UNITS.replace(RECORD_DIR, CREATED_DIR): A_UNITS}), [["advance", "deployment-pipeline"]], direct)
    check_("advance/per_unit_inputs_dag_malformed", created(operation, **per_unit, **{UNITS.replace(RECORD_DIR, CREATED_DIR): "```yaml\nunits:\n  - name: api\n    depends_on: [nope]\n```\n"}),
           [["advance", "deployment-pipeline"]], direct)
    check_("advance/per_unit_inputs_stage_level", created(operation.replace("- [ ] units-generation \u2014 EXECUTE", "- [S] units-generation \u2014 SKIP"), **per_unit,
                                                           **{f"{CREATED_DIR}/construction/infrastructure-design/cicd-pipeline.md": "# all\n"}),
           [["advance", "deployment-pipeline"]], direct)
    check_("advance/per_unit_inputs_units_done", created(operation.replace("- [ ] units-generation \u2014 EXECUTE", "- [x] units-generation \u2014 SKIP"), **per_unit),
           [["advance", "deployment-pipeline"]], direct)
    check_("advance/per_unit_inputs_cached_dag", created(operation, **per_unit, **{f"{CREATED_DIR}/runtime-graph.json": json.dumps(
        {"bolt_dag": {"batches": [["auth"], ["api"]], "units": [{"name": "auth", "kind": "service"}]}})}),
           [["advance", "deployment-pipeline"]], direct)
    check_("advance/fence", created(**ic), [["advance", "intent-capture"]])
    check_("advance/fence_up", created(CREATED_STATE.replace("off (from scope feature)", "strict (set by you)"), **ic), [["advance", "intent-capture"], ["resume"]])
    check_("advance/no_state_file", created(**{f"{CREATED_DIR}/aidlc-state.md": None}), [["advance", "intent-capture"], ["resume"]], direct)

    # finalize and complete-workflow.
    check_("finalize/usage", created(), [["finalize"], ["finalize", "--x"]], direct)
    check_("finalize/unknown_stage", created(), [["finalize", "nope"]], direct)
    check_("finalize/no_artifacts", created(), [["finalize", "intent-capture"]], direct)
    check_("finalize/whole", created(**ic), [["finalize", "intent-capture"], ["resume"], ["finalize", "intent-capture"], ["advance", "intent-capture"]], direct)
    check_("finalize/phase_boundary", created(at_stage(CREATED_STATE, "approval-handoff", done=ideation), **artifact("approval-handoff", "initiative-brief")),
           [["finalize", "approval-handoff"], ["resume"]], direct)
    check_("finalize/last_stage", created(at_stage(CREATED_STATE, "feedback-optimization", done=ideation), **artifact("feedback-optimization", "slo-report", phase="operation")),
           [["finalize", "feedback-optimization"], ["resume"]], direct)
    check_("finalize/no_scope", created(CREATED_STATE.replace("- **Scope**: feature\n", ""), **ic), [["finalize", "intent-capture"]], direct)
    check_("complete/usage", created(), [["complete-workflow"], ["complete-workflow", "--reason", "x"]], direct)
    check_("complete/unknown_stage", created(), [["complete-workflow", "nope"]], direct)
    check_("complete/no_artifacts", created(), [["complete-workflow", "intent-capture", "--reason", "done"]], direct)
    check_("complete/whole", created(**ic), [["complete-workflow", "intent-capture", "--reason", "enough"], ["resume"], ["get", "Status"]], direct)
    check_("complete/without_reason", created(**ic), [["complete-workflow", "intent-capture"], ["complete-workflow", "intent-capture"]], direct)
    check_("complete/reason_is_a_flag", created(**ic), [["complete-workflow", "intent-capture", "--reason", "--route"], ["complete-workflow", "--reason"]], direct)
    check_("complete/after_finalize", created(**ic), [["finalize", "intent-capture"], ["complete-workflow", "intent-capture"]], direct)
    check_("complete/no_scope", created(CREATED_STATE.replace("- **Scope**: feature\n", ""), **ic), [["complete-workflow", "intent-capture"]], direct)
    check_("complete/registry_without_row", created(registry="[]\n", **ic), [["complete-workflow", "intent-capture"]], direct)
    check_("complete/registry_malformed", created(registry="{not a list", **ic), [["complete-workflow", "intent-capture"]], direct)

    # skip: plain, routed, and what a replay writes once.
    # A stage that produces code starts from a source baseline, kept beside the record under the stage's own name:
    # reached by an advance, and by a skip of the stage before it.
    source = {"src/main.ts": "export const a = 1;\n", "src/crlf.txt": "a\r\nb\r\n", "build/out.js": "o\n", "docs/guide.md": "# g\n"}
    unbindable = {".aidlc-source-paths.json": "{", ".git/HEAD": "ref: refs/heads/main\n"}
    into_code = [["advance", "intent-capture", "code-generation"], ["resume"]]
    check_("advance/into_code_stage", created(**ic, **source), into_code, direct)
    check_("advance/into_code_stage_unreadable", created(**ic, **source, **{".aidlc-source-paths.json": "{"}), into_code, direct)
    check_("advance/into_code_stage_unbindable", created(**ic, **source, **unbindable), into_code, direct)
    answered = CREATED_AUDIT + block("DECISION_RECORDED", T(1), "**Stage**: intent-capture", "**Decision**: How would you like to answer them?",
                                     "**Options**: Guide me, I'll edit the file, Chat") + block("QUESTION_ANSWERED", T(2), "**Stage**: intent-capture", "**Details**: Chat")
    check_("advance/into_code_stage_answer_mode", created(audit=answered, **ic, **source), into_code, direct)
    with open(os.path.join(DIST, ".claude", "tools", "data", "stage-graph.json"), encoding="utf-8") as handle:
        order = [stage["slug"] for stage in json.load(handle)]
    before_code = at_stage(CREATED_STATE, "infrastructure-design", done=order[:order.index("infrastructure-design")], phase="CONSTRUCTION")
    routed = ["skip", "infrastructure-design", "--route", "--reason", "not needed"]
    check_("skip/into_code_stage", created(before_code, **source), [routed, ["resume"]], direct)
    check_("skip/into_code_stage_unbindable", created(before_code, **source, **unbindable), [routed], direct)
    check_("skip/before_code_stage_unrouted", created(before_code, **source), [["skip", "infrastructure-design"], ["resume"]], direct)
    check_("skip/usage", created(), [["skip"]], direct)
    check_("skip/unknown_stage", created(), [["skip", "nope"], ["skip", "--route"]], direct)
    check_("skip/plain", created(), [["skip", "market-research"], ["skip", "feasibility", "--reason", "  not needed  "], ["skip", "intent-capture"],
                                     ["count", "skipped"], ["skip", "market-research"], ["resume"]], direct)
    check_("skip/plain_refused_states", created(at_stage(CREATED_STATE, "market-research", done=["intent-capture"])),
           [["skip", "intent-capture"], ["skip", "nope"]], direct)
    check_("skip/reason_without_value", created(), [["skip", "market-research", "--reason"], ["skip", "market-research", "--reason", "--route"]], direct)
    check_("skip/route_and_unit", created(), [["skip", "intent-capture", "--route", "--unit", "api", "--reason", "x"]], direct)
    check_("skip/unit", created(), [["skip", "intent-capture", "--unit", "api", "--reason", "x"], ["skip", "intent-capture", "--unit"]], direct)
    check_("skip/route_needs_reason", created(), [["skip", "intent-capture", "--route"], ["skip", "intent-capture", "--route", "--reason", " "]], direct)
    check_("skip/route_whole", created(), [["skip", "intent-capture", "--route", "--reason", "not this one"], ["resume"],
                                           ["skip", "intent-capture", "--route", "--reason", "again"]], direct)
    check_("skip/route_not_current", created(), [["skip", "market-research", "--route", "--reason", "x"]], direct)
    check_("skip/route_awaiting_approval", created(CREATED_STATE.replace("- [-] intent-capture \u2014 EXECUTE", "- [?] intent-capture \u2014 EXECUTE")),
           [["skip", "intent-capture", "--route", "--reason", "x"]], direct)
    check_("skip/route_awaiting_approval_plan_skip", created(CREATED_STATE.replace("- [-] intent-capture \u2014 EXECUTE", "- [?] intent-capture \u2014 SKIP")),
           [["skip", "intent-capture", "--route", "--reason", "new project"], ["resume"]], direct)
    check_("skip/route_phase_boundary", created(at_stage(CREATED_STATE, "approval-handoff", done=ideation)),
           [["skip", "approval-handoff", "--route", "--reason", "x"], ["resume"]], direct)
    check_("skip/route_last_stage", created(at_stage(CREATED_STATE, "feedback-optimization", done=ideation)),
           [["skip", "feedback-optimization", "--route", "--reason", "x"], ["resume"]], direct)
    check_("skip/route_no_scope", created(CREATED_STATE.replace("- **Scope**: feature\n", "")), [["skip", "intent-capture", "--route", "--reason", "x"]], direct)
    skipped_state = CREATED_STATE.replace("- [-] intent-capture \u2014 EXECUTE", "- [S] intent-capture \u2014 EXECUTE")
    check_("skip/route_replay_after_crash", created(skipped_state, CREATED_AUDIT + block("STAGE_SKIPPED", T(1), "**Stage**: intent-capture", "**Reason**: x", "**Skip Kind**: conditional-runtime")),
           [["skip", "intent-capture", "--route", "--reason", "x"], ["resume"]], direct)
    check_("skip/route_replay_partial", created(skipped_state, CREATED_AUDIT + block("STAGE_SKIPPED", T(1), "**Stage**: intent-capture", "**Reason**: x", "**Skip Kind**: conditional-runtime") +
                                                STARTED_ROW(1, "market-research")),
           [["skip", "intent-capture", "--route", "--reason", "x"]], direct)
    check_("skip/route_replay_after_restart", created(skipped_state, CREATED_AUDIT + block("STAGE_SKIPPED", T(1), "**Stage**: intent-capture", "**Reason**: x", "**Skip Kind**: conditional-runtime") +
                                                      STARTED_ROW(2, "intent-capture")),
           [["skip", "intent-capture", "--route", "--reason", "x"]], direct)
    # A workflow started again: what the earlier one recorded is not this one's.
    check_("skip/route_replay_after_workflow_restart", created(skipped_state, CREATED_AUDIT + block("STAGE_SKIPPED", T(1), "**Stage**: intent-capture", "**Reason**: x", "**Skip Kind**: conditional-runtime") +
                                                               block("WORKFLOW_STARTED", T(2), "**Scope**: feature")),
           [["skip", "intent-capture", "--route", "--reason", "x"]], direct)
    last_skipped = at_stage(CREATED_STATE, "feedback-optimization", done=ideation).replace("- [-] feedback-optimization", "- [S] feedback-optimization")
    check_("skip/route_last_stage_replay", created(last_skipped, CREATED_AUDIT + block("STAGE_SKIPPED", T(1), "**Stage**: feedback-optimization", "**Reason**: x", "**Skip Kind**: conditional-runtime") +
                                                   block("PHASE_COMPLETED", T(1), "**From phase**: operation", "**To phase**: (end)", "**Stages completed**: 9")),
           [["skip", "feedback-optimization", "--route", "--reason", "x"]], direct)
    check_("skip/route_usage_ledger", created(at_stage(CREATED_STATE, "feedback-optimization", done=ideation), **usage),
           [["skip", "feedback-optimization", "--route", "--reason", "x"]], direct)
    check_("skip/fence", created(), [["skip", "market-research"]])
    check_("skip/fence_up", created(CREATED_STATE.replace("off (from scope feature)", "strict (set by you)")), [["skip", "market-research"]])

    # resume.
    check_("resume/fresh", created(), [["resume", "extra", "--flags"]])
    check_("resume/compaction_pending", created(audit=CREATED_AUDIT + block("SESSION_COMPACTED", T(1), "**Details**: x")), [["resume"]])
    check_("resume/compaction_handled", created(audit=CREATED_AUDIT + block("SESSION_COMPACTED", T(1), "**Details**: x") + block("RECOVERY_COMPLETED", T(2), "**Choice**: continue")), [["resume"]])
    check_("resume/unit_fields", created(CREATED_STATE + "- **Active Unit**: api\n- **Unit Pause Reason**: \n"), [["resume"]])
    check_("resume/unit_fields_full", created(CREATED_STATE + "- **Active Unit**: api\n- **Unit State**: paused\n- **Unit Pause Reason**: lunch\n- **Unit Next Action**: resume\n"), [["resume"]])
    check_("resume/sparse_state", created("# State\n- **Scope**: \n"), [["resume"]])
    check_("resume/no_shard", created(**{CREATED_SHARD: None}), [["resume"]])
    check_("resume/no_state_file", created(**{f"{CREATED_DIR}/aidlc-state.md": None}), [["resume"]])
    check_("resume/needs_no_gate", created(CREATED_STATE.replace("off (from scope feature)", "strict (set by you)")), [["resume"]])
    for marker in ("STOP_HOOK_PROBE", "ROUTE_CHECK"):
        check_(f"observer/{marker.lower()}_advance", created(**ic), [["advance", "intent-capture"], ["skip", "market-research"], ["resume"]], {marker: "1", **direct})


GATE_STATE = CREATED_STATE.replace("- **Sensors**: on (from scope feature)", "- **Sensors**: off (set by a command)")
GATE_DIR = f"{CREATED_DIR}/.aidlc-engine"
HUMAN = lambda minute, second=0, *fields: block("HUMAN_TURN", T(minute, second), "**Session**: s1", *fields)



# --------------------------------------------------------------------------
# The hooks (M4): each one on the host's payload, in a record this conversation has joined or not.

def hook_checks():
    """`aidlc.ts engine hook <name>` for the observers Focus has: what each records, where, and when it stands aside."""
    check_ = lambda name, files, steps, env=None, edits=(): runtime_check(name, files, steps, env, edits, group="hook")
    binding = lambda intent="260105-add-login", source="create", space="default": {
        "aidlc/.aidlc-sessions/s1.binding.json": json.dumps({"space": space, "intent": intent, "boundAt": "2026-01-05T10:00:00Z", "source": source})}
    payload = lambda **fields: json.dumps({"session_id": "s1", **fields})
    joined = created(**binding())
    no_cursor = {**created(**binding()), "aidlc/spaces/default/intents/active-intent": None}
    engine = f"{CREATED_DIR}/.aidlc-engine"

    # validate-state: the breadcrumb, the heartbeat and the row.
    compact = lambda **fields: ["validate-state", payload(hook_event_name="PreCompact", **fields)]
    check_("validate-state/joined", joined, [compact()])
    check_("validate-state/cursor_only", created(), [compact()])
    check_("validate-state/no_session", created(), [["validate-state", json.dumps({"hook_event_name": "PreCompact"})]])
    check_("validate-state/empty_payload", created(), [["validate-state", ""]])
    check_("validate-state/not_json", created(), [["validate-state", "{nope"]])
    check_("validate-state/session_id_alt", created(), [["validate-state", json.dumps({"sessionId": "s1"})]])
    check_("validate-state/invalid_session", created(), [["validate-state", json.dumps({"session_id": "../x"})]])
    check_("validate-state/outsider", {**no_cursor, **binding("other-9f8e7d6c")}, [compact()])
    check_("validate-state/unjoined", {**no_cursor, **binding(None, "unjoined")}, [compact()])
    check_("validate-state/untrusted_source", no_cursor | binding(source="worktree"), [compact()])
    for source in ("migration", "switch", "space-switch-cursor", "cursor", "stamp"):
        check_(f"validate-state/bound_by_{source}", no_cursor | binding(source=source), [compact()])
    check_("validate-state/bound_elsewhere_cursor_here", {**created(), **binding("other-9f8e7d6c", "worktree")}, [compact()])
    check_("validate-state/no_state", {**CLONE, "aidlc/spaces/default/intents/intents.json": CREATED_REGISTRY}, [compact()])
    check_("validate-state/missing_sections", created(CREATED_STATE.replace("## Stage Progress", "## Progress").replace("## Current Status", "## Status")), [compact()])
    check_("validate-state/missing_one_section", created(CREATED_STATE.replace("## Current Status", "## Status")), [compact()])
    check_("validate-state/no_shard", created(**{CREATED_SHARD: None}), [compact()])
    check_("validate-state/twice", joined, [compact(), compact()])
    check_("validate-state/rules_held", {**joined, "aidlc/.aidlc-sessions/s1.rules-held.json": '{"v": 2}'}, [compact()])
    check_("validate-state/directive_marker", {**joined, f"{engine}/active-directive.json": '{"stage": "intent-capture"}'}, [compact()])
    check_("validate-state/engine_dir_linked", joined, [compact()], edits=(lambda tree: os.symlink(tree.path("elsewhere"), tree.path(engine)),))
    check_("validate-state/unit_scope_stamp", {**joined, "aidlc/.aidlc-unit-scope.json": '{"version": 1}'}, [compact()])
    for marker in ("STOP_HOOK_PROBE", "ROUTE_CHECK"):
        check_(f"validate-state/observer_{marker.lower()}", joined, [compact()], {marker: "1"})
    # The resolutions Focus has not ported: a hook with no session of its own, and one whose binding settles it.
    pids = {"aidlc/.aidlc-sessions/pids/1.json": "{}\n"}
    worktree = {".aidlc/worktree-meta.json": "{}\n"}
    for hook, step in (("validate-state", compact), ("session-end", lambda: ["session-end", payload(reason="exit")]),
                       ("log-subagent", lambda: ["log-subagent", payload(agent_type="x")]),
                       ("sync-workflow-state", lambda: ["sync-workflow-state", payload(tool_name="TaskUpdate", tool_input={"status": "in_progress", "activeForm": "x [market-research]"})]),
                       ("write-audit-log", lambda: ["write-audit-log", payload(tool_name="Write", tool_input={"file_path": "{P}/" + CREATED_DIR + "/ideation/intent-capture/intent-statement.md"})]),
                       ("rebuild-stage-graph", lambda: ["rebuild-stage-graph", payload(tool_name="Bash", tool_input={"command": "aidlc state approve x"})])):
        short = {"sync-workflow-state": "sync", "rebuild-stage-graph": "rebuild"}.get(hook, hook)
        anonymous = json.loads(step()[1]); del anonymous["session_id"]
        check_(f"{short}/sessions_by_process_unmapped", {**created(), **pids}, [[hook, json.dumps(anonymous)]])
        check_(f"{short}/sessions_by_process_unbound", {**created(), **pids}, [step()])
        check_(f"{short}/sessions_by_process_bound", {**joined, **pids}, [step()])
        # This process mapped as the conversation's: a payload with no session still finds it, and its binding.
        check_(f"{short}/sessions_by_process", {**no_cursor, **binding(), **my_pid_entry()}, [[hook, json.dumps(anonymous)]])
        UNRECORDED.add(f"hook/{short}/sessions_by_process")
        check_(f"{short}/delegated_worktree", {**joined, **worktree}, [step()])

    # session-end: whose session, and where its end goes.
    end = lambda **fields: ["session-end", payload(**fields)]
    check_("session-end/joined", joined, [end(reason="exit")])
    check_("session-end/reasons", joined, [end(reason=reason) for reason in ("clear", "", 0, 7, True, False, None, {"a": 1}, ["x", "y"])])
    check_("session-end/no_reason", joined, [end()])
    check_("session-end/no_session", created(), [["session-end", json.dumps({"reason": "exit"})]])
    check_("session-end/empty_payload", created(), [["session-end", ""]])
    check_("session-end/not_json", created(), [["session-end", "nope"]])
    check_("session-end/not_object", created(), [["session-end", "[1]"]])
    check_("session-end/invalid_session", created(), [["session-end", json.dumps({"session_id": "x/y", "reason": "exit"})]])
    check_("session-end/bound_to_none", {**no_cursor, **binding(None, "none")}, [end(reason="exit")])
    check_("session-end/bound_unjoined", {**no_cursor, **binding(None, "unjoined")}, [end(reason="exit")])
    check_("session-end/bound_elsewhere", {**no_cursor, **binding("other-9f8e7d6c", "create")}, [end(reason="exit")])
    check_("session-end/bound_untrusted", {**no_cursor, **binding(source="cursor")}, [end(reason="exit")])
    check_("session-end/bound_worktree_source", {**no_cursor, **binding(source="worktree")}, [end(reason="exit")])
    check_("session-end/stamped", {**no_cursor, "aidlc/.aidlc-sessions/s1": "01a138fa-d37d-787d-af4a-966efbb0b6fd\n"}, [end(reason="exit")])
    check_("session-end/stamped_cursor", {**created(), "aidlc/.aidlc-sessions/s1": "01a138fa-d37d-787d-af4a-966efbb0b6fd\n"}, [end(reason="exit")])
    check_("session-end/stamped_unknown", {**created(), "aidlc/.aidlc-sessions/s1": "01a138fa-0000-787d-af4a-966efbb0b6fd\n"}, [end(reason="exit")])
    check_("session-end/stamped_blank", {**created(), "aidlc/.aidlc-sessions/s1": "  \n"}, [end(reason="exit")])
    check_("session-end/session_without_binding_uuid_backed", created(), [end(reason="exit")])
    legacy = CREATED_REGISTRY.replace('    "uuid": "01a138fa-d37d-787d-af4a-966efbb0b6fd",\n', "")
    check_("session-end/session_without_binding_flat", created(registry=legacy), [end(reason="exit")])
    check_("session-end/no_session_flat", created(registry=legacy), [["session-end", json.dumps({"reason": "exit"})]])
    check_("session-end/no_state", {**CLONE, "aidlc/spaces/default/intents/intents.json": CREATED_REGISTRY, **binding()}, [end(reason="exit")])
    check_("session-end/no_shard", created(**binding(), **{CREATED_SHARD: None}), [end(reason="exit")])
    check_("session-end/other_space", {**created(), "aidlc/active-space": "payments\n", "aidlc/spaces/payments/intents/intents.json": "[]\n", **binding(space="default")}, [end(reason="exit")])
    check_("session-end/unit_scope_stamp", {**joined, "aidlc/.aidlc-unit-scope.json": '{"version": 1}'}, [end(reason="exit")])

    # log-subagent: the completion row, when the workflow runs.
    done = lambda **fields: ["log-subagent", payload(hook_event_name="SubagentStop", **fields)]
    check_("log-subagent/plain", joined, [done(agent_type="aidlc-product-agent", agent_id="a1", last_assistant_message="Done.")])
    check_("log-subagent/minimal", joined, [done()])
    check_("log-subagent/blank_type", joined, [done(agent_type="  ", agent_id="", last_assistant_message="")])
    check_("log-subagent/long_message", joined, [done(agent_type="x", last_assistant_message="é" * 150 + "😀" * 40 + "tail")])
    check_("log-subagent/odd_values", joined, [done(agent_type=3, agent_id=7, last_assistant_message=["a", None, 2])])
    check_("log-subagent/multiline", joined, [done(agent_type="x", last_assistant_message="one\\ntwo\\n")])
    check_("log-subagent/cursor_only", created(), [done(agent_type="x")])
    check_("log-subagent/no_session", created(), [["log-subagent", json.dumps({"agent_type": "x"})]])
    check_("log-subagent/empty_payload", created(), [["log-subagent", ""]])
    check_("log-subagent/not_object", created(), [["log-subagent", "3"]])
    check_("log-subagent/invalid_session", created(), [["log-subagent", json.dumps({"session_id": "a/b", "agent_type": "x"})]])
    check_("log-subagent/not_running", created(CREATED_STATE.replace("- **Status**: Running", "- **Status**: Completed"), **binding()), [done(agent_type="x")])
    check_("log-subagent/no_state", {**CLONE, "aidlc/spaces/default/intents/intents.json": CREATED_REGISTRY, **binding()}, [done(agent_type="x")])
    check_("log-subagent/outsider", {**no_cursor, **binding("other-9f8e7d6c")}, [done(agent_type="x")])
    check_("log-subagent/inflight_ledger", {**joined, "aidlc/.aidlc-subagent-inflight": '{"version": 1, "entries": []}'}, [done(agent_type="x")])
    check_("log-subagent/no_shard", created(**binding(), **{CREATED_SHARD: None}), [done(agent_type="x")])

    # sync-workflow-state: the task list, and the audit tail.
    task = lambda **fields: ["sync-workflow-state", payload(hook_event_name="PostToolUse", tool_name="TaskUpdate", tool_input=fields)]
    check_("sync/in_progress", joined, [task(status="in_progress", activeForm="Researching [market-research]"), ["@state", "get", "Current Stage"], ["@state", "get", "Status"]])
    check_("sync/same_stage", joined, [task(status="in_progress", activeForm="Capturing [intent-capture]")])
    check_("sync/other_status", joined, [task(status="completed", activeForm="Researching [market-research]")])
    check_("sync/no_slug", joined, [task(status="in_progress", activeForm="Researching")])
    check_("sync/bad_slug", joined, [task(status="in_progress", activeForm="x [Market-Research]")])
    check_("sync/unknown_stage", joined, [task(status="in_progress", activeForm="x [not-a-stage]")])
    check_("sync/no_active_form", joined, [task(status="in_progress")])
    check_("sync/no_tool_input", joined, [["sync-workflow-state", payload(tool_name="TaskUpdate")]])
    check_("sync/cursor_only", created(), [task(status="in_progress", activeForm="x [market-research]"), ["@state", "get", "Current Stage"]])
    check_("sync/outsider", {**no_cursor, **binding("other-9f8e7d6c")}, [task(status="in_progress", activeForm="x [market-research]")])
    check_("sync/no_state", {**CLONE, "aidlc/spaces/default/intents/intents.json": CREATED_REGISTRY, **binding()}, [task(status="in_progress", activeForm="x [market-research]")])
    check_("sync/empty_payload", joined, [["sync-workflow-state", ""]])
    check_("sync/not_object", joined, [["sync-workflow-state", '"x"']])
    check_("sync/directive_marker", {**joined, f"{engine}/active-directive.json": '{"stage": "intent-capture"}'}, [task(status="in_progress", activeForm="x [market-research]")])
    check_("sync/completed_stage", created(at_stage(CREATED_STATE, "market-research", done=("intent-capture",)), **binding()),
           [task(status="in_progress", activeForm="x [intent-capture]"), ["@state", "get", "Current Stage"]])
    ide = lambda: ["sync-workflow-state", payload(hook_event_name="PostToolUse", tool_input={"source": "ide-audit-sync"})]
    check_("sync/ide_in_step", joined, [ide()])
    check_("sync/ide_forward", created(audit=CREATED_AUDIT + block("STAGE_STARTED", T(1), "**Stage**: market-research", "**Agent**: x"), **binding()), [ide(), ["@state", "get", "Current Stage"]])
    check_("sync/ide_single_stage_ignored", created(audit=CREATED_AUDIT + block("STAGE_STARTED", T(1), "**Stage**: market-research", "**Workflow**: single-stage:market-research"), **binding()), [ide()])
    check_("sync/ide_backward", created(at_stage(CREATED_STATE, "market-research", done=("intent-capture",)), **binding()), [ide(), ["@state", "get", "Current Stage"]])
    check_("sync/ide_not_running", created(CREATED_STATE.replace("- **Status**: Running", "- **Status**: Parked"), CREATED_AUDIT + block("STAGE_STARTED", T(1), "**Stage**: market-research"), **binding()), [ide()])
    check_("sync/ide_no_current", created(CREATED_STATE.replace("- **Current Stage**: intent-capture\\n", "- **Current Stage**: none\\n"), CREATED_AUDIT + block("STAGE_STARTED", T(1), "**Stage**: market-research"), **binding()), [ide()])
    check_("sync/ide_no_audit_slug", created(audit=TITLE + WORKFLOW, **binding()), [ide()])
    check_("sync/ide_bad_slug", created(audit=CREATED_AUDIT + block("STAGE_STARTED", T(1), "**Stage**: Not-A-Slug"), **binding()), [ide()])
    check_("sync/ide_unknown_stage", created(audit=CREATED_AUDIT + block("STAGE_STARTED", T(1), "**Stage**: not-a-stage"), **binding()), [ide()])
    check_("sync/ide_other_shard", created(**binding(), **{f"{CREATED_DIR}/audit/desktop-def456.md": TITLE + block("STAGE_STARTED", T(1), "**Stage**: market-research")}), [ide(), ["@state", "get", "Current Stage"]])
    for marker in ("STOP_HOOK_PROBE", "ROUTE_CHECK"):
        check_(f"sync/observer_{marker.lower()}", joined, [task(status="in_progress", activeForm="x [market-research]")], {marker: "1"})

    # write-audit-log: which writes are the record's artifacts.
    doc = f"{CREATED_DIR}/ideation/intent-capture/intent-statement.md"
    wrote = lambda tool="Write", path=None, **more: ["write-audit-log", payload(hook_event_name="PostToolUse", tool_name=tool, tool_input={"file_path": "{P}/" + (path or doc), **more})]
    with_doc = {**joined, doc: "# Intent\\n"}
    check_("write-audit-log/written", with_doc, [wrote()])
    check_("write-audit-log/edited", with_doc, [wrote("Edit")])
    check_("write-audit-log/other_tool", with_doc, [wrote("NotebookEdit")])
    check_("write-audit-log/missing_file", joined, [wrote()])
    # The record directory itself: created or updated by how long after its birth it was last written, which a
    # replay's own tree decides anew, so it is compared live and not recorded.
    check_("write-audit-log/record_root_itself", joined, [["write-audit-log", payload(tool_name="Write", tool_input={"file_path": "{P}/" + CREATED_DIR})]])
    UNRECORDED.add("hook/write-audit-log/record_root_itself")
    check_("write-audit-log/outside_record", {**joined, "src/main.ts": "x"}, [wrote(path="src/main.ts")])
    check_("write-audit-log/other_record", {**joined, "aidlc/spaces/default/intents/other-9f8e7d6c/x.md": "x"}, [wrote(path="aidlc/spaces/default/intents/other-9f8e7d6c/x.md")])
    check_("write-audit-log/codekb", {**joined, "aidlc/spaces/default/codekb/repo/summary.md": "x"}, [wrote(path="aidlc/spaces/default/codekb/repo/summary.md")])
    check_("write-audit-log/codekb_root_itself", joined, [wrote(path="aidlc/spaces/default/codekb")])
    check_("write-audit-log/audit_shard", joined, [wrote(path=CREATED_SHARD)])
    check_("write-audit-log/audit_md", {**joined, f"{CREATED_DIR}/audit.md": "x"}, [wrote(path=f"{CREATED_DIR}/audit.md")])
    check_("write-audit-log/audit_sibling", {**joined, f"{CREATED_DIR}/audit/notes.txt": "x"}, [wrote(path=f"{CREATED_DIR}/audit/notes.txt")])
    check_("write-audit-log/relative", with_doc, [["write-audit-log", payload(tool_name="Write", tool_input={"file_path": doc})]])
    check_("write-audit-log/backslashes", with_doc, [["write-audit-log", payload(tool_name="Write", tool_input={"file_path": "{P}/" + doc.replace("/", "\\\\")})]])
    check_("write-audit-log/no_file_path", joined, [["write-audit-log", payload(tool_name="Write", tool_input={})]])
    check_("write-audit-log/no_tool_input", joined, [["write-audit-log", payload(tool_name="Write")]])
    check_("write-audit-log/empty_payload", joined, [["write-audit-log", ""]])
    check_("write-audit-log/not_object", joined, [["write-audit-log", "null"]])
    check_("write-audit-log/no_shard", {**with_doc, CREATED_SHARD: None}, [wrote()])
    check_("write-audit-log/no_state", {**CLONE, "aidlc/spaces/default/intents/intents.json": CREATED_REGISTRY, **binding(), doc: "x"}, [wrote()])
    check_("write-audit-log/cursor_only", {**created(), doc: "x"}, [wrote()])
    check_("write-audit-log/outsider", {**no_cursor, **binding("other-9f8e7d6c"), doc: "x"}, [wrote()])
    check_("write-audit-log/authorization_registry", {**with_doc, f"{engine}/summary-authorization/x.json": "{}"}, [wrote()])
    check_("write-audit-log/authorization_registry_empty", with_doc, [wrote()], edits=(lambda tree: os.makedirs(tree.path(f"{engine}/summary-authorization")),))
    check_("write-audit-log/deep", {**joined, f"{CREATED_DIR}/construction/u1/code-generation/notes.md": "x"}, [wrote(path=f"{CREATED_DIR}/construction/u1/code-generation/notes.md")])
    check_("write-audit-log/twice", with_doc, [wrote(), wrote("Edit")])
    for marker in ("STOP_HOOK_PROBE", "ROUTE_CHECK"):
        check_(f"write-audit-log/observer_{marker.lower()}", with_doc, [wrote()], {marker: "1"})

    # Payload fields of the wrong kind.
    check_("validate-state/session_id_number", created(), [["validate-state", json.dumps({"session_id": 5})]])
    check_("validate-state/session_id_null_then_alt", no_cursor, [["validate-state", json.dumps({"session_id": None, "sessionId": "s1"})]])
    check_("validate-state/session_id_number_then_alt", no_cursor, [["validate-state", json.dumps({"session_id": 5, "sessionId": "s1"})]])
    check_("session-end/session_id_number", created(), [["session-end", json.dumps({"session_id": 5, "reason": "exit"})]])
    check_("session-end/session_id_empty", created(), [["session-end", json.dumps({"session_id": "", "reason": "exit"})]])
    check_("log-subagent/message_object", joined, [done(agent_type="x", last_assistant_message={"a": 1})])
    check_("log-subagent/session_id_number", joined, [["log-subagent", json.dumps({"session_id": 5, "agent_type": "x"})]])
    check_("sync/other_tool", joined, [["sync-workflow-state", payload(tool_name="Write", tool_input={"status": "in_progress", "activeForm": "x [market-research]"})]])
    check_("sync/active_form_number", joined, [task(status="in_progress", activeForm=5)])
    check_("sync/tool_input_string", joined, [["sync-workflow-state", payload(tool_name="TaskUpdate", tool_input="x")]])
    check_("sync/ide_source_other", joined, [["sync-workflow-state", payload(tool_input={"source": "other"})]])
    check_("write-audit-log/file_path_number", joined, [["write-audit-log", payload(tool_name="Write", tool_input={"file_path": 5})]])
    check_("write-audit-log/trailing_slash", with_doc, [["write-audit-log", payload(tool_name="Write", tool_input={"file_path": "{P}/" + doc + "/"})]])
    check_("write-audit-log/dotted", with_doc, [["write-audit-log", payload(tool_name="Write", tool_input={"file_path": "{P}/" + CREATED_DIR + "/ideation/../ideation/intent-capture/intent-statement.md"})]])
    check_("write-audit-log/directory", joined, [wrote(path=f"{CREATED_DIR}/ideation")])
    check_("write-audit-log/no_tool_name", with_doc, [["write-audit-log", payload(tool_input={"file_path": "{P}/" + doc})]])
    check_("write-audit-log/file_path_falsy", with_doc, [["write-audit-log", payload(tool_name="Write", tool_input={"file_path": 0})], ["write-audit-log", payload(tool_name="Write", tool_input={"file_path": False})], ["write-audit-log", payload(tool_name="Write", tool_input={"file_path": None})]])
    check_("sync/active_form_falsy", joined, [task(status="in_progress", activeForm=0), task(status="in_progress", activeForm=False), task(status="in_progress", activeForm=None)])
    check_("log-subagent/message_kinds", joined, [done(agent_type="x", last_assistant_message=[]), done(agent_type="x", last_assistant_message=[["a", "b"], {"k": 1}, 2.5]), done(agent_type="x", agent_id={"k": 1}), done(agent_type="x", agent_id=False)])

    # rebuild-stage-graph: which calls recompile the runtime graph, and the creation that binds a session.
    ran = lambda command, out="", **more: ["rebuild-stage-graph", payload(hook_event_name="PostToolUse", tool_name="Bash", tool_input={"command": command}, tool_response={"stdout": out, "stderr": "", "interrupted": False}, **more)]
    chatty = CREATED_AUDIT + block("DECISION_RECORDED", T(1), "**Decision**: x") + block("QUESTION_ANSWERED", T(2), "**Question**: y") + block("ERROR_LOGGED", T(3), "**Tool**: x")
    for name, command in (("tool_state", "bun .claude/tools/aidlc-state.ts approve intent-capture"), ("tool_utility", "bun .claude/tools/aidlc-utility.ts status"),
                          ("native_state", "aidlc engine state approve intent-capture"), ("native_report", "aidlc next --resume && aidlc report --result done"),
                          ("dispatcher", 'bun "{P}/.claude/tools/aidlc.ts" engine jump execute --target x'), ("quoted_mention", "echo 'aidlc state approve'")):
        check_(f"rebuild/fire_{name}", joined, [ran(command)])
    check_("rebuild/fire_twice", joined, [ran("aidlc state approve x"), ran("aidlc state approve x")])
    check_("rebuild/pass", joined, [ran("echo hi"), ran("git status"), ran("aidlc intent create --scope feature"), ran("bun .claude/tools/aidlc-log.ts decision x")])
    check_("rebuild/reject", joined, [ran("aidlc engine runtime compile"), ran("bun .claude/tools/aidlc-runtime.ts compile"), ran("aidlc state approve x && aidlc engine runtime compile")])
    check_("rebuild/no_transition", created(audit=chatty, **binding()), [ran("aidlc state approve x")])
    check_("rebuild/transition_third_from_end", created(audit=CREATED_AUDIT + block("DECISION_RECORDED", T(1), "**Decision**: x") + block("QUESTION_ANSWERED", T(2), "**Question**: y"), **binding()), [ran("aidlc state approve x")])
    check_("rebuild/transition_fourth_from_end", created(audit=chatty, **binding()), [ran("aidlc state approve x")])
    check_("rebuild/completed", created(audit=CREATED_AUDIT + block("PHASE_COMPLETED", T(1)) + block("PHASE_VERIFIED", T(2)) + block("WORKFLOW_COMPLETED", T(3)), **binding()), [ran("aidlc state approve x")])
    check_("rebuild/awaiting", created(audit=CREATED_AUDIT + block("STAGE_AWAITING_APPROVAL", T(1), "**Stage**: intent-capture"), **binding()), [ran("aidlc state gate-start x")])
    check_("rebuild/event_spaced", created(audit=CREATED_AUDIT + "\n## X\n**Timestamp**: " + T(1) + "\n**Event**:  GATE_APPROVED \n\n---\n", **binding()), [ran("aidlc state approve x")])
    check_("rebuild/event_not_alone", created(audit=CREATED_AUDIT + block("DECISION_RECORDED", T(1), "**Event**: GATE_APPROVED now"), **binding()), [ran("aidlc state approve x")])
    check_("rebuild/crlf_audit", created(audit=CREATED_AUDIT.replace("\n", "\r\n"), **binding()), [ran("aidlc state approve x")])
    check_("rebuild/other_shard", created(audit=chatty, **binding(), **{f"{CREATED_DIR}/audit/desktop-def456.md": TITLE + block("STAGE_STARTED", T(9), "**Stage**: market-research")}), [ran("aidlc state approve x")])
    check_("rebuild/no_shard", created(**binding(), **{CREATED_SHARD: None}), [ran("aidlc state approve x")])
    check_("rebuild/no_state", {**CLONE, "aidlc/spaces/default/intents/intents.json": CREATED_REGISTRY, **binding()}, [ran("aidlc state approve x")])
    check_("rebuild/cursor_only", created(), [ran("aidlc state approve x")])
    check_("rebuild/outsider", {**no_cursor, **binding("other-9f8e7d6c")}, [ran("aidlc state approve x")])
    check_("rebuild/unjoined", {**no_cursor, **binding(None, "unjoined")}, [ran("aidlc state approve x")])
    check_("rebuild/no_session", created(), [["rebuild-stage-graph", json.dumps({"tool_name": "Bash", "tool_input": {"command": "aidlc state approve x"}})]])
    check_("rebuild/empty_payload", joined, [["rebuild-stage-graph", ""], ["rebuild-stage-graph", "[]"], ["rebuild-stage-graph", "{}"]])
    check_("rebuild/no_command", joined, [["rebuild-stage-graph", payload(tool_name="Bash", tool_input={})], ["rebuild-stage-graph", payload(tool_name="Bash")]])
    check_("rebuild/command_number", joined, [["rebuild-stage-graph", payload(tool_name="Bash", tool_input={"command": 5})]])
    check_("rebuild/command_falsy", joined, [["rebuild-stage-graph", payload(tool_name="Bash", tool_input={"command": None})]])
    ide_ran = lambda **more: ["rebuild-stage-graph", payload(hook_event_name="PostToolUse", tool_input={"source": "ide-audit-sync", **more})]
    check_("rebuild/ide_fires", joined, [ide_ran()])
    check_("rebuild/ide_idempotent", joined, [ide_ran(), ide_ran()])
    check_("rebuild/ide_no_transition", created(audit=chatty, **binding()), [ide_ran()])
    # The graph's age is set by hand, which a recording does not carry: compared live.
    check_("rebuild/ide_stale_graph", created(**binding(), **{f"{CREATED_DIR}/runtime-graph.json": "{}"}), [ide_ran()],
           edits=(lambda tree: os.utime(tree.path(f"{CREATED_DIR}/runtime-graph.json"), (1_600_000_000, 1_600_000_000)),))
    UNRECORDED.add("hook/rebuild/ide_stale_graph")
    check_("rebuild/ide_command_number", joined, [ide_ran(command=5)])
    check_("rebuild/ide_no_shard", created(**binding(), **{CREATED_SHARD: None}), [ide_ran()])
    # A creation observed in the call binds the session that made it.
    receipt = {f"{CREATED_DIR}/.aidlc-engine/creation-receipt": "01a138fa-d37d-787d-af4a-966efbb0b6fd\n"}
    made = "Intent created: 260105-add-login (space: default)\nState initialized: feature scope, 32 stages, Standard depth\n"
    create = "bun .claude/tools/aidlc-utility.ts intent-create --scope feature --arguments x"
    check_("rebuild/bind_created", {**created(), **receipt}, [ran(create, made)])
    check_("rebuild/bind_observed", created(), [ran(create, made)])
    check_("rebuild/bind_native", {**created(), **receipt}, [ran("aidlc intent create --scope feature", made)])
    check_("rebuild/bind_migrated", {**created(), **receipt}, [ran("aidlc intent create", "Migrated flat workspace into intent: 260105-add-login (space: default)\n")])
    check_("rebuild/bind_string_response", {**created(), **receipt}, [["rebuild-stage-graph", payload(tool_name="Bash", tool_input={"command": create}, tool_response=made)]])
    check_("rebuild/bind_object_response", {**created(), **receipt}, [["rebuild-stage-graph", payload(tool_name="Bash", tool_input={"command": create}, tool_response={"output": made})]])
    check_("rebuild/bind_no_response", {**created(), **receipt}, [["rebuild-stage-graph", payload(tool_name="Bash", tool_input={"command": create})]])
    check_("rebuild/bind_other_command", {**created(), **receipt}, [ran("aidlc status", made)])
    check_("rebuild/bind_unknown_record", {**created(), **receipt}, [ran(create, "Intent created: 260106-other (space: default)\n")])
    check_("rebuild/bind_other_space", {**created(), **receipt}, [ran(create, "Intent created: 260105-add-login (space: payments)\n")])
    check_("rebuild/bind_receipt_mismatch", {**created(), f"{CREATED_DIR}/.aidlc-engine/creation-receipt": "01a138fa-0000-787d-af4a-966efbb0b6fd\n"}, [ran(create, made)])
    check_("rebuild/bind_receipt_once", {**created(), **receipt}, [ran(create, made), ran(create, made)])
    check_("rebuild/bind_already_bound", {**created(), **receipt, **binding(source="observed-create")}, [ran(create, made)])
    check_("rebuild/bind_already_bound_sourceless", {**created(), **receipt, "aidlc/.aidlc-sessions/s1.binding.json": '{"space":"default","intent":"260105-add-login","boundAt":"2026-01-05T10:00:00Z"}'}, [ran(create, made)])
    check_("rebuild/bind_stamped_elsewhere", {**created(), **receipt, "aidlc/.aidlc-sessions/s1": "01a138fa-ffff-787d-af4a-966efbb0b6fd\n"}, [ran(create, made)])
    check_("rebuild/bind_stamped_same", {**created(), **receipt, "aidlc/.aidlc-sessions/s1": "01a138fa-d37d-787d-af4a-966efbb0b6fd\n"}, [ran(create, made)])
    other_record = {"aidlc/spaces/default/intents/other-9f8e7d6c/aidlc-state.md": CREATED_STATE,
                    "aidlc/spaces/default/intents/intents.json": CREATED_REGISTRY.replace("]\n", ',\n  {"uuid": "01a138fa-ffff-787d-af4a-966efbb0b6fd", "slug": "other", "dirName": "other-9f8e7d6c", "scope": "feature", "status": "in-flight"}\n]\n')}
    check_("rebuild/bind_observed_keeps_participant", {**created(), **other_record, **binding("other-9f8e7d6c", "create"), "aidlc/spaces/default/intents/active-intent": "other-9f8e7d6c\n"}, [ran(create, made)])
    check_("rebuild/bind_observed_moves_outsider", {**created(), **other_record, **binding("other-9f8e7d6c", "worktree"), "aidlc/spaces/default/intents/active-intent": None}, [ran(create, made)])
    check_("rebuild/bind_proven_moves_participant", {**created(), **other_record, **receipt, **binding("other-9f8e7d6c", "create"), "aidlc/spaces/default/intents/active-intent": "other-9f8e7d6c\n"}, [ran(create, made)])
    check_("rebuild/bind_ide", {**created(), **receipt}, [["rebuild-stage-graph", payload(tool_input={"source": "ide-audit-sync"}, tool_response=made)]])
    check_("rebuild/bind_flat_registry", {**created(registry=legacy), **receipt}, [ran(create, made)])
    # The engine's error directive in the call's output.
    directive = '{"kind":"error","message":"No workflow is active."}\n'
    check_("rebuild/relay", joined, [ran("aidlc next", directive)])
    check_("rebuild/relay_dispatcher", joined, [ran("cd {P} && bun .claude/tools/aidlc.ts engine orchestrate report --result done 2>&1", directive)])
    check_("rebuild/relay_not_literal", joined, [ran("aidlc next | tee log", directive), ran("aidlc state approve x", directive), ran("aidlc next", '{"kind":"print","message":"x"}\n')])
    check_("rebuild/relay_string_response", joined, [["rebuild-stage-graph", payload(tool_name="Bash", tool_input={"command": "aidlc park"}, tool_response=directive)]])
    for marker in ("STOP_HOOK_PROBE", "ROUTE_CHECK"):
        check_(f"rebuild/observer_{marker.lower()}", joined, [ran("aidlc state approve x")], {marker: "1"})

    # session-start: the session identity, the binding, the event and the context handed to the model.
    # A fresh distribution tree: config never ran, so the engine would add its part of the root files; the hook
    # checks run as a configured project, which records that config did.
    configured = {".claude/tools/data/aidlc-manifest.json": '{"schemaVersion": 1}\n'}
    start = lambda **fields: ["session-start", payload(hook_event_name="SessionStart", **fields)]
    begun = {**joined, **configured}
    check_("session-start/startup", begun, [start(source="startup")])
    check_("session-start/resume", begun, [start(source="resume")])
    check_("session-start/clear", begun, [start(source="clear")])
    check_("session-start/compact", begun, [start(source="compact")])
    check_("session-start/unknown_source", begun, [start(source="weird")])
    check_("session-start/no_source", begun, [start()])
    check_("session-start/source_number", begun, [start(source=0), start(source=7)])
    check_("session-start/empty_payload", begun, [["session-start", ""]])
    check_("session-start/not_json", begun, [["session-start", "{nope"]])
    check_("session-start/not_object", begun, [["session-start", "[1]"]])
    check_("session-start/no_session", begun, [["session-start", json.dumps({"source": "startup"})]])
    check_("session-start/invalid_session", begun, [["session-start", json.dumps({"session_id": "../x", "source": "startup"})]])
    check_("session-start/transcript", begun, [start(source="startup", transcript_path="/tmp/t.jsonl")])
    check_("session-start/transcript_settings_refused", {**begun, "aidlc.settings.local.json": '{"flags": {}}\n'}, [start(source="startup", transcript_path="/tmp/t.jsonl")], {"DISABLE_PLAN_APPROVAL_GUARD": "1"})
    check_("session-start/transcript_tracking_off", {**begun, "aidlc.settings.local.json": '{"schemaVersion": 1, "flags": {"schemaVersion": 1, "bypasses": ["AIDLC_DISABLE_USAGE_TRACKING"]}}\n'}, [start(source="startup", transcript_path="/tmp/t.jsonl")])
    check_("session-start/twice", begun, [start(source="startup"), start(source="resume")])
    check_("session-start/cursor_only", {**created(), **configured}, [start(source="startup")])
    check_("session-start/cold", {**CLONE, **configured}, [start(source="startup")])
    check_("session-start/cold_no_session", {**CLONE, **configured}, [["session-start", json.dumps({"source": "startup"})]])
    check_("session-start/no_state", {**CLONE, **configured, "aidlc/spaces/default/intents/intents.json": CREATED_REGISTRY, **binding()}, [start(source="startup")])
    check_("session-start/unjoined_lone_record", {**no_cursor, **configured}, [start(source="startup")])
    check_("session-start/rejoin_offer", {**no_cursor, **configured, **binding(source="worktree")}, [start(source="startup")])
    check_("session-start/rejoin_offer_other_space", {**no_cursor, **configured, "aidlc/active-space": "payments\n", "aidlc/spaces/payments/intents/intents.json": "[]\n", **binding(source="worktree")}, [start(source="startup")])
    check_("session-start/rejoin_probe_not_true", {**no_cursor, **configured, **binding(source="worktree")}, [start(source="resume", rebind_check=False), start(source="resume", rebind_check="yes")])
    # A probe that already made this offer (the binding still names the record) says nothing.
    check_("session-start/rejoin_probe_offered", {**no_cursor, **configured, **binding(source="worktree"), "aidlc/.aidlc-sessions/s1.rebind-offer": "rejoin:default/260105-add-login\n"}, [start(source="resume", rebind_check=True)])
    check_("session-start/rejoin_probe", {**no_cursor, **configured, **binding(source="worktree")}, [start(source="resume", rebind_check=True), start(source="resume", rebind_check=True)])
    check_("session-start/bound_elsewhere", {**no_cursor, **configured, **binding("other-9f8e7d6c")}, [start(source="startup")])
    check_("session-start/bound_unjoined", {**no_cursor, **configured, **binding(None, "unjoined")}, [start(source="startup")])
    check_("session-start/bound_none_cold", {**CLONE, **configured, **binding(None, "none")}, [start(source="startup")])
    check_("session-start/stamped_resume", {**no_cursor, **configured, "aidlc/.aidlc-sessions/s1": "01a138fa-d37d-787d-af4a-966efbb0b6fd\n"}, [start(source="resume")])
    check_("session-start/stamped_startup", {**no_cursor, **configured, "aidlc/.aidlc-sessions/s1": "01a138fa-d37d-787d-af4a-966efbb0b6fd\n"}, [start(source="startup")])
    check_("session-start/stamped_unknown_resume", {**created(), **configured, "aidlc/.aidlc-sessions/s1": "01a138fa-0000-787d-af4a-966efbb0b6fd\n"}, [start(source="resume")])
    other = {"aidlc/spaces/default/intents/other-9f8e7d6c/aidlc-state.md": CREATED_STATE,
             "aidlc/spaces/default/intents/intents.json": CREATED_REGISTRY.replace("]\n", ',\n  {"uuid": "01a138fa-ffff-787d-af4a-966efbb0b6fd", "slug": "other", "dirName": "other-9f8e7d6c", "scope": "feature", "status": "in-flight"}\n]\n')}
    drifted = {**created(), **configured, **other, "aidlc/spaces/default/intents/active-intent": "other-9f8e7d6c\n", **binding()}
    check_("session-start/rebind_offer", drifted, [start(source="resume")])
    check_("session-start/rebind_offer_once", drifted, [start(source="resume"), start(source="resume")])
    check_("session-start/rebind_offer_probe", drifted, [start(source="resume", rebind_check=True), start(source="resume", rebind_check=True)])
    check_("session-start/rebind_offer_stamp_only", {**created(), **configured, **other, "aidlc/spaces/default/intents/active-intent": "other-9f8e7d6c\n", "aidlc/.aidlc-sessions/s1": "01a138fa-d37d-787d-af4a-966efbb0b6fd\n"}, [start(source="resume")])
    flat_other = CREATED_REGISTRY.replace('    "uuid": "01a138fa-d37d-787d-af4a-966efbb0b6fd",\n', "").replace("]\n", ',\n  {"uuid": "01a138fa-ffff-787d-af4a-966efbb0b6fd", "slug": "other", "dirName": "other-9f8e7d6c", "scope": "feature", "status": "in-flight"}\n]\n')
    check_("session-start/rebind_stamp_with_flat_binding", {**created(registry=flat_other), **configured, **other, **binding(), "aidlc/.aidlc-sessions/s1": "01a138fa-ffff-787d-af4a-966efbb0b6fd\n"}, [start(source="resume")])
    check_("session-start/rebind_cleared", {**begun, "aidlc/.aidlc-sessions/s1.rebind-offer": "x\n"}, [start(source="resume")])
    check_("session-start/rebind_cleared_startup", {**no_cursor, **configured, "aidlc/.aidlc-sessions/s1.rebind-offer": "x\n"}, [start(source="startup")])
    check_("session-start/flat_registry", {**created(registry=legacy), **configured, **binding()}, [start(source="startup"), start(source="resume")])
    check_("session-start/binding_sourceless", {**created(), **configured, "aidlc/.aidlc-sessions/s1.binding.json": '{"space":"default","intent":"260105-add-login","boundAt":"2026-01-05T10:00:00Z"}'}, [start(source="startup")])
    check_("session-start/sources", {**begun, **{f"aidlc/.aidlc-sessions/s{n}.binding.json": json.dumps({"space": "default", "intent": "260105-add-login", "boundAt": "2026-01-05T10:00:00Z", "source": src}) for n, src in enumerate(("migration", "switch", "stamp"), start=2)}},
           [["session-start", json.dumps({"session_id": f"s{n}", "source": "startup"})] for n in (2, 3, 4)])
    check_("session-start/presence_bypass", begun, [start(source="startup")], {"SKIP_HUMAN_PRESENCE_GUARD": "1"})
    check_("session-start/presence_bypass_settings", {**begun, "aidlc.settings.local.json": '{"schemaVersion": 1, "flags": {"schemaVersion": 1, "bypasses": ["AIDLC_SKIP_HUMAN_PRESENCE_GUARD"]}}\n',
           "aidlc/.aidlc-sessions/recorded-switches.json": '{"version": 1, "switches": [{"name": "AIDLC_SKIP_HUMAN_PRESENCE_GUARD", "target": "local", "since": "2026-01-05T10:00:00Z", "how": "other"}]}\n'}, [start(source="startup")])
    # A switch nobody recorded is dated by its settings file's modification time, which is the run's.
    check_("session-start/switch_off_unrecorded", {**begun, "aidlc.settings.local.json": '{"schemaVersion": 1, "flags": {"schemaVersion": 1, "bypasses": ["AIDLC_SKIP_HUMAN_PRESENCE_GUARD"]}}\n'}, [start(source="startup")])
    UNRECORDED.add("hook/session-start/switch_off_unrecorded")
    check_("session-start/plan_approval_bypass", begun, [start(source="startup")], {"DISABLE_PLAN_APPROVAL_GUARD": "1"})
    check_("session-start/plan_approval_bypass_cleared", {**begun, "aidlc/.aidlc-sessions/plan-approval/plan-approval-bypass-s1": "2026-01-05T10:00:00Z\n"}, [start(source="startup")])
    check_("session-start/unit_line", created(CREATED_STATE.replace("- **Active Agent**:", "- **Active Unit**: auth\n- **Unit Stage**: code-generation\n- **Unit State**: paused\n- **Unit Pause Reason**: tests red\n- **Unit Next Action**: fix them\n- **Active Agent**:"), **binding(), **configured), [start(source="startup")])
    check_("session-start/unit_line_in_unit_stages", created(at_stage(CREATED_STATE, "functional-design").replace("- **Active Agent**:", "- **Active Unit**: auth\n- **Unit Stage**: code-generation\n- **Active Agent**:"), **binding(), **configured), [start(source="startup")])
    # One of the five stages that always run per unit, in a compiled graph that lost its marker, is still one; a
    # stage that is none of the five is per unit by its marker alone.
    for label, slugs, unit_stage in (("unit_stage", ["code-generation"], "code-generation"), ("current_stage", ["functional-design"], "code-generation"),
                                     ("every", ["functional-design", "nfr-requirements", "nfr-design", "infrastructure-design", "code-generation"], "nfr-design"),
                                     ("not_one_of_the_five", ["code-generation"], "build-and-test")):
        check_(f"session-start/unit_line_no_marker_{label}", created(at_stage(CREATED_STATE, "functional-design").replace("- **Active Agent**:", f"- **Active Unit**: auth\n- **Unit Stage**: {unit_stage}\n- **Active Agent**:"), **binding(), **configured),
               [start(source="startup")], edits=[lambda tree, slugs=slugs: no_marker(tree, slugs)])
    check_("session-start/unit_line_team", created(at_stage(CREATED_STATE, "functional-design").replace("- **Active Agent**:", "- **Active Unit**: auth\n- **Unit Stage**: code-generation\n- **Unit Ownership**: team\n- **Active Agent**:"), **binding(), **configured), [start(source="startup")])
    check_("session-start/unit_line_bad_name", created(CREATED_STATE.replace("- **Active Agent**:", "- **Active Unit**: ../x\n- **Unit Stage**: code-generation\n- **Active Agent**:"), **binding(), **configured), [start(source="startup")])
    check_("session-start/sparse_state", created("# AI-DLC State Tracking\n", **binding(), **configured), [start(source="startup")])
    check_("session-start/recovery_note", {**begun, f"{CREATED_DIR}/.aidlc-engine/recovery.md": "# x\n"}, [start(source="startup")])
    check_("session-start/drift_note", {**begun, ".claude/aidlc-common/stages/ideation/my-stage.md": "---\nslug: my-stage\n---\n", ".claude/aidlc-common/stages/construction/other.md": "x"}, [start(source="startup")])
    check_("session-start/memory_copy_current", begun, [start(source="startup"), start(source="startup")])
    check_("session-start/memory_copy_stale", {**begun, "aidlc/active-memory/team.md": "stale\n", "aidlc/active-memory/gone.md": "x\n"}, [start(source="resume")])
    small = {f"aidlc/spaces/default/memory/{rel}": f"# {rel}\n\nShort.\n" for rel in ("org.md", "team.md", "project.md", "phases/ideation.md", "phases/inception.md", "phases/construction.md", "phases/operation.md")}
    check_("session-start/memory_inline", {**begun, **small}, [start(source="startup"), start(source="resume")])
    check_("session-start/memory_inline_one_stale", {**begun, **small, "aidlc/active-memory/team.md": "stale\n"}, [start(source="resume")])
    check_("session-start/memory_inline_cold", {**CLONE, **configured, **small}, [start(source="startup")])
    check_("session-start/memory_handoff_by_name", {**begun, "aidlc/spaces/default/memory/org.md": "# Org\n" + "x" * 9000}, [start(source="startup")])
    check_("session-start/memory_not_utf8", {**begun}, [start(source="startup")], edits=(lambda tree: open(tree.path("aidlc/spaces/default/memory/team.md"), "wb").write(b"\xff\xfe bad"),))
    # A recorded tree is text: a file that is not UTF-8 stays a live check.
    UNRECORDED.add("hook/session-start/memory_not_utf8")
    check_("session-start/switch_off_recorded", {**begun, "aidlc.settings.local.json": '{"schemaVersion": 1, "flags": {"schemaVersion": 1, "bypasses": ["AIDLC_DISABLE_REVIEW_FREEZE_HOOK"]}}\n',
           "aidlc/.aidlc-sessions/recorded-switches.json": '{"version": 1, "switches": [{"name": "AIDLC_DISABLE_REVIEW_FREEZE_HOOK", "target": "local", "since": "2026-01-05T10:00:00Z", "how": "chat", "words": "turn  the \\"freeze\\" off\\nplease"}]}\n'}, [start(source="startup")])
    check_("session-start/switch_off_other", {**begun, "aidlc.settings.json": '{"schemaVersion": 1, "flags": {"schemaVersion": 1, "bypasses": ["AIDLC_SKIP_HUMAN_PRESENCE_GUARD", "AIDLC_DISABLE_USAGE_TRACKING"]}}\n',
           "aidlc/.aidlc-sessions/recorded-switches.json": '{"version": 1, "switches": [{"name": "AIDLC_SKIP_HUMAN_PRESENCE_GUARD", "target": "project", "since": "nope", "how": "other"}]}\n'}, [start(source="startup")])
    check_("session-start/switch_off_env_wins", {**begun, "aidlc.settings.local.json": '{"schemaVersion": 1, "flags": {"schemaVersion": 1, "bypasses": ["AIDLC_DISABLE_REVIEW_FREEZE_HOOK"]}}\n'}, [start(source="startup")], {"DISABLE_REVIEW_FREEZE_HOOK": ""})
    check_("session-start/switch_off_record_malformed", {**begun, "aidlc.settings.local.json": '{"schemaVersion": 1, "flags": {"schemaVersion": 1, "bypasses": ["AIDLC_DISABLE_REVIEW_FREEZE_HOOK"]}}\n',
           "aidlc/.aidlc-sessions/recorded-switches.json": '{"version": 2, "switches": [{"name": "AIDLC_DISABLE_REVIEW_FREEZE_HOOK", "target": "local", "since": "2026-01-05T10:00:00Z", "how": "other"}]}'}, [start(source="startup")])
    UNRECORDED.add("hook/session-start/switch_off_record_malformed")
    check_("session-start/switch_off_probe", {**CLONE, **configured, **binding(None, "none"), "aidlc.settings.local.json": '{"schemaVersion": 1, "flags": {"schemaVersion": 1, "bypasses": ["AIDLC_DISABLE_REVIEW_FREEZE_HOOK"]}}\n',
           "aidlc/.aidlc-sessions/recorded-switches.json": '{"version": 1, "switches": [{"name": "AIDLC_DISABLE_REVIEW_FREEZE_HOOK", "target": "local", "since": "2026-01-05T10:00:00Z", "how": "other"}]}\n'}, [start(source="resume", rebind_check=True)])
    check_("session-start/switch_off_cold", {**CLONE, **configured, "aidlc.settings.local.json": '{"schemaVersion": 1, "flags": {"schemaVersion": 1, "bypasses": ["AIDLC_DISABLE_REVIEW_FREEZE_HOOK"]}}\n',
           "aidlc/.aidlc-sessions/recorded-switches.json": '{"version": 1, "switches": [{"name": "AIDLC_DISABLE_REVIEW_FREEZE_HOOK", "target": "local", "since": "2026-01-05T10:00:00Z", "how": "other"}]}\n'}, [start(source="startup")])
    check_("session-start/root_blocks_unconfigured", joined, [start(source="startup")])
    check_("session-start/anchor", begun, [start(source="startup")], {"SESSION_ANCHOR": "1"})
    check_("session-start/unit_scope_stamp", {**begun, "aidlc/.aidlc-unit-scope.json": '{"version": 1}'}, [start(source="startup")])
    check_("session-start/rules_held_previous", {**begun, "aidlc/.aidlc-sessions/s1.rules-held.json": '{"v":1,"harness":"claude","source":"startup","at":"x","space":"default","refresh":"at-load","files":{}}\n',
           "aidlc/.aidlc-sessions/s1.rules-delivered.json": '{"v":2,"last":"b","full":{"bundle":"b","at":"x"}}\n'}, [start(source="startup"), start(source="resume")])
    check_("session-start/no_shard", {**begun, CREATED_SHARD: None}, [start(source="startup")])
    for marker in ("STOP_HOOK_PROBE", "ROUTE_CHECK"):
        check_(f"session-start/observer_{marker.lower()}", begun, [start(source="startup")], {marker: "1", "SKIP_HUMAN_PRESENCE_GUARD": "1"})
    # validate-state no longer refuses the rules record: a compaction forgets the full text handed to the thread.
    check_("validate-state/rules_delivered", {**joined, "aidlc/.aidlc-sessions/s1.rules-delivered.json": '{"v":2,"last":"b","full":{"bundle":"b","at":"x"},"persona":{"sha256":"p","at":"x"}}\n'}, [compact()])
    check_("validate-state/rules_delivered_malformed", {**joined, "aidlc/.aidlc-sessions/s1.rules-delivered.json": '{"v":1}\n'}, [compact()])

    # review-freeze: which writes reach the review receipt scan.
    freeze = lambda tool="Write", path=None, **more: ["review-freeze", payload(hook_event_name="PreToolUse", tool_name=tool, tool_input={"file_path": "{P}/" + (path or doc), **more})]
    shell = lambda command: ["review-freeze", payload(hook_event_name="PreToolUse", tool_name="Bash", tool_input={"command": command.replace("{D}", "{P}/" + CREATED_DIR)})]
    reviewed = {**joined, CREATED_SHARD: CREATED_AUDIT + block("REVIEW_REQUESTED", T(2), "**Stage**: intent-capture", "**Reviewer**: aidlc-product-lead-agent", "**Iteration**: 1")
                + block("REVIEW_COMPLETED", T(3), "**Stage**: intent-capture", "**Reviewer**: aidlc-product-lead-agent", "**Iteration**: 1", "**Verdict**: READY")}
    check_("review-freeze/read_tool", reviewed, [["review-freeze", payload(hook_event_name="PreToolUse", tool_name="Read", tool_input={"file_path": "{P}/" + doc})]])
    check_("review-freeze/unrelated_write", reviewed, [freeze(path="README.md")])
    check_("review-freeze/other_stage_artifact", reviewed, [freeze(path=f"{CREATED_DIR}/ideation/market-research/competitive-analysis.md")])
    check_("review-freeze/artifact_no_review_rows", joined, [freeze(), freeze("Edit"), freeze("MultiEdit"), freeze("NotebookEdit", path=f"{CREATED_DIR}/ideation/intent-capture/stakeholder-map.md")])
    check_("review-freeze/questions_file", reviewed, [freeze(path=f"{CREATED_DIR}/ideation/intent-capture/intent-capture-questions.md")])
    check_("review-freeze/completed_stage", {**reviewed, f"{CREATED_DIR}/aidlc-state.md": at_stage(CREATED_STATE, "market-research", done=["intent-capture"])}, [freeze()])
    check_("review-freeze/skipped_stage", {**reviewed, f"{CREATED_DIR}/aidlc-state.md": at_stage(CREATED_STATE, "market-research", skipped=["intent-capture"])}, [freeze()])
    check_("review-freeze/review_rows", reviewed, [freeze()])
    check_("review-freeze/review_rows_edit", reviewed, [freeze("Edit", path=f"{CREATED_DIR}/ideation/intent-capture/stakeholder-map.md")])
    check_("review-freeze/review_rows_request_only", {**joined, CREATED_SHARD: CREATED_AUDIT + block("REVIEW_REQUESTED", T(2), "**Stage**: intent-capture", "**Reviewer**: aidlc-product-lead-agent", "**Iteration**: 1")}, [freeze()])
    # Rows of another stage, and rows of a stage with no reviewer, hold no receipt for the write.
    other_rows = CREATED_AUDIT + block("REVIEW_COMPLETED", T(2), "**Stage**: market-research", "**Verdict**: READY")
    check_("review-freeze/other_stage_rows", {**joined, CREATED_SHARD: other_rows}, [freeze()])
    check_("review-freeze/no_reviewer_stage_rows", {**joined, CREATED_SHARD: other_rows}, [freeze(path=f"{CREATED_DIR}/ideation/market-research/competitive-analysis.md")])
    check_("review-freeze/review_rows_switch_zero", reviewed, [freeze()], {"DISABLE_REVIEW_FREEZE_HOOK": "0"})
    # No target: decided before the ledger, the state or the graph is read (no drop for a missing state).
    check_("review-freeze/read_tool_no_state", {**CLONE, "aidlc/spaces/default/intents/intents.json": CREATED_REGISTRY, **binding(), CREATED_SHARD: reviewed[CREATED_SHARD]}, [["review-freeze", payload(hook_event_name="PreToolUse", tool_name="Read", tool_input={"file_path": "{P}/" + doc})]])
    check_("review-freeze/review_rows_shell", reviewed, [shell("echo x > {D}/ideation/intent-capture/intent-statement.md")])
    check_("review-freeze/shell_read_only", reviewed, [shell("cat {D}/ideation/intent-capture/intent-statement.md")])
    check_("review-freeze/shell_other_target", reviewed, [shell("echo x > {D}/ideation/intent-capture/notes.md")])
    check_("review-freeze/review_rows_subagent", reviewed, [["review-freeze", payload(hook_event_name="PreToolUse", tool_name="Write", agent_type="aidlc-product-agent", tool_input={"file_path": "{P}/" + doc})]])
    check_("review-freeze/off_switch", reviewed, [freeze()], {"DISABLE_REVIEW_FREEZE_HOOK": "1"})
    check_("review-freeze/off_switch_settings", {**reviewed, "aidlc.settings.local.json": '{"schemaVersion": 1, "flags": {"schemaVersion": 1, "bypasses": ["AIDLC_DISABLE_REVIEW_FREEZE_HOOK"]}}\n'}, [freeze()])
    check_("review-freeze/settings_refused", {**reviewed, "aidlc.settings.local.json": '{"flags": {}}\n'}, [freeze()])
    check_("review-freeze/outsider", {**no_cursor, **binding("other-9f8e7d6c"), CREATED_SHARD: reviewed[CREATED_SHARD]}, [freeze()])
    check_("review-freeze/unjoined", {**no_cursor, **binding(None, "unjoined"), CREATED_SHARD: reviewed[CREATED_SHARD]}, [freeze()])
    check_("review-freeze/no_ledger", {**joined, CREATED_SHARD: None}, [freeze()])
    check_("review-freeze/no_state", {**CLONE, "aidlc/spaces/default/intents/intents.json": CREATED_REGISTRY, **binding(), CREATED_SHARD: reviewed[CREATED_SHARD]}, [freeze()])
    check_("review-freeze/cold", CLONE, [freeze(path="README.md"), freeze()])
    check_("review-freeze/malformed", reviewed, [["review-freeze", "{nope"], ["review-freeze", ""], ["review-freeze", "[1]"]])
    check_("review-freeze/no_tool_input", reviewed, [["review-freeze", payload(hook_event_name="PreToolUse", tool_name="Write")]])
    check_("review-freeze/directive_marker", {**reviewed, f"{engine}/active-directive.json": '{"stage": "intent-capture"}'}, [freeze()])
    for marker in ("STOP_HOOK_PROBE", "ROUTE_CHECK"):
        check_(f"review-freeze/observer_{marker.lower()}", joined, [freeze()], {marker: "1"})

    # reviewer-scope: the dispatched reviewer's read bound.
    REVIEWER = "aidlc-architecture-reviewer-agent"
    dispatch = lambda unit="U01", exempt=(), reviewer=REVIEWER, stage="nfr-design": {f"{engine}/reviewer-dispatch.json": json.dumps({"reviewer": reviewer, "stage": stage, "unit": unit, "exempt": list(exempt)})}
    ideation = ["intent-capture", "market-research", "feasibility", "scope-definition", "team-formation", "rough-mockups"]
    construction = at_stage(CREATED_STATE, "nfr-design", done=ideation + ["approval-handoff", "practices-discovery", "requirements-analysis", "user-stories", "refined-mockups", "domain-design", "units-generation", "contract-design", "delivery-planning", "functional-design", "nfr-requirements"], phase="CONSTRUCTION")
    reviewing = {**created(construction), **binding(), **dispatch()}
    scope = lambda tool="Read", agent=REVIEWER, **input: ["reviewer-scope", payload(hook_event_name="PreToolUse", tool_name=tool, tool_input={k: (v.replace("{D}", "{P}/" + CREATED_DIR) if isinstance(v, str) else v) for k, v in input.items()}, **({"agent_type": agent} if agent is not None else {}))]
    check_("reviewer-scope/own_unit", reviewing, [scope(file_path="{D}/construction/U01/nfr-design/security-design.md"), scope("Grep", pattern="x", path="{D}/construction/U01"), scope("Bash", command="cat {D}/construction/U01/design.md")])
    check_("reviewer-scope/sibling_unit", reviewing, [scope(file_path="{D}/construction/U02/nfr-design/security-design.md")])
    check_("reviewer-scope/sibling_relative", reviewing, [scope(file_path="construction/U02/design.md")])
    check_("reviewer-scope/sibling_shell", reviewing, [scope("Bash", command="cat {D}/construction/U02/design.md")])
    check_("reviewer-scope/sibling_shell_cd", reviewing, [scope("Bash", command="cd {D}/construction && cat U02/design.md")])
    check_("reviewer-scope/sweep_glob", reviewing, [scope("Glob", pattern="construction/*/design.md")])
    check_("reviewer-scope/sweep_grep_pathless", reviewing, [["reviewer-scope", payload(hook_event_name="PreToolUse", tool_name="Grep", tool_input={"pattern": "x"}, agent_type=REVIEWER, cwd="{P}/" + CREATED_DIR)]])
    check_("reviewer-scope/sweep_rg_default_root", reviewing, [["reviewer-scope", payload(hook_event_name="PreToolUse", tool_name="Bash", tool_input={"command": "rg TODO"}, agent_type=REVIEWER, cwd="{P}/" + CREATED_DIR)]])
    check_("reviewer-scope/grep_inside_unit_cwd", reviewing, [["reviewer-scope", payload(hook_event_name="PreToolUse", tool_name="Bash", tool_input={"command": "grep -r TODO ."}, agent_type=REVIEWER, cwd="{P}/" + CREATED_DIR + "/construction/U01")]])
    check_("reviewer-scope/exempt_file", {**reviewing, **dispatch(exempt=["{P}/" + CREATED_DIR + "/construction/U02/nfr-design/security-design.md"])}, [scope(file_path="{D}/construction/U02/nfr-design/security-design.md"), scope("LS", path="{D}/construction/U02/nfr-design")])
    check_("reviewer-scope/exempt_suffix", {**reviewing, **dispatch(exempt=["construction/U02/x.md"])}, [scope(file_path="{D}/construction/U02/x.md"), scope(file_path="{D}/construction/U02/y.md")])
    check_("reviewer-scope/outside_construction", reviewing, [scope(file_path="{D}/inception/domain-design/components.md"), scope("Bash", command="ls {D}/inception"), scope("LS", path="{D}")])
    check_("reviewer-scope/other_agent", reviewing, [scope(agent="aidlc-product-agent", file_path="{D}/construction/U02/design.md"), scope(agent=None, file_path="{D}/construction/U02/design.md")])
    check_("reviewer-scope/scoped_registration", reviewing, [["reviewer-scope", payload(hook_event_name="PreToolUse", tool_name="Read", tool_input={"file_path": "{P}/" + CREATED_DIR + "/construction/U02/design.md"}, scoped_registration=True)],
                                                            ["reviewer-scope", payload(hook_event_name="PreToolUse", tool_name="Read", tool_input={"file_path": "{P}/" + CREATED_DIR + "/construction/U02/design.md"}, scoped_registration="yes")]])
    check_("reviewer-scope/other_tool", reviewing, [scope("Task", file_path="{D}/construction/U02/design.md"), scope("WebFetch", url="x")])
    check_("reviewer-scope/no_record", {**created(construction), **binding()}, [scope(file_path="{D}/construction/U02/design.md")])
    check_("reviewer-scope/no_record_advisory", {**created(construction), **binding()}, [scope(file_path="{D}/construction/U02/design.md"), scope(file_path="{D}/construction/U02/design.md")])
    check_("reviewer-scope/malformed_record", {**reviewing, f"{engine}/reviewer-dispatch.json": '{"reviewer": "r"}'}, [scope(file_path="{D}/construction/U02/design.md")])
    check_("reviewer-scope/stale_record", reviewing, [scope(file_path="{D}/construction/U02/design.md")], edits=(lambda tree: os.utime(tree.path(f"{engine}/reviewer-dispatch.json"), (0, 0)),))
    # The record's age is its modification time, which a replayed tree does not carry.
    UNRECORDED.add("hook/reviewer-scope/stale_record")
    check_("reviewer-scope/off_switch", reviewing, [scope(file_path="{D}/construction/U02/design.md")], {"DISABLE_REVIEWER_SCOPE_HOOK": "1"})
    check_("reviewer-scope/off_switch_zero", reviewing, [scope(file_path="{D}/construction/U02/design.md")], {"DISABLE_REVIEWER_SCOPE_HOOK": "0"})
    check_("reviewer-scope/off_switch_settings", {**reviewing, "aidlc.settings.local.json": '{"schemaVersion": 1, "flags": {"schemaVersion": 1, "bypasses": ["AIDLC_DISABLE_REVIEWER_SCOPE_HOOK"]}}\n'}, [scope(file_path="{D}/construction/U02/design.md")])
    check_("reviewer-scope/settings_refused", {**reviewing, "aidlc.settings.local.json": '{"flags": {}}\n'}, [scope(file_path="{D}/construction/U02/design.md")])
    check_("reviewer-scope/policy_off", {**created(construction.replace("- **Guard Policy**: off (from scope feature)", "- **Guard Policy**: off (set by a command)")), **binding(), **dispatch()}, [scope(file_path="{D}/construction/U02/design.md")])
    check_("reviewer-scope/policy_relaxed", {**created(construction.replace("- **Guard Policy**: off (from scope feature)", "- **Guard Policy**: relaxed (set by a command)")), **binding(), **dispatch()}, [scope(file_path="{D}/construction/U02/design.md")])
    check_("reviewer-scope/policy_strict_switch_off", {**created(construction.replace("- **Guard Policy**: off (from scope feature)", "- **Guard Policy**: strict (set by a command)\n- **Guards Off**: reviewer-scope")), **binding(), **dispatch()}, [scope(file_path="{D}/construction/U02/design.md")])
    check_("reviewer-scope/policy_strict", {**created(construction.replace("- **Guard Policy**: off (from scope feature)", "- **Guard Policy**: strict (set by a command)")), **binding(), **dispatch()}, [scope(file_path="{D}/construction/U02/design.md"), scope("Bash", command="rg x construction/U0?")])
    check_("reviewer-scope/outsider", {**no_cursor, **binding("other-9f8e7d6c"), **dispatch()}, [scope(file_path="{D}/construction/U02/design.md")])
    check_("reviewer-scope/no_shard", {**reviewing, CREATED_SHARD: None}, [scope(file_path="{D}/construction/U02/design.md")], )
    check_("reviewer-scope/cold", CLONE, [scope(file_path="{D}/construction/U02/design.md")])
    check_("reviewer-scope/malformed", reviewing, [["reviewer-scope", "{nope"], ["reviewer-scope", ""], ["reviewer-scope", "[1]"]])
    check_("reviewer-scope/unit_scope_stamp_team", {**created(construction.replace("- **Active Agent**:", "- **Unit Ownership**: team\n- **Active Agent**:")), **binding(), **dispatch(), "aidlc/.aidlc-unit-scope.json": '{"version": 1, "space": "default", "intent_uuid": "01a138fa-d37d-787d-af4a-966efbb0b6fd", "intent_id8": "01a138fa", "unit": "U01", "claimed_at": "2026-01-05T10:00:00Z"}'}, [scope("Write", file_path="{D}/construction/U02/design.md")])
    check_("reviewer-scope/unit_scope_stamp", {**reviewing, "aidlc/.aidlc-unit-scope.json": '{"version": 1}'}, [scope(file_path="{D}/construction/U02/design.md")])
    check_("reviewer-scope/directive_marker", {**created(construction), **binding(), f"{engine}/active-directive.json": '{"stage": "nfr-design", "unit": "U01"}'}, [scope(file_path="{D}/construction/U02/design.md")])
    check_("reviewer-scope/directive_marker_dispatched", {**reviewing, f"{engine}/active-directive.json": '{"stage": "nfr-design"}'}, [scope(file_path="{D}/construction/U02/design.md")])
    for marker in ("STOP_HOOK_PROBE", "ROUTE_CHECK"):
        check_(f"reviewer-scope/observer_{marker.lower()}", reviewing, [scope(file_path="{D}/construction/U02/design.md")], {marker: "1"})

    # state-transition-guard: the direct transitions, the delegated agent's commands, the fence.
    AGENT = "aidlc-product-agent"
    guard = lambda command, agent=None, tool="Bash", **more: ["state-transition-guard", payload(hook_event_name="PreToolUse", tool_name=tool, tool_input={"command": command.replace("{D}", "{P}/" + CREATED_DIR)} if tool == "Bash" else more.pop("tool_input"), **({"agent_type": agent} if agent is not None else {}), **more)]
    strict_state = CREATED_STATE.replace("- **Guard Policy**: off (from scope feature)", "- **Guard Policy**: strict (set by you)")
    strict = {**created(strict_state), **binding()}
    speaking = {**created(CREATED_STATE.replace("off (from scope feature)", "relaxed (set by you)") + "- **Guards Off**: state-transition (set by you)\n"), **binding()}
    check_("state-transition-guard/direct_strict", strict, [guard("bun .claude/tools/aidlc-state.ts approve intent-capture"), guard("bun .claude/tools/aidlc-state.ts set 'Scope=x'"), guard("cd {P} && bun .claude/tools/aidlc-state.ts checkbox intent-capture=completed")])
    check_("state-transition-guard/direct_native", strict, [guard("aidlc engine state approve intent-capture"), guard("./aidlc engine state skip x"), guard("bun .claude/tools/aidlc.ts engine state park")])
    check_("state-transition-guard/direct_reads", strict, [guard("bun .claude/tools/aidlc-state.ts get Scope"), guard("aidlc engine state count completed"), guard("bun .claude/tools/aidlc-state.ts resume")])
    check_("state-transition-guard/direct_in_prose", strict, [guard("echo 'bun aidlc-state.ts approve x'"), guard("cat <<EOF\nbun aidlc-state.ts approve x\nEOF"), guard("f() { bun aidlc-state.ts approve x; }")])
    check_("state-transition-guard/direct_policy_off", joined, [guard("bun .claude/tools/aidlc-state.ts approve intent-capture")])
    check_("state-transition-guard/direct_relaxed_switch_off", speaking, [guard("bun .claude/tools/aidlc-state.ts approve intent-capture")])
    check_("state-transition-guard/direct_strict_switch_off", {**created(strict_state + "- **Guards Off**: state-transition (set by you)\n"), **binding()}, [guard("aidlc engine state advance")])
    check_("state-transition-guard/direct_memory_strict", {**joined, "aidlc/spaces/default/memory/team.md": "# Team\n\n## Guard Policy\n\nMode: strict\n"}, [guard("bun .claude/tools/aidlc-state.ts approve intent-capture")])
    check_("state-transition-guard/direct_policy_unreadable_state", {**created(CREATED_STATE.replace("off (from scope feature)", "sometimes")), **binding()}, [guard("bun .claude/tools/aidlc-state.ts approve intent-capture")])
    check_("state-transition-guard/direct_policy_unreadable_memory", {**joined, "aidlc/spaces/default/memory/org.md": "## Guard Policy\n\nMode: maybe\n"}, [guard("bun .claude/tools/aidlc-state.ts approve intent-capture")])
    check_("state-transition-guard/direct_outsider", {**no_cursor, **binding("other-9f8e7d6c")}, [guard("bun .claude/tools/aidlc-state.ts approve intent-capture")])
    check_("state-transition-guard/direct_unjoined", {**no_cursor, **binding(None, "unjoined")}, [guard("bun .claude/tools/aidlc-state.ts approve intent-capture")])
    check_("state-transition-guard/direct_cursor_only", created(strict_state), [guard("bun .claude/tools/aidlc-state.ts approve intent-capture")])
    check_("state-transition-guard/direct_no_state", {**CLONE, "aidlc/spaces/default/intents/intents.json": CREATED_REGISTRY, **binding()}, [guard("bun .claude/tools/aidlc-state.ts approve intent-capture")])
    check_("state-transition-guard/direct_cold", CLONE, [guard("bun .claude/tools/aidlc-state.ts approve intent-capture")])
    check_("state-transition-guard/direct_subagent_strict", strict, [guard("bun .claude/tools/aidlc-state.ts approve intent-capture", AGENT)])
    check_("state-transition-guard/direct_subagent_type", strict, [["state-transition-guard", payload(hook_event_name="PreToolUse", tool_name="Bash", tool_input={"command": "aidlc engine state approve x", "subagent_type": " aidlc-product-agent "})]])
    check_("state-transition-guard/direct_subagent_policy_off", joined, [guard("bun .claude/tools/aidlc-state.ts approve intent-capture", AGENT)])
    check_("state-transition-guard/direct_subagent_outsider", {**no_cursor, **binding("other-9f8e7d6c")}, [guard("bun .claude/tools/aidlc-state.ts approve intent-capture", AGENT)])
    check_("state-transition-guard/direct_blank_agent", strict, [guard("bun .claude/tools/aidlc-state.ts approve intent-capture", "  ")])
    check_("state-transition-guard/delegated_script", strict, [guard("bun .claude/tools/aidlc-orchestrate.ts next", AGENT), guard("bun .claude/tools/aidlc-orchestrate.ts report --stage x --result completed", AGENT), guard("bun .claude/tools/aidlc-jump.ts execute --stage x", AGENT), guard("bun .claude/tools/aidlc-state.ts unpark", AGENT)])
    check_("state-transition-guard/delegated_dispatcher", strict, [guard("aidlc next", AGENT), guard("aidlc intent switch other", AGENT), guard("aidlc engine orchestrate park", AGENT), guard("aidlc --json space-create x", AGENT)])
    check_("state-transition-guard/delegated_utility", strict, [guard("bun .claude/tools/aidlc-utility.ts scope-change --guard-policy off", AGENT), guard("bun .claude/tools/aidlc-utility.ts select-plugins", AGENT)])
    check_("state-transition-guard/delegated_nested", strict, [guard("echo $(aidlc next)", AGENT), guard("sh -c 'aidlc report'", AGENT), guard("X='aidlc next'\neval \"$X\"", AGENT), guard("env -S 'aidlc next'", AGENT)])
    check_("state-transition-guard/delegated_dynamic", strict, [guard("$CMD next", AGENT), guard("sh -c \"$X\"", AGENT), guard("eval \"$X\"", AGENT), guard("env -S '$X'", AGENT)])
    check_("state-transition-guard/delegated_allowed", strict, [guard("bun .claude/tools/aidlc-state.ts get Scope", AGENT), guard("bun .claude/tools/aidlc-orchestrate.ts wait", AGENT), guard("aidlc intent list", AGENT), guard("ls -la && git status", AGENT), guard("bun .claude/tools/aidlc-machine-config.ts get x", AGENT)])
    check_("state-transition-guard/delegated_main_session", strict, [guard("bun .claude/tools/aidlc-orchestrate.ts next"), guard("aidlc next")])
    check_("state-transition-guard/delegated_policy_off", joined, [guard("bun .claude/tools/aidlc-orchestrate.ts next", AGENT)])
    check_("state-transition-guard/delegated_relaxed_switch_off", speaking, [guard("aidlc next", AGENT)])
    check_("state-transition-guard/delegated_outsider", {**no_cursor, **binding("other-9f8e7d6c")}, [guard("bun .claude/tools/aidlc-orchestrate.ts next", AGENT)])
    check_("state-transition-guard/delegated_memory_strict", {**joined, "aidlc/spaces/default/memory/team.md": "# Team\n\n## Guard Policy\n\nMode: strict\n"}, [guard("aidlc next", AGENT)])
    check_("state-transition-guard/delegated_no_shard", {**speaking, CREATED_SHARD: None}, [guard("aidlc next", AGENT)])
    check_("state-transition-guard/direct_no_shard", {**speaking, CREATED_SHARD: None}, [guard("aidlc engine state approve x")])
    check_("state-transition-guard/other_tool", strict, [guard("", tool="Write", tool_input={"file_path": "{P}/README.md"}), guard("", tool="Task", tool_input={"prompt": "bun aidlc-state.ts approve x"}), guard("", tool="Task", agent=AGENT, tool_input={"command": "bun aidlc-state.ts approve x && aidlc next"})])
    check_("state-transition-guard/no_tool_input", strict, [["state-transition-guard", payload(hook_event_name="PreToolUse", tool_name="Bash")]])
    check_("state-transition-guard/malformed", strict, [["state-transition-guard", "{nope"], ["state-transition-guard", ""], ["state-transition-guard", "[1]"], ["state-transition-guard", "null"]])
    check_("state-transition-guard/integrity_hook_write", joined, [guard("echo x > {P}/.claude/hooks/aidlc-x.ts"), guard("", tool="Write", tool_input={"file_path": "{P}/aidlc/.aidlc-sessions/s1.binding.json", "content": "{}"}), guard("bun .claude/hooks/aidlc-review-freeze.ts")])
    check_("state-transition-guard/integrity_audit_write", joined, [guard("echo x >> {D}/audit/laptop-abc123.md")])
    check_("state-transition-guard/integrity_before_fence", joined, [guard("bun .claude/tools/aidlc-state.ts approve x > {P}/.claude/hooks/aidlc-x.ts")])
    check_("state-transition-guard/directive_marker_direct", {**strict, f"{engine}/active-directive.json": '{"stage": "intent-capture"}'}, [guard("bun .claude/tools/aidlc-state.ts approve intent-capture")])
    check_("state-transition-guard/directive_marker_delegated", {**strict, f"{engine}/active-directive.json": '{"stage": "intent-capture"}'}, [guard("aidlc next", AGENT)])
    for marker in ("STOP_HOOK_PROBE", "ROUTE_CHECK"):
        check_(f"state-transition-guard/observer_{marker.lower()}", speaking, [guard("bun .claude/tools/aidlc-state.ts approve intent-capture"), guard("aidlc next", AGENT)], {marker: "1"})

    # record-human-turn: the person's turn on the ledger, the message record, the words kept for a gate.
    turn = lambda prompt, **more: ["record-human-turn", payload(hook_event_name="UserPromptSubmit", prompt=prompt, **more)]
    pick = lambda question, answer, options=("Approve", "Request Changes"), **more: ["record-human-turn", payload(
        hook_event_name="PostToolUse", tool_name="AskUserQuestion",
        tool_input={"questions": [{"question": question, "options": [{"label": o} for o in options], **more}]},
        tool_response={"answers": {question: answer}})]
    check_("record-human-turn/typed", joined, [turn("use postgres please")])
    check_("record-human-turn/typed_twice", joined, [turn("use postgres"), turn("  and redis  ")])
    check_("record-human-turn/typed_outsider", {**no_cursor, **binding("other-9f8e7d6c")}, [turn("use postgres")])
    check_("record-human-turn/typed_unjoined", {**no_cursor, **binding(None, "unjoined")}, [turn("use postgres")])
    check_("record-human-turn/typed_cursor_only", created(), [turn("use postgres")])
    check_("record-human-turn/typed_cold", CLONE, [turn("use postgres")])
    check_("record-human-turn/typed_no_state", {**CLONE, "aidlc/spaces/default/intents/intents.json": CREATED_REGISTRY, **binding()}, [turn("use postgres")])
    check_("record-human-turn/typed_no_session", joined, [["record-human-turn", json.dumps({"hook_event_name": "UserPromptSubmit", "prompt": "use postgres"})]])
    check_("record-human-turn/typed_bad_session", joined, [["record-human-turn", json.dumps({"session_id": "../x", "hook_event_name": "UserPromptSubmit", "prompt": "use postgres"})]])
    check_("record-human-turn/typed_user_prompt_key", joined, [["record-human-turn", payload(hook_event_name="UserPromptSubmit", user_prompt="use postgres")], ["record-human-turn", payload(hook_event_name="UserPromptSubmit", message="and redis")]])
    check_("record-human-turn/typed_empty_prompt", joined, [turn(""), turn("   ")])
    check_("record-human-turn/typed_json_scalar", joined, [turn("1"), turn("true"), turn('"quoted"'), turn('{"answer": "two"}'), turn("[\"three\"]")])
    check_("record-human-turn/typed_envelope", joined, [turn("memory\n[CURRENT USER REQUEST -- respond to this]\nuse postgres"), turn("a\n[CURRENT USER REQUEST — respond to this]\r\nb\n[CURRENT USER REQUEST -- respond to this]\nc")])
    check_("record-human-turn/command_prompt", joined, [turn("/aidlc"), turn("$aidlc"), turn("/aidlc-runner go"), turn("/AIDLC")])
    check_("record-human-turn/slash_words", joined, [turn("/api/users returns 500"), turn("aidlc on the login page")])
    check_("record-human-turn/entry_words", joined, [turn("/aidlc next"), turn("/aidlc use postgres")])
    check_("record-human-turn/switch_question", joined, [turn("skip plan approval?"), turn("guard policy relaxed?")])
    check_("record-human-turn/typed_switch", joined, [turn("skip plan approval")])
    check_("record-human-turn/typed_switch_flag", joined, [turn("/aidlc --guard-policy off")])
    check_("record-human-turn/typed_switch_outsider", {**no_cursor, **binding("other-9f8e7d6c")}, [turn("skip plan approval")])
    check_("record-human-turn/typed_switch_unattended", joined, [turn("skip plan approval")], {"UNATTENDED": "1"})
    check_("record-human-turn/unattended", joined, [turn("use postgres")], {"UNATTENDED": "1"})
    check_("record-human-turn/unattended_picker", joined, [pick("Approve the plan?", "Approve")], {"UNATTENDED": "1"})
    check_("record-human-turn/override_phrase", joined, [turn("Override Plan Approval: the reviewer is away")])
    check_("record-human-turn/picker_gate", joined, [pick("Approve the plan?", "Approve")])
    check_("record-human-turn/picker_gate_recommended", joined, [pick("Approve the plan?", "Request Changes (Recommended)", options=("Approve", "Request Changes (Recommended)"))])
    check_("record-human-turn/picker_free_text", joined, [pick("Which database?", "use oauth", options=("Postgres", "MySQL"))])
    check_("record-human-turn/picker_label", joined, [pick("Which database?", "Postgres", options=("Postgres", "MySQL"))])
    check_("record-human-turn/picker_accept_as_is", joined, [pick("Approve?", "Accept as-is", options=("Approve", "Request Changes", "Accept as-is"))])
    check_("record-human-turn/picker_multi", joined, [["record-human-turn", payload(hook_event_name="PostToolUse", tool_name="AskUserQuestion",
        tool_input={"questions": [{"question": "Pick", "multiSelect": True, "options": [{"label": "a"}, {"label": "b"}]}]}, tool_response={"answers": {"Pick": ["a", "b"]}})]])
    check_("record-human-turn/picker_several_questions", joined, [["record-human-turn", payload(hook_event_name="PostToolUse", tool_name="AskUserQuestion",
        tool_input={"questions": [{"question": "One", "options": ["a", "b"]}, {"question": "Two", "options": ["c"]}]}, tool_response={"answers": {"One": "a", "Two": "c"}})]])
    check_("record-human-turn/picker_blank_question", joined, [["record-human-turn", payload(hook_event_name="PostToolUse", tool_name="AskUserQuestion",
        tool_input={"questions": [{"question": "One", "options": ["a"]}, {"question": "Two", "options": ["c"]}]}, tool_response={"answers": {"One": "a", "Extra": "z"}})]])
    check_("record-human-turn/picker_codex", joined, [["record-human-turn", payload(hook_event_name="PostToolUse", tool_name="request_user_input",
        tool_input={"questions": [{"id": "q1", "question": "Approve the plan?", "options": [{"label": "Approve"}, {"label": "Request Changes"}]}]},
        picker_reply={"answers": {"q1": {"answers": ["Approve"]}}}, tool_response="{}")]])
    check_("record-human-turn/picker_string_response", joined, [["record-human-turn", payload(hook_event_name="PostToolUse", tool_name="AskUserQuestion",
        tool_input={"questions": [{"question": "Q", "options": ["a"]}]}, tool_response=json.dumps({"answers": {"Q": "a"}}))]])
    check_("record-human-turn/picker_empty", joined, [["record-human-turn", payload(hook_event_name="PostToolUse", tool_name="AskUserQuestion", tool_input={"questions": [{"question": "Q", "options": ["a"]}]}, tool_response={"answers": {}})]])
    check_("record-human-turn/picker_empty_string", joined, [["record-human-turn", payload(hook_event_name="PostToolUse", tool_name="AskUserQuestion", tool_input={"questions": [{"question": "Q"}]}, tool_response='{"answers":{}}')]])
    check_("record-human-turn/picker_null_response", joined, [["record-human-turn", payload(hook_event_name="PostToolUse", tool_name="AskUserQuestion", tool_input={"questions": [{"question": "Q"}]}, tool_response=None)]])
    check_("record-human-turn/picker_cancelled", joined, [pick("Q", "Cancelled", options=("a", "b"))])
    check_("record-human-turn/picker_no_input", joined, [["record-human-turn", payload(hook_event_name="PostToolUse", tool_name="AskUserQuestion", tool_response={"answers": {"Q": "a"}})]])
    check_("record-human-turn/other_event", joined, [["record-human-turn", payload(hook_event_name="PostToolUse", tool_name="Edit")], ["record-human-turn", payload(hook_event_name="Stop")]])
    check_("record-human-turn/malformed", joined, [["record-human-turn", "{nope"], ["record-human-turn", ""], ["record-human-turn", "[1]"], ["record-human-turn", "null"]])
    check_("record-human-turn/malformed_cold", CLONE, [["record-human-turn", "{nope"]])
    check_("record-human-turn/host_notification", joined, [turn("<task-notification>\nsomething finished\n</task-notification>")])
    check_("record-human-turn/host_notification_not_whole", joined, [turn("<task-notification>\nx\n</task-notification> and more")])
    check_("record-human-turn/host_origin_supplied", joined, [turn("use postgres", origin={"kind": "host", "reason": "  kiro workflow brief  ", "source": "record"})])
    check_("record-human-turn/host_origin_long_reason", joined, [turn("use postgres", origin={"kind": "host", "reason": "r" * 300, "source": "template"})])
    check_("record-human-turn/origin_malformed", joined, [turn("use postgres", origin={"kind": "host", "reason": "", "source": "record"}), turn("and redis", origin={"kind": "host", "reason": "x", "source": "other"}), turn("and more", origin="host")])
    check_("record-human-turn/person_origin_supplied", joined, [turn("use postgres", origin={"kind": "person", "source": "record"})])
    check_("record-human-turn/host_origin_no_state", {**CLONE, "aidlc/spaces/default/intents/intents.json": CREATED_REGISTRY, **binding()}, [turn("use postgres", origin={"kind": "host", "reason": "x", "source": "record"})])
    transcript = lambda rows: "\n".join(json.dumps(row) for row in rows) + "\n"
    system_row = {"type": "user", "promptId": "p1", "promptSource": "system", "turnOrigin": "task_notification", "message": {"role": "user", "content": [{"type": "text", "text": "use postgres"}]}}
    human_row = {"type": "user", "promptId": "p1", "turnOrigin": "human", "message": {"role": "user", "content": "use postgres"}}
    check_("record-human-turn/transcript_host", {**joined, "p/chat.jsonl": transcript([human_row, system_row])}, [turn("use postgres", transcript_path="{P}/p/chat.jsonl", prompt_id="p1")])
    check_("record-human-turn/transcript_person", {**joined, "p/chat.jsonl": transcript([system_row, human_row])}, [turn("use postgres", transcript_path="{P}/p/chat.jsonl", prompt_id="p1")])
    check_("record-human-turn/transcript_other_prompt", {**joined, "p/chat.jsonl": transcript([system_row])}, [turn("use postgres", transcript_path="{P}/p/chat.jsonl", prompt_id="p2"), turn("and redis", transcript_path="{P}/p/chat.jsonl", prompt_id="p1")])
    check_("record-human-turn/transcript_no_id", {**joined, "p/chat.jsonl": transcript([{**system_row, "promptId": "zz"}])}, [turn("use postgres", transcript_path="{P}/p/chat.jsonl")])
    check_("record-human-turn/transcript_origin_kind", {**joined, "p/chat.jsonl": transcript([{"type": "user", "origin": {"kind": "task-notification"}, "content": "use postgres"}])}, [turn("use postgres", transcript_path="{P}/p/chat.jsonl")])
    check_("record-human-turn/transcript_missing", joined, [turn("use postgres", transcript_path="{P}/p/nope.jsonl", prompt_id="p1"), turn("and redis", transcript_path="{P}/p/chat.txt", prompt_id="p1")])
    check_("record-human-turn/transcript_malformed", {**joined, "p/chat.jsonl": "{nope\n\n" + json.dumps(system_row) + "\n"}, [turn("use postgres", transcript_path="{P}/p/chat.jsonl", prompt_id="p1")])
    check_("record-human-turn/transcript_supplied_wins", {**joined, "p/chat.jsonl": transcript([system_row])}, [turn("use postgres", transcript_path="{P}/p/chat.jsonl", prompt_id="p1", origin={"kind": "person"})])
    check_("record-human-turn/long_prompt", joined, [turn("x" * 8100)])
    check_("record-human-turn/long_prompt_exact", joined, [turn("y" * 8000), turn("z" * 8001)])
    check_("record-human-turn/gate_words_cap", joined, [turn(f"message {n}") for n in range(10)])
    check_("record-human-turn/gate_words_other_shard", {**joined, f"{engine}/gate-words/s1.json": json.dumps({"version": 1, "session": "s1", "shard": "other/audit/x.md", "dropped": 0, "messages": [{"offset": 1, "text": "old"}]})}, [turn("use postgres")])
    # (The prior record's offsets are 0: upstream's are read one byte back before comparing, as SHAPES says.)
    check_("record-human-turn/gate_words_same_shard", {**joined, f"{engine}/gate-words/s1.json": json.dumps({"version": 1, "session": "s1", "shard": CREATED_SHARD, "dropped": 0, "messages": [{"offset": 0, "text": "old"}]})}, [turn("use postgres")])
    check_("record-human-turn/message_prune_old", {**joined, "aidlc/.aidlc-sessions/messages/0000aaaa.json": json.dumps({"session": "s1", "at": "2020-01-01T00:00:00Z", "source": "prompt", "text": "old", "picker": None, "words": "old", "settings": [], "route": {"scope": None, "newIntent": False, "skip": [], "add": [], "projectType": None}, "applied": [], "id": "0000aaaa"}) + "\n", "aidlc/.aidlc-sessions/messages/not-a-record.json": "{}", "aidlc/.aidlc-sessions/messages/0000bbbb.json": "{nope"}, [turn("use postgres")])
    check_("record-human-turn/retired_field", {**joined, f"{CREATED_DIR}/aidlc-state.md": CREATED_STATE.replace("- **Guard Policy**: off (from scope feature)", "- **Change Control**: relaxed (set by you)")}, [turn("use postgres")])
    check_("record-human-turn/retired_field_strict", {**joined, f"{CREATED_DIR}/aidlc-state.md": CREATED_STATE.replace("- **Guard Policy**: off (from scope feature)", "- **Change Control**: strict (set by you)")}, [turn("use postgres")])
    check_("record-human-turn/retired_field_picker", {**joined, f"{CREATED_DIR}/aidlc-state.md": CREATED_STATE.replace("- **Guard Policy**: off (from scope feature)", "- **Change Control**: off (set by you)")}, [pick("Q", "a", options=("a", "b"))])
    check_("record-human-turn/handoff_cleared", {**joined, "aidlc/.aidlc-sessions/s1.handoff.json": '{"from": "x"}'}, [turn("use postgres")])
    check_("record-human-turn/handoff_kept_by_picker", {**joined, "aidlc/.aidlc-sessions/s1.handoff.json": '{"from": "x"}'}, [pick("Q", "a", options=("a", "b"))])
    check_("record-human-turn/person_lines", {**joined, "aidlc/.aidlc-sessions/s1.person-lines": '{"work": "x", "lines": [], "said": []}'}, [turn("use postgres")])
    check_("record-human-turn/plan_challenge", {**joined, "aidlc/.aidlc-sessions/plan-approval/challenge-s1.json": '{"version": 1}'}, [turn("use postgres")])
    check_("record-human-turn/protected_question", {**joined, "aidlc/.aidlc-sessions/plan-approval/protected-question-s2.json": '{"version": 1}'}, [turn("use postgres")])
    check_("record-human-turn/directive_marker", {**joined, f"{engine}/active-directive.json": '{"version": 2, "kind": "ask"}'}, [turn("use postgres")])
    check_("record-human-turn/open_review", {**joined, CREATED_SHARD: CREATED_AUDIT + block("REVIEW_REQUESTED", T(2), "**Stage**: intent-capture", "**Reviewer**: aidlc-product-lead-agent", "**Iteration**: 1")}, [turn("use postgres")])
    check_("record-human-turn/open_review_completed", {**joined, CREATED_SHARD: CREATED_AUDIT + block("REVIEW_REQUESTED", T(2), "**Stage**: intent-capture", "**Reviewer**: aidlc-product-lead-agent", "**Iteration**: 1") + block("REVIEW_COMPLETED", T(3), "**Stage**: intent-capture", "**Reviewer**: aidlc-product-lead-agent", "**Iteration**: 1", "**Verdict**: READY")}, [turn("use postgres")])
    check_("record-human-turn/open_review_reviewed", {**joined, f"{CREATED_DIR}/aidlc-state.md": CREATED_STATE.replace("- **Review Override**: none", "- **Review Override**: advisory"), CREATED_SHARD: CREATED_AUDIT + block("REVIEW_REQUESTED", T(2), "**Stage**: intent-capture", "**Reviewer**: aidlc-product-lead-agent", "**Iteration**: 1")}, [turn("use postgres")])
    check_("record-human-turn/open_review_reviewed_done", {**joined, f"{CREATED_DIR}/aidlc-state.md": CREATED_STATE.replace("- **Review Override**: none", "- **Review Override**: advisory"), CREATED_SHARD: CREATED_AUDIT + block("REVIEW_REQUESTED", T(2), "**Stage**: intent-capture", "**Reviewer**: aidlc-product-lead-agent", "**Iteration**: 1") + block("REVIEW_COMPLETED", T(3), "**Stage**: intent-capture", "**Reviewer**: aidlc-product-lead-agent", "**Iteration**: 1", "**Verdict**: READY")}, [turn("use postgres")])
    check_("record-human-turn/open_review_reviewed_passed", {**joined, f"{CREATED_DIR}/aidlc-state.md": CREATED_STATE.replace("- **Review Override**: none", "- **Review Override**: advisory"), CREATED_SHARD: CREATED_AUDIT + block("REVIEW_REQUESTED", T(2), "**Stage**: intent-capture", "**Reviewer**: aidlc-product-lead-agent", "**Iteration**: 1") + block("DECISION_RECORDED", T(3), "**Stage**: intent-capture", "**Decision**: x")}, [turn("use postgres")])
    check_("record-human-turn/open_review_picker", {**joined, CREATED_SHARD: CREATED_AUDIT + block("REVIEW_REQUESTED", T(2), "**Stage**: intent-capture", "**Reviewer**: aidlc-product-lead-agent", "**Iteration**: 1")}, [pick("Q", "a", options=("a", "b"))])
    check_("record-human-turn/no_shard", {**joined, CREATED_SHARD: None}, [turn("use postgres")])
    for marker in ("STOP_HOOK_PROBE", "ROUTE_CHECK"):
        check_(f"record-human-turn/observer_{marker.lower()}", joined, [turn("use postgres"), pick("Q", "a", options=("a", "b"))], {marker: "1"})

    # plan-approval-guard: what waits for the plan at Code Generation, what never does, and the refusal while the
    # engine's step is not there to read.
    DEVELOPER = "aidlc-developer-agent"
    before_code = ideation + ["approval-handoff", "practices-discovery", "requirements-analysis", "user-stories", "refined-mockups", "domain-design", "units-generation", "contract-design", "delivery-planning", "functional-design", "nfr-requirements", "nfr-design", "infrastructure-design"]
    lowered_state = at_stage(CREATED_STATE, "code-generation", done=before_code, phase="CONSTRUCTION")
    building_state = lowered_state.replace("- **Guard Policy**: off (from scope feature)", "- **Guard Policy**: strict (set by you)")
    building = {**created(building_state), **binding()}
    plan = lambda tool="Write", more=None, **input: ["plan-approval-guard", payload(hook_event_name="PreToolUse", tool_name=tool, tool_input={k: (v.replace("{D}", "{P}/" + CREATED_DIR) if isinstance(v, str) else v) for k, v in input.items()}, **(more or {}))]
    run = lambda command, **more: ["plan-approval-guard", payload(hook_event_name="PreToolUse", tool_name="Bash", tool_input={"command": command.replace("{D}", "{P}/" + CREATED_DIR)}, **more)]
    source = plan(file_path="{P}/src/app.ts")
    check_("plan-approval-guard/write_source", building, [source, plan("Edit", file_path="src/app.ts"), plan("MultiEdit", file_path="{P}/src/a.ts"), plan("NotebookEdit", notebook_path="{P}/n.ipynb")])
    check_("plan-approval-guard/write_source_policy_off", {**created(lowered_state), **binding()}, [source, run("rm -rf src")])
    check_("plan-approval-guard/write_source_relaxed", {**created(lowered_state.replace("off (from scope feature)", "relaxed (set by you)")), **binding()}, [source])
    check_("plan-approval-guard/write_source_switch_off", {**created(building_state + "- **Guards Off**: plan-approval (set by you)\n"), **binding()}, [source])
    check_("plan-approval-guard/write_source_plan_approval_off", {**created(building_state.replace("- **Plan Approval**: on (from scope feature)", "- **Plan Approval**: off (set by you)")), **binding()}, [source])
    check_("plan-approval-guard/write_source_memory_strict", {**created(lowered_state), **binding(), "aidlc/spaces/default/memory/team.md": "# Team\n\n## Guard Policy\n\nMode: strict\n"}, [source])
    check_("plan-approval-guard/write_source_policy_unreadable", {**created(lowered_state.replace("off (from scope feature)", "sometimes")), **binding()}, [source])
    check_("plan-approval-guard/write_source_subagent", building, [plan(file_path="{P}/src/app.ts", more={"agent_type": DEVELOPER})])
    check_("plan-approval-guard/write_outside", building, [plan(file_path="{P}/../outside.md"), plan(file_path="/tmp/outside-of-the-project.md"), plan("MultiEdit", file_path="{P}/../a.md", paths=["/tmp/outside-b.md"])])
    check_("plan-approval-guard/write_outside_and_inside", building, [plan("MultiEdit", file_path="{P}/../a.md", paths=["{P}/src/b.ts"])])
    check_("plan-approval-guard/write_no_target", building, [plan(), plan(file_path=""), plan(content="x")])
    check_("plan-approval-guard/write_engine_records", building, [plan(file_path="{D}/construction/code-generation/memory.md"), plan(file_path="{D}/.aidlc-engine/answer-text/a.txt"),
                                                                  plan(file_path="{P}/aidlc/spaces/default/intents/.aidlc-engine/composer-proposal.json")])
    check_("plan-approval-guard/write_record_other", building, [plan(file_path="{D}/construction/code-generation/code-generation-plan.md"), plan(file_path="{D}/aidlc-state.md"), plan(file_path="{D}/construction/build-and-test/memory.md"),
                                                                plan(file_path="{D}/.aidlc-engine/other/a.txt")])
    check_("plan-approval-guard/write_diary_and_source", building, [plan("MultiEdit", file_path="{D}/construction/code-generation/memory.md", paths=["{P}/src/a.ts"])])
    check_("plan-approval-guard/write_diary_linked", building, [plan(file_path="{D}/construction/code-generation/memory.md")],
           edits=(lambda tree: (os.makedirs(tree.path(f"{CREATED_DIR}/construction"), exist_ok=True), os.symlink(tree.path("elsewhere"), tree.path(f"{CREATED_DIR}/construction/code-generation"))),))
    check_("plan-approval-guard/read_tools", building, [plan("Read", file_path="{P}/src/app.ts"), plan("Grep", pattern="x"), plan("AskUserQuestion"), plan("TodoRead"), plan("WebFetch", url="x")])
    check_("plan-approval-guard/unknown_tool", building, [plan("TodoWrite", todos=[]), plan("mcp__x__write", path="a")])
    check_("plan-approval-guard/no_tool", building, [["plan-approval-guard", payload(hook_event_name="PreToolUse")], ["plan-approval-guard", payload(hook_event_name="PreToolUse", tool_name="Write")]])
    check_("plan-approval-guard/other_dispatch", building, [plan("Task", subagent_type="aidlc-product-agent", prompt="x"), plan("Task", prompt="x"), plan("Agent", subagent_type=" " + DEVELOPER)])
    check_("plan-approval-guard/dispatch_developer", building, [plan("Task", subagent_type=DEVELOPER, prompt="Build it"), plan("Agent", subagent_type=DEVELOPER, prompt="AIDLC-UNIT: auth\nBuild it")])
    check_("plan-approval-guard/dispatch_developer_policy_off", {**created(lowered_state), **binding()}, [plan("Task", subagent_type=DEVELOPER, prompt="Build it")])
    check_("plan-approval-guard/dispatch_developer_other_stage", {**created(building_state.replace("- **Current Stage**: code-generation", "- **Current Stage**: build-and-test")), **binding()},
           [plan("Task", subagent_type=DEVELOPER, prompt="Build it"), plan("Task", subagent_type=DEVELOPER, prompt="AIDLC-UNIT: auth"), plan("Task", subagent_type=DEVELOPER, description="AIDLC-STAGE: Code Generation"),
            plan("Task", subagent_type=DEVELOPER, prompt="x", description="AIDLC-TESTING-CONTRACT: abc")])
    check_("plan-approval-guard/other_stage", {**created(CREATED_STATE.replace("off (from scope feature)", "strict (set by you)")), **binding()}, [source, run("rm -rf src"), plan("TodoWrite", todos=[])])
    check_("plan-approval-guard/stage_spelled", {**created(building_state.replace("- **Current Stage**: code-generation", "- **Current Stage**:  Code  Generation ")), **binding()}, [source])
    check_("plan-approval-guard/shell_reads", building, [run("cat README.md"), run("ls -la && git status"), run("npm test"), run("grep -r TODO src | head -3"), run("git diff --stat; git log -1")])
    check_("plan-approval-guard/shell_writes", building, [run("echo x > src/a.ts"), run("rm -rf src"), run("cp a.ts b.ts"), run("sed -i s/a/b/ src/a.ts"), run("cat a.ts | tee b.ts")])
    check_("plan-approval-guard/shell_writes_outside", building, [run("echo x > {P}/../out.txt"), run("touch /tmp/outside-of-the-project"), run("echo x > a.ts", cwd="{P}/..")])
    check_("plan-approval-guard/shell_unreadable", building, [run("echo $HOME"), run("ls\r"), run("sh build.sh"), run("node -e 'x'"), run("sort -o a b"), run("echo a \\> b"), run("./run.sh")])
    check_("plan-approval-guard/shell_engine_admitted", building, [run("aidlc engine orchestrate next"), run("aidlc status"), run("aidlc engine now"), run("aidlc engine graph topo"), run("aidlc doctor"),
                                                                  run("aidlc engine log decision --stage code-generation --checkpoint plan-approval --note x"), run("aidlc engine config set guard.review-freeze on"),
                                                                  run("aidlc config flags --clear-bypass AIDLC_DISABLE_SENSORS --project-dir {P}"), run("cd {P} && aidlc status")])
    check_("plan-approval-guard/shell_engine_refused", building, [run("aidlc engine state approve code-generation"), run("aidlc engine bolt start"), run("env aidlc status"), run("aidlc engine jump execute --target build-and-test"),
                                                                 run("cd {P}/src && aidlc status"), run("aidlc status > out.txt"), run("aidlc config flags --clear-bypass AIDLC_DISABLE_SENSORS --project-dir {P}/..")])
    asked_for = [run("aidlc engine config set guard.review-freeze off"), run("aidlc config flags --bypass AIDLC_DISABLE_SENSORS"), run("aidlc engine intent add-repo api"), run("aidlc engine config set plan-approval off"),
                 run("aidlc engine jump execute --target build-and-test")]
    spoken = {**building, CREATED_SHARD: CREATED_AUDIT + HUMAN(1)}
    parked_state = building_state.replace("## Runtime State\n", "## Runtime State\n- **Parked**: 2026-01-05T10:02:00Z\n")
    check_("plan-approval-guard/person_silent", building, asked_for)
    check_("plan-approval-guard/person_spoke", spoken, asked_for)
    check_("plan-approval-guard/person_spoke_parked", {**created(parked_state, CREATED_AUDIT + HUMAN(1)), **binding()}, asked_for + [run("aidlc engine recompose --skip ci-pipeline"), run("aidlc engine scope change --scope bugfix")])
    check_("plan-approval-guard/person_silent_parked", {**created(parked_state), **binding()}, asked_for)
    check_("plan-approval-guard/person_asked_a_question", {**building, CREATED_SHARD: CREATED_AUDIT + HUMAN(1, 0, "**Reply**: question")}, asked_for)
    check_("plan-approval-guard/person_turn_used", {**building, CREATED_SHARD: CREATED_AUDIT + HUMAN(1) + block("GATE_APPROVED", T(2), "**Stage**: infrastructure-design")}, asked_for)
    check_("plan-approval-guard/person_unattended", spoken, asked_for, {"UNATTENDED": "1"})
    gate = [run("aidlc engine orchestrate report --stage code-generation --result approved"), run("aidlc engine orchestrate report --stage code-generation --result rejected")]
    check_("plan-approval-guard/gate_closed", building, gate)
    check_("plan-approval-guard/gate_open", {**created(building_state.replace("- [-] code-generation ", "- [?] code-generation ")), **binding()}, gate)
    check_("plan-approval-guard/powershell", building, [run("Get-Content a.txt", aidlc_shell="powershell"), run("aidlc.cmd status", aidlc_shell="powershell"), run("Remove-Item a.txt", aidlc_shell="powershell"),
                                                        run("Set-Content -Path src\\a.ts -Value x", aidlc_shell="powershell"), run("C:\\Tools\\aidlc.exe status", aidlc_shell="powershell"), run("aidlc.cmd status"),
                                                        run("Get-Content $x", aidlc_shell="powershell"), run("aidlc.cmd status", aidlc_shell="posix")])
    check_("plan-approval-guard/tool_script", building, [run("bun .claude/tools/aidlc-utility.ts status"), run("bun .claude/tools/aidlc.ts status"), run("bun .claude/tools/aidlc-graph.ts topo")])
    check_("plan-approval-guard/tool_script_refused", building, [run("bun .claude/tools/aidlc-state.ts approve code-generation"), run("bun -r x .claude/tools/aidlc-utility.ts status"), run("bun src/aidlc-utility.ts status")])
    check_("plan-approval-guard/integrity", building, [run("echo x >> {D}/audit/laptop-abc123.md"), plan(file_path="{D}/audit/laptop-abc123.md")])
    off = {"DISABLE_PLAN_APPROVAL_GUARD": "1"}
    check_("plan-approval-guard/off_switch", building, [source, plan(file_path="{P}/src/b.ts"), run("rm -rf src"), plan("Task", subagent_type=DEVELOPER, prompt="Build it")], off)
    check_("plan-approval-guard/off_switch_zero", building, [source], {"DISABLE_PLAN_APPROVAL_GUARD": "0"})
    check_("plan-approval-guard/off_switch_settings", {**building, "aidlc.settings.local.json": '{"schemaVersion": 1, "flags": {"schemaVersion": 1, "bypasses": ["AIDLC_DISABLE_PLAN_APPROVAL_GUARD"]}}\n'}, [source, source])
    check_("plan-approval-guard/off_switch_already_said", {**building, CREATED_SHARD: CREATED_AUDIT + block("GUARD_DISABLED", T(2), "**Guard**: plan-approval-guard", "**Tool**: Write")}, [source], off)
    check_("plan-approval-guard/off_switch_said_by_another", {**building, CREATED_SHARD: CREATED_AUDIT + block("GUARD_DISABLED", T(2), "**Guard**: review-freeze", "**Tool**: Write")}, [source], off)
    check_("plan-approval-guard/off_switch_said_before_a_row", {**building, CREATED_SHARD: CREATED_AUDIT + block("GUARD_DISABLED", T(2), "**Guard**: plan-approval-guard", "**Tool**: Write") + HUMAN(3)}, [source], off)
    check_("plan-approval-guard/off_switch_other_stage", joined, [source, plan("Read", file_path="x"), ["plan-approval-guard", payload(hook_event_name="PreToolUse")]], off)
    check_("plan-approval-guard/off_switch_no_shard", {**building, CREATED_SHARD: None}, [source], off)
    check_("plan-approval-guard/off_switch_no_state", {**CLONE, "aidlc/spaces/default/intents/intents.json": CREATED_REGISTRY, **binding()}, [source], off)
    check_("plan-approval-guard/off_switch_outsider", {**building, "aidlc/spaces/default/intents/active-intent": None, **binding("other-9f8e7d6c")}, [source], off)
    check_("plan-approval-guard/settings_refused", {**building, "aidlc.settings.local.json": '{"flags": {}}\n'}, [source])
    check_("plan-approval-guard/outsider", {**building, "aidlc/spaces/default/intents/active-intent": None, **binding("other-9f8e7d6c")}, [source, run("rm -rf src"), plan("Task", subagent_type=DEVELOPER, prompt="Build it"), plan("Task", subagent_type="aidlc-product-agent")])
    check_("plan-approval-guard/unjoined", {**building, "aidlc/spaces/default/intents/active-intent": None, **binding(None, "unjoined")}, [source, plan("Agent", subagent_type=DEVELOPER)])
    check_("plan-approval-guard/cursor_only", created(building_state), [source])
    check_("plan-approval-guard/no_session", building, [["plan-approval-guard", json.dumps({"hook_event_name": "PreToolUse", "tool_name": "Write", "tool_input": {"file_path": "{P}/src/app.ts"}})]])
    check_("plan-approval-guard/no_state", {**CLONE, "aidlc/spaces/default/intents/intents.json": CREATED_REGISTRY, **binding()}, [source, plan("Task", subagent_type=DEVELOPER, prompt="AIDLC-UNIT: auth")])
    check_("plan-approval-guard/cold", CLONE, [source, run("rm -rf src")])
    check_("plan-approval-guard/no_shard", {**building, CREATED_SHARD: None}, [source])
    check_("plan-approval-guard/malformed", building, [["plan-approval-guard", "{nope"], ["plan-approval-guard", ""], ["plan-approval-guard", "[1]"]])
    check_("plan-approval-guard/registry_malformed", {**building, "aidlc/spaces/default/intents/intents.json": "{nope"}, [source])
    check_("plan-approval-guard/directive_marker", {**building, f"{engine}/active-directive.json": '{"version": 2, "stage": "code-generation"}'}, [source])
    check_("plan-approval-guard/directive_marker_passing", {**building, f"{engine}/active-directive.json": '{"version": 2, "stage": "code-generation"}'}, [plan("Read", file_path="x"), plan("Task", prompt="x")])
    check_("plan-approval-guard/unit_scope_stamp", {**building, "aidlc/.aidlc-unit-scope.json": '{"version": 1}'}, [source])
    for marker in ("STOP_HOOK_PROBE", "ROUTE_CHECK"):
        check_(f"plan-approval-guard/observer_{marker.lower()}", building, [source], {marker: "1"})
    # The edges a mutation sweep asked for.
    check_("plan-approval-guard/write_naming_the_developer", building, [plan(file_path="{P}/../outside.md", subagent_type=DEVELOPER), plan(file_path="{P}/src/app.ts", subagent_type=DEVELOPER)])
    check_("plan-approval-guard/write_with_markers_other_stage", {**created(building_state.replace("- **Current Stage**: code-generation", "- **Current Stage**: build-and-test")), **binding()},
           [plan(file_path="{P}/src/app.ts", prompt="AIDLC-UNIT: auth"), plan("Task", subagent_type=DEVELOPER, prompt="x", description="AIDLC-UNIT: auth"),
            plan("Task", subagent_type=DEVELOPER, prompt="AIDLC-TESTING-CONTRACT: sha256:" + "a" * 64), plan("Task", subagent_type=DEVELOPER, prompt="x AIDLC-UNIT: auth")])
    check_("plan-approval-guard/shell_write_of_the_diary", building, [run("echo x >> {D}/construction/code-generation/memory.md"), run("echo x > {P}/aidlc/spaces/default/intents/.aidlc-engine/composer-proposal.json")])
    check_("plan-approval-guard/write_source_memory_unreadable", {**created(lowered_state), **binding(), "aidlc/spaces/default/memory/org.md": "## Guard Policy\n\nMode: maybe\n"}, [source])
    # A recorded tree holds files: a state that is a folder stays a live check.
    UNRECORDED.update({"hook/plan-approval-guard/state_is_directory", "hook/plan-approval-guard/state_is_directory_policy_off"})
    check_("plan-approval-guard/state_is_directory", building, [source],
           edits=(lambda tree: (os.remove(tree.path(f"{CREATED_DIR}/aidlc-state.md")), os.makedirs(tree.path(f"{CREATED_DIR}/aidlc-state.md"))),))
    check_("plan-approval-guard/state_is_directory_policy_off", {**created(lowered_state), **binding()}, [source],
           edits=(lambda tree: (os.remove(tree.path(f"{CREATED_DIR}/aidlc-state.md")), os.makedirs(tree.path(f"{CREATED_DIR}/aidlc-state.md"))),))
    check_("plan-approval-guard/off_switch_no_tool", building, [["plan-approval-guard", payload(hook_event_name="PreToolUse")]], off)
    check_("plan-approval-guard/off_switch_guard_named_by_another_row", {**building, CREATED_SHARD: CREATED_AUDIT + block("ERROR_LOGGED", T(2), "**Guard**: plan-approval-guard", "**Tool**: Write")}, [source], off)
    check_("plan-approval-guard/off_switch_already_said_crlf", {**building, CREATED_SHARD: (CREATED_AUDIT + block("GUARD_DISABLED", T(2), "**Guard**: plan-approval-guard", "**Tool**: Write")).replace("\n", "\r\n")}, [source], off)
    # The adapter's shell, read by the freeze as well.
    check_("review-freeze/review_rows_powershell", reviewed, [["review-freeze", payload(hook_event_name="PreToolUse", tool_name="Bash", tool_input={"command": "Set-Content -Path " + doc.replace("/", "\\") + " -Value x"}, aidlc_shell="powershell")]])
    check_("review-freeze/powershell_other_target", reviewed, [["review-freeze", payload(hook_event_name="PreToolUse", tool_name="Bash", tool_input={"command": "Set-Content -Path notes.md -Value x"}, aidlc_shell="powershell")]])

    # guard-tool-call: one registration that runs the four checks before a tool call in one process, each for the
    # tools its own row matched.
    group = lambda tool="Write", more=None, **input: ["guard-tool-call", payload(hook_event_name="PreToolUse", tool_name=tool, tool_input={k: (v.replace("{D}", "{P}/" + CREATED_DIR) if isinstance(v, str) else v) for k, v in input.items()}, **(more or {}))]
    transition = "bun .claude/tools/aidlc-state.ts approve code-generation"
    check_("guard-tool-call/quiet", building, [group("Read", file_path="{P}/src/app.ts"), group("Glob", pattern="src/*"), group("LS", path="{P}"), group("Bash", command="ls -la")])
    check_("guard-tool-call/plan_refuses", building, [group(file_path="{P}/src/app.ts"), group("Bash", command="echo x > src/a.ts"), group("Task", subagent_type=DEVELOPER, prompt="Build it")])
    check_("guard-tool-call/plan_passes", building, [group(file_path="{P}/../outside.md"), group(file_path="{D}/construction/code-generation/memory.md"), group("Task", subagent_type="aidlc-product-agent")])
    check_("guard-tool-call/two_refuse", building, [group("Bash", command=transition)])
    check_("guard-tool-call/same_refusal_once", building, [group("Bash", command="echo x >> {D}/audit/laptop-abc123.md"), group(file_path="{D}/audit/laptop-abc123.md")])
    check_("guard-tool-call/transition_refuses", strict, [group("Bash", command="bun .claude/tools/aidlc-state.ts approve intent-capture"), group("Read", file_path="{P}/src/app.ts")])
    check_("guard-tool-call/reviewer_refuses", {**created(construction.replace("- **Guard Policy**: off (from scope feature)", "- **Guard Policy**: strict (set by a command)")), **binding(), **dispatch()}, [group("Read", file_path="{D}/construction/U02/nfr-design/security-design.md", more={"agent_type": REVIEWER}),
                                                       group("Read", file_path="{D}/construction/U01/nfr-design/security-design.md", more={"agent_type": REVIEWER})])
    check_("guard-tool-call/review_rows", reviewed, [group(file_path=doc)])
    check_("guard-tool-call/unmatched_tool", building, [group("TodoWrite", todos=[]), group("WebFetch", url="x"), group("mcp__x__write", path="{P}/src/a.ts"), group("write", file_path="{P}/src/app.ts"),
                                                      group("Write ", file_path="{P}/src/app.ts"), group("Bash\n", command="rm -rf src")])
    check_("guard-tool-call/dispatch_reaches_the_plan_only", strict, [group("Task", subagent_type=DEVELOPER, prompt="AIDLC-UNIT: auth"), group("Agent", subagent_type=DEVELOPER, prompt="Build it")])
    check_("guard-tool-call/read_skips_the_plan", {**building, f"{engine}/active-directive.json": '{"version": 2, "stage": "code-generation"}'}, [group("NotebookRead", notebook_path="{P}/n.ipynb")],
           {"DISABLE_REVIEWER_SCOPE_HOOK": "1", "DISABLE_REVIEW_FREEZE_HOOK": "1"})
    check_("guard-tool-call/no_tool_name", building, [["guard-tool-call", payload(hook_event_name="PreToolUse")], ["guard-tool-call", payload(hook_event_name="PreToolUse", tool_input={"file_path": "{P}/src/app.ts"})]])
    check_("guard-tool-call/tool_name_not_text", building, [["guard-tool-call", payload(hook_event_name="PreToolUse", tool_name=3, tool_input={"file_path": "{P}/src/app.ts"})],
                                                          ["guard-tool-call", payload(hook_event_name="PreToolUse", tool_name=None)]])
    check_("guard-tool-call/malformed", building, [["guard-tool-call", "{nope"], ["guard-tool-call", ""], ["guard-tool-call", "[1]"], ["guard-tool-call", "null"], ["guard-tool-call", '"Write"']])
    check_("guard-tool-call/stood_aside_and_refused", {**created(lowered_state), **binding()}, [group("Bash", command=transition), group(file_path="{P}/src/app.ts")])
    check_("guard-tool-call/stood_aside", joined, [group("Bash", command="bun .claude/tools/aidlc-state.ts approve intent-capture")])
    check_("guard-tool-call/settings_refused", {**building, "aidlc.settings.local.json": '{"flags": {}}\n'}, [group(file_path="{P}/src/app.ts"), group("Read", file_path="{P}/src/app.ts")])
    check_("guard-tool-call/off_switch", building, [group(file_path="{P}/src/app.ts"), group("Bash", command=transition)], off)
    check_("guard-tool-call/every_switch_off", building, [group("Bash", command=transition), group(file_path="{P}/src/app.ts")],
           {**off, "ALLOW_DIRECT_STATE_TRANSITIONS": "1", "DISABLE_REVIEWER_SCOPE_HOOK": "1", "DISABLE_REVIEW_FREEZE_HOOK": "1"})
    check_("guard-tool-call/outsider", {**building, "aidlc/spaces/default/intents/active-intent": None, **binding("other-9f8e7d6c")}, [group(file_path="{P}/src/app.ts"), group("Bash", command=transition), group("Task", subagent_type=DEVELOPER)])
    check_("guard-tool-call/cold", CLONE, [group(file_path="{P}/src/app.ts"), group("Bash", command=transition)])
    check_("guard-tool-call/no_event", building, [["guard-tool-call", payload(tool_name="Write", tool_input={"file_path": "{P}/src/app.ts"})], ["plan-approval-guard", payload(tool_name="Write", tool_input={"file_path": "{P}/src/app.ts"})]])
    check_("guard-tool-call/other_event", building, [["guard-tool-call", payload(hook_event_name="PostToolUse", tool_name="Write", tool_input={"file_path": "{P}/src/app.ts"})],
                                                   ["plan-approval-guard", payload(hook_event_name="PostToolUse", tool_name="Write", tool_input={"file_path": "{P}/src/app.ts"})]])
    check_("guard-tool-call/directive_marker", {**building, f"{engine}/active-directive.json": '{"version": 2, "stage": "code-generation"}'}, [group(file_path="{P}/src/app.ts")])
    # The edges a mutation sweep asked for.
    strict_reviewing = {**created(construction.replace("- **Guard Policy**: off (from scope feature)", "- **Guard Policy**: strict (set by a command)")), **binding(), **dispatch()}
    check_("guard-tool-call/reviewer_lists_and_searches", strict_reviewing, [group("LS", path="{D}/construction/U02", more={"agent_type": REVIEWER}), group("Glob", pattern="construction/*/design.md", more={"agent_type": REVIEWER}),
                                                                           group("NotebookRead", notebook_path="{D}/construction/U02/n.ipynb", more={"agent_type": REVIEWER}),
                                                                           group("Grep", pattern="x", path="{D}/construction/U02", more={"agent_type": REVIEWER})])
    check_("guard-tool-call/two_refuse_in_order", strict_reviewing, [group("Bash", command="cat {D}/construction/U02/design.md && bun .claude/tools/aidlc-state.ts approve nfr-design", more={"agent_type": REVIEWER})])
    check_("guard-tool-call/agent_dispatch", building, [group("Agent", subagent_type=DEVELOPER, prompt="Build it"), group("Task", subagent_type=DEVELOPER, prompt="Build it")])
    check_("guard-tool-call/prefixed_tool", building, [group("mcp__x__Write", file_path="{P}/src/app.ts"), group("xBash", command="rm -rf src"), group("TaskX", subagent_type=DEVELOPER), group("AgentTask", subagent_type=DEVELOPER)])
    spoken_aside = lowered_state.replace("off (from scope feature)", "relaxed (set by you)") + "- **Guards Off**: state-transition (set by you)\n"
    check_("guard-tool-call/stood_aside_speaks", speaking, [group("Bash", command="bun .claude/tools/aidlc-state.ts approve intent-capture")])
    check_("guard-tool-call/stood_aside_speaks_and_refused", {**created(spoken_aside), **binding()}, [group("Bash", command=transition)])
    check_("guard-tool-call/refusal_then_failure", {**building, "aidlc.settings.local.json": '{"flags": {}}\n'}, [group("Bash", command=transition)])

    for marker in ("STOP_HOOK_PROBE", "ROUTE_CHECK"):
        check_(f"guard-tool-call/observer_{marker.lower()}", building, [group(file_path="{P}/src/app.ts")], {marker: "1"})

    # The dispatcher: a hook it does not have, a name it refuses, the trace.
    check_("dispatch/unknown", created(), [["nope", "{}"]])
    check_("dispatch/unsafe_name", created(), [["../x", "{}"]])
    check_("dispatch/no_name", created(), [["", "{}"]])
    for name in ("continue-workflow",):
        check_(f"dispatch/{name.replace('-', '_')}_not_ported", created(), [[name, payload()]])


DEVIATIONS.update({
    r"hook/[a-z-]+/(directive_marker|unit_scope_stamp|inflight_ledger|authorization_registry)(_[a-z_]+)?":
        "a record of a later milestone beside the hook's own: the orchestrator's directive marker, a unit claim, "
        "the ledger of background subagents, the summary authorizations (M4/M5); upstream reads or clears it and "
        "goes on; focus refuses the hook by name while it is there",
    "hook/record-human-turn/entry_words":
        "a `/focus` line with words after the entry: upstream reads it with the orchestrator's own flag parser "
        "(the settings, the route, the person's words after them: M5) for the message record and the reply; "
        "focus refuses the turn by name (exit 1) while it does not read `/focus` lines typed in chat",
    r"hook/record-human-turn/typed_switch(_flag)?":
        "a typed guard switch (the plain words, or a `/focus` line with a setting flag): upstream applies it to "
        "the piece of work this chat has open through the config setter, records the message with the lines it "
        "said, and queues them for the person (the setter is the utility's config-change, not ported); focus "
        "refuses the turn by name (exit 1)",
    "hook/record-human-turn/override_phrase":
        "the plan approval break-glass line: upstream records the override request for the answer to pair with "
        "(plan approval, 4e); focus refuses the turn by name (exit 1)",
    r"hook/record-human-turn/(person_lines|plan_challenge|protected_question|open_review_reviewed)":
        "a record of a later slice beside the turn's own: the lines queued for the person, this chat's plan "
        "approval challenge, a protected question any chat asked (4e), a review request with no verdict at a stage "
        "whose class reviews (the receipts, M5); upstream carries, answers or notes it and goes on; focus refuses "
        "the turn by name while it is there",
    r"hook/(review-freeze|guard-tool-call)/review_rows(_[a-z_]+)?":
        "a write to the output document of an open reviewer-bearing stage whose ledger holds a review row: upstream "
        "weighs the review receipt scan (the attempt window, the source bindings, the Unit claims: the attestation, "
        "M5) and freezes the document when a fresh terminal review covers it, with a REVIEW_FREEZE_BLOCKED row and "
        "the recovery ask on stderr (exit 2), or passes the write when none does; focus refuses the write by name "
        "(exit 1) while it does not weigh receipts",
    "hook/session-start/root_blocks_unconfigured":
        "a distribution tree config never ran in: upstream adds the engine's part of the team's root files "
        "(.gitignore) at session start and tells the chat its include was re-pointed; focus refuses by name until "
        "the distribution's merge (M8) is ported",
    "hook/session-start/anchor":
        "FOCUS_SESSION_ANCHOR=1 opts into the commit-provenance sweep, which reads the attestation's receipts and "
        "writes SOURCE_COMMITTED rows (M5); upstream runs it after the session row; focus refuses by name after "
        "the same row",
    r"hook/[a-z-]+/delegated_worktree":
        "upstream resolves a delegated worktree to the main checkout's intent; focus refuses it by name (a hook "
        "stands outside on any other failure of the resolution, as upstream's does)",
    r"hook/rebuild/(relay|relay_dispatcher|relay_string_response)":
        "a literal call of the engine's orchestrator whose output names an error directive: upstream validates "
        "the directive and relays its message to the person through the harness's own channel (the directive is the "
        "orchestrator's, M5); focus refuses the hook by name, before validating",
    r"hook/[a-z-]+/(message_object|active_form_number|file_path_number|command_number|ide_command_number)":
        "a payload field of a kind the hook does not expect: upstream falls over on it and its dispatcher prints "
        "the runtime's TypeError; focus refuses the field by name (exit 1 either way, and nothing recorded past "
        "the heartbeat by either)",
    "hook/plan-approval-guard/tool_script":
        "one of the engine's installed tool scripts run by Bun (by name, or by the absolute path of the Bun that runs "
        "upstream's hook), doing what a waiting plan admits: upstream admits it as it admits the engine by name, "
        "and by the absolute path of its installed launcher or active executable. A Focus install has no tool "
        "scripts, its hook is not run by a Bun, and the install's layout is the distribution's (M8): focus admits "
        "the engine by name, and the command waits for the engine's step like any command it cannot see into",
    r"hook/dispatch/[a-z_]+_not_ported":
        "a hook Focus does not run yet: upstream runs it; focus refuses it by name",
    r"hook/dispatch/(unknown|unsafe_name|no_name)":
        "upstream's dispatcher words the refusal of a hook it cannot find its own way, and reaches the file system "
        "for it; focus refuses the name before looking",
})

def gate_checks():
    """`aidlc-state.ts gate-start`, `approve`, `reject` and `revise`: a stage's approval gate, what admits each
    decision (the person's presence, the artifacts, the sensors, the review) and the rows each writes."""
    check_ = lambda name, files, steps, env=None, edits=(): runtime_check(name, files, steps, env, edits, group="state")
    direct = {"ALLOW_DIRECT_STATE_TRANSITIONS": "1"}
    quiet = {**direct, "SKIP_HUMAN_PRESENCE_GUARD": "1"}
    ic = artifact("intent-capture", "intent-statement")
    awaiting = GATE_STATE.replace("- [-] intent-capture — EXECUTE", "- [?] intent-capture — EXECUTE")
    revising = GATE_STATE.replace("- [-] intent-capture — EXECUTE", "- [R] intent-capture — EXECUTE")
    opened = CREATED_AUDIT + block("STAGE_AWAITING_APPROVAL", T(1), "**Stage**: intent-capture", "**Next Stage**: market-research",
                                   "**Brief Digest**: none", "**Sensor State**: not-run", "**Open Decisions**: 0")
    approve = ["approve", "intent-capture", "--user-input", "Approve"]
    reject = ["reject", "intent-capture", "--feedback", "needs more"]

    # Usage, the slugs, the states each verb admits.
    check_("gates/usage", created(GATE_STATE, **ic), [["gate-start"], ["approve"], ["reject"], ["revise"]], quiet)
    check_("gates/unknown_stage", created(GATE_STATE, **ic), [["gate-start", "nope"], ["approve", "nope"], ["reject", "nope"], ["revise", "nope"]], quiet)
    check_("gates/wrong_states", created(GATE_STATE, **ic), [["gate-start", "market-research"], ["approve", "intent-capture", "--user-input", "Approve"],
                                                           reject, ["revise", "intent-capture"]], quiet)
    check_("gates/archived", created(GATE_STATE.replace("- **Status**: Running", "- **Status**: Archived"), **ic),
           [["gate-start", "intent-capture"], approve, reject, ["revise", "intent-capture"]], quiet)
    check_("gates/no_scope", created(awaiting.replace("- **Scope**: feature\n", ""), opened, **ic), [approve], quiet)
    # The whole gate: opened, shown again, approved; rejected, revised, approved; and what a replay writes once.
    check_("gates/open_and_approve", created(GATE_STATE, **ic), [["gate-start", "intent-capture"], ["resume"], ["gate-start", "intent-capture"],
                                                                 approve, ["resume"], approve, ["get", "Completed"]], quiet)
    check_("gates/reject_revise_approve", created(GATE_STATE, **ic), [["gate-start", "intent-capture"], reject, ["resume"], ["get", "Revision Count"],
                                                                      ["revise", "intent-capture"], ["resume"], ["reject", "intent-capture", "--reason", "again"],
                                                                      ["revise", "intent-capture"], approve, ["get", "Revision Count"]], quiet)
    check_("gates/revise_states", created(GATE_STATE, **ic), [["revise", "intent-capture"], ["gate-start", "intent-capture"], ["revise", "intent-capture"]], quiet)
    check_("gates/reject_in_progress", created(GATE_STATE, **ic), [reject, ["resume"]], quiet)
    check_("gates/approve_phase_boundary", created(at_stage(awaiting, "approval-handoff", done=["intent-capture", "market-research", "feasibility", "scope-definition", "team-formation", "rough-mockups"]).replace(
        "- [-] approval-handoff", "- [?] approval-handoff"), **artifact("approval-handoff", "initiative-brief")),
           [["approve", "approval-handoff", "--user-input", "Approve"], ["resume"]], quiet)
    check_("gates/approve_last_stage", created(at_stage(awaiting, "feedback-optimization", done=["intent-capture", "market-research", "feasibility", "scope-definition", "team-formation", "rough-mockups"], phase="OPERATION").replace(
        "- [-] feedback-optimization", "- [?] feedback-optimization"), **artifact("feedback-optimization", "slo-report", phase="operation")),
           [["approve", "feedback-optimization", "--user-input", "Approve"], ["resume"], ["get", "Status"]], quiet)
    # The person's input, read only with the presence guard on (a turn after the gate admits the decision):
    # the choices, a rejection by choice, none, a non-answer, one the agent attributed to itself.
    replied = opened + HUMAN(2)
    other = f"{CREATED_DIR}/audit/desktop-def456.md"
    check_("gates/approve_choices", created(awaiting, replied, **ic), [["approve", "intent-capture", "--user-input", "  Approve and continue  "]], direct)
    check_("gates/approve_reject_choice", created(awaiting, replied, **ic), [["approve", "intent-capture", "--user-input", "Request Changes"], ["resume"]], direct)
    check_("gates/approve_free_text", created(awaiting, replied, **ic), [["approve", "intent-capture", "--user-input", "yes, looks good to me"]], direct)
    check_("gates/approve_without_input", created(awaiting, replied, **ic), [["approve", "intent-capture"], ["approve", "intent-capture", "--user-input"],
                                                                            ["approve", "intent-capture", "--user-input", " "]], direct)
    inputs = ["...", "Cancelled", "timed out", "No answer.", "user dismissed", "cancel the standing order",
              "Approve (agent's default)", "Going with the conductor's default.", "Answer - conductor's default", "Approve (Recommended)",
              "Not a human decision", "Agent-initiated this approval.", "The model generated this choice", "decision was made by the assistant",
              "I, as the agent, am approving", "the ai chose this decision", "approved by the stop hook", "by hook trigger",
              "`conductor's default` is what they said", "> conductor's default\nApprove", "'conductor's default' was the reply",
              "the assistant rejected this", "Approve. Not the human's choice, but mine", "Approve \u2014 conductor default \u2014 fine",
              # Quoted examples are masked: in code, in quotes, in a block quote, in a fence; and the same words unquoted.
              "Approve `the agent chose this decision.`", "Approve. \"decision was made by the assistant.\" is a phrase I dislike",
              "> decision was made by the assistant.\nApprove", "```\ndecision was made by the assistant.\n```\nApprove",
              "Approve, the agent chose this decision.", "Approve: 'not a human decision' is what it said"]
    for index, input_ in enumerate(inputs):
        check_(f"gates/approve_input_{index}", created(awaiting, replied, **ic), [["approve", "intent-capture", "--user-input", input_]], direct)
    check_("gates/approve_accept_as_is", created(awaiting, replied, **ic), [["approve", "intent-capture", "--user-input", "Accept as-is"]], direct)
    check_("gates/approve_accept_as_is_after_three", created(awaiting.replace("- **Revision Count**: 0", "- **Revision Count**: 3"), replied, **ic),
           [["approve", "intent-capture", "--user-input", "Accept as-is"]], direct)
    check_("gates/approve_input_unchecked_when_presence_off", created(awaiting, opened, **ic), [["approve", "intent-capture", "--user-input", "Cancelled"]], quiet)
    check_("gates/approve_without_input_presence_off", created(awaiting, opened, **ic), [["approve", "intent-capture"]], quiet)
    # The gate shown on another clone, or backfilled: this shard holds no presentation to reply after.
    check_("gates/presence_gate_shown_elsewhere", created(awaiting, CREATED_AUDIT, **ic, **{other: TITLE + block("STAGE_AWAITING_APPROVAL", T(1), "**Stage**: intent-capture") + HUMAN(2)}), [approve], direct)
    check_("gates/presence_gate_recovered", created(awaiting, CREATED_AUDIT + block("STAGE_AWAITING_APPROVAL", T(1), "**Stage**: intent-capture", "**Recovered**: true") + HUMAN(2), **ic), [approve], direct)
    # The artifacts: none, the guard off, declared by flag, and a directory.
    check_("gates/start_no_artifacts", created(GATE_STATE), [["gate-start", "intent-capture"], ["resume"]], quiet)
    check_("gates/start_artifact_guard_off", created(GATE_STATE), [["gate-start", "intent-capture"], ["resume"]], {**quiet, "SKIP_ARTIFACT_GUARD": "1"})
    check_("gates/start_artifacts_flag", created(GATE_STATE, **ic), [["gate-start", "intent-capture", "--artifacts", "ideation/intent-capture/intent-statement.md,other.md"], ["resume"]], quiet)
    check_("gates/start_artifacts_flag_without_value", created(GATE_STATE, **ic), [["gate-start", "intent-capture", "--artifacts"]], quiet)
    check_("gates/start_artifacts_flag_empty", created(GATE_STATE, **ic), [["gate-start", "intent-capture", "--artifacts", ""]], quiet)
    check_("gates/start_recovered", created(GATE_STATE, **ic), [["gate-start", "intent-capture", "--recovered"], ["gate-start", "intent-capture", "--recovered", "--person-approves"]], quiet)
    # The protected questions kept for every chat are withdrawn when a gate opens.
    protected = {"aidlc/.aidlc-sessions/plan-approval/protected-question-s1.json": "{}\n", "aidlc/.aidlc-sessions/plan-approval/protected-question-response-s1.json": "{}\n",
                 "aidlc/.aidlc-sessions/plan-approval/receipt-1.json": "{}\n"}
    check_("gates/start_withdraws_protected_questions", created(GATE_STATE, **ic, **protected), [["gate-start", "intent-capture"]], quiet)
    check_("gates/start_withdraws_behind_link", created(GATE_STATE, **ic, **{"elsewhere/protected-question-s1.json": "{}\n"}), [["gate-start", "intent-capture"]], quiet,
           edits=(lambda tree: (os.makedirs(tree.path("aidlc/.aidlc-sessions")), os.symlink(tree.path("elsewhere"), tree.path("aidlc/.aidlc-sessions/plan-approval"))),))
    check_("gates/approve_no_artifacts", created(awaiting, opened), [approve, ["resume"]], quiet)
    check_("gates/approve_artifact_guard_off", created(awaiting, opened), [approve, ["resume"]], {**quiet, "SKIP_ARTIFACT_GUARD": "1"})
    # Approves together: a flag whose stages come from the plan (not ported), and its field on the rows.
    check_("gates/approves_together", created(GATE_STATE, **ic), [["gate-start", "intent-capture", "--approves-together", "market-research"],
                                                                 ["gate-start", "intent-capture", "--approves-together", ""],
                                                                 ["gate-start", "intent-capture", "--approves-together"]], quiet)
    listed = CREATED_AUDIT + block("STAGE_AWAITING_APPROVAL", T(1), "**Stage**: intent-capture", "**Approves Together**: market-research",
                                   "**Next Stage**: market-research", "**Brief Digest**: none", "**Sensor State**: not-run", "**Open Decisions**: 0")
    check_("gates/approve_approved_together", created(awaiting, listed + HUMAN(2), **ic), [approve], direct)
    check_("gates/approve_approved_together_presence_off", created(awaiting, listed, **ic), [approve], quiet)
    covered = CREATED_AUDIT + block("STAGE_AWAITING_APPROVAL", T(1), "**Stage**: market-research", "**Approves Together**: intent-capture") + HUMAN(2) + block(
        "GATE_APPROVED", T(3), "**Stage**: market-research", "**User Input**: Approve", "**Approves Together**: intent-capture")
    check_("gates/approve_covered_by_another", created(awaiting, covered, **ic), [["approve", "intent-capture"]], direct)
    # Sensors: the ceremony on (the sensor tool, not ported), off by switch, and the rows an earlier run left.
    check_("gates/sensors_on", created(**ic), [["gate-start", "intent-capture"], ["resume"]], quiet)
    check_("gates/sensors_on_no_artifacts", created(), [["gate-start", "intent-capture"]], {**quiet, "SKIP_ARTIFACT_GUARD": "1"})
    check_("gates/sensors_off_by_switch", created(**ic), [["gate-start", "intent-capture"], ["resume"]], {**quiet, "DISABLE_SENSORS": "1"})
    check_("gates/sensors_override", created(GATE_STATE, **ic), [["gate-start", "intent-capture", "--override-blocking-sensors"], ["revise", "intent-capture", "--override-blocking-sensors", "--user-input", "x"]], quiet)
    output = f"{CREATED_DIR}/ideation/intent-capture/intent-statement.md"
    ended = lambda kind, minute, second, sensor, *fields: block(f"SENSOR_{kind}", T(minute, second), "**Stage slug**: intent-capture", f"**Sensor ID**: {sensor}",
                                                                 f"**Output path**: {output}", *fields)
    rows = (ended("PASSED", 1, 2, "claim-sources", "**Findings count**: 0") + ended("FAILED", 1, 4, "required-sections", "**Findings count**: 2", "**Detail path**: x/y.md") +
            ended("FAILED", 1, 5, "upstream-coverage") + ended("PASSED", 1, 5, "upstream-coverage", "**Findings count**: 0"))
    check_("gates/sensor_rows", created(GATE_STATE, CREATED_AUDIT + rows, **ic), [["gate-start", "intent-capture"]], quiet)
    check_("gates/sensor_rows_latest_wins", created(GATE_STATE, CREATED_AUDIT + ended("FAILED", 1, 9, "claim-sources", "**Findings count**: 1") + ended("PASSED", 1, 2, "claim-sources", "**Findings count**: 0"), **ic),
           [["gate-start", "intent-capture"]], quiet)
    check_("gates/sensor_rows_other_stage", created(GATE_STATE, CREATED_AUDIT + block("SENSOR_FAILED", T(1), "**Stage slug**: market-research", "**Sensor ID**: claim-sources", f"**Output path**: {output}", "**Findings count**: 3") +
                                                    block("SENSOR_FAILED", T(1, 1), "**Stage slug**: intent-capture", "**Sensor ID**: claim-sources", "**Findings count**: 3"), **ic),
           [["gate-start", "intent-capture"]], quiet)
    check_("gates/sensor_rows_not_run", created(GATE_STATE, CREATED_AUDIT + block("SENSOR_FIRED", T(1), "**Stage**: intent-capture", "**Sensor**: claim-sources", "**Fire id**: f1"), **ic), [["gate-start", "intent-capture"]], quiet)
    # The gate's brief, read from the review store when one is there.
    briefs = f"{GATE_DIR}/reviews/intent-capture/stage/briefs/latest.json"
    check_("gates/brief_digest", created(GATE_STATE, **ic, **{briefs: json.dumps({"digest": "ab" * 32, "stage": "intent-capture"})}), [["gate-start", "intent-capture"], approve], quiet)
    check_("gates/brief_digest_not_hex", created(GATE_STATE, **ic, **{briefs: json.dumps({"digest": "abc123"})}), [["gate-start", "intent-capture"]], quiet)
    check_("gates/brief_digest_other_stage", created(GATE_STATE, **ic, **{briefs.replace("intent-capture", "market-research"): json.dumps({"digest": "ab" * 32})}), [["gate-start", "intent-capture"]], quiet)
    check_("gates/brief_digest_malformed", created(GATE_STATE, **ic, **{briefs: "{nope"}), [["gate-start", "intent-capture"]], quiet)
    check_("gates/brief_digest_not_text", created(GATE_STATE, **ic, **{briefs: json.dumps({"digest": 12})}), [["gate-start", "intent-capture"]], quiet)
    # Open decisions in the artifact: a decisions block, one that cannot be read, and duplicates.
    decisions = "# Intent\n\n```aidlc-decisions\ndecisions:\n  - id: D1\n    decision: Which store?\n    owner: person\n  - id: D2\n    decision: When?\n    blocking: no\n```\n"
    check_("gates/open_decisions", created(GATE_STATE, **artifact("intent-capture", "intent-statement", decisions)), [["gate-start", "intent-capture"], approve], quiet)
    check_("gates/open_decisions_unreadable", created(GATE_STATE, **artifact("intent-capture", "intent-statement", "```aidlc-decisions\ndecisions:\n  - id: D1\n    question: a\n```\n")), [["gate-start", "intent-capture"]], quiet)
    check_("gates/open_decisions_no_header", created(GATE_STATE, **artifact("intent-capture", "intent-statement", "```aidlc-decisions\n  - id: D1\n```\n")), [["gate-start", "intent-capture"]], quiet)
    check_("gates/open_decisions_empty", created(GATE_STATE, **artifact("intent-capture", "intent-statement", "```aidlc-decisions\ndecisions:\n```\n")), [["gate-start", "intent-capture"]], quiet)
    check_("gates/open_decisions_duplicate", created(GATE_STATE, **artifact("intent-capture", "intent-statement", "```aidlc-decisions\ndecisions:\n  - id: D1\n  - id: D1\n```\n\n```aidlc-decisions\ndecisions:\n  - id: D2\r\n```\n")), [["gate-start", "intent-capture"], approve], quiet)
    check_("gates/open_decisions_two_blocks_one_unreadable", created(GATE_STATE, **artifact("intent-capture", "intent-statement", "```aidlc-decisions\ndecisions:\n  - id: D1\n```\n```aidlc-decisions\nnope\n```\n")), [["gate-start", "intent-capture"]], quiet)
    # The evidence a decision owes to aidlc-log records (M5): refused by name where upstream weighs it.
    check_("gates/summary_owed", created(GATE_STATE.replace("- **Summary Confirmation**: off (set by a command)", "- **Summary Confirmation**: on (from scope feature)"), **ic),
           [["gate-start", "intent-capture"]], quiet)
    check_("gates/review_owed", created(GATE_STATE.replace("- **Review Override**: none", "- **Review Override**: "), **ic), [["gate-start", "intent-capture"]], quiet)
    check_("gates/review_guard_off", created(GATE_STATE.replace("- **Review Override**: none", "- **Review Override**: "), **ic), [["gate-start", "intent-capture"], approve], {**quiet, "SKIP_REVIEWER_GATE_GUARD": "1"})
    check_("gates/review_findings_rows", created(awaiting, opened + block("REVIEW_FINDING", T(2), "**Stage**: intent-capture", "**Finding**: R-01", "**Status**: open"), **ic), [approve], quiet)
    # The review artifact's `## Review` section: findings (not read), a heading inside a fence, an empty section.
    check_("gates/review_findings_section", created(awaiting, opened, **artifact("intent-capture", "intent-statement", "# Intent\n\n## Review\n- R-01 open: the title\n")), [approve], quiet)
    check_("gates/review_section_fenced", created(awaiting, opened, **artifact("intent-capture", "intent-statement", "# Intent\n\n```\n## Review\n- R-01\n```\n")), [approve], quiet)
    check_("gates/review_section_empty", created(awaiting, opened, **artifact("intent-capture", "intent-statement", "# Intent\n\n## Review\n\n## After\ntext\n")), [approve], quiet)
    check_("gates/reject_findings", created(awaiting, opened, **ic), [["reject", "intent-capture", "--feedback", "x", "--reject-finding", "intent-review#R-01=no"],
                                                                     ["reject", "intent-capture", "--feedback", "x", "--reopen-finding", "intent-review#R-01=yes"]], quiet)
    check_("gates/reject_findings_with_reviewer", created(awaiting.replace("- **Review Override**: none", "- **Review Override**: "), opened, **ic),
           [["reject", "intent-capture", "--feedback", "x", "--reject-finding", "intent-review#R-01=no"]], quiet)
    # Feedback: its forms and what is refused.
    check_("gates/reject_feedback_forms", created(awaiting, opened, **ic), [["reject", "intent-capture", "--feedback=--x"], ["reject", "intent-capture", "--feedback", "a", "--reason", "b"],
                                                                           ["reject", "intent-capture", "--feedback"], ["reject", "intent-capture", "--feedback", "--reason"],
                                                                           ["reject", "intent-capture"], ["reject", "intent-capture", "--feedback", "  "]], quiet)
    feedbacks = ["...", "Cancelled", "Abort!", "Reject (agent's pick)", "The assistant rejected this", "Changes \u2014 conductor's default",
                 "I, as the conductor, have requested changes", "This rejection was entered by an AI.", "model-initiated the change-request",
                 "Not the human's request changes", "more detail, please", "rejection confirmed by hook"]
    for index, feedback in enumerate(feedbacks):
        check_(f"gates/reject_feedback_{index}", created(awaiting, replied, **ic), [["reject", "intent-capture", "--feedback", feedback]], direct)
    check_("gates/reject_self_attributed_unchecked_when_presence_off", created(awaiting, opened, **ic), [["reject", "intent-capture", "--feedback", "Reject (agent's pick)"]], quiet)
    mr_awaiting = at_stage(GATE_STATE, "market-research", done=["intent-capture"]).replace("- [-] market-research", "- [?] market-research")
    check_("gates/reject_findings_no_reviewer", created(mr_awaiting, **artifact("market-research", "competitive-analysis")),
           [["reject", "market-research", "--feedback", "x", "--reject-finding", "R-01=no"]], quiet)
    check_("gates/reject_revision_count_nan", created(awaiting.replace("- **Revision Count**: 0", "- **Revision Count**: many"), opened, **ic), [reject, ["get", "Revision Count"]], quiet)
    check_("gates/reject_revision_count_huge", created(awaiting.replace("- **Revision Count**: 0", "- **Revision Count**: " + "9" * 400), opened, **ic), [reject, ["get", "Revision Count"]], quiet)
    check_("gates/review_rows_no_reviewer", created(mr_awaiting, CREATED_AUDIT + block("REVIEW_COMPLETED", T(2), "**Stage**: market-research", "**Verdict**: READY"), **artifact("market-research", "competitive-analysis")),
           [["approve", "market-research", "--user-input", "Approve"]], quiet)
    check_("gates/reject_revision_count_missing", created(awaiting.replace("- **Revision Count**: 0\n", ""), opened, **ic), [reject, ["get", "Revision Count"]], quiet)
    # The person's presence: the guard on, with turns before and after the gate, a command turn, and unattended.
    check_("gates/presence_no_turn", created(awaiting, opened, **ic), [approve, reject], direct)
    check_("gates/presence_turn_after_gate", created(awaiting, opened + HUMAN(2), **ic), [approve, ["resume"]], direct)
    check_("gates/presence_turn_before_gate", created(awaiting, CREATED_AUDIT + HUMAN(0, 30) + block("STAGE_AWAITING_APPROVAL", T(1), "**Stage**: intent-capture", "**Next Stage**: market-research",
                                                                                                        "**Brief Digest**: none", "**Sensor State**: not-run", "**Open Decisions**: 0"), **ic), [approve], direct)
    check_("gates/presence_turn_consumed", created(awaiting, opened + HUMAN(2) + block("GATE_REJECTED", T(3), "**Stage**: intent-capture", "**Feedback**: x") + block("STAGE_AWAITING_APPROVAL", T(4), "**Stage**: intent-capture"), **ic), [approve], direct)
    check_("gates/presence_command_turn", created(awaiting, opened + HUMAN(2, 0, "**Reply**: command"), **ic), [approve], direct)
    check_("gates/presence_question_turn", created(awaiting, opened + HUMAN(2, 0, "**Reply**: question"), **ic), [approve], direct)
    check_("gates/presence_reject", created(awaiting, opened + HUMAN(2), **ic), [reject, ["revise", "intent-capture"], approve], direct)
    check_("gates/presence_unattended", created(awaiting, opened, **ic), [approve], {**direct, "UNATTENDED": "1"})
    check_("gates/presence_own_terminal", created(awaiting, opened, **ic), [approve], {**direct, "TEST_CONFIG_TTY": "1"})
    check_("gates/presence_hooks_heartbeat", created(awaiting, opened, **ic, **{f"{GATE_DIR}/hooks-health/record-human-turn.last": STARTED + "\n"}), [approve], direct)
    # A heartbeat is recent against the clock: the replay runs later.
    UNRECORDED.add("state/gates/presence_hooks_heartbeat")
    check_("gates/presence_hooks_heartbeat_old", created(awaiting, opened, **ic, **{f"{GATE_DIR}/hooks-health/record-human-turn.last": T(0) + "\n"}), [approve], direct)
    check_("gates/presence_hooks_off_step", created(awaiting, opened, **ic, **{".claude/settings.local.json": "{}\n"}), [approve], direct)
    check_("gates/presence_recovered_person_approves", created(GATE_STATE, CREATED_AUDIT + HUMAN(1), **ic), [["gate-start", "intent-capture", "--recovered", "--person-approves"]], direct)
    check_("gates/presence_turn_on_other_shard", created(awaiting, opened, **ic, **{other: TITLE + HUMAN(2)}), [approve], direct)
    check_("gates/presence_tie_across_shards", created(awaiting, opened + HUMAN(2), **ic, **{other: TITLE + block("GATE_REJECTED", T(2), "**Stage**: intent-capture")}), [approve], direct)
    check_("gates/presence_turn_used_by_answer", created(awaiting, opened + HUMAN(2) + block("QUESTION_ANSWERED", T(3), "**Stage**: intent-capture", "**Details**: Chat"), **ic), [approve], direct)
    check_("gates/presence_turn_used_by_summary", created(awaiting, opened + HUMAN(2) + block("SUMMARY_CONFIRMATION_RECORDED", T(3), "**Stage**: intent-capture"), **ic), [approve], direct)
    check_("gates/presence_two_turns_same_second", created(awaiting, opened + HUMAN(2) + HUMAN(2), **ic), [approve], direct)
    check_("gates/presence_turn_no_session", created(awaiting, opened + block("HUMAN_TURN", T(2)), **ic), [approve], direct)
    check_("gates/presence_shard_unreadable", created(awaiting, opened + HUMAN(2), **ic, **{other + "/x": ""}), [approve], direct)
    check_("gates/presence_reject_unattended", created(awaiting, opened, **ic), [reject], {**direct, "UNATTENDED": "1"})
    check_("gates/presence_revise_needs_no_turn", created(revising, opened + block("GATE_REJECTED", T(2), "**Stage**: intent-capture", "**Feedback**: x"), **ic), [["revise", "intent-capture"]], direct)
    check_("gates/start_user_input", created(GATE_STATE, **ic), [["gate-start", "intent-capture", "--user-input", "Approve"], ["revise", "intent-capture", "--user-input", "x", "--approves-together", "a"]], quiet)
    # The words the human-turn hook kept for the chat: the person's exact pick decides, their words ride on the row.
    words = lambda *texts, session="s1", shard=CREATED_SHARD, dropped=0, offset=100000: {f"{GATE_DIR}/gate-words/{session}.json": json.dumps(
        {"version": 1, "session": "s1", "shard": shard, "dropped": dropped, "messages": [{"offset": offset + n, "text": text} for n, text in enumerate(texts)]})}
    # The presence guard stays on: the pick is read only where a person answers the gate.
    spoken = {**direct, "SESSION_OVERRIDE": "s1"}
    gate_turns = opened + HUMAN(2) + HUMAN(3)
    check_("gates/gate_words_malformed", created(awaiting, opened, **ic, **{f"{GATE_DIR}/gate-words/s1.json": json.dumps({"stage": "intent-capture", "words": "approve"})}), [approve], spoken)
    check_("gates/gate_words_approve", created(awaiting, gate_turns, **ic, **words("looks good", "Approve")), [approve], spoken)
    check_("gates/gate_words_approve_pick", created(awaiting, gate_turns, **ic, **words("1")), [["approve", "intent-capture", "--user-input", "yes"]], spoken)
    check_("gates/gate_words_approve_picks_changes", created(awaiting, gate_turns, **ic, **words("looks good", "2")), [approve], spoken)
    check_("gates/gate_words_approve_picks_changes_text", created(awaiting, gate_turns, **ic, **words("Request Changes (Recommended)")), [approve], spoken)
    check_("gates/gate_words_approve_cancelled", created(awaiting, gate_turns, **ic, **words("Cancelled")), [approve], spoken)
    check_("gates/gate_words_approve_cancelled_then", created(awaiting, gate_turns, **ic, **words("Approve", "Cancelled")), [approve], spoken)
    check_("gates/gate_words_reject", created(awaiting, gate_turns, **ic, **words("Request Changes", "the login flow must use oauth")), [["reject", "intent-capture", "--feedback", "use oauth"]], spoken)
    check_("gates/gate_words_reject_same", created(awaiting, gate_turns, **ic, **words("the  login flow\nmust use oauth")), [["reject", "intent-capture", "--feedback", "the login flow must use oauth "]], spoken)
    check_("gates/gate_words_reject_pick_only", created(awaiting, gate_turns, **ic, **words("2")), [["reject", "intent-capture", "--feedback", "needs more"]], spoken)
    check_("gates/gate_words_reject_pick_only_no_feedback", created(awaiting, gate_turns, **ic, **words("Request Changes")), [["reject", "intent-capture"]], spoken)
    check_("gates/gate_words_reject_no_feedback", created(awaiting, gate_turns, **ic, **words("use oauth")), [["reject", "intent-capture"]], spoken)
    check_("gates/gate_words_reject_picks_approve", created(awaiting, gate_turns, **ic, **words("fine", "Approve")), [reject], spoken)
    check_("gates/gate_words_reject_picks_accept", created(awaiting.replace("- **Revision Count**: 0", "- **Revision Count**: 3"), gate_turns, **ic, **words("Accept as-is")), [reject], spoken)
    check_("gates/gate_words_reject_accept_not_offered", created(awaiting, gate_turns, **ic, **words("Accept as-is")), [reject], spoken)
    check_("gates/gate_words_dropped", created(awaiting, gate_turns, **ic, **words("2", dropped=100000)), [approve], spoken)
    check_("gates/gate_words_dropped_before", created(awaiting, gate_turns, **ic, **words("2", dropped=1)), [approve], spoken)
    check_("gates/gate_words_before_presentation", created(awaiting, gate_turns, **ic, **words("2", offset=1)), [approve], spoken)
    check_("gates/gate_words_other_shard", created(awaiting, gate_turns, **ic, **words("2", shard="other/audit/x.md")), [approve], spoken)
    check_("gates/gate_words_other_session", created(awaiting, gate_turns, **ic, **words("2", session="s2")), [approve], spoken)
    check_("gates/gate_words_no_session", created(awaiting, gate_turns, **ic, **words("2")), [approve], quiet)
    check_("gates/gate_words_after_decision", created(awaiting, gate_turns + block("GATE_APPROVED", T(4), "**Stage**: intent-capture", "**User Input**: Approve"), **ic, **words("2")), [approve], spoken)
    check_("gates/gate_words_answered_question", created(awaiting, opened + HUMAN(2) + block("QUESTION_ANSWERED", T(3), "**Stage**: intent-capture") + HUMAN(4), **ic, **words("2", "Approve")), [approve], spoken)
    check_("gates/gate_words_other_stage", created(awaiting, gate_turns, **ic, **words("2")), [["approve", "market-research", "--user-input", "Approve"]], spoken)
    check_("gates/gate_words_bad_record", created(awaiting, gate_turns, **ic, **{f"{GATE_DIR}/gate-words/s1.json": json.dumps({"version": 2, "session": "s1", "shard": CREATED_SHARD, "dropped": 0, "messages": [{"offset": 100000, "text": "2"}]})}), [approve], spoken)
    check_("gates/gate_words_bad_offset", created(awaiting, gate_turns, **ic, **{f"{GATE_DIR}/gate-words/s1.json": json.dumps({"version": 1, "session": "s1", "shard": CREATED_SHARD, "dropped": 0, "messages": [{"offset": 1.5, "text": "2"}]})}), [approve], spoken)
    # Accept as-is is their pick only once the revision cap offers it; the pick names the approval whatever the conductor typed.
    capped = awaiting.replace("- **Revision Count**: 0", "- **Revision Count**: 3")
    check_("gates/gate_words_approve_accept_pick", created(capped, gate_turns, **ic, **words("3")), [["approve", "intent-capture", "--user-input", "yes"]], spoken)
    check_("gates/gate_words_approve_accept_not_offered", created(awaiting, gate_turns, **ic, **words("Accept as-is")), [approve], spoken)
    # A gate shown again after a revision counts from its latest showing: words typed before it are not at this gate.
    rejected = opened + HUMAN(2) + block("GATE_REJECTED", T(3), "**Stage**: intent-capture", "**Feedback**: x") + block("STAGE_REVISING", T(3), "**Stage**: intent-capture", "**Revision count**: 1", "**Feedback**: x")
    reshown = block("STAGE_AWAITING_APPROVAL", T(4), "**Stage**: intent-capture", "**Next Stage**: market-research", "**Brief Digest**: none", "**Sensor State**: not-run", "**Open Decisions**: 0")
    before_reshown = len(rejected.encode("utf-8")) - 2  # under both headers (gate-words-offset): after the first showing, before the second
    check_("gates/gate_words_reentered", created(awaiting.replace("- **Revision Count**: 0", "- **Revision Count**: 1"), rejected + reshown + HUMAN(5), **ic, **words("2", offset=before_reshown)), [approve], spoken)
    check_("gates/gate_words_reentered_after", created(awaiting.replace("- **Revision Count**: 0", "- **Revision Count**: 1"), rejected + reshown + HUMAN(5), **ic, **words("2")), [approve], spoken)
    # A stage approved under another's cover carries that gate's words, not words typed at its own earlier showing.
    covered_shown = CREATED_AUDIT + block("STAGE_AWAITING_APPROVAL", T(1), "**Stage**: intent-capture") + block("STAGE_AWAITING_APPROVAL", T(1), "**Stage**: market-research", "**Approves Together**: intent-capture") + HUMAN(2) + block(
        "GATE_APPROVED", T(3), "**Stage**: market-research", "**User Input**: Approve", "**Approves Together**: intent-capture")
    check_("gates/gate_words_covered", created(awaiting, covered_shown, **ic, **words("2")), [["approve", "intent-capture"]], spoken)
    check_("gates/gate_words_covered_words", created(awaiting, covered_shown, **ic, **words("fine by me")), [approve], spoken)
    # A revision the backstop records: the artifact changed after the gate opened without a decision.
    backstop = CREATED_AUDIT + block("STAGE_AWAITING_APPROVAL", T(1), "**Stage**: intent-capture") + HUMAN(2) + block("ARTIFACT_UPDATED", T(3), "**Stage**: intent-capture", "**Artifact**: intent-statement") + HUMAN(4)
    check_("gates/revision_backstop", created(awaiting, backstop, **ic), [approve, ["get", "Revision Count"]], quiet)
    # Under a strict policy the backstop runs: nothing to record here, with and without member repositories; and a
    # row whose `repos` is not a list of text, which the backstop only looks names up in.
    strict = awaiting.replace("- **Guard Policy**: off (from scope feature)", "- **Guard Policy**: strict (set by you)")
    check_("gates/revision_backstop_strict", created(strict, backstop, **ic), [approve, ["get", "Revision Count"]], quiet)
    check_("gates/revision_backstop_strict_repos", created(strict, backstop, registry=CREATED_REGISTRY.replace('"status": "in-flight"', '"status": "in-flight",\n    "repos": ["app"]'), **ic), [approve, ["get", "Revision Count"]], quiet)
    for label, listed in NOT_NAMES:
        check_(f"gates/revision_backstop_repos_{label}", created(strict, backstop, registry=CREATED_REGISTRY.replace('"status": "in-flight"', '"status": "in-flight",\n    "repos": ' + listed), **ic), [approve, ["get", "Revision Count"]], quiet)
    check_("gates/revision_backstop_off", created(awaiting, backstop, **ic), [approve], {**quiet, "SKIP_REVISION_BACKSTOP": "1"})
    check_("gates/revision_backstop_relaxed", created(awaiting.replace("- **Guard Policy**: off (from scope feature)", "- **Guard Policy**: relaxed (set by you)"), backstop, **ic), [approve], quiet)
    check_("gates/revision_backstop_with_reviewer", created(awaiting.replace("- **Review Override**: none", "- **Review Override**: "), backstop, **ic), [approve], {**quiet, "SKIP_REVIEWER_GATE_GUARD": "1"})
    # Units: a team gate needs the units' lifecycle (M4).
    check_("gates/unit_without_team", created(GATE_STATE, **ic), [["gate-start", "intent-capture", "--unit", "api"], ["approve", "intent-capture", "--unit", "api", "--user-input", "Approve"]], quiet)
    team = at_stage(GATE_STATE, "functional-design", done=["intent-capture", "market-research", "feasibility", "scope-definition", "team-formation", "rough-mockups", "approval-handoff"], phase="CONSTRUCTION") + "- **Unit Ownership**: team\n"
    check_("gates/unit_team", created(team, **{f"{CREATED_DIR}/construction/api/functional-design/entities.md": "# E\n"}),
           [["gate-start", "functional-design", "--unit", "api"], ["gate-start", "intent-capture", "--unit", "api"]], quiet)
    # A unit's gate is asked of a stage that carries the per-unit marker: one of the five stages that always run
    # per unit, in a compiled graph that lost its marker, is told it is not one.
    check_("gates/unit_team_no_marker", created(team, **{f"{CREATED_DIR}/construction/api/functional-design/entities.md": "# E\n"}),
           [["gate-start", "functional-design", "--unit", "api"], ["approve", "functional-design", "--unit", "api", "--user-input", "Approve"]], quiet,
           edits=[lambda tree: no_marker(tree, ["functional-design"])])
    # Usage ledger, the directive marker, the fence, a missing state file.
    usage = {"aidlc/.aidlc-sessions/usage-ledger.json": json.dumps({"schemaVersion": 3, "cursors": {}, "workflows": {"w": {
        "totals": {"tokens": {"input": 10, "output": 5, "cacheRead": 0, "cacheCreate5m": 0, "cacheCreate1h": 0}}, "byStage": {"intent-capture": {}}}}})}
    check_("gates/approve_usage_ledger", created(awaiting, opened, **ic, **usage), [approve], quiet)
    check_("gates/reject_active_directive", created(awaiting, opened, **ic, **{f"{GATE_DIR}/active-directive.json": json.dumps({"instruction": "x"})}), [reject], quiet)
    check_("gates/fence", created(GATE_STATE, **ic), [["gate-start", "intent-capture"], approve, reject, ["revise", "intent-capture"]], {"SKIP_HUMAN_PRESENCE_GUARD": "1"})
    check_("gates/no_state_file", created(**{f"{CREATED_DIR}/aidlc-state.md": None}), [["gate-start", "intent-capture"], approve], quiet)
    for marker in ("STOP_HOOK_PROBE", "ROUTE_CHECK"):
        check_(f"observer/{marker.lower()}_gates", created(GATE_STATE, **ic), [["gate-start", "intent-capture"], ["resume"]], {marker: "1", **quiet})


def park_checks():
    """`aidlc-state.ts park` and `unpark`: where the work stopped, whose stop it was, and taking it up again."""
    check_ = lambda name, files, steps, env=None, edits=(): runtime_check(name, files, steps, env, edits, group="state")
    direct = {"ALLOW_DIRECT_STATE_TRANSITIONS": "1"}
    parked = ["park"]
    reads = [["get", "Parked"], ["get", "Parked At Stage"], ["get", "Parked By"]]
    check_("park/plain", created(), [parked] + reads + [["resume"], ["unpark"], ["get", "Parked"], ["unpark"]], direct)
    check_("park/twice", created(), [parked, parked] + reads, direct)
    check_("park/completed", created(CREATED_STATE.replace("- **Status**: Running", "- **Status**: Completed")), [parked, ["unpark"]], direct)
    check_("park/archived", created(CREATED_STATE.replace("- **Status**: Running", "- **Status**: Archived")), [parked, ["unpark"]], direct)
    check_("park/no_current_stage", created(CREATED_STATE.replace("- **Current Stage**: intent-capture\n", "- **Current Stage**: \n")), [parked], direct)
    check_("park/no_runtime_heading", created(CREATED_STATE.replace("## Runtime State\n", "## Runtime\n")), [parked] + reads, direct)
    check_("park/fence", created(), [parked, ["unpark"]])
    # Whose stop: a turn on record and nothing after it, one an approval used, one the run's approvals used up.
    spoke = CREATED_AUDIT + HUMAN(1)
    check_("park/person_spoke", created(audit=spoke), [parked] + reads, direct)
    check_("park/person_spoke_unattended", created(audit=spoke), [parked] + reads, {**direct, "UNATTENDED": "1"})
    check_("park/person_spoke_command", created(audit=CREATED_AUDIT + HUMAN(1, 0, "**Reply**: command")), [parked] + reads, direct)
    check_("park/person_spoke_question", created(audit=CREATED_AUDIT + HUMAN(1, 0, "**Reply**: question")), [parked] + reads, direct)
    approved = lambda minute, *fields: block("GATE_APPROVED", T(minute), "**Stage**: intent-capture", *fields)
    check_("park/turn_outlives_one_approval", created(audit=spoke + approved(2, "**User Input**: Approve")), [parked] + reads, direct)
    check_("park/turn_used_by_two_approvals", created(audit=spoke + approved(2, "**User Input**: Approve") + approved(3, "**User Input**: Approve")), [parked] + reads, direct)
    check_("park/turn_outlives_autonomous_approvals", created(audit=spoke + approved(2, "**User Input**: Approve", "**Autonomous**: true") + approved(3) +
                                                              approved(4, "**User Input**: Approve", "**Approved Together With**: market-research")), [parked] + reads, direct)
    check_("park/turn_outlives_plan_approval", created(audit=spoke + block("PLAN_APPROVAL_RECORDED", T(2), "**Plan**: x")), [parked] + reads, direct)
    check_("park/turn_used_by_answer", created(audit=spoke + block("QUESTION_ANSWERED", T(2), "**Stage**: intent-capture", "**Details**: Chat")), [parked] + reads, direct)
    check_("park/turn_used_by_autonomy", created(audit=spoke + block("AUTONOMY_MODE_SET", T(2), "**Mode**: autonomous")), [parked] + reads, direct)
    check_("park/turn_then_plan_and_approval", created(audit=spoke + block("PLAN_APPROVAL_RECORDED", T(2), "**Plan**: x") + approved(3, "**User Input**: Approve")), [parked] + reads, direct)
    check_("park/latest_turn_tied_across_shards", created(audit=spoke + approved(2, "**User Input**: Approve"), **{f"{CREATED_DIR}/audit/desktop-def456.md": TITLE + HUMAN(1)}), [parked] + reads, direct)
    check_("park/turn_on_other_shard", created(**{f"{CREATED_DIR}/audit/desktop-def456.md": TITLE + HUMAN(1)}), [parked] + reads, direct)
    check_("park/no_turn_at_all", created(), [parked] + reads, direct)
    check_("park/question_turn_then_run_approval", created(audit=CREATED_AUDIT + HUMAN(1, 0, "**Reply**: question") + approved(2)), [parked] + reads, direct)
    check_("park/question_turn_used_by_approval", created(audit=CREATED_AUDIT + HUMAN(1, 0, "**Reply**: question") + approved(2, "**User Input**: Approve")), [parked] + reads, direct)
    check_("park/command_turn_used_by_approval", created(audit=CREATED_AUDIT + HUMAN(1, 0, "**Reply**: command") + approved(2, "**User Input**: Approve")), [parked] + reads, direct)
    check_("park/reparked_unattended", created(CREATED_STATE.replace("- **Construction Execution**: serial\n", "- **Construction Execution**: serial\n- **Parked By**: person\n")),
           [parked] + reads, {**direct, "UNATTENDED": "1"})
    # The autonomous grant: the person's stop wins over it, an unattended run never parks itself.
    autonomous = CREATED_STATE.replace("- **Construction Execution**: serial\n", "- **Construction Execution**: serial\n- **Construction Autonomy Mode**: autonomous\n")
    check_("park/autonomous_no_reply", created(autonomous), [parked], direct)
    check_("park/autonomous_replied", created(autonomous, spoke), [parked] + reads, direct)
    check_("park/autonomous_unattended", created(autonomous, spoke), [parked], {**direct, "UNATTENDED": "1"})
    check_("park/team_units", created(CREATED_STATE + "- **Unit Ownership**: team\n"), [parked, ["unpark"]], direct)
    # unpark: the fields gone and the row written once.
    with_fields = CREATED_STATE.replace("- **Construction Execution**: serial\n", "- **Construction Execution**: serial\n- **Parked**: 2026-01-05T10:05:00Z\n- **Parked At Stage**: intent-capture\n- **Parked By**: person\n")
    check_("unpark/parked", created(with_fields), [["unpark"], ["get", "Parked"], ["unpark"]], direct)
    check_("unpark/parked_empty", created(with_fields.replace("- **Parked**: 2026-01-05T10:05:00Z", "- **Parked**:  ")), [["unpark"]], direct)
    check_("unpark/usage_ignored", created(with_fields), [["unpark", "--x", "y"]], direct)
    for marker in ("STOP_HOOK_PROBE", "ROUTE_CHECK"):
        check_(f"observer/{marker.lower()}_park", created(), [parked, ["unpark"]], {marker: "1", **direct})


def log_checks():
    """`aidlc-log.ts decision`: a question the agent asks, what is refused before it is recorded, and the row."""
    check_ = lambda name, files, steps, env=None, edits=(): runtime_check(name, files, steps, env, edits, group="log")
    ask = ["decision", "--stage", "intent-capture", "--decision", "Which database?"]
    owing = CREATED_STATE.replace("- **Summary Confirmation**: off (set by a command)", "- **Summary Confirmation**: on (from scope feature)")

    # The command: no verb, one it does not have, the verbs Focus has not ported, and no piece of work to record in.
    check_("usage/none", created(), [[]])
    check_("usage/unknown", created(), [["nope"], ["Decision", "--stage", "x"]])
    check_("usage/no_workflow", CLONE, [ask, ["nope"]])
    check_("usage/no_state_file", created(**{f"{CREATED_DIR}/aidlc-state.md": None}), [ask])
    for verb in ("link", "review"):
        check_(f"usage/{verb}_not_ported", created(), [[verb, "--stage", "intent-capture"]])

    # The flags: one with no value, one followed by another flag, the two a question needs, and an empty value.
    check_("decision/flags", created(), [["decision", "--stage"], ["decision", "--stage", "intent-capture", "--decision"], ["decision", "--stage", "--decision", "x"],
                                         ["decision"], ["decision", "--stage", "intent-capture"], ["decision", "--decision", "x"],
                                         ["decision", "--stage", "", "--decision", "x"], ["decision", "--stage", "intent-capture", "--decision", ""]])
    # A value that reached the engine in pieces: a word no flag owns, words after a value, a fragment that starts
    # with dashes, Command Prompt's single quotes, and a flag the verb does not read.
    check_("decision/split_values", created(), [
        ask + ["extra"], ["stray", *ask], ["decision", "--stage", "intent-capture", "--decision", "Chose", "Option", "A", "for", "auth"],
        ["decision", "--stage", "intent-capture", "extra", "--decision", "Which", "one"],
        ["decision", "--stage", "intent-capture", "--decision", "Run todo", "--help", "first"],
        ["decision", "--stage", "intent-capture", "--decision", "'Chose", "Option", "A'"],
        ["decision", "--stage", "intent-capture", "--decision", "'Chose", "Option", "A"],
        ["decision", "--stage", "intent-capture", "--answer", "yes"], ["decision", "--details", "x", "--stage", "intent-capture"],
        ["decision", "--single", "word", "--stage", "intent-capture", "--decision", "x"],
        ["decision", "decision", "--stage", "intent-capture", "--decision", "x"],
        ["--stage", "intent-capture", "words", "decision", "--decision", "x"],
        ["decision", "stray", "words", "--stage", "intent-capture", "--decision", "x"],
        ["decision", "--stage", "intent-capture", "--decision", "Chose", "Option", "A'"]])
    # The same, with nothing after the words: the step names the project itself, first, so nothing is added after it.
    check_("decision/split_values_at_the_end", created(), [["--project-dir", "{P}", "decision", "--stage", "intent-capture", "--decision", "Chose", "Option", "A"],
                                                           ["--project-dir", "{P}", *ask, "decision"], ["--project-dir", "{P}", *ask, "extra"],
                                                           ["--project-dir", "{P}", "decision", "extra", "--stage", "intent-capture", "--decision", "x"]])
    check_("decision/split_after_project_dir", created(), [["--project-dir", "{P}", "tail", *ask], ["decision", "--project-dir", "--stage", "intent-capture", "--decision", "x"]])

    # The row: the question alone, with its options and rationale, for a single stage, twice, for a stage that is
    # none of the graph's, and with the flags a plain question does not read.
    check_("decision/plain", created(), [ask])
    check_("decision/whole", created(), [ask + ["--options", "Postgres,SQLite", "--rationale", "Both fit"], ask + ["--single"], ask])
    check_("decision/empty_options", created(), [ask + ["--options", "", "--rationale", ""]])
    check_("decision/unknown_stage", created(), [["decision", "--stage", "nope", "--decision", "Which?"]])
    check_("decision/flag_twice", created(), [["decision", "--stage", "nope", "--stage", "intent-capture", "--decision", "a", "--decision", "b"]])
    check_("decision/unread_flags", created(), [ask + ["--questions-file", "q.md", "--exact-option-labels", "true", "--hash-option-labels", "true", "--stage-level", "--field", "f", "--value", "v"]])
    check_("decision/multiline", created(), [["decision", "--stage", "intent-capture", "--decision", "First line\nSecond **Event**: X\n---\n## Heading", "--options", "a\nb"]])
    check_("decision/observer", created(), [ask], {"STOP_HOOK_PROBE": "1"})
    check_("decision/project_dir_first", created(), [["--project-dir", "{P}", *ask]])

    # The checkpoints: one the verb does not know, the learnings question, and the three Focus has not ported.
    check_("decision/checkpoint_unknown", created(), [ask + ["--checkpoint", "nope"], ask + ["--checkpoint", "learnings"], ask + ["--checkpoint", "guard-recovery"], ask + ["--checkpoint", ""]])
    for checkpoint in ("verification-command", "construction-policy", "plan-approval"):
        check_(f"decision/checkpoint_{checkpoint.replace('-', '_')}_not_ported", created(), [ask + ["--checkpoint", checkpoint]])
    check_("decision/unit_not_ported", created(), [ask + ["--unit", "api"]])
    check_("decision/unit_empty", created(), [ask + ["--unit", ""]])
    check_("decision/batch_file_not_ported", created(), [ask + ["--batch-file", "b.json"]])

    # A question logged after the person already replied names the message that answered it. The engine proves
    # the message: it is a record of the store, it carries their words, no later decision spent it, and it came
    # through the chat this command serves (or the only chat that has spoken, when the engine cannot tell).
    def sent(ident, at=T(5), session="s1", words="use postgres", **more):
        record = {"session": session, "at": at, "source": "prompt", "text": words or "/aidlc", "picker": None, "words": words, "settings": [],
                  "route": {"scope": None, "newIntent": False, "skip": [], "add": [], "projectType": None}, "applied": [], "id": ident, **more}
        return {f"aidlc/.aidlc-sessions/messages/{ident}.json": json.dumps(record) + "\n"}
    named = lambda ident: ask + ["--message", ident]
    turn = lambda ident, mark=None, minute=5: block("HUMAN_TURN", T(minute), "**Session**: s1", f"**Message Id**: {ident}", *([f"**Reply**: {mark}"] if mark else []))
    check_("decision/message", created(**sent("0000aaaa")), [named("0000aaaa"), ["decision", "--stage", "intent-capture", "--decision", "And the cache?", "--message", "0000aaaa"]])
    check_("decision/message_absent", created(**sent("0000aaaa")), [named("0000bbbb"), named("m-1"), named(""), named("0000AAAA"), named("0000aaaa.json"), named("../0000aaaa")])
    check_("decision/message_terminal", created(**sent("0000aaaa", session=None)), [named("0000aaaa")])
    check_("decision/message_with_turn", created(audit=CREATED_AUDIT + turn("0000aaaa"), **sent("0000aaaa")), [named("0000aaaa")])
    check_("decision/message_no_words", created(**sent("0000aaaa", words=None)), [named("0000aaaa")])
    check_("decision/message_empty_words", created(**sent("0000aaaa", words="")), [named("0000aaaa")])
    for mark in ("command", "question", "words"):
        check_(f"decision/message_turn_marked_{mark}", created(audit=CREATED_AUDIT + turn("0000aaaa", mark), **sent("0000aaaa")), [named("0000aaaa")])
    check_("decision/message_turn_of_another", created(audit=CREATED_AUDIT + turn("0000bbbb", "command"), **sent("0000aaaa")), [named("0000aaaa")])
    # Spent by a later decision of another kind than an answer; a tie favours the message; an answer spends nothing.
    spent = lambda event, minute, second=0: CREATED_AUDIT + block(event, T(minute, second), "**Stage**: intent-capture")
    for event in ("GATE_APPROVED", "GATE_REJECTED", "PLAN_APPROVAL_RECORDED", "SUMMARY_CONFIRMATION_RECORDED", "VERIFICATION_COMMAND_RECORDED",
                  "CONSTRUCTION_POLICY_RECORDED", "QUESTION_UNANSWERED", "REQUEST_ROUTED"):
        check_(f"decision/message_spent_by_{event.lower()}", created(audit=spent(event, 6), **sent("0000aaaa")), [named("0000aaaa")])
    check_("decision/message_same_second", created(audit=spent("GATE_APPROVED", 5), **sent("0000aaaa")), [named("0000aaaa")])
    check_("decision/message_after_spending", created(audit=spent("GATE_APPROVED", 4, 59), **sent("0000aaaa")), [named("0000aaaa")])
    check_("decision/message_not_spent_by_answers", created(audit=spent("QUESTION_ANSWERED", 6) + block("DECISION_RECORDED", T(7), "**Stage**: intent-capture", "**Decision**: x"),
                                                            **sent("0000aaaa")), [named("0000aaaa")])
    check_("decision/message_spent_latest_of_two", created(audit=spent("GATE_APPROVED", 6) + block("GATE_REJECTED", T(4), "**Stage**: intent-capture"), **sent("0000aaaa")),
           [named("0000aaaa")])
    check_("decision/message_spent_latest_last", created(audit=spent("GATE_REJECTED", 4) + block("GATE_APPROVED", T(6), "**Stage**: intent-capture"), **sent("0000aaaa")),
           [named("0000aaaa")])
    check_("decision/message_marked_on_another_row", created(audit=CREATED_AUDIT + block("QUESTION_ANSWERED", T(5), "**Stage**: intent-capture", "**Message Id**: 0000aaaa", "**Reply**: command"),
                                                             **sent("0000aaaa")), [named("0000aaaa")])
    # The chat: the one the command serves, another, and none the engine can tell.
    mine = {**my_pid_entry("s1"), "aidlc/.aidlc-sessions/s1.binding.json": '{"space":"default","intent":"260105-add-login","boundAt":"2026-01-05T10:00:00Z","source":"create"}'}
    check_("decision/message_this_chat", created(**sent("0000aaaa"), **sent("0000bbbb", session="s2"), **mine), [named("0000aaaa")])
    check_("decision/message_another_chat", created(**sent("0000aaaa"), **sent("0000bbbb", session="s2"), **mine), [named("0000bbbb")])
    check_("decision/message_terminal_not_this_chat", created(**sent("0000aaaa", session=None), **mine), [named("0000aaaa")])
    UNRECORDED.update(f"log/decision/message_{case}" for case in ("this_chat", "another_chat", "terminal_not_this_chat"))
    check_("decision/message_two_chats", created(**sent("0000aaaa"), **sent("0000bbbb", session="s2")), [named("0000aaaa")])
    check_("decision/message_two_chats_one_terminal", created(**sent("0000aaaa"), **sent("0000bbbb", session=None)), [named("0000aaaa")])
    check_("decision/message_two_chats_one_spent", created(audit=spent("GATE_APPROVED", 4), **sent("0000aaaa"), **sent("0000bbbb", session="s2", at=T(3))), [named("0000aaaa"), named("0000bbbb")])
    check_("decision/message_other_chat_spoke_since", created(audit=spent("GATE_APPROVED", 4), **sent("0000aaaa", at=T(4)), **sent("0000bbbb", session="s2", at=T(3))), [named("0000aaaa")])
    check_("decision/message_other_chat_at_the_spending_second", created(audit=spent("GATE_APPROVED", 4), **sent("0000aaaa"), **sent("0000bbbb", session="s2", at=T(4))), [named("0000aaaa")])
    check_("decision/message_same_chat_twice", created(**sent("0000aaaa"), **sent("0000bbbb", at=T(6))), [named("0000aaaa"), named("0000bbbb")])
    # A record that is not one of the store's stands for nothing.
    for label, more in (("other_id", {"id": "0000bbbb"}), ("bad_at", {"at": "yesterday"}), ("bad_source", {"source": "voice"}), ("cut_false", {"cut": False}), ("cut", {"cut": True}),
                        ("picker", {"picker": [{"question": "q", "reply": None}, {"question": "q", "reply": "r"}]}), ("bad_picker", {"picker": [{"question": "q"}]}),
                        ("bad_settings", {"settings": [{"key": "k"}]}), ("settings", {"settings": [{"key": "k", "value": "v"}]}), ("bad_route", {"route": None}),
                        ("bad_project_type", {"route": {"scope": None, "newIntent": False, "skip": [], "add": [], "projectType": "mixed"}}),
                        ("project_type", {"route": {"scope": "feature", "newIntent": True, "skip": ["a"], "add": ["b"], "projectType": "brownfield"}}),
                        ("bad_skip", {"route": {"scope": None, "newIntent": False, "skip": [1], "add": [], "projectType": None}}), ("bad_applied", {"applied": "x"}),
                        ("bad_text", {"text": 7}), ("bad_session", {"session": 7}), ("bad_words", {"words": 7})):
        check_(f"decision/message_record_{label}", created(**sent("0000aaaa", **more)), [named("0000aaaa")])
    whole = json.loads(list(sent("0000aaaa").values())[0])
    for label, record in (("bad_words_alone", {**whole, "words": 7}), ("no_words_key", {k: v for k, v in whole.items() if k != "words"}),
                          ("bad_scope", {**whole, "route": {**whole["route"], "scope": 7}}), ("bad_new_intent", {**whole, "route": {**whole["route"], "newIntent": "no"}}),
                          ("bad_add", {**whole, "route": {**whole["route"], "add": [1]}}), ("no_scope_key", {**whole, "route": {k: v for k, v in whole["route"].items() if k != "scope"}}),
                          ("too_large", {**whole, "text": "x" * 300000})):
        check_(f"decision/message_record_{label}", created(**{"aidlc/.aidlc-sessions/messages/0000aaaa.json": json.dumps(record) + "\n"}), [named("0000aaaa")])
    check_("decision/message_record_named_like_no_id", created(**{"aidlc/.aidlc-sessions/messages/m-1.json": json.dumps({**whole, "id": "m-1"}) + "\n"}), [named("m-1")])
    check_("decision/message_record_not_json", created(**{"aidlc/.aidlc-sessions/messages/0000aaaa.json": "{nope"}), [named("0000aaaa")])
    check_("decision/message_record_a_list", created(**{"aidlc/.aidlc-sessions/messages/0000aaaa.json": "[]\n"}), [named("0000aaaa")])
    check_("decision/message_record_linked", created(**{"elsewhere/0000aaaa.json": list(sent("0000aaaa").values())[0]}), [named("0000aaaa")],
           edits=(lambda tree: (os.makedirs(tree.path("aidlc/.aidlc-sessions")), os.symlink(tree.path("elsewhere"), tree.path("aidlc/.aidlc-sessions/messages"))),))

    # A question that offers the summary's two choices: an ordinary one where no summary is owed, refused where it is.
    summary = ["decision", "--stage", "intent-capture", "--decision", "Does this look right?", "--options", "Looks correct, Request changes."]
    check_("decision/summary_options_not_owed", created(), [summary])
    check_("decision/summary_options_owed", created(owing), [summary, summary[:-1] + ["LOOKS CORRECT!, request changes (Recommended), "], summary + ["--single"], summary + ["--unit", "api"],
                                                           summary + ["--rationale", "x", "--message", "nope"]])
    # The questions file the refusal names: the stage's own, the first by name when it has two, none under a unit.
    qdir = f"{CREATED_DIR}/ideation/intent-capture"
    qfile = f"{qdir}/intent-capture-questions.md"
    SUMMARY = "# Questions\n\n## Q1. Who is it for?\n\n[Answer]: A\n\n## Consolidated Summary Confirmation\n\nA tool for the team.\n\n[Answer]:\n"
    check_("decision/summary_options_owed_names_the_file", created(owing, **{qfile: SUMMARY}), [summary, summary + ["--unit", "api"]])
    check_("decision/summary_options_owed_two_files", created(owing, **{qfile: SUMMARY, f"{qdir}/b-questions.md": SUMMARY, f"{qdir}/Z-questions.md": SUMMARY, f"{qdir}/questions.md": SUMMARY,
                                                                        f"{qdir}/a-questions.md.bak": SUMMARY, f"{qdir}/0-notes.md": "x"}), [summary])
    check_("decision/summary_options_owed_no_questions_file_among_others", created(owing, **{f"{qdir}/0-notes.md": "x", f"{qdir}/questions.md": "x"}), [summary])
    check_("decision/summary_options_owed_unit_empty", created(owing, **{qfile: SUMMARY}), [summary + ["--unit", ""]])
    check_("decision/summary_options_owed_no_state_file", created(**{f"{CREATED_DIR}/aidlc-state.md": None, qfile: SUMMARY}), [summary])
    check_("decision/summary_options_owed_a_folder_named_like_one", created(owing, **{f"{qdir}/a-questions.md/x": "x"}), [summary])
    check_("decision/summary_options_owed_a_path_with_spaces", created(owing, **{f"{qdir}/my $HOME's-questions.md": SUMMARY}), [summary])
    # A stage that runs per unit: the commands name the unit, and the file is the unit's.
    building_owing = at_stage(owing, "functional-design", done=["intent-capture"], phase="CONSTRUCTION")
    unit_summary = ["decision", "--stage", "functional-design", "--decision", "Does this look right?", "--options", "Looks correct,Request changes"]
    unit_files = {f"{CREATED_DIR}/construction/api/functional-design/functional-design-questions.md": SUMMARY, f"{CREATED_DIR}/construction/web/functional-design/functional-design-questions.md": SUMMARY,
                  f"{CREATED_DIR}/construction/functional-design/functional-design-questions.md": SUMMARY}
    check_("decision/summary_options_owed_per_unit", created(building_owing, **unit_files), [unit_summary, unit_summary + ["--unit", "web"], unit_summary + ["--unit", "nope"]])
    # Units Generation off the plan: a stage that runs per unit keeps its files at the stage, and they are no unit's.
    stage_level = building_owing.replace("- [ ] units-generation \u2014 EXECUTE", "- [S] units-generation \u2014 SKIP")
    check_("decision/summary_options_owed_per_unit_at_the_stage", created(stage_level, **unit_files), [unit_summary, unit_summary + ["--unit", "web"]])
    check_("decision/summary_options_owed_per_unit_no_units", created(building_owing), [unit_summary, unit_summary + ["--unit", "web"]])
    check_("decision/summary_options_owed_per_unit_with_units_planned", created(building_owing.replace("- [ ] units-generation \u2014 SKIP", "- [ ] units-generation \u2014 EXECUTE"), **unit_files),
           [unit_summary, unit_summary + ["--unit", "web"]])
    # The summary's question, with its checkpoint: bound to the questions file, whose one answer line is still blank.
    asks = ["decision", "--stage", "intent-capture", "--decision", "Does this all look correct?", "--options", "Looks correct,Request changes", "--checkpoint", "summary-confirmation"]
    with_file = asks + ["--questions-file", qfile]
    check_("decision/summary_checkpoint", created(owing, **{qfile: SUMMARY}), [with_file, with_file + ["--single", "--rationale", "the end of the questions"]])
    check_("decision/summary_checkpoint_not_owed", created(**{qfile: SUMMARY}), [with_file])
    check_("decision/summary_checkpoint_other_options", created(owing, **{qfile: SUMMARY}), [with_file[:6] + ["Yes,No"] + with_file[7:], with_file[:5] + with_file[7:]])
    check_("decision/summary_checkpoint_no_file_named", created(owing, **{qfile: SUMMARY}), [asks, asks + ["--questions-file", ""], asks + ["--questions-file"]])
    check_("decision/summary_checkpoint_named_in_full", created(owing, **{qfile: SUMMARY}), [asks + ["--questions-file", "{P}/" + qfile], asks + ["--questions-file", "./" + qfile],
                                                                                           asks + ["--questions-file", qdir + "/../intent-capture/intent-capture-questions.md"]])
    elsewhere = {qfile: SUMMARY, "x-questions.md": SUMMARY, "aidlc/spaces/default/intents/x-questions.md": SUMMARY, f"{CREATED_DIR}x/x-questions.md": SUMMARY,
                 f"{CREATED_DIR}/x-questions.md": SUMMARY, f"{CREATED_DIR}/ideation/market-research/other-questions.md": SUMMARY, f"{qdir}/questions.md": SUMMARY,
                 f"{qdir}/intent-statement.md": SUMMARY, f"{qdir}/a-questions.md.bak": SUMMARY, f"{qdir}/-questions.md": SUMMARY}
    check_("decision/summary_checkpoint_outside_the_record", created(owing, **elsewhere), [asks + ["--questions-file", name] for name in (
        "x-questions.md", "../x-questions.md", "aidlc/spaces/default/intents/x-questions.md", f"{CREATED_DIR}x/x-questions.md", "{P}/x-questions.md", "/x-questions.md", CREATED_DIR,
        f"{CREATED_DIR}/../x-questions.md")])
    check_("decision/summary_checkpoint_any_questions_file_of_the_record", created(owing, **elsewhere), [asks + ["--questions-file", name] for name in (
        f"{CREATED_DIR}/x-questions.md", f"{CREATED_DIR}/ideation/market-research/other-questions.md", f"{qdir}/-questions.md")])
    check_("decision/summary_checkpoint_not_a_questions_file", created(owing, **elsewhere), [asks + ["--questions-file", name] for name in (
        f"{qdir}/questions.md", f"{qdir}/intent-statement.md", f"{qdir}/a-questions.md.bak", qdir, f"{qdir}/", f"{qdir}/intent-capture-questions.MD")])
    check_("decision/summary_checkpoint_missing_file", created(owing), [with_file])
    # What the summary section has to hold before the question is recorded: its one answer line, blank.
    for label, text in (("answered", SUMMARY.replace("[Answer]:\n", "[Answer]: Looks correct\n")), ("no_answer_line", SUMMARY.replace("[Answer]:\n", "")),
                        ("two_answer_lines", SUMMARY + "\n[Answer]:\n"), ("no_section", SUMMARY.replace("## Consolidated", "## Final")), ("empty", ""),
                        ("blank_with_spaces", SUMMARY.replace("[Answer]:\n", "[Answer]: \t \n")), ("answer_in_a_comment", SUMMARY.replace("[Answer]:\n", "[Answer]: <!-- pending -->\n")),
                        ("section_twice", SUMMARY + "\n## Consolidated Summary Confirmation\n"), ("question_twice", SUMMARY.replace("## Consolidated", "## Q1\n\n## Consolidated")),
                        ("heading_after", SUMMARY + "\n## Notes\n"), ("assumptions_after", SUMMARY + "\n---\n\n## Assumption Confirmation\n\n[Answer]: A\n"),
                        ("crlf_and_mark", "\ufeff" + SUMMARY.replace("\n", "\r\n"))):
        check_(f"decision/summary_checkpoint_file_{label}", created(owing, **{qfile: text}), [with_file])
    def not_text(tree):
        os.makedirs(tree.path(qdir), exist_ok=True)
        with open(tree.path(qfile), "wb") as handle:
            handle.write(b"## Consolidated Summary Confirmation\n\nA \xff tool\n\n[Answer]:\n")
    # (Compared live: a file that is not text cannot be kept in the recording.)
    UNRECORDED.add("log/decision/summary_checkpoint_file_not_text")
    check_("decision/summary_checkpoint_file_not_text", created(owing), [with_file, with_file], edits=[not_text])
    # Not opened by name: a link, a file behind a linked folder, a second name, a folder.
    check_("decision/summary_checkpoint_a_link", created(owing, **{"elsewhere-questions.md": SUMMARY}), [with_file],
           edits=[lambda tree: (os.makedirs(tree.path(qdir), exist_ok=True), os.symlink(tree.path("elsewhere-questions.md"), tree.path(qfile)))])
    check_("decision/summary_checkpoint_behind_a_link", created(owing, **{"elsewhere/intent-capture-questions.md": SUMMARY}), [with_file],
           edits=[lambda tree: (os.makedirs(os.path.dirname(tree.path(qdir)), exist_ok=True), os.symlink(tree.path("elsewhere"), tree.path(qdir)))])
    check_("decision/summary_checkpoint_second_name", created(owing, **{qfile: SUMMARY}), [with_file], edits=[lambda tree: os.link(tree.path(qfile), tree.path("other-name.md"))])
    check_("decision/summary_checkpoint_a_folder", created(owing, **{f"{qfile}/x": "x"}), [with_file])
    check_("decision/summary_checkpoint_a_dangling_link", created(owing), [with_file],
           edits=[lambda tree: (os.makedirs(tree.path(qdir), exist_ok=True), os.symlink(tree.path("nowhere-questions.md"), tree.path(qfile)))])
    # The order of the refusals: an unknown stage records; the claim of a confirmation, then the unit, come first.
    check_("decision/summary_checkpoint_order", created(owing, **{qfile: SUMMARY}), [with_file[:2] + ["nope"] + with_file[3:], with_file[:4] + ["Continue (user implicitly confirmed by hook trigger)"] + with_file[5:],
                                                                                    asks + ["--message", "nope"], with_file + ["--message", "nope"]])
    check_("decision/summary_checkpoint_observer", created(owing, **{qfile: SUMMARY}), [with_file], {"STOP_HOOK_PROBE": "1"})
    check_("decision/summary_checkpoint_no_workflow", CLONE, [with_file, asks])
    check_("decision/summary_options_other", created(owing), [summary[:-1] + ["Looks correct,Other"], summary[:-1] + ["Looks correct"], summary[:-1] + ["Looks correct,Request changes,Looks correct"],
                                                              summary[:-1] + ["Looks correct?,Request changes"]])
    check_("decision/summary_options_unknown_stage", created(owing), [["decision", "--stage", "nope", "--decision", "x", "--options", "Looks correct,Request changes"]])
    check_("decision/summary_options_guard_off", created(owing), [summary], {"SKIP_SUMMARY_CONFIRMATION_GUARD": "1"})

    # The person's confirmation credited to the hook, or implied: refused; quoted, a question like any other.
    said = lambda text: ["decision", "--stage", "intent-capture", "--decision", text]
    check_("decision/implied_confirmation", created(), [said("Continue with X (user implicitly confirmed by hook trigger)"), said("Proceed? Approval from the stop hook."),
                                                        said("The Person tacitly agreed, continue?"), said("confirmed by   the Stop-Hook"), said("human implicit consents"),
                                                        said('Go on: approval from "the" hook'), said("APPROVED BY HOOK")])
    check_("decision/implied_confirmation_quoted", created(), [said('Should the banner say "confirmed by the hook"?'), said("Is `user implicitly confirmed` the right label?"),
                                                               said("orders are implicitly confirmed after 48 hours: keep?")])
    check_("decision/implied_confirmation_after_quote", created(), [said('“é” — then: user implicitly approved'), said("'😀 it' (user tacitly agrees) 😀 confirmed by hook")])
    check_("decision/implied_confirmation_guard_off", created(), [said("user implicitly confirmed")], {"SKIP_HUMAN_PRESENCE_GUARD": "1"})

    # A new question retires the consent kept for the chat that asks it: the one named, every one when none is, and
    # nothing for a name that leaves no file name.
    kept = {"aidlc/.aidlc-sessions/plan-approval/protected-question-s1.json": "{}\n", "aidlc/.aidlc-sessions/plan-approval/protected-question-response-s1.json": "{}\n",
            "aidlc/.aidlc-sessions/plan-approval/protected-question-s2.json": "{}\n", "aidlc/.aidlc-sessions/plan-approval/receipt-1.json": "{}\n"}
    check_("decision/withdraws_named_session", created(**kept), [ask + ["--session", " s1 "]])
    check_("decision/withdraws_every_session", created(**kept), [ask])
    check_("decision/withdraws_no_session_name", created(**kept, **{"aidlc/.aidlc-sessions/plan-approval/protected-question-.json": "{}\n",
                                                                    "aidlc/.aidlc-sessions/plan-approval/protected-question-response-.json": "{}\n"}), [ask + ["--session", "///"]])
    # The chat that asks, told from the process that runs the command.
    check_("decision/withdraws_calling_session", created(**kept, **my_pid_entry("s1"),
                                                           **{"aidlc/.aidlc-sessions/s1.binding.json": '{"space":"default","intent":"260105-add-login","boundAt":"2026-01-05T10:00:00Z","source":"create"}'}), [ask])
    UNRECORDED.add("log/decision/withdraws_calling_session")
    check_("decision/withdraws_blank_session", created(**kept), [ask + ["--session", "  "]])
    check_("decision/withdraws_odd_session", created(**kept, **{"aidlc/.aidlc-sessions/plan-approval/protected-question-a-b.json": "{}\n"}), [ask + ["--session", "/a b/"]])
    check_("decision/withdraws_behind_link", created(**{"elsewhere/protected-question-s1.json": "{}\n"}), [ask],
           edits=(lambda tree: (os.makedirs(tree.path("aidlc/.aidlc-sessions")), os.symlink(tree.path("elsewhere"), tree.path("aidlc/.aidlc-sessions/plan-approval"))),))

    # answers: the questions of a stage read back with the answers they have. It writes nothing, and its own
    # refusal leaves no row.
    read = ["answers", "--stage", "intent-capture"]
    Q = lambda minute, text, *fields: block("DECISION_RECORDED", T(minute), "**Stage**: intent-capture", f"**Decision**: {text}", *fields)
    A = lambda minute, text, *fields: block("QUESTION_ANSWERED", T(minute), "**Stage**: intent-capture", f"**Details**: {text}", *fields)
    told = lambda *rows, **more: created(audit=CREATED_AUDIT + "".join(rows), **more)
    other = lambda *rows: {f"{CREATED_DIR}/audit/desktop-def456.md": TITLE + "".join(rows)}
    check_("answers/usage", created(), [["answers"], ["answers", "--stage"], ["answers", "--stage", ""], read + ["extra"], read + ["--single"], read + ["--decision", "x"],
                                        read + ["--unit"], ["answers", "--unit", "api"], read + ["--stage", "intent-capture"], ["answers", "extra", "--decision", "x"]])
    check_("answers/no_workflow", CLONE, [read])
    check_("answers/no_state_file", created(**{f"{CREATED_DIR}/aidlc-state.md": None}), [read])
    check_("answers/state_linked", created(**{f"{CREATED_DIR}/aidlc-state.md": None, "elsewhere.md": CREATED_STATE}), [read],
           edits=(lambda tree: os.symlink(tree.path("elsewhere.md"), tree.path(f"{CREATED_DIR}/aidlc-state.md")),))
    check_("answers/shard_unreadable", created(**{f"{CREATED_DIR}/audit/desktop-def456.md/x": "x"}), [read])
    check_("answers/none", created(), [read, ["answers", "--stage", "nope"], read + ["--unit", "api"]])
    check_("answers/open", told(Q(1, "Which database?", "**Options**: Postgres, SQLite ,,"), Q(2, "Which cache?")), [read])
    check_("answers/answered", told(Q(1, "Which database?", "**Options**: Postgres,SQLite"), A(2, "Postgres")), [read])
    check_("answers/menu", told(Q(1, "Which database?"), Q(2, "Which cache?"), A(3, "Postgres and Redis"), Q(4, "Which queue?")), [read])
    check_("answers/continued", told(Q(1, "Which database?"), Q(2, "Which cache?"), A(3, "Postgres"), A(4, "Redis"), A(5, "and that is all")), [read])
    check_("answers/not_continued_over_a_question", told(Q(1, "Which database?"), A(2, "Postgres"), Q(3, "Which cache?"), A(4, "Redis"), A(5, "really"), Q(6, "x")), [read])
    check_("answers/answer_alone", told(A(1, "Postgres"), Q(2, "Which database?")), [read])
    check_("answers/answer_before_its_question", told(A(1, "Postgres"), Q(2, "Which database?"), A(3, "SQLite")), [read])
    for label, said in (("cancelled", "Cancelled."), ("empty", ""), ("timed_out", "timed out"), ("no_response", "No response!"), ("dismissed", "User dismissed"),
                        ("words", "cancel the standing order")):
        check_(f"answers/non_answer_{label}", told(Q(1, "Which database?"), A(2, said)), [read])
    check_("answers/non_answer_then_answer", told(Q(1, "Which database?"), A(2, "Cancelled"), A(3, "Postgres")), [read])
    check_("answers/non_answer_alone", told(A(1, "Cancelled"), Q(2, "Which database?")), [read])
    check_("answers/non_answer_after_exchange", told(Q(1, "Which database?"), A(2, "Postgres"), A(3, "Cancelled"), A(4, "and Redis")), [read])
    check_("answers/message_id", told(Q(1, "Which database?", "**Message Id**: 0000aaaa"), A(2, "Postgres"), Q(3, "Which cache?", "**Message Id**: 0000aaaa"),
                                      A(4, "Redis", "**Message Id**: 0000bbbb")), [read])
    check_("answers/not_a_question", told(block("DECISION_RECORDED", T(1), "**Stage**: intent-capture"), A(2, "Postgres"), Q(3, "", "**Options**: a")), [read])
    check_("answers/checkpoint_rows", told(Q(1, "Does this look right?", "**Checkpoint**: summary-confirmation"), A(2, "Looks correct", "**Checkpoint**: summary-confirmation"),
                                           Q(3, "Which database?"), A(4, "Postgres")), [read])
    check_("answers/other_stage", told(block("DECISION_RECORDED", T(1), "**Stage**: market-research", "**Decision**: Who buys?"), A(2, "Postgres"),
                                       block("QUESTION_ANSWERED", T(3), "**Stage**: market-research", "**Details**: Teams")), [read, ["answers", "--stage", "market-research"]])
    check_("answers/scopes", told(Q(1, "For api?", "**Unit**: api"), Q(2, "For web?", "**Unit**: web"), Q(3, "For all?"), Q(4, "Single?", "**Workflow**: single-stage:intent-capture"),
                                  Q(5, "Again?", "**Unit**: api", "**Attempt Generation**: 2"), A(6, "web says yes", "**Unit**: web"), A(7, "all say yes"),
                                  A(8, "second attempt", "**Unit**: api", "**Attempt Generation**: 2"), A(9, "nobody's", "**Unit**: lib")),
           [read, read + ["--unit", "api"], read + ["--unit", "web"], read + ["--unit", ""], read + ["--unit", "nope"]])
    # Two shards: an instant shared between them orders nothing.
    check_("answers/other_shard_later", told(Q(1, "Which database?"), **other(A(2, "Postgres"))), [read])
    check_("answers/other_shard_tied", told(Q(1, "Which database?"), **other(A(1, "Postgres"))), [read])
    check_("answers/other_shard_tied_then_later", told(Q(1, "Which database?"), A(3, "SQLite"), **other(A(1, "Postgres"))), [read])
    check_("answers/other_shard_earlier", told(Q(2, "Which database?"), **other(A(1, "Postgres"))), [read])
    check_("answers/two_answers_tied", told(Q(1, "Which database?"), A(2, "Postgres"), **other(A(2, "SQLite"))), [read])
    check_("answers/answer_tied_with_a_cancellation", told(Q(1, "Which database?"), A(2, "Postgres"), **other(A(2, "Cancelled"))), [read])
    check_("answers/cancellation_tied_with_an_answer", told(Q(1, "Which database?"), A(2, "Cancelled"), **other(A(2, "Postgres"))), [read])
    check_("answers/questions_in_two_shards", told(Q(1, "Which database?"), A(3, "Both"), **other(Q(2, "Which cache?"))), [read])
    check_("answers/question_tied_with_the_answer", told(Q(1, "Which database?"), A(2, "Both"), **other(Q(2, "Which cache?"))), [read])
    check_("answers/open_in_the_ledgers_order", told(Q(3, "Third?"), Q(1, "First?"), **other(Q(2, "Second?"), Q(1, "Also first?"))), [read])
    check_("answers/continued_across_shards", told(Q(1, "Which database?"), A(2, "Postgres"), **other(A(3, "and Redis"), A(2, "tied"))), [read])
    check_("answers/scope_fields_in_order", told(Q(1, "Both?", "**Workflow**: single-stage:intent-capture", "**Attempt Generation**: 2", "**Unit**: api"),
                                                 A(2, "x", "**Workflow**: single-stage:intent-capture", "**Unit**: api")), [read])
    # What ends an exchange: a question between it and the answer, even one a cancellation already spent; a
    # question tied with the answer; an answer tied with the one that closed it.
    check_("answers/not_continued_over_a_cancelled_question", told(Q(1, "Which database?"), A(2, "Postgres"), Q(3, "Which cache?"), A(4, "Cancelled"), A(5, "Redis")), [read])
    check_("answers/not_continued_with_a_tied_question", told(Q(1, "Which database?"), A(2, "Postgres"), A(3, "more"), **other(Q(3, "Which cache?"))), [read])
    check_("answers/not_continued_by_a_tied_answer", told(A(2, "in the file before its question"), Q(1, "Which database?"), **other(A(2, "Postgres"))), [read])
    check_("answers/continued_once_of_two_tied", told(Q(1, "Which database?"), A(2, "Postgres"), A(3, "laptop's"), **other(A(3, "desktop's"))), [read])
    # A shard whose rows are out of the order of their instants: every answer has another before it.
    check_("answers/out_of_order_in_two_scopes", told(Q(1, "For api?", "**Unit**: api"), Q(1, "For web?", "**Unit**: web"), A(7, "api says", "**Unit**: api"),
                                                      A(5, "web says", "**Unit**: web")), [read])
    check_("answers/cycle", told(Q(1, "Which database?"), A(7, "first in the file"), A(5, "second in the file"), **other(A(6, "between them"))), [read])
    check_("answers/cycle_with_a_cancellation", told(Q(1, "Which database?"), A(7, "Cancelled"), A(5, "second in the file"), **other(A(6, "between them"))), [read])

    # answer: the person's reply to a question, as the agent read it.
    reply = ["answer", "--stage", "intent-capture", "--details", "Postgres"]
    replied = lambda text, *more: ["answer", "--stage", "intent-capture", "--details", text, *more]
    H = lambda minute, *fields: block("HUMAN_TURN", T(minute), "**Session**: s1", *fields)
    asked = told(Q(1, "Which database?"), H(2))
    check_("answer/usage", created(), [["answer"], ["answer", "--stage"], ["answer", "--stage", "intent-capture"], ["answer", "--details", "x"],
                                       ["answer", "--stage", "intent-capture", "--details", ""], ["answer", "--stage", "intent-capture", "--details"],
                                       ["answer", "--stage", "", "--details", "x"]])
    check_("answer/split_values", created(), [reply + ["extra"], ["answer", "--stage", "intent-capture", "--details", "Chose", "Option", "A"],
                                              ["answer", "--stage", "intent-capture", "--details", "'Chose", "Option", "A'"], reply + ["--decision", "x"], reply + ["--rationale", "x"],
                                              ["answer", "--stage", "intent-capture", "--details", "Run todo", "--help", "first"], reply + ["--units", "a"], reply + ["--park"],
                                              ["--project-dir", "{P}", "answer", "--stage", "intent-capture", "--details", "Chose", "Option"]])
    check_("answer/no_workflow", CLONE, [reply])
    check_("answer/unknown_checkpoint", asked, [reply + ["--checkpoint", "nope"], reply + ["--checkpoint", "learnings"], reply + ["--checkpoint", ""]])
    for flag, value in (("override", "because"), ("override-file", "o.txt"), ("batch-file", "b.json"), ("unit", "api")):
        check_(f"answer/{flag.replace('-', '_')}_not_ported", asked, [reply + [f"--{flag}", value]])
    for checkpoint in ("guard-recovery", "verification-command", "construction-policy", "plan-approval"):
        check_(f"answer/checkpoint_{checkpoint.replace('-', '_')}_not_ported", asked, [reply + ["--checkpoint", checkpoint]])
    # The reply an answer needs: one that has answered nothing yet. One reply answers every question that was open
    # when it arrived; a command to the engine is no reply; with the guard off, or unattended, each says so.
    check_("answer/replied", asked, [reply, reply])
    check_("answer/no_reply", told(Q(1, "Which database?")), [reply])
    check_("answer/no_question", told(H(1)), [reply, reply])
    check_("answer/menu", told(Q(1, "Which database?"), Q(2, "Which cache?"), H(3)), [reply, replied("Redis"), replied("and a third"), ["answers", "--stage", "intent-capture"]])
    check_("answer/reply_spent_by_an_answer", told(Q(1, "Which database?"), H(2), A(3, "Postgres"), Q(4, "Which cache?")), [replied("Redis")])
    check_("answer/reply_spent_by_a_gate", told(H(1), block("GATE_APPROVED", T(2), "**Stage**: workspace-detection"), Q(3, "Which database?")), [reply])
    check_("answer/reply_before_the_question", told(H(1), Q(2, "Which database?")), [reply])
    check_("answer/command_turn_after_an_answered_reply", told(Q(1, "Which database?"), Q(2, "Which cache?"), H(3), A(4, "Postgres"), H(5, "**Reply**: command")), [replied("Redis")])
    check_("answer/command_turn", told(Q(1, "Which database?"), H(2, "**Reply**: command")), [reply])
    check_("answer/question_turn", told(Q(1, "Which database?"), H(2, "**Reply**: question")), [reply])
    check_("answer/guard_off", told(Q(1, "Which database?")), [reply], {"SKIP_HUMAN_PRESENCE_GUARD": "1"})
    check_("answer/unattended", told(Q(1, "Which database?")), [reply], {"UNATTENDED": "1"})
    check_("answer/unattended_replied", asked, [reply, reply], {"UNATTENDED": "1"})
    check_("answer/single", asked, [reply + ["--single"]])
    check_("answer/unknown_stage", told(H(1)), [["answer", "--stage", "nope", "--details", "x"]])
    check_("answer/observer", asked, [reply], {"STOP_HOOK_PROBE": "1"})
    check_("answer/other_shard_turn", told(Q(1, "Which database?"), **other(H(2))), [reply])
    # A dismissed question box is no answer; a choice the text credits to the assistant is not the person's.
    check_("answer/dismissed", asked, [replied("Cancelled."), replied("timed out"), replied("  "), replied("User dismissed"), replied("cancel the standing order")])
    check_("answer/self_attributed", asked, [replied("A. Nothing to add - CONDUCTOR DEFAULT, session unattended"), replied("The assistant chose this answer."),
                                             replied('The label reads "assistant-selected answer"'), replied("“é” not a human answer.")])
    check_("answer/self_attributed_guard_off", asked, [replied("The assistant chose this answer.")], {"SKIP_HUMAN_PRESENCE_GUARD": "1"})
    # An approval is a gate's: at an open gate with no question asked since it opened, the answer is acknowledged
    # and not written; with one, it is that question's.
    gate_open = lambda *rows: created(CREATED_STATE.replace("- [-] intent-capture — EXECUTE", "- [?] intent-capture — EXECUTE"),
                                      CREATED_AUDIT + block("STAGE_AWAITING_APPROVAL", T(1), "**Stage**: intent-capture") + "".join(rows))
    check_("answer/at_gate", gate_open(H(2)), [replied("Approve"), replied("Approve")])
    check_("answer/at_gate_no_reply", gate_open(), [replied("Approve")])
    check_("answer/at_gate_guard_off", gate_open(), [replied("Approve")], {"SKIP_HUMAN_PRESENCE_GUARD": "1"})
    check_("answer/at_gate_question_open", gate_open(Q(2, "Which database?"), H(3)), [reply, reply])
    check_("answer/at_gate_question_answered", gate_open(Q(2, "Which database?"), A(3, "Postgres"), H(4)), [reply])
    check_("answer/at_gate_question_before_it", created(CREATED_STATE.replace("- [-] intent-capture — EXECUTE", "- [?] intent-capture — EXECUTE"),
                                                        CREATED_AUDIT + Q(1, "Which database?") + block("STAGE_AWAITING_APPROVAL", T(2), "**Stage**: intent-capture") + H(3)), [reply])
    check_("answer/at_gate_opened_twice", gate_open(Q(2, "Which database?"), block("STAGE_AWAITING_APPROVAL", T(3), "**Stage**: intent-capture"), H(4)), [reply])
    check_("answer/at_gate_another_gate_after", gate_open(Q(2, "Which database?"), block("STAGE_AWAITING_APPROVAL", T(3), "**Stage**: market-research"), H(4)), [reply])
    check_("answer/at_gate_question_of_another_stage", gate_open(block("DECISION_RECORDED", T(2), "**Stage**: market-research", "**Decision**: Who buys?"), H(3)), [reply])
    check_("answer/at_another_stages_gate", created(CREATED_STATE.replace("- [ ] market-research — EXECUTE", "- [?] market-research — EXECUTE"),
                                                    CREATED_AUDIT + block("STAGE_AWAITING_APPROVAL", T(1), "**Stage**: market-research") + Q(2, "Which database?") + H(3)), [reply])
    # The mode question: the agent passes the way the person chose, and its option is what the row keeps.
    mode = told(Q(1, "How would you like to answer them?", "**Options**: Guide me, I'll edit the file, Chat"), H(2))
    for label, text in (("label", "chat"), ("number", "3"), ("numbered_label", "2. I'll edit the file"), ("words_after", "Guide me, please"), ("quoted", "'Chat'"),
                        ("number_and_another", "1. Chat"), ("other", "whatever works"), ("dismissed", "Cancelled"), ("decorated", "Chat (Recommended)"),
                        ("quoted_words_after", '"Chat, please"'), ("capitals_words_after", "CHAT, please"), ("longer_word", "Chatty"), ("lettered", "C) chat: yes"),
                        ("quoted_then_decorated", "'Chat' (Recommended)")):
        check_(f"answer/mode_{label}", mode, [replied(text)])
    check_("answer/mode_by_options", told(Q(1, "Pick one", "**Options**: chat, guide me, i’ll edit the file"), H(2)), [replied("first one")])
    check_("answer/mode_question_of_another_stage", told(Q(1, "Which database?"), block("DECISION_RECORDED", T(2), "**Stage**: market-research",
                                                              "**Decision**: How would you like to answer them?"), H(3)), [replied("whatever works")])
    check_("answer/mode_picker_note", told(Q(1, "How would you like to answer them?", "**Options**: Guide me, I'll edit the file, Chat"), H(2, '**Picked**: ["Guide me"]')), [replied("3")])
    check_("answer/mode_question_closed", told(Q(1, "How would you like to answer them?"), A(2, "Chat"), Q(3, "Which database?"), H(4)), [replied("whatever works")])
    # The person's own words beside the choice: kept by their chat right after the turn's row.
    def kept(audit, *texts, at=20, shard=CREATED_SHARD, session="s1"):
        return {f"{CREATED_DIR}/.aidlc-engine/gate-words/{session}.json": json.dumps(
            {"version": 1, "session": session, "shard": shard, "dropped": 0, "messages": [{"offset": len(audit.encode()) + at + n, "text": text} for n, text in enumerate(texts)]})}
    before_turn = CREATED_AUDIT + Q(1, "Which database?")
    check_("answer/person_reply", told(Q(1, "Which database?"), H(2), **kept(before_turn, "the first one, postgres")), [reply])
    check_("answer/person_reply_before_the_turn", told(Q(1, "Which database?"), H(2), **kept(before_turn, "old words", at=-5)), [reply])
    check_("answer/person_reply_after_the_turn", told(Q(1, "Which database?"), H(2), **kept(before_turn, "later words", at=4000)), [reply])
    check_("answer/person_reply_other_shard", told(Q(1, "Which database?"), H(2), **kept(before_turn, "words", shard="other/audit/x.md")), [reply])
    check_("answer/person_reply_dismissal", told(Q(1, "Which database?"), H(2), **kept(before_turn, "Cancelled")), [reply])
    check_("answer/person_reply_of_the_last_turn", told(Q(1, "Which database?"), H(2), H(3), **kept(before_turn, "first turn's words")), [reply])
    check_("answer/person_reply_turn_without_session", told(Q(1, "Which database?"), block("HUMAN_TURN", T(2)), **kept(before_turn, "words")), [reply])
    check_("answer/person_reply_guard_off_no_turn", told(Q(1, "Which database?")), [reply], {"SKIP_HUMAN_PRESENCE_GUARD": "1"})
    # A reply in a question box: an answer none of the picks carried is recorded with a note.
    for label, picked, text in (("other", '["Postgres"]', "SQLite"), ("same", '["Postgres (Recommended)"]', "postgres"), ("contains", '["Postgres"]', "Postgres with pgvector"),
                                ("contained", '["Postgres with pgvector"]', "Postgres"), ("letter", '["Postgres"]', "B."), ("number", '["Postgres"]', "2"),
                                ("two", '["Postgres", "Redis"]', "SQLite"), ("not_a_list", '"Postgres"', "SQLite"), ("empty", "[]", "SQLite"), ("not_text", "[1]", "SQLite"),
                                ("not_json", "[Postgres", "SQLite"), ("empty_pick", '["", "Redis"]', "SQLite")):
        check_(f"answer/picker_{label}", told(Q(1, "Which database?"), H(2, f"**Picked**: {picked}")), [replied(text)])
    check_("answer/picker_decorated_pick", told(Q(1, "Which database?"), H(2, '**Picked**: ["Postgres (Recommended)"]')), [replied("Postgres!")])
    check_("answer/picker_of_the_last_turn", told(Q(1, "Which database?"), H(2, '**Picked**: ["Postgres"]'), H(3)), [replied("SQLite")])
    # The message that answered: named, or inherited from the question it was logged with.
    check_("answer/message", told(Q(1, "Which database?"), **sent("0000aaaa")), [reply + ["--message", "0000aaaa"], reply + ["--message", "0000bbbb"], reply + ["--message", ""]])
    check_("answer/message_no_words", told(Q(1, "Which database?"), **sent("0000aaaa", words="")), [reply + ["--message", "0000aaaa"]])
    check_("answer/message_dismissal", told(Q(1, "Which database?"), **sent("0000aaaa", words="Cancelled")), [reply + ["--message", "0000aaaa"]])
    check_("answer/message_inherited", told(Q(1, "Which database?", "**Message Id**: 0000aaaa"), **sent("0000aaaa")), [reply, reply])
    check_("answer/message_inherited_unproved", told(Q(1, "Which database?", "**Message Id**: 0000bbbb"), **sent("0000aaaa")), [reply])
    check_("answer/message_inherited_unproved_replied", told(Q(1, "Which database?", "**Message Id**: 0000bbbb"), H(2), **sent("0000aaaa")), [reply])
    check_("answer/message_of_another_stages_question", told(Q(1, "Which database?"), block("DECISION_RECORDED", T(2), "**Stage**: market-research", "**Decision**: Who buys?",
                                                                                           "**Message Id**: 0000aaaa"), **sent("0000aaaa")), [reply])
    check_("answer/message_inherited_from_the_latest", told(Q(1, "Which database?", "**Message Id**: 0000aaaa"), Q(2, "Which cache?"), **sent("0000aaaa")), [reply])
    # A question logged after the reply: the refusal names the messages that arrived before it.
    late = lambda *rows, **more: told(H(1, "**Message Id**: 0000aaaa"), A(2, "an earlier answer"), *rows, Q(5, "Which database?"), **more)
    check_("answer/late_question", late(**sent("0000aaaa", at=T(1), words="postgres   please,\nit is what we know and it has served us well for years")), [reply])
    check_("answer/late_question_text_and_words", late(**sent("0000aaaa", at=T(1), words="x" * 40, text="/aidlc " + "x" * 40)), [reply])
    check_("answer/late_question_before_the_latest", told(H(1, "**Message Id**: 0000aaaa"), A(2, "x"), Q(3, "First?"), H(4, "**Message Id**: 0000bbbb"), A(5, "y"), Q(7, "Second?"),
                                                          **sent("0000aaaa", at=T(1)), **sent("0000bbbb", at=T(4))), [reply])
    check_("answer/late_question_turn_in_another_shard", told(H(1, "**Message Id**: 0000aaaa"), A(2, "x"), Q(3, "Which database?"),
                                                              **other(H(4, "**Message Id**: 0000bbbb"), A(5, "y"), Q(6, "Which cache?")),
                                                              **sent("0000aaaa", at=T(1)), **sent("0000bbbb", at=T(4))), [reply])
    check_("answer/late_question_out_of_order", told(H(9, "**Message Id**: 0000aaaa"), A(10, "x"), Q(5, "Which database?"), Q(11, "Which cache?"), **sent("0000aaaa", at=T(1))),
           [reply])
    check_("answer/late_question_before_in_the_file_only", told(H(9, "**Message Id**: 0000aaaa"), A(10, "x"), Q(5, "Which database?"), **sent("0000aaaa", at=T(1))), [reply])
    check_("answer/late_question_before_in_the_file_spent_turn", told(H(9, "**Message Id**: 0000aaaa"), Q(5, "Which database?"), block("GATE_APPROVED", T(10), "**Stage**: workspace-detection"),
                                                                     **sent("0000aaaa", at=T(11))), [reply])
    check_("answer/late_question_named_by_two_turns", told(H(1, "**Message Id**: 0000aaaa"), A(2, "x"), Q(5, "Which database?"), H(6, "**Message Id**: 0000aaaa"),
                                                           block("GATE_APPROVED", T(7), "**Stage**: workspace-detection"), **sent("0000aaaa", at=T(8))), [reply])
    check_("answer/late_question_this_chat", late(**sent("0000aaaa", at=T(1)), **sent("0000bbbb", at=T(1), session="s2"), **mine), [reply])
    UNRECORDED.add("log/answer/late_question_this_chat")
    check_("answer/late_question_two", late(H(3, "**Message Id**: 0000bbbb"), A(4, "another"), **sent("0000aaaa", at=T(1)), **sent("0000bbbb", at=T(3), words=None, text="/aidlc go")), [reply])
    check_("answer/late_question_two_replies", late(H(3, "**Message Id**: 0000bbbb"), A(4, "another"), **sent("0000aaaa", at=T(1)), **sent("0000bbbb", at=T(3), words="x" * 41)), [reply])
    check_("answer/late_question_no_turn_row", told(H(1), A(2, "an earlier answer"), Q(5, "Which database?"), **sent("0000aaaa", at=T(4)), **sent("0000bbbb", at=T(5)),
                                                    **sent("0000cccc", at=T(6))), [reply])
    check_("answer/late_question_message_after", late(**sent("0000aaaa", at=T(1)), **sent("0000bbbb", at=T(9))), [reply])
    check_("answer/late_question_spent", told(H(1, "**Message Id**: 0000aaaa"), block("GATE_APPROVED", T(2), "**Stage**: workspace-detection"), Q(5, "Which database?"),
                                              **sent("0000aaaa", at=T(1))), [reply])
    check_("answer/late_question_two_chats", late(**sent("0000aaaa", at=T(1)), **sent("0000bbbb", at=T(1), session="s2")), [reply])
    check_("answer/late_question_command", told(H(1, "**Message Id**: 0000aaaa", "**Reply**: command"), A(2, "an earlier answer"), Q(5, "Which database?"), **sent("0000aaaa", at=T(1))), [reply])
    check_("answer/late_question_other_shard", told(A(2, "an earlier answer"), Q(5, "Which database?"), **other(H(5, "**Message Id**: 0000aaaa"), H(4, "**Message Id**: 0000bbbb")),
                                                    **sent("0000aaaa", at=T(1)), **sent("0000bbbb", at=T(1))), [reply])
    # A stage that owes a summary confirmation, with that summary as its open question.
    summary_q = Q(1, "Does this look right?", "**Options**: Looks correct,Request changes")
    choices = [replied("Looks correct"), replied("Request changes"), replied("B) request changes: rename  the\thandler,\nplease"), replied("'Looks correct.'"), reply,
               replied("Request changes."), replied("Looks correct (Recommended)")]
    check_("answer/summary_question_open", created(owing, CREATED_AUDIT + summary_q + H(2)), choices)
    recorded_q = Q(1, "Does this look right?", "**Checkpoint**: Consolidated Summary Confirmation")
    check_("answer/summary_checkpoint_open", created(owing, CREATED_AUDIT + recorded_q + H(2)), choices)
    qfile = f"{CREATED_DIR}/ideation/intent-capture/intent-capture-questions.md"
    check_("answer/summary_checkpoint_open_names_the_file", created(owing, CREATED_AUDIT + recorded_q + H(2), **{qfile: "x"}), [replied("Looks correct"), replied("Looks correct", "--single")])
    check_("answer/summary_question_open_names_the_file", created(owing, CREATED_AUDIT + summary_q + H(2), **{qfile: "x"}), [replied("Request changes: the title")])
    check_("answer/summary_both_forms_open", created(owing, CREATED_AUDIT + summary_q + recorded_q.replace("laptop", "laptop") + H(2)), [replied("Looks correct")])
    check_("answer/summary_question_open_guard_off", created(owing, CREATED_AUDIT + summary_q + H(2)), [replied("Looks correct")], {"SKIP_SUMMARY_CONFIRMATION_GUARD": "1"})
    # Three questions, each with another after it: position orders the two of one shard, the instant orders the
    # shards. The questions of the latest instant stand for all, and that one is the summary (or is not).
    plain_q = lambda minute: Q(minute, "Does this look right?", "**Options**: Looks correct,Request changes")
    check_("answer/summary_question_in_a_cycle", created(owing, CREATED_AUDIT + plain_q(7) + Q(5, "Which database?") + H(8), **other(Q(6, "Which cache?"))), [replied("Looks correct")])
    check_("answer/summary_question_in_a_cycle_not_latest", created(owing, CREATED_AUDIT + Q(7, "Which database?") + plain_q(5) + H(8), **other(Q(6, "Which cache?"))), [replied("Looks correct")])
    check_("answer/summary_question_in_another_shard", created(owing, CREATED_AUDIT + Q(1, "Which database?") + H(3), **other(plain_q(2))), [replied("Looks correct")])
    check_("answer/summary_question_tied_with_another", created(owing, CREATED_AUDIT + Q(2, "Which database?") + H(3), **other(plain_q(2))), [replied("Looks correct")])
    check_("answer/summary_question_of_a_unit", created(owing, CREATED_AUDIT + Q(1, "Does this look right?", "**Options**: Looks correct,Request changes", "**Unit**: api") + H(2)), [replied("Looks correct")])
    check_("answer/summary_question_single_stage", created(owing, CREATED_AUDIT + Q(1, "Does this look right?", "**Options**: Looks correct,Request changes", "**Workflow**: single-stage:intent-capture") + H(2)),
           [replied("Looks correct"), replied("Looks correct", "--single")])
    check_("answer/summary_question_open_no_state_file", created(audit=CREATED_AUDIT + summary_q + H(2), **{f"{CREATED_DIR}/aidlc-state.md": None}), [replied("Looks correct")])
    check_("answer/summary_choice_with_no_question_no_state_file", created(audit=CREATED_AUDIT + H(2), **{f"{CREATED_DIR}/aidlc-state.md": None}), [replied("Looks correct")])
    # The summary's choice, with its checkpoint. The question is recorded and unanswered, the person has replied
    # since their last answer, and the questions file holds the choice on its one answer line.
    SUMMARY = "# Questions\n\n## Q1. Who is it for?\n\n[Answer]: A\n\n## Consolidated Summary Confirmation\n\nA tool for the team.\n\n[Answer]:\n"
    confirmed, changed = SUMMARY.replace("[Answer]:\n", "[Answer]: Looks correct\n"), SUMMARY.replace("[Answer]:\n", "[Answer]: Request changes\n")
    sq = lambda minute, *more, file=qfile: Q(minute, "Does this all look correct?", "**Options**: Looks correct,Request changes", "**Checkpoint**: Consolidated Summary Confirmation",
                                             f"**Questions File**: {file}", *more)
    chooses = lambda details, *more: ["answer", "--stage", "intent-capture", "--details", details, "--checkpoint", "summary-confirmation", "--questions-file", qfile, *more]
    asked_and_replied = CREATED_AUDIT + sq(1) + H(2)
    registry = f"{CREATED_DIR}/.aidlc-engine/summary-authorization/intent-capture/stage.json"
    check_("answer/summary_confirmed", created(owing, asked_and_replied, **{qfile: confirmed}), [chooses("Looks correct"), chooses("Looks correct")])
    check_("answer/summary_confirmed_not_owed", created(audit=asked_and_replied, **{qfile: confirmed}), [chooses("A) looks correct.")])
    check_("answer/summary_changes_asked", created(owing, asked_and_replied, **{qfile: changed}), [chooses("Request changes: rename  the\thandler,\nplease"), chooses("Request changes")])
    check_("answer/summary_changes_asked_bare", created(owing, asked_and_replied, **{qfile: changed}), [chooses("request changes.")])
    check_("answer/summary_registry_replaced", created(owing, asked_and_replied, **{qfile: confirmed, registry: '{"id": "old"}\n'}), [chooses("Looks correct")])
    check_("answer/summary_registry_withdrawn", created(owing, asked_and_replied, **{qfile: changed, registry: '{"id": "old"}\n'}), [chooses("Request changes")])
    check_("answer/summary_choice_not_named", created(owing, asked_and_replied, **{qfile: confirmed}), [chooses("Yes"), chooses("Postgres"), chooses(""), chooses("Cancelled"), chooses("Looks correctly")])
    check_("answer/summary_file_says_otherwise", created(owing, asked_and_replied, **{qfile: changed}), [chooses("Looks correct")])
    check_("answer/summary_file_still_blank", created(owing, asked_and_replied, **{qfile: SUMMARY}), [chooses("Looks correct"), chooses("Request changes")])
    check_("answer/summary_file_not_named", created(owing, asked_and_replied, **{qfile: confirmed}), [chooses("Looks correct")[:-2], chooses("Looks correct")[:-1] + ["x-questions.md"]])
    check_("answer/summary_file_invalid", created(owing, asked_and_replied, **{qfile: confirmed + "\n## Notes\n"}), [chooses("Looks correct")])
    # The question it answers: none, of another file, of another stage, asked plainly, answered already.
    check_("answer/summary_no_question", created(owing, CREATED_AUDIT + H(2), **{qfile: confirmed}), [chooses("Looks correct")])
    check_("answer/summary_question_of_another_file", created(owing, CREATED_AUDIT + sq(1, file=qfile.replace("intent-capture-questions", "other-questions")) + H(2), **{qfile: confirmed}), [chooses("Looks correct")])
    check_("answer/summary_question_of_another_stage", created(owing, CREATED_AUDIT + sq(1).replace("**Stage**: intent-capture", "**Stage**: feasibility") + H(2), **{qfile: confirmed}), [chooses("Looks correct")])
    check_("answer/summary_question_asked_plainly", created(owing, CREATED_AUDIT + summary_q + H(2), **{qfile: confirmed}), [chooses("Looks correct")])
    check_("answer/summary_question_of_a_unit_or_a_single_stage", created(owing, CREATED_AUDIT + sq(1, "**Unit**: api") + sq(2, "**Workflow**: single-stage:intent-capture") + H(3), **{qfile: confirmed}),
           [chooses("Looks correct")])
    check_("answer/summary_already_answered", created(owing, CREATED_AUDIT + sq(1) + H(2) + block("SUMMARY_CONFIRMATION_RECORDED", T(3), "**Stage**: intent-capture", "**Details**: Looks correct",
                                                      "**Checkpoint**: Consolidated Summary Confirmation", f"**Questions File**: {qfile}") + H(4), **{qfile: confirmed}), [chooses("Looks correct")])
    check_("answer/summary_asked_again_after_an_answer", created(owing, CREATED_AUDIT + sq(1) + H(2) + block("SUMMARY_CONFIRMATION_RECORDED", T(3), "**Stage**: intent-capture", "**Details**: Request changes",
                                                             "**Checkpoint**: Consolidated Summary Confirmation", f"**Questions File**: {qfile}") + sq(4) + H(5), **{qfile: confirmed}), [chooses("Looks correct")])
    # The person's reply: none, before the question was recorded (said back), a command, spent by an answer.
    check_("answer/summary_no_reply", created(owing, CREATED_AUDIT + sq(1), **{qfile: confirmed}), [chooses("Looks correct")])
    check_("answer/summary_no_reply_guard_off", created(owing, CREATED_AUDIT + sq(1), **{qfile: confirmed}), [chooses("Looks correct")], {"SKIP_HUMAN_PRESENCE_GUARD": "1"})
    check_("answer/summary_no_reply_unattended", created(owing, CREATED_AUDIT + sq(1), **{qfile: confirmed}), [chooses("Looks correct")], {"UNATTENDED": "1"})
    check_("answer/summary_reply_before_the_question", created(owing, CREATED_AUDIT + H(1) + sq(2), **{qfile: confirmed}), [chooses("Looks correct")])
    check_("answer/summary_reply_before_the_question_with_words", created(owing, CREATED_AUDIT + H(1) + sq(2), **{qfile: confirmed}, **kept(CREATED_AUDIT, "looks  right\nto me", at=1)), [chooses("Looks correct")])
    check_("answer/summary_reply_with_words", created(owing, asked_and_replied, **{qfile: confirmed}, **kept(CREATED_AUDIT + sq(1), "looks right to me")), [chooses("Looks correct")])
    check_("answer/summary_reply_was_a_command", created(owing, CREATED_AUDIT + sq(1) + H(2, "**Reply**: command"), **{qfile: confirmed}), [chooses("Looks correct")])
    check_("answer/summary_reply_spent", created(owing, CREATED_AUDIT + sq(1) + H(2) + A(3, "Postgres"), **{qfile: confirmed}), [chooses("Looks correct")])
    check_("answer/summary_self_attributed", created(owing, asked_and_replied, **{qfile: confirmed}), [chooses("Looks correct - chosen by the assistant")])
    # Two shards at one instant: nothing says which came first.
    check_("answer/summary_question_tied_with_the_reply", created(owing, CREATED_AUDIT + H(2), **{qfile: confirmed}, **other(sq(2))), [chooses("Looks correct")])
    check_("answer/summary_question_and_answer_tied", created(owing, CREATED_AUDIT + sq(2) + H(3), **{qfile: confirmed}, **other(block(
        "SUMMARY_CONFIRMATION_RECORDED", T(2), "**Stage**: intent-capture", "**Checkpoint**: Consolidated Summary Confirmation", f"**Questions File**: {qfile}"))), [chooses("Looks correct")])
    check_("answer/summary_question_in_another_shard", created(owing, CREATED_AUDIT + H(3), **{qfile: confirmed}, **other(sq(2))), [chooses("Looks correct")])
    # A stage run by itself: its last completion is the boundary, of the question and of the attempt.
    done = lambda minute: block("STAGE_COMPLETED", T(minute), "**Stage**: intent-capture", "**Workflow**: single-stage:intent-capture")
    alone = lambda minute: sq(minute, "**Workflow**: single-stage:intent-capture")
    check_("answer/summary_single", created(owing, CREATED_AUDIT + alone(1) + H(2), **{qfile: confirmed}), [chooses("Looks correct", "--single"), chooses("Looks correct")])
    check_("answer/summary_single_after_a_run", created(owing, CREATED_AUDIT + done(1) + alone(2) + H(3), **{qfile: confirmed}), [chooses("Looks correct", "--single")])
    check_("answer/summary_single_question_of_the_run_before", created(owing, CREATED_AUDIT + alone(1) + done(2) + H(3), **{qfile: confirmed}), [chooses("Looks correct", "--single")])
    check_("answer/summary_single_run_tied_with_the_question", created(owing, CREATED_AUDIT + alone(2) + H(3), **{qfile: confirmed}, **other(done(2))), [chooses("Looks correct", "--single")])
    # The attempt the authorization binds to: the newest of the creation, a jump that reaches the stage, a start of it.
    jumped = lambda minute, target: block("STAGE_JUMPED", T(minute), f"**Target**: {target}", "**Direction**: backward")
    started = lambda minute, stage="intent-capture": block("STAGE_STARTED", T(minute), f"**Stage**: {stage}")
    for label, history in (("started", started(1)), ("started_twice", started(1) + started(2)), ("another_stage_started", started(1, "feasibility")),
                           ("jumped_to_it", jumped(1, "intent-capture")), ("jumped_before_it", jumped(1, "workspace-detection")), ("jumped_after_it", jumped(1, "feasibility")),
                           ("jumped_nowhere_known", jumped(1, "nope")), ("jumped_with_no_target", block("STAGE_JUMPED", T(1), "**Direction**: backward")),
                           ("single_stage_rows_apart", block("STAGE_STARTED", T(1), "**Stage**: intent-capture", "**Workflow**: single-stage:intent-capture")),
                           ("no_boundary", None)):
        audit = (TITLE if history is None else CREATED_AUDIT + history) + sq(3) + H(4)
        check_(f"answer/summary_attempt_{label}", created(owing, audit, **{qfile: confirmed}), [chooses("Looks correct")])
    check_("answer/summary_attempt_two_shards", created(owing, CREATED_AUDIT + sq(3) + H(4), **{qfile: confirmed}, **other(started(0) + started(1))), [chooses("Looks correct")])
    check_("answer/summary_attempt_two_shards_tied", created(owing, CREATED_AUDIT + started(1) + sq(3) + H(4), **{qfile: confirmed}, **other(started(1))), [chooses("Looks correct")])
    # What the sweep left unpinned.
    answered_row = lambda minute: block("SUMMARY_CONFIRMATION_RECORDED", T(minute), "**Stage**: intent-capture", "**Details**: Looks correct", "**Checkpoint**: Consolidated Summary Confirmation",
                                        f"**Questions File**: {qfile}")
    # A turn that was only a command is no reply after the question: the reply before it is said back.
    check_("answer/summary_reply_before_then_a_command", created(owing, CREATED_AUDIT + H(1) + sq(2) + H(3, "**Reply**: command"), **{qfile: confirmed}), [chooses("Looks correct")])
    # The completion of another stage, or of this one in the ordinary walk, is no boundary of a question.
    check_("answer/summary_another_stage_completed_since", created(owing, CREATED_AUDIT + sq(1) + block("STAGE_COMPLETED", T(2), "**Stage**: feasibility") + H(3), **{qfile: confirmed}), [chooses("Looks correct")])
    check_("answer/summary_single_other_completions_since", created(owing, CREATED_AUDIT + done(1) + alone(2) + block("STAGE_COMPLETED", T(3), "**Stage**: intent-capture")
                                                                 + block("STAGE_COMPLETED", T(3), "**Stage**: feasibility", "**Workflow**: single-stage:feasibility") + H(4), **{qfile: confirmed}),
           [chooses("Looks correct", "--single")])
    # A question of another checkpoint that names the same file is not the summary's.
    check_("answer/summary_another_checkpoint_names_the_file", created(owing, CREATED_AUDIT + sq(1) + H(2) + answered_row(3) + Q(4, "Approve the plan?", "**Checkpoint**: Plan Approval", f"**Questions File**: {qfile}") + H(5),
                                                                       **{qfile: confirmed}), [chooses("Looks correct")])
    # An older run that ties with the question in another shard does not count once a newer run is the boundary.
    check_("answer/summary_single_older_run_tied", created(owing, CREATED_AUDIT + done(2) + alone(1) + H(3), **{qfile: confirmed}, **other(done(1))), [chooses("Looks correct", "--single")])
    # A question that ties with the run's end is left behind by a newer one.
    check_("answer/summary_single_tied_question_then_a_newer_one", created(owing, CREATED_AUDIT + alone(1) + alone(2) + H(3), **{qfile: confirmed}, **other(done(1))), [chooses("Looks correct", "--single")])
    # The question asked twice at one instant in two shards, and a reply that is before one of them.
    check_("answer/summary_reply_before_one_of_two_questions", created(owing, CREATED_AUDIT + H(2) + sq(2), **{qfile: confirmed}, **other(sq(2))), [chooses("Looks correct")])
    # The hint of a reply not given yet says nothing of one the hooks missed.
    health = f"{CREATED_DIR}/.aidlc-engine/hooks-health/record-human-turn.last"
    check_("answer/summary_no_reply_hooks_ran_long_ago", created(owing, CREATED_AUDIT + sq(1), **{qfile: confirmed, health: T(0) + "\n"}), [chooses("Looks correct"), reply])
    check_("answer/summary_no_reply_hooks_ran_just_now", created(owing, CREATED_AUDIT + sq(1), **{qfile: confirmed, health: STARTED + "\n"}), [chooses("Looks correct")])
    # (A heartbeat is recent against the clock: the replay runs later.)
    UNRECORDED.add("log/answer/summary_no_reply_hooks_ran_just_now")
    # The last turn's words were a dismissed box: they are not the person's reply.
    check_("answer/summary_reply_words_dismissed", created(owing, asked_and_replied, **{qfile: confirmed}, **kept(CREATED_AUDIT + sq(1), "Cancelled")), [chooses("Looks correct")])
    check_("answer/summary_reply_before_the_question_words_dismissed", created(owing, CREATED_AUDIT + H(1) + sq(2), **{qfile: confirmed}, **kept(CREATED_AUDIT, "Cancelled", at=1)), [chooses("Looks correct")])
    # The attempt: the creation alone, a creation newer than a start, two boundaries at one instant in two shards.
    creation = lambda minute: block("WORKFLOW_STARTED", T(minute), "**Scope**: feature", "**Request**: Add login")
    check_("answer/summary_attempt_created_only", created(owing, TITLE + creation(1) + sq(3) + H(4), **{qfile: confirmed}), [chooses("Looks correct")])
    check_("answer/summary_attempt_created_after_a_start", created(owing, TITLE + started(0) + creation(1) + sq(3) + H(4), **{qfile: confirmed}), [chooses("Looks correct")])
    check_("answer/summary_attempt_two_kinds_tied", created(owing, TITLE + started(1) + sq(3) + H(4), **{qfile: confirmed}, **other(creation(1))), [chooses("Looks correct")])
    check_("answer/summary_single_attempt", created(owing, CREATED_AUDIT + done(1) + block("STAGE_COMPLETED", T(2), "**Stage**: intent-capture")
                                                    + block("STAGE_COMPLETED", T(2), "**Stage**: feasibility", "**Workflow**: single-stage:intent-capture") + alone(3) + H(4), **{qfile: confirmed}),
           [chooses("Looks correct", "--single")])
    # A stage whose name is none: there is nowhere to keep its authorization.
    check_("answer/summary_stage_name_is_none", created(owing, CREATED_AUDIT + sq(1).replace("**Stage**: intent-capture", "**Stage**: Bad_Stage") + H(2), **{qfile: confirmed}),
           [["answer", "--stage", "Bad_Stage", "--details", "Looks correct", "--checkpoint", "summary-confirmation", "--questions-file", qfile]])
    check_("answer/summary_registry_too_large", created(owing, asked_and_replied, **{qfile: confirmed, registry: "x" * 65537}), [chooses("Looks correct")])
    check_("answer/summary_registry_at_the_limit", created(owing, asked_and_replied, **{qfile: confirmed, registry: "x" * 65536}), [chooses("Looks correct")])
    check_("answer/summary_observer_changes_asked", created(owing, asked_and_replied, **{qfile: changed}), [chooses("Request changes")], {"STOP_HOOK_PROBE": "1"})
    # The registry: a link on the way, a folder where the file goes, an observer.
    engine = f"{CREATED_DIR}/.aidlc-engine"
    check_("answer/summary_registry_a_link", created(owing, asked_and_replied, **{qfile: confirmed, "elsewhere/keep": ""}), [chooses("Looks correct")],
           edits=[lambda tree: (os.makedirs(tree.path(engine), exist_ok=True), os.symlink(tree.path("elsewhere"), tree.path(f"{engine}/summary-authorization")))])
    check_("answer/summary_registry_file_a_link", created(owing, asked_and_replied, **{qfile: confirmed, "elsewhere.json": "{}\n"}), [chooses("Looks correct")],
           edits=[lambda tree: (os.makedirs(os.path.dirname(tree.path(registry)), exist_ok=True), os.symlink(tree.path("elsewhere.json"), tree.path(registry)))])
    check_("answer/summary_registry_a_folder_where_the_file_goes", created(owing, asked_and_replied, **{qfile: confirmed, f"{registry}/x": "x"}), [chooses("Looks correct")])
    check_("answer/summary_registry_a_file_where_the_folder_goes", created(owing, asked_and_replied, **{qfile: changed, f"{engine}/summary-authorization": "x"}), [chooses("Request changes")])
    check_("answer/summary_observer", created(owing, asked_and_replied, **{qfile: confirmed}), [chooses("Looks correct")], {"STOP_HOOK_PROBE": "1"})
    check_("answer/summary_per_unit_not_ported", created(at_stage(owing, "functional-design", done=["intent-capture"], phase="CONSTRUCTION")),
           [["answer", "--stage", "functional-design", "--details", "Looks correct", "--checkpoint", "summary-confirmation", "--questions-file", "x-questions.md"]])
    # --- link: a link of a pipeline stage returned ---
    # Reverse Engineering is the pipeline: the developer scans, then the architect. A created record has the
    # collaborators off, so its chain is the developer alone.
    lead_only = at_stage(CREATED_STATE.replace("- [ ] reverse-engineering \u2014 SKIP", "- [ ] reverse-engineering \u2014 EXECUTE"), "reverse-engineering",
                         done=["intent-capture"], phase="INCEPTION")
    chained = lead_only.replace("- **Collaborators**: off (from scope feature)", "- **Collaborators**: on")
    dev, arch = "aidlc-developer-agent", "aidlc-architect-agent"
    scan_dir = f"{CREATED_DIR}/inception/reverse-engineering"
    scan, scan_app = f"{scan_dir}/developer-scan.md", f"{scan_dir}/developer-scan-app.md"
    returned = lambda agent, *more: ["link", "--stage", "reverse-engineering", "--link", agent, *more]
    scanned = lambda *more, file=scan: returned(dev, "--artifact", file, *more)
    # Times are whole hundreds of nanoseconds: what every filesystem the replay runs on keeps.
    WRITTEN = 1767610800123456700  # 2026-01-05T11:00:00.1234567Z
    at_time = lambda file=scan, ns=WRITTEN: lambda tree: os.utime(tree.path(file), ns=(ns, ns))
    SCAN = "# Developer scan\n\nWhat the code is.\n"
    scan_sha = "sha256:" + hashlib.sha256(SCAN.encode()).hexdigest()
    receipt = lambda minute, agent=dev, position="1/2", *more: block("PIPELINE_LINK_COMPLETED", T(minute), "**Stage**: reverse-engineering", f"**Link**: {agent}", f"**Position**: {position}", *more)
    scan_receipt = lambda minute, file=scan, sha=scan_sha, mtime="1767610800123.4567", *more: receipt(
        minute, dev, "1/2", f"**Artifact Path**: {file}", f"**Artifact SHA256**: {sha}", f"**Artifact Mtime Ms**: {mtime}", *more)
    check_("link/usage", created(chained), [["link"], ["link", "--stage", "reverse-engineering"], ["link", "--link", dev], ["link", "--stage", "", "--link", dev], returned(""),
                                              returned(dev, "--intent", "x"), returned(dev, "--space", "x"), returned(dev, "--intent", "")])
    check_("link/not_a_pipeline", created(chained), [["link", "--stage", "intent-capture", "--link", dev], ["link", "--stage", "nope", "--link", dev]])
    check_("link/not_in_the_chain", created(chained), [returned("aidlc-product-agent"), returned("nope"), returned(dev.upper())])
    check_("link/the_chain", created(chained, **{scan: SCAN}), [scanned(), scanned(), returned(arch), returned(arch), returned(dev)], edits=[at_time()])
    check_("link/out_of_order", created(chained, **{scan: SCAN}), [returned(arch), scanned(), returned(arch)], edits=[at_time()])
    check_("link/lead_alone", created(lead_only, **{scan: SCAN}), [returned(arch), scanned(), scanned()], edits=[at_time()])
    check_("link/collaborators_off_by_the_switch", created(chained, **{scan: SCAN}), [scanned(), returned(arch)], {"DISABLE_COLLABORATORS": "1"}, edits=[at_time()])
    check_("link/not_the_stage_in_progress", created(**{scan: SCAN}), [scanned(), returned(arch)], edits=[at_time()])
    check_("link/collaborators_not_said", created(chained.replace("- **Collaborators**: on\n", ""), **{scan: SCAN}), [scanned(), returned(arch)], edits=[at_time()])
    for label, said in (("another_word", "maybe"), ("in_capitals", "ON"), ("blank", ""), ("on_and_why", "on (set by you)"), ("off", "off")):
        check_(f"link/collaborators_{label}", created(chained.replace("- **Collaborators**: on", f"- **Collaborators**: {said}"), **{scan: SCAN}), [scanned(), returned(arch)], edits=[at_time()])
    unsaid = chained.replace("- **Collaborators**: on\n", "")
    for label, scope in (("in_capitals", "FEATURE"), ("unknown", "nope"), ("with_collaborators", "brownfield")):
        check_(f"link/collaborators_not_said_scope_{label}", created(unsaid.replace("- **Scope**: feature", f"- **Scope**: {scope}"), **{scan: SCAN}), [scanned(), returned(arch)], edits=[at_time()])
    check_("link/second_names_a_file", created(chained, **{scan: SCAN}), [scanned("--nope", "x"), returned(arch, "--artifact", "nope.md"), returned(dev, "--nope", "x")], edits=[at_time()])
    check_("link/flag_twice", created(chained, **{scan: SCAN}), [returned(arch, "--link", dev, "--artifact", "nope.md", "--artifact", scan, "--stage", "reverse-engineering")], edits=[at_time()])
    # A state that cannot be read: the chain is the one the stage declares.
    check_("link/state_a_folder", created(chained, **{f"{CREATED_DIR}/aidlc-state.md": None, f"{CREATED_DIR}/aidlc-state.md/x": "x", scan: SCAN}), [scanned(), returned(arch)], edits=[at_time()])
    check_("link/state_not_text", created(chained, **{scan: SCAN}), [scanned(), returned(arch)],
           edits=[at_time(), lambda tree: open(tree.path(f"{CREATED_DIR}/aidlc-state.md"), "wb").write(b"- **Collaborators**: off\n\xff\xfe\n")])
    # A chain of three, and a pipeline that is not Reverse Engineering: the graph says who is in it.
    def chain_of(slug, *supports):
        def edit(tree):
            graph = tree.json(".claude/tools/data/stage-graph.json")
            for stage in graph:
                if stage["slug"] == slug:
                    stage["mode"] = "pipeline"
                    stage["support_agents"] = [f"{stage['lead_agent'].split('-')[0]}-{name}-agent" for name in supports]
            tree.write_json(".claude/tools/data/stage-graph.json", graph)
        return edit
    quality = "aidlc-quality-agent"
    three = chain_of("reverse-engineering", "architect", "quality")
    check_("link/three_links", created(chained, **{scan: SCAN}), [returned(quality), scanned(), returned(quality), returned(arch), returned(arch), returned(quality), returned(quality), scanned()],
           edits=[at_time(), three])
    check_("link/three_links_the_middle_one_missing", created(chained, CREATED_AUDIT + scan_receipt(1).replace("1/2", "1/3") + receipt(2, quality, "3/3"), **{scan: SCAN}),
           [returned(quality), returned(arch), returned(quality)], edits=[at_time(), three])
    check_("link/three_links_receipts_of_two", created(chained, CREATED_AUDIT + scan_receipt(1) + receipt(2, arch, "2/2"), **{scan: SCAN}), [returned(arch), returned(quality)], edits=[at_time(), three])
    check_("link/three_links_the_last_of_a_longer_chain", created(chained, CREATED_AUDIT + scan_receipt(1) + receipt(2, arch, "2/4") + receipt(3, quality, "3/4"), **{scan: SCAN}),
           [returned(quality), returned(arch)], edits=[at_time(), three])
    check_("link/another_pipeline", created(chained), [["link", "--stage", "intent-capture", "--link", arch], ["link", "--stage", "intent-capture", "--link", "aidlc-product-agent", "--artifact", "nope.md"],
                                                       ["link", "--stage", "intent-capture", "--link", "aidlc-product-agent"], ["link", "--stage", "intent-capture", "--link", arch],
                                                       ["link", "--stage", "intent-capture", "--link", dev]], edits=[chain_of("intent-capture", "architect")])
    check_("link/a_pipeline_with_no_support", created(chained, **{scan: SCAN}), [scanned(), returned(arch)], edits=[at_time(), chain_of("reverse-engineering")])
    check_("link/no_state_file", created(audit=CREATED_AUDIT, **{f"{CREATED_DIR}/aidlc-state.md": None, scan: SCAN}), [scanned()], edits=[at_time()])
    # The developer's handoff: named, the stage's own, there, a plain file, written in this attempt.
    elsewhere = {scan: SCAN, f"{scan_dir}/other.md": SCAN, "developer-scan.md": SCAN, scan_app: SCAN, f"{CREATED_DIR}/inception/developer-scan.md": SCAN}
    check_("link/handoff_not_named", created(chained, **{scan: SCAN}), [returned(dev), returned(dev, "--artifact", "")])
    check_("link/handoff_not_the_stages", created(chained, **elsewhere), [scanned(file=name) for name in (
        f"{scan_dir}/other.md", "developer-scan.md", scan_app, f"{CREATED_DIR}/inception/developer-scan.md", scan_dir, "../x.md")])
    check_("link/handoff_named_in_full", created(chained, **{scan: SCAN}), [scanned(file="{P}/" + scan)], edits=[at_time()])
    check_("link/handoff_named_through_steps", created(chained, **{scan: SCAN}), [scanned(file=f"./{scan_dir}/../reverse-engineering/developer-scan.md")], edits=[at_time()])
    check_("link/handoff_missing", created(chained), [scanned()])
    check_("link/handoff_a_folder", created(chained, **{f"{scan}/x": "x"}), [scanned()])
    check_("link/handoff_a_link", created(chained, **{"elsewhere.md": SCAN}), [scanned()],
           edits=[lambda tree: (os.makedirs(tree.path(scan_dir), exist_ok=True), os.symlink(tree.path("elsewhere.md"), tree.path(scan)))])
    check_("link/handoff_behind_a_link", created(chained, **{"elsewhere/developer-scan.md": SCAN}), [scanned()],
           edits=[lambda tree: (os.makedirs(os.path.dirname(tree.path(scan_dir)), exist_ok=True), os.symlink(tree.path("elsewhere"), tree.path(scan_dir)))])
    # A second name is a link no recording carries, the project's own name is the replay's own, and a state that is
    # not text is no text a recording holds: compared live.
    UNRECORDED.update({"log/link/handoff_a_second_name", "log/link/root_handoff_by_its_name", "log/link/state_not_text"})
    check_("link/handoff_a_second_name", created(chained, **{scan: SCAN}), [scanned()], edits=[lambda tree: os.link(tree.path(scan), tree.path("other-name.md"))])
    check_("link/handoff_a_link_to_nothing", created(chained), [scanned()],
           edits=[lambda tree: (os.makedirs(tree.path(scan_dir), exist_ok=True), os.symlink(tree.path("nowhere.md"), tree.path(scan)))])
    check_("link/handoff_under_a_file", created(chained, **{scan_dir: "x"}), [scanned()])
    check_("link/handoff_written_before_1970", created(chained, TITLE + block("WORKFLOW_STARTED", "1969-12-31T23:59:59Z", "**Scope**: feature"), **{scan: SCAN}), [scanned()],
           edits=[at_time(ns=-2 * 10**9)])
    check_("link/handoff_written_before_a_start_before_1970", created(chained, TITLE + block("WORKFLOW_STARTED", "1969-12-31T23:59:59Z", "**Scope**: feature"), **{scan: SCAN}), [scanned()],
           edits=[at_time(ns=-5 * 10**8)])
    check_("link/handoff_empty", created(chained, **{scan: ""}), [scanned()], edits=[at_time()])
    # When it was written: before the attempt, at its very start, in whole seconds, with a start that is no instant.
    START = 1767607200000000000  # 2026-01-05T10:00:00Z, when the work was created
    for label, ns in (("before_the_attempt", START - 100), ("at_the_attempts_start", START), ("a_millisecond_before", START - 1000000), ("whole_seconds", START + 60 * 10**9),
                      ("half_a_millisecond", START + 500000), ("long_ago", 100)):
        check_(f"link/handoff_written_{label}", created(chained, **{scan: SCAN}), [scanned()], edits=[at_time(ns=ns)])
    check_("link/handoff_written_before_the_stage_started", created(chained, CREATED_AUDIT + block("STAGE_STARTED", T(5), "**Stage**: reverse-engineering"), **{scan: SCAN}), [scanned()],
           edits=[at_time(ns=START + 60 * 10**9)])
    check_("link/handoff_no_attempt", created(chained, TITLE, **{scan: SCAN}), [scanned()], edits=[at_time()])
    check_("link/handoff_start_is_no_instant", created(chained, TITLE + block("WORKFLOW_STARTED", "soon", "**Scope**: feature"), **{scan: SCAN}), [scanned()], edits=[at_time()])
    # An earlier receipt of the link: the file has to have been rewritten since, in any attempt.
    restarted = block("STAGE_STARTED", T(5), "**Stage**: reverse-engineering")
    check_("link/handoff_not_rewritten", created(chained, CREATED_AUDIT + scan_receipt(1) + restarted, **{scan: SCAN}), [scanned()], edits=[at_time()])
    check_("link/handoff_rewritten", created(chained, CREATED_AUDIT + scan_receipt(1, scan, scan_sha, "1767610800123.4565") + restarted, **{scan: SCAN}), [scanned()], edits=[at_time()])
    check_("link/handoff_older_than_a_receipt", created(chained, CREATED_AUDIT + scan_receipt(1, scan, scan_sha, "1767610800124") + scan_receipt(2, scan, scan_sha, "17") + restarted, **{scan: SCAN}),
           [scanned()], edits=[at_time()])
    check_("link/handoff_receipt_time_is_no_number", created(chained, CREATED_AUDIT + scan_receipt(1, scan, scan_sha, "1e99") + scan_receipt(2, scan, scan_sha, "") + restarted, **{scan: SCAN}),
           [scanned()], edits=[at_time()])
    # The receipts of this attempt: the developer's counts while its file is the one it recorded, unless the
    # record's guard policy accepts a changed file (relaxed, off).
    off = "- **Guard Policy**: off (from scope feature)"
    policies = (("", chained), ("_strict", chained.replace(off, "- **Guard Policy**: strict (set by you)")))
    relaxed = chained.replace(off, "- **Guard Policy**: relaxed (set by you)")
    for suffix, state in policies:
        check_(f"link/second_after_a_recorded_first{suffix}", created(state, CREATED_AUDIT + scan_receipt(1), **{scan: SCAN}), [returned(arch), scanned()], edits=[at_time()])
        check_(f"link/first_receipt_file_changed{suffix}", created(state, CREATED_AUDIT + scan_receipt(1), **{scan: SCAN + "more\n"}), [returned(arch)], edits=[at_time()])
        check_(f"link/first_receipt_file_touched{suffix}", created(state, CREATED_AUDIT + scan_receipt(1), **{scan: SCAN}), [returned(arch)], edits=[at_time(ns=WRITTEN + 20000)])
        check_(f"link/first_receipt_file_touched_a_little{suffix}", created(state, CREATED_AUDIT + scan_receipt(1), **{scan: SCAN}), [returned(arch)], edits=[at_time(ns=WRITTEN + 5000)])
        check_(f"link/first_receipt_file_touched_back{suffix}", created(state, CREATED_AUDIT + scan_receipt(1), **{scan: SCAN}), [returned(arch)], edits=[at_time(ns=WRITTEN - 20000)])
        check_(f"link/first_receipt_file_gone{suffix}", created(state, CREATED_AUDIT + scan_receipt(1)), [returned(arch)])
        check_(f"link/first_receipt_file_a_folder{suffix}", created(state, CREATED_AUDIT + scan_receipt(1), **{f"{scan}/x": "x"}), [returned(arch)])
        for label, row in (("no_file", receipt(1)), ("no_hash", receipt(1, dev, "1/2", f"**Artifact Path**: {scan}", "**Artifact Mtime Ms**: 1767610800123.4567")),
                           ("hash_not_one", scan_receipt(1, scan, "sha256:XYZ")), ("hash_in_capitals", scan_receipt(1, scan, scan_sha.upper())), ("hash_bare", scan_receipt(1, scan, scan_sha[7:])),
                           ("no_time", receipt(1, dev, "1/2", f"**Artifact Path**: {scan}", f"**Artifact SHA256**: {scan_sha}")),
                           ("time_not_a_number", scan_receipt(1, scan, scan_sha, "soon")), ("time_with_words_after", scan_receipt(1, scan, scan_sha, "1767610800123.4567ms")),
                           ("time_in_another_notation", scan_receipt(1, scan, scan_sha, "1.7676108001234567e12")), ("time_blank", scan_receipt(1, scan, scan_sha, "")),
                           ("file_outside_the_record", scan_receipt(1, "developer-scan.md")), ("file_is_the_record", scan_receipt(1, CREATED_DIR)),
                           ("file_named_through_steps", scan_receipt(1, f"{scan_dir}/./developer-scan.md")),
                           ("position_of_another_chain", scan_receipt(1).replace("1/2", "1/3")), ("no_position", scan_receipt(1).replace("**Position**: 1/2\n", "")),
                           ("no_link", scan_receipt(1).replace(f"**Link**: {dev}\n", "")), ("of_another_stage", scan_receipt(1).replace("reverse-engineering", "intent-capture", 1)),
                           ("of_a_stage_run_alone", scan_receipt(1, scan, scan_sha, "1767610800123.4567", "**Workflow**: single-stage:reverse-engineering")),
                           ("of_another_stage_run_alone", scan_receipt(1, scan, scan_sha, "1767610800123.4567", "**Workflow**: single-stage:intent-capture")),
                           ("of_a_workflow_by_another_name", scan_receipt(1, scan, scan_sha, "1767610800123.4567", "**Workflow**: main")),
                           ("of_a_repo", scan_receipt(1, scan, scan_sha, "1767610800123.4567", "**Repo**: app")), ("of_a_blank_repo", scan_receipt(1, scan, scan_sha, "1767610800123.4567", "**Repo**: "))):
            check_(f"link/first_receipt_{label}{suffix}", created(state, CREATED_AUDIT + row, **{scan: SCAN, "developer-scan.md": SCAN}), [returned(arch)], edits=[at_time()])
    check_("link/first_receipt_file_changed_relaxed", created(relaxed, CREATED_AUDIT + scan_receipt(1), **{scan: SCAN + "more\n"}), [returned(arch)], edits=[at_time()])
    check_("link/first_receipt_file_a_link", created(policies[0][1], CREATED_AUDIT + scan_receipt(1), **{"elsewhere.md": SCAN}), [returned(arch)],
           edits=[lambda tree: (os.makedirs(tree.path(scan_dir), exist_ok=True), os.symlink(tree.path("elsewhere.md"), tree.path(scan)))])
    check_("link/last_receipt_of_another_chain", created(chained, CREATED_AUDIT + scan_receipt(1) + receipt(2, arch, "2/3"), **{scan: SCAN}), [returned(arch)], edits=[at_time()])
    check_("link/last_receipt_with_no_position", created(chained, CREATED_AUDIT + scan_receipt(1) + receipt(2, arch, "2/2").replace("**Position**: 2/2\n", ""), **{scan: SCAN}), [returned(arch)],
           edits=[at_time()])
    check_("link/first_again_starts_the_chain_again", created(chained, CREATED_AUDIT + scan_receipt(1) + receipt(2, arch, "2/2") + scan_receipt(3), **{scan: SCAN}), [returned(arch), returned(arch)],
           edits=[at_time()])
    check_("link/second_before_the_first", created(chained, CREATED_AUDIT + receipt(1, arch, "2/2") + scan_receipt(2), **{scan: SCAN}), [returned(arch)], edits=[at_time()])
    check_("link/second_twice", created(chained, CREATED_AUDIT + scan_receipt(1) + receipt(2, arch, "2/2") + receipt(3, arch, "2/2"), **{scan: SCAN}), [returned(arch), scanned()], edits=[at_time()])
    # A chain that is the developer alone: its receipt is the last one, and counts as the last of a chain this long.
    check_("link/lead_alone_receipt_of_a_longer_chain", created(lead_only, CREATED_AUDIT + scan_receipt(1), **{scan: SCAN}), [scanned()], edits=[at_time(ns=WRITTEN + 10**12)])
    check_("link/lead_alone_receipt_of_its_own_chain", created(lead_only, CREATED_AUDIT + scan_receipt(1).replace("1/2", "1/1"), **{scan: SCAN}), [scanned()], edits=[at_time()])
    # A row that says the stage's work was kept: nothing a link reads.
    kept_row = lambda *more: block("ARTIFACT_REUSED", T(2), "**Stage**: reverse-engineering", "**Decision**: keep", *more)
    alone = "**Workflow**: single-stage:reverse-engineering"
    store = "aidlc/spaces/default/codekb"
    check_("link/work_kept", created(chained, CREATED_AUDIT + kept_row(), **{scan: SCAN}), [returned(arch), scanned(), returned(arch)], edits=[at_time()])
    check_("link/work_kept_single", created(chained, CREATED_AUDIT + block("STAGE_STARTED", T(1), "**Stage**: reverse-engineering", alone, "**Scope**: brownfield") + kept_row(alone) + kept_row(alone, "**Repo**: app"),
                                            **{scan: SCAN, f"{store}/app/reverse-engineering-timestamp.md": "not a scope\n", f"{store}/reverse-engineering-timestamp.md/x": "x"}),
           [returned(arch, "--single"), scanned("--single"), returned(arch, "--single")], edits=[at_time()])
    # What starts the attempt again: a start of the stage, a jump that reaches it, a rejection of its gate (which
    # keeps the receipts where the policy accepts changes); and what does not.
    rejected = block("GATE_REJECTED", T(5), "**Stage**: reverse-engineering", "**Feedback**: again")
    for suffix, state in policies:
        for label, boundary in (("restarted", restarted), ("jumped_to", block("STAGE_JUMPED", T(5), "**Target**: reverse-engineering")), ("jumped_before", block("STAGE_JUMPED", T(5), "**Target**: intent-capture")),
                                ("jumped_after", block("STAGE_JUMPED", T(5), "**Target**: requirements-analysis")), ("jumped_nowhere", block("STAGE_JUMPED", T(5), "**Target**: nope")),
                                ("jumped_no_target", block("STAGE_JUMPED", T(5))), ("rejected", rejected), ("another_stage_rejected", rejected.replace("reverse-engineering", "intent-capture")),
                                ("created_again", block("WORKFLOW_STARTED", T(5), "**Scope**: feature")), ("another_stage_started", block("STAGE_STARTED", T(5), "**Stage**: intent-capture")),
                                ("completed", block("STAGE_COMPLETED", T(5), "**Stage**: reverse-engineering")), ("approved", block("GATE_APPROVED", T(5), "**Stage**: reverse-engineering")),
                                ("started_alone", block("STAGE_STARTED", T(5), "**Stage**: reverse-engineering", alone)),
                                ("rejected_alone", rejected.replace("**Feedback**", alone + "\n**Feedback**"))):
            check_(f"link/attempt_{label}{suffix}", created(state, CREATED_AUDIT + scan_receipt(1) + boundary, **{scan: SCAN}), [returned(arch), scanned()], edits=[at_time()])
            # The same with the scan written again since.
            check_(f"link/attempt_{label}_rewritten{suffix}", created(state, CREATED_AUDIT + scan_receipt(1) + boundary, **{scan: SCAN}), [scanned(), returned(arch)], edits=[at_time(ns=WRITTEN + 10**12)])
    # The attempt a handoff has to be written in starts at a rejection whatever the policy keeps.
    for suffix, state in policies:
        check_(f"link/handoff_written_before_a_rejection{suffix}", created(state, CREATED_AUDIT + rejected, **{scan: SCAN}), [scanned()], edits=[at_time(ns=START + 120 * 10**9)])
    # A receipt at the boundary's own instant: after it in the boundary's shard, unordered in another.
    check_("link/receipt_at_the_boundarys_instant", created(chained, CREATED_AUDIT + block("STAGE_STARTED", T(5), "**Stage**: reverse-engineering") + scan_receipt(5), **{scan: SCAN}), [returned(arch)], edits=[at_time()])
    check_("link/receipt_before_the_boundary_at_its_instant", created(chained, CREATED_AUDIT + scan_receipt(5) + block("STAGE_STARTED", T(5), "**Stage**: reverse-engineering"), **{scan: SCAN}), [returned(arch)],
           edits=[at_time()])
    check_("link/receipt_between_two_boundaries_at_one_instant", created(chained, CREATED_AUDIT + block("STAGE_STARTED", T(5), "**Stage**: reverse-engineering") + scan_receipt(5)
                                                                        + block("STAGE_STARTED", T(5), "**Stage**: reverse-engineering"), **{scan: SCAN}), [returned(arch)], edits=[at_time()])
    check_("link/receipt_after_two_boundaries_at_one_instant", created(chained, CREATED_AUDIT + block("STAGE_STARTED", T(5), "**Stage**: reverse-engineering")
                                                                      + block("STAGE_STARTED", T(5), "**Stage**: reverse-engineering") + scan_receipt(5), **{scan: SCAN}), [returned(arch)], edits=[at_time()])
    check_("link/receipt_tied_with_an_older_boundarys_shard", created(chained, CREATED_AUDIT + block("STAGE_STARTED", T(5), "**Stage**: reverse-engineering") + scan_receipt(5), **{scan: SCAN},
                                                                     **other(block("STAGE_STARTED", T(4), "**Stage**: reverse-engineering"))), [returned(arch)], edits=[at_time()])
    check_("link/receipt_tied_in_another_shard", created(chained, CREATED_AUDIT + block("STAGE_STARTED", T(5), "**Stage**: reverse-engineering"), **{scan: SCAN}, **other(scan_receipt(5))), [returned(arch)],
           edits=[at_time()])
    check_("link/boundary_in_two_shards", created(chained, CREATED_AUDIT + block("STAGE_STARTED", T(5), "**Stage**: reverse-engineering") + scan_receipt(5), **{scan: SCAN},
                                                **other(block("STAGE_STARTED", T(5), "**Stage**: reverse-engineering"))), [returned(arch)], edits=[at_time()])
    check_("link/receipt_in_another_shard_later", created(chained, CREATED_AUDIT + block("STAGE_STARTED", T(5), "**Stage**: reverse-engineering"), **{scan: SCAN}, **other(scan_receipt(6))), [returned(arch)],
           edits=[at_time()])
    # A piece of work over repositories: the chain runs once for each, and the handoff carries the repository's name.
    over_app = CREATED_REGISTRY.replace('"status": "in-flight"', '"status": "in-flight",\n    "repos": ["app", "web"]')
    check_("link/repos", created(chained, registry=over_app, **{scan_app: SCAN, scan: SCAN}), [scanned(), scanned("--repo", "nope", file=scan_app), scanned("--repo", "app"), scanned("--repo", "app", file=scan_app),
                                                                                          returned(arch, "--repo", "web"), returned(arch, "--repo", "app"), returned(arch, "--repo", "app"),
                                                                                          scanned("--repo", "app", file=scan_app), scanned("--repo", "")], edits=[at_time(scan_app)])
    check_("link/repo_where_none_is_registered", created(chained, **{scan: SCAN}), [scanned("--repo", "app"), scanned("--repo", "")], edits=[at_time()])
    for label, listed in NOT_NAMES:
        check_(f"link/repos_{label}", created(chained, registry=CREATED_REGISTRY.replace('"status": "in-flight"', '"status": "in-flight",\n    "repos": ' + listed), **{scan: SCAN, scan_app: SCAN}),
               [scanned(), scanned("--repo", "app", file=scan_app)], edits=[at_time(), at_time(scan_app)])
    # The project's own name stands for the root: as `--repo`, and in the handoff's name.
    def root_scan(tree):
        named = f"{scan_dir}/developer-scan-{os.path.basename(tree.root)}.md"
        tree.write(named, SCAN)
        os.utime(tree.path(named), ns=(WRITTEN, WRITTEN))
    check_("link/root_by_its_name", created(chained, **{scan: SCAN}), [scanned("--repo", "{N}"), returned(arch, "--repo", "{N}"), returned(arch)], edits=[at_time()])
    check_("link/root_handoff_by_its_name", created(chained), [scanned(file=f"{scan_dir}/developer-scan-{{N}}.md"), returned(arch)], edits=[root_scan])
    # A stage run by itself: its own starts are its attempts, its own receipts its chain, the scope its start recorded its ceremony.
    alone_started = lambda minute, *more: block("STAGE_STARTED", T(minute), "**Stage**: reverse-engineering", "**Workflow**: single-stage:reverse-engineering", *more)
    check_("link/single", created(chained, CREATED_AUDIT + alone_started(1, "**Scope**: brownfield"), **{scan: SCAN}), [scanned("--single"), scanned("--single"), returned(arch, "--single"), returned(arch), scanned()],
           edits=[at_time()])
    check_("link/single_not_started", created(chained, **{scan: SCAN}), [scanned("--single")], edits=[at_time()])
    check_("link/single_scope_with_collaborators_off", created(chained, CREATED_AUDIT + alone_started(1, "**Scope**: feature"), **{scan: SCAN}), [returned(arch, "--single"), scanned("--single")], edits=[at_time()])
    for label, said in (("in_capitals", "FEATURE"), ("unknown", "nope"), ("blank", "")):
        check_(f"link/single_scope_{label}", created(chained, CREATED_AUDIT + alone_started(1, f"**Scope**: {said}"), **{scan: SCAN}), [scanned("--single"), returned(arch, "--single")], edits=[at_time()])
    check_("link/single_start_of_another_stage", created(chained, CREATED_AUDIT + alone_started(1, "**Scope**: feature") + alone_started(2, "**Scope**: brownfield").replace("reverse-engineering", "intent-capture"), **{scan: SCAN}),
           [scanned("--single"), returned(arch, "--single")], edits=[at_time()])
    check_("link/single_start_naming_another_stage", created(chained, CREATED_AUDIT + alone_started(1, "**Scope**: feature")
                                                            + alone_started(2, "**Scope**: brownfield").replace("**Stage**: reverse-engineering", "**Stage**: intent-capture"), **{scan: SCAN}),
           [scanned("--single"), returned(arch, "--single")], edits=[at_time()])
    check_("link/single_start_in_the_workflow", created(chained, CREATED_AUDIT + alone_started(1, "**Scope**: feature") + block("STAGE_STARTED", T(2), "**Stage**: reverse-engineering", "**Scope**: brownfield"), **{scan: SCAN}),
           [scanned("--single"), returned(arch, "--single")], edits=[at_time()])
    check_("link/single_no_scope_recorded", created(lead_only, CREATED_AUDIT + alone_started(1), **{scan: SCAN}), [scanned("--single"), returned(arch, "--single")], edits=[at_time()])
    check_("link/single_newest_start_decides", created(chained, CREATED_AUDIT + alone_started(1, "**Scope**: feature") + alone_started(2, "**Scope**: brownfield"), **{scan: SCAN}),
           [scanned("--single"), returned(arch, "--single")], edits=[at_time()])
    check_("link/single_then_a_word", created(chained, **{scan: SCAN}), [scanned("--single", "yes"), scanned("--single", "false")], edits=[at_time()])
    # A stage run by itself has attempts of its own: the piece of work's boundaries are not its, and its are its own.
    for label, rows in (("restarted_alone", alone_started(5, "**Scope**: brownfield")), ("workflow_restarted", restarted), ("rejected_alone", rejected.replace("**Feedback**", alone + "\n**Feedback**")),
                        ("rejected_in_the_workflow", rejected), ("jumped_to", block("STAGE_JUMPED", T(5), "**Target**: reverse-engineering")), ("created_again", block("WORKFLOW_STARTED", T(5), "**Scope**: feature"))):
        for suffix, state in policies:
            check_(f"link/single_attempt_{label}{suffix}", created(state, CREATED_AUDIT + alone_started(1, "**Scope**: brownfield") + scan_receipt(2, scan, scan_sha, "1767610800123.4567", alone) + rows, **{scan: SCAN}),
                   [returned(arch, "--single"), scanned("--single")], edits=[at_time()])
            check_(f"link/single_attempt_{label}_rewritten{suffix}", created(state, CREATED_AUDIT + alone_started(1, "**Scope**: brownfield") + scan_receipt(2, scan, scan_sha, "1767610800123.4567", alone) + rows, **{scan: SCAN}),
                   [scanned("--single"), returned(arch, "--single")], edits=[at_time(ns=WRITTEN + 10**12)])
    # A row of the stage that another stage's run wrote is no row of this run.
    another = "**Workflow**: single-stage:intent-capture"
    check_("link/single_receipt_of_another_run", created(chained, CREATED_AUDIT + alone_started(1, "**Scope**: brownfield") + scan_receipt(2, scan, scan_sha, "1767610800123.4567", another), **{scan: SCAN}),
           [returned(arch, "--single")], edits=[at_time()])
    check_("link/single_start_of_another_run", created(chained, CREATED_AUDIT + block("STAGE_STARTED", T(1), "**Stage**: reverse-engineering", another, "**Scope**: brownfield"), **{scan: SCAN}),
           [scanned("--single")], edits=[at_time()])
    check_("link/single_restarted_by_another_run", created(chained, CREATED_AUDIT + alone_started(1, "**Scope**: brownfield") + scan_receipt(2, scan, scan_sha, "1767610800123.4567", alone)
                                                          + block("STAGE_STARTED", T(5), "**Stage**: reverse-engineering", another), **{scan: SCAN}), [returned(arch, "--single")], edits=[at_time()])
    check_("link/single_receipt_of_the_workflow", created(chained, CREATED_AUDIT + alone_started(1, "**Scope**: brownfield") + scan_receipt(2), **{scan: SCAN}), [returned(arch, "--single")], edits=[at_time()])
    check_("link/observer", created(chained, **{scan: SCAN}), [scanned()], {"STOP_HOOK_PROBE": "1"}, edits=[at_time()])
    check_("link/no_workflow", CLONE, [returned(dev)])
    check_("answer/summary_choice_with_no_question", created(owing, CREATED_AUDIT + H(2)), [replied("Looks correct"), replied("Request changes: x")])
    check_("answer/summary_question_not_owed", told(summary_q, H(2)), [replied("Looks correct")])
    check_("answer/summary_owed_other_question", created(owing, CREATED_AUDIT + Q(1, "Which database?") + H(2)), [replied("Looks correct"), reply])
    check_("answer/summary_owed_question_superseded", created(owing, CREATED_AUDIT + summary_q + Q(2, "Which database?") + H(3)), [reply])
    check_("answer/summary_owed_of_another_stage", created(owing, CREATED_AUDIT + Q(1, "Which database?") + block(
        "DECISION_RECORDED", T(2), "**Stage**: market-research", "**Decision**: Right?", "**Options**: Looks correct,Request changes") + H(3)), [reply])
    check_("answer/summary_owed_another_checkpoint", created(owing, CREATED_AUDIT + Q(1, "Run it?", "**Checkpoint**: Construction Verification Command") + H(2)), [reply])
    check_("answer/summary_owed_out_of_order", created(owing, CREATED_AUDIT + Q(7, "Does this look right?", "**Options**: Looks correct,Request changes") + Q(5, "Which database?") + H(8)),
           [reply])
    check_("answer/summary_owed_single", created(owing, CREATED_AUDIT + summary_q + H(2)), [reply + ["--single"]])
    # Autonomous Construction: nobody is at the keyboard, and nothing of a person's is asked for or kept.
    building = at_stage(CREATED_STATE.replace("- **Construction Execution**: serial\n", "- **Construction Execution**: serial\n- **Construction Autonomy Mode**: autonomous\n"),
                        "functional-design", done=["intent-capture"], phase="CONSTRUCTION")
    built = lambda text, *more: ["answer", "--stage", "functional-design", "--details", text, *more]
    check_("answer/autonomous_construction", created(building, CREATED_AUDIT + H(1, '**Picked**: ["x"]') + block("DECISION_RECORDED", T(2), "**Stage**: functional-design", "**Decision**: Which?")),
           [built("The assistant chose this answer."), built("y", "--message", "nope"), replied("Postgres")])
    check_("answer/autonomous_construction_at_gate", created(building.replace("- [-] functional-design", "- [?] functional-design"),
                                                             CREATED_AUDIT + block("STAGE_AWAITING_APPROVAL", T(1), "**Stage**: functional-design")), [built("Approve")])
    check_("answer/autonomous_construction_words_kept", created(building, CREATED_AUDIT + H(1), **kept(CREATED_AUDIT, "the person's words")), [built("y")])
    check_("answer/construction_not_autonomous", created(at_stage(CREATED_STATE, "functional-design", done=["intent-capture"], phase="CONSTRUCTION")), [built("y")])

    # An answer's text in a file the agent wrote, read once and removed: named in the record, from the project or
    # in full; nothing outside the record's answer-text folder is read, and nothing through a link.
    folder = f"{CREATED_DIR}/.aidlc-engine/answer-text"
    filed = lambda name=".aidlc-engine/answer-text/answer.txt", *more: ["answer", "--stage", "intent-capture", "--details-file", name, *more]
    text_file = lambda text, name="answer.txt": {**asked, f"{folder}/{name}": text}
    check_("answer/details_file", text_file('Postgres & "quotes" | $HOME\n'), [filed(), filed()])
    check_("answer/details_file_and_details", text_file("Postgres\n"), [filed() + ["--details", "x"], reply + ["--details-file", ".aidlc-engine/answer-text/answer.txt"]])
    check_("answer/details_file_named_from_the_project", text_file("Postgres\n"), [filed(f"{folder}/answer.txt")])
    check_("answer/details_file_named_in_full", text_file("Postgres\n"), [filed("{P}/" + f"{folder}/answer.txt")])
    check_("answer/details_file_backslashes", text_file("Postgres\n"), [filed(".aidlc-engine\\answer-text\\answer.txt")])
    check_("answer/details_file_in_a_subfolder", text_file("Postgres\n", "deep/answer.txt"), [filed(".aidlc-engine/answer-text/deep/answer.txt")])
    check_("answer/details_file_outside_the_folder", {**text_file("Postgres\n"), f"{CREATED_DIR}/answer.txt": "x\n", "answer.txt": "y\n"}, [
        filed("answer.txt"), filed(".aidlc-engine/answer.txt"), filed(".aidlc-engine/answer-text/"), filed(".aidlc-engine/answer-text"), filed(".aidlc-engine/answer-text/../answer.txt"),
        filed(".aidlc-engine/answer-text/./answer.txt"), filed("{P}/answer.txt"), filed(""), filed(f"{CREATED_DIR}/answer.txt"), filed("/.aidlc-engine/answer-text/answer.txt")])
    check_("answer/details_file_missing", asked, [filed()])
    for label, text in (("empty", ""), ("blank", "  \n"), ("newline", "\n"), ("two_newlines", "Postgres\n\n"), ("crlf", "Postgres\r\n"), ("no_newline", "Postgres"),
                        ("inner_lines", "first\nsecond\n"), ("dismissal", "Cancelled\n"), ("at_the_limit", "x" * 65536), ("too_large", "x" * 65537)):
        check_(f"answer/details_file_{label}", text_file(text), [filed()])
    check_("answer/details_file_a_folder", {**asked, f"{folder}/answer.txt/x": "x"}, [filed()])
    check_("answer/details_file_a_link", {**asked, "elsewhere.txt": "Postgres\n"}, [filed()],
           edits=(lambda tree: (os.makedirs(tree.path(folder)), os.symlink(tree.path("elsewhere.txt"), tree.path(f"{folder}/answer.txt"))),))
    check_("answer/details_file_behind_a_link", {**asked, "elsewhere/answer.txt": "Postgres\n"}, [filed()],
           edits=(lambda tree: (os.makedirs(os.path.dirname(tree.path(folder)), exist_ok=True), os.symlink(tree.path("elsewhere"), tree.path(folder))),))
    check_("answer/details_file_no_workflow", CLONE, [filed()])
    check_("answer/details_file_observer", text_file("Postgres\n"), [filed()], {"STOP_HOOK_PROBE": "1"})
    check_("answer/details_file_refused_after_reading", {**told(Q(1, "Which database?")), f"{folder}/answer.txt": "Postgres\n"}, [filed(), filed()])

    # A choice the person left to the agent: recorded as the agent's at their word, with no reply needed, where
    # the person has said something in this piece of work. An approval and a checkpoint are never left to it.
    left = lambda words, *more: reply + ["--on-instruction", words, *more]
    silent = TITLE + block("WORKFLOW_STARTED", T(0), "**Scope**: feature") + Q(1, "Which database?")
    check_("answer/on_instruction", told(Q(1, "Which database?")), [left("  you   decide,\tI trust you  "), left("again")])
    check_("answer/on_instruction_empty", told(Q(1, "Which database?")), [left(""), left("   "), reply + ["--on-instruction"]])
    check_("answer/on_instruction_checkpoint", told(Q(1, "Which database?")), [left("you decide", "--checkpoint", "summary-confirmation"), left("you decide", "--checkpoint", "nope"),
                                                                              left("", "--checkpoint", "nope")])
    check_("answer/on_instruction_unattended", told(Q(1, "Which database?")), [left("you decide")], {"UNATTENDED": "1"})
    check_("answer/on_instruction_nothing_said", created(audit=silent), [left("you decide")])
    check_("answer/on_instruction_blank_request", created(audit=TITLE + block("WORKFLOW_STARTED", T(0), "**Scope**: feature", "**Request**:  ") + Q(1, "Which database?")), [left("you decide")])
    check_("answer/on_instruction_after_a_turn", created(audit=silent + H(2)), [left("you decide")])
    check_("answer/on_instruction_after_a_command_turn", created(audit=silent + H(2, "**Reply**: command")), [left("you decide")])
    check_("answer/on_instruction_after_a_switch_question", created(audit=silent + H(2, "**Reply**: question")), [left("you decide")])
    check_("answer/on_instruction_at_gate", gate_open(H(2)), [left("you decide")])
    check_("answer/on_instruction_at_gate_question_open", gate_open(Q(2, "Which database?")), [left("you decide")])
    check_("answer/on_instruction_dismissed", told(Q(1, "Which database?")), [replied("Cancelled", "--on-instruction", "you decide")])
    check_("answer/on_instruction_self_attributed", told(Q(1, "Which database?")), [replied("The assistant chose this answer.", "--on-instruction", "you decide")])
    check_("answer/on_instruction_keeps_nothing_of_the_turn", told(Q(1, "Which database?"), H(2, '**Picked**: ["SQLite"]'), **kept(before_turn, "the person's words"),
                                                                    **sent("0000aaaa")), [left("you decide", "--message", "0000aaaa"), left("you decide", "--message", "nope")])
    check_("answer/on_instruction_mode", told(Q(1, "How would you like to answer them?", "**Options**: Guide me, I'll edit the file, Chat")), [replied("3", "--on-instruction", "you decide"),
                                                                                                                                             replied("whatever", "--on-instruction", "x")])
    check_("answer/on_instruction_single", told(Q(1, "Which database?")), [left("you decide", "--single")])
    check_("answer/on_instruction_file", {**told(Q(1, "Which database?")), f"{folder}/i.txt": "you decide: I'm & \"away\"\n"},
           [reply + ["--on-instruction-file", ".aidlc-engine/answer-text/i.txt"], reply + ["--on-instruction-file", ".aidlc-engine/answer-text/i.txt"]])
    check_("answer/on_instruction_file_and_words", {**told(Q(1, "Which database?")), f"{folder}/i.txt": "you decide\n"},
           [left("x", "--on-instruction-file", ".aidlc-engine/answer-text/i.txt")])
    check_("answer/both_files", {**told(Q(1, "Which database?")), f"{folder}/i.txt": "you decide\n", f"{folder}/answer.txt": "Postgres\n"},
           [["answer", "--stage", "intent-capture", "--on-instruction-file", ".aidlc-engine/answer-text/i.txt", "--details-file", ".aidlc-engine/answer-text/answer.txt"]])
    check_("answer/on_instruction_autonomous_construction", created(building, CREATED_AUDIT + block("DECISION_RECORDED", T(2), "**Stage**: functional-design", "**Decision**: Which?")),
           [built("y", "--on-instruction", "you decide")])


def jump_checks():
    """`aidlc-jump.ts resolve`, `execute` and `reopen`."""
    check_ = lambda name, files, steps, env=None, edits=(): runtime_check(name, files, steps, env, edits, group="jump")
    ic = artifact("intent-capture", "intent-statement")
    ideation = ["intent-capture", "market-research", "feasibility", "scope-definition", "team-formation", "rough-mockups"]
    at_feasibility = at_stage(CREATED_STATE, "feasibility", done=["intent-capture", "market-research"])
    check_("usage/none", created(), [[]])
    check_("usage/unknown", created(), [["nope"]])
    check_("usage/flag_without_its_value", created(), [["resolve", "--project-dir"]])
    check_("resolve/usage", created(), [["resolve"], ["resolve", "--stage"], ["resolve", "--stage", ""], ["resolve", "--allow-skipped"]])
    check_("resolve/unknown", created(), [["resolve", "--stage", "nope"], ["resolve", "--phase", "nope"], ["resolve", "--stage", "feasibility", "--scope", "nope"], ["resolve", "--phase", "5"]])
    check_("resolve/forward", created(), [["resolve", "--stage", "feasibility"], ["resolve", "--stage", "1.3"], ["resolve", "--stage", "feedback-optimization"]])
    check_("resolve/phase", created(), [["resolve", "--phase", "inception"], ["resolve", "--phase", "2"], ["resolve", "--phase", "INCEPTION"], ["resolve", "--phase", "0"],
                                        ["resolve", "--phase", "ideation"], ["resolve", "--phase", "operation", "--scope", "bugfix"]])
    check_("resolve/backward", created(at_feasibility), [["resolve", "--stage", "intent-capture"], ["resolve", "--stage", "market-research"]])
    check_("resolve/redo", created(at_feasibility), [["resolve", "--stage", "feasibility"], ["resolve", "--stage", "1.3"]])
    check_("resolve/skipped_target", created(), [["resolve", "--stage", "reverse-engineering"], ["resolve", "--stage", "reverse-engineering", "--allow-skipped"],
                                                 ["resolve", "--allow-skipped", "--stage", "feasibility"]])
    check_("resolve/foreign_scope", created(), [["resolve", "--stage", "feasibility", "--scope", "bugfix"], ["resolve", "--stage", "reverse-engineering", "--scope", "bugfix"],
                                                ["resolve", "--phase", "inception", "--scope", "poc"]])
    suffixed = CREATED_STATE.replace("- [ ] market-research \u2014 EXECUTE", "- [ ] market-research \u2014 SKIP").replace("- [ ] reverse-engineering \u2014 SKIP", "- [ ] reverse-engineering \u2014 EXECUTE")
    check_("resolve/suffix_override", created(suffixed), [["resolve", "--stage", "feasibility"], ["resolve", "--stage", "market-research"], ["resolve", "--stage", "reverse-engineering"],
                                                          ["resolve", "--phase", "inception"], ["resolve", "--stage", "market-research", "--scope", "feature"], ["resolve", "--stage", "market-research", "--scope", "bugfix"]])
    inception_off = CREATED_STATE
    for slug in ("practices-discovery", "requirements-analysis", "user-stories", "refined-mockups", "domain-design", "units-generation", "contract-design", "delivery-planning"):
        inception_off = inception_off.replace(f"- [ ] {slug} \u2014 EXECUTE", f"- [ ] {slug} \u2014 SKIP")
    check_("resolve/phase_all_suffixed_skip", created(inception_off), [["resolve", "--phase", "inception"], ["resolve", "--phase", "inception", "--scope", "bugfix"]])
    check_("resolve/no_current_stage", created(CREATED_STATE.replace("- **Current Stage**: intent-capture\n", "- **Current Stage**: \n")), [["resolve", "--stage", "feasibility"]])
    check_("resolve/current_unknown", created(CREATED_STATE.replace("- **Current Stage**: intent-capture\n", "- **Current Stage**: nope\n")), [["resolve", "--stage", "feasibility"]])
    check_("resolve/no_scope", created(CREATED_STATE.replace("- **Scope**: feature\n", "")), [["resolve", "--stage", "feasibility"]])
    check_("resolve/empty_scope", created(CREATED_STATE.replace("- **Scope**: feature\n", "- **Scope**: \n")), [["resolve", "--stage", "feasibility"]])
    check_("resolve/flag_forms", created(), [["resolve", "--stage", "--phase", "inception"], ["resolve", "--phase", "inception", "--stage"], ["--stage", "feasibility", "resolve"]])
    check_("resolve/no_state_file", created(**{f"{CREATED_DIR}/aidlc-state.md": None}), [["resolve", "--stage", "feasibility"]])
    # execute: forward, across a phase boundary, backward with what it invalidates, redo.
    forward = ["execute", "--target", "feasibility", "--direction", "forward"]
    check_("execute/usage", created(), [["execute"], ["execute", "--target"], ["execute", "--target", "feasibility"], ["execute", "--target", "feasibility", "--direction", "sideways"],
                                        ["execute", "--target", "nope", "--direction", "forward"], ["execute", "--target", "reverse-engineering", "--direction", "forward"],
                                        ["execute", "--target", "feasibility", "--direction", "forward", "--scope", "nope"]])
    check_("execute/forward", created(**ic), [forward, ["resolve", "--stage", "intent-capture"]])
    check_("execute/forward_from_awaiting", created(CREATED_STATE.replace("- [-] intent-capture", "- [?] intent-capture")), [forward])
    check_("execute/forward_from_revising", created(CREATED_STATE.replace("- [-] intent-capture", "- [R] intent-capture")), [forward])
    check_("execute/forward_from_pending", created(CREATED_STATE.replace("- [-] intent-capture", "- [ ] intent-capture")), [forward])
    check_("execute/forward_over_done", created(at_stage(CREATED_STATE, "market-research", done=["intent-capture"])), [["execute", "--target", "scope-definition", "--direction", "forward"]])
    check_("execute/forward_phase_boundary", created(**ic), [["execute", "--target", "requirements-analysis", "--direction", "forward"]])
    check_("execute/forward_two_phases", created(**ic), [["execute", "--target", "code-generation", "--direction", "forward"]])
    check_("execute/forward_suffix_override", created(suffixed), [forward, ["execute", "--target", "reverse-engineering", "--direction", "forward"]])
    check_("execute/forward_foreign_scope", created(suffixed), [["execute", "--target", "scope-definition", "--direction", "forward", "--scope", "bugfix"]])
    check_("execute/forward_to_current", created(), [["execute", "--target", "intent-capture", "--direction", "forward"]])
    check_("execute/forward_mis_specified", created(at_feasibility), [["execute", "--target", "intent-capture", "--direction", "forward"]])
    # The source baseline both rows of a jump carry, with the listing kept beside the record: the project's source
    # files; what the registry adds and takes out; a project whose source cannot be read, which starts from an
    # empty listing, or is unbindable when it is a git checkout; and the snapshot's place already taken.
    source = {"src/main.ts": "export const a = 1;\n", "src/crlf.txt": "a\r\nb\r\n", "build/out.js": "o\n", ".DS_Store": "d\n", "docs/guide.md": "# g\n"}
    registry_file = ".aidlc-source-paths.json"
    empty_baseline = f"{CREATED_DIR}/.aidlc-engine/source-review/code-generation/baseline-e3b0c44298fc.tsv"
    check_("execute/source_files", created(**source), [forward, ["execute", "--target", "intent-capture", "--direction", "backward"]])
    check_("execute/source_registry", created(**source, **{"build/keep.bin": "k\n", registry_file: '{"version":1,"paths":["build/keep.bin"],"exclude":["docs"]}\n'}), [forward])
    check_("execute/source_unreadable", created(**source, **{registry_file: "{"}), [forward])
    check_("execute/source_unbindable", created(**source, **{registry_file: "{", ".git/HEAD": "ref: refs/heads/main\n"}), [forward])
    check_("execute/source_embedded_unresolved", created(**{"vendor/lib/.git/HEAD": "ref: refs/heads/gone\n", "vendor/lib/.git/config": "[core]\n", "vendor/lib/x.c": "x\n"}), [forward])
    check_("execute/delegated_worktree", created(**source, **{".aidlc/worktree-meta.json": "{}\n"}), [forward])
    check_("execute/source_repos_delegated_worktree", created(registry=CREATED_REGISTRY.replace('"status": "in-flight"', '"status": "in-flight",\n    "repos": ["app"]'),
                                                              **{"app/a.ts": "a\n", "app/.aidlc/worktree-meta.json": "{}\n"}), [forward])
    check_("execute/source_snapshot_taken", created(**{registry_file: "{", empty_baseline: "another listing\n"}), [forward])
    check_("execute/source_snapshot_kept", created(**{registry_file: "{", empty_baseline: ""}), [forward])
    # The member repositories of the piece of work are the source, as its own row of the registry lists them (and
    # not the first row's); a checkout among them is what makes an unreadable source unbindable, and the project's
    # own is not; and the snapshot's place is never reached through a link.
    def with_repos(*names, registry=CREATED_REGISTRY):
        listed = ", ".join(f'"{name}"' for name in names)
        return registry.replace('"status": "in-flight"', f'"status": "in-flight",\n    "repos": [{listed}]')
    members = {"app/a.ts": "a\n", "lib/b.ts": "b\n", "top.txt": "t\n"}
    other_first = with_repos("app").replace("[\n", '[\n  {\n    "uuid": "01a138fa-d37d-787d-af4a-966efbb0b6f0",\n    "slug": "other",\n    "dirName": "260101-other",\n'
                                            '    "scope": "feature",\n    "status": "in-flight",\n    "repos": ["lib"]\n  },\n', 1)
    check_("execute/source_repos", created(registry=with_repos("app", "lib"), **members), [forward])
    check_("execute/source_repos_own_row", created(registry=other_first, **members), [forward])
    check_("execute/source_repos_one_absent", created(registry=with_repos("app", "gone"), **members), [forward])
    check_("execute/source_repos_unreadable", created(registry=with_repos("app", "lib"), **members, **{f"app/{registry_file}": "{", ".git/HEAD": "ref: refs/heads/main\n"}), [forward])
    check_("execute/source_repos_unbindable", created(registry=with_repos("app", "lib"), **members, **{f"app/{registry_file}": "{", "lib/.git/HEAD": "ref: refs/heads/main\n"}), [forward])
    review = f"{CREATED_DIR}/.aidlc-engine/source-review"
    check_("execute/source_snapshot_behind_link", created(**source, **{"elsewhere/keep": ""}), [forward],
           edits=(lambda tree: (os.makedirs(os.path.dirname(tree.path(review))), os.symlink(tree.path("elsewhere"), tree.path(review))),))
    check_("execute/source_snapshot_behind_dangling_link", created(**source), [forward],
           edits=(lambda tree: (os.makedirs(os.path.dirname(tree.path(review))), os.symlink(tree.path("nowhere"), tree.path(review))),))
    # A row whose `repos` is not a list of names (refused by focus, by name), and names that are no directory of
    # the project, which the listing's writer refuses in both.
    for label, listed in (("not_a_name", '["app", 7]'), ("null_name", '[null]'), ("a_list_inside", '[["app"]]'), ("not_a_list", '"app"'), ("empty_name", '[""]')):
        check_(f"execute/source_repos_refused_{label}", created(registry=with_repos("app").replace('["app"]', listed), **members), [forward])
    check_("execute/source_repos_null", created(registry=with_repos("app").replace('["app"]', "null"), **members), [forward])
    check_("execute/source_repos_dots", created(registry=with_repos(".."), **members), [forward])
    check_("execute/source_repos_nested", created(registry=with_repos("app/sub"), **members, **{"app/sub/c.ts": "c\n"}), [forward])
    dotnet = {"App.csproj": "<Project />\n", "Program.cs": "class P {}\n", "bin/Debug/app.dll": "b\n", "obj/project.assets.json": "{}\n"}
    check_("execute/source_dotnet_outputs", created(**dotnet), [forward])
    check_("execute/source_snapshot_stage_behind_link", created(**source, **{"elsewhere/keep": "", f"{review}/keep": ""}), [forward],
           edits=(lambda tree: os.symlink(tree.path("elsewhere"), tree.path(f"{review}/code-generation")),))
    backward = ["execute", "--target", "intent-capture", "--direction", "backward"]
    mr = artifact("market-research", "competitive-analysis")
    check_("execute/backward", created(at_feasibility, **ic, **mr), [backward, ["resolve", "--stage", "feasibility"]])
    check_("execute/backward_no_artifacts", created(at_feasibility), [backward])
    check_("execute/backward_review_section", created(at_feasibility, **ic, **artifact("market-research", "competitive-analysis", "# MR\n\n## Review\n- R-01 New: x\n")), [backward])
    check_("execute/backward_review_section_fenced", created(at_feasibility, **ic, **artifact("market-research", "competitive-analysis", "```\n## Review\nx\n```\n")), [backward])
    check_("execute/backward_review_rows", created(at_feasibility, CREATED_AUDIT + block("REVIEW_COMPLETED", T(2), "**Stage**: intent-capture", "**Verdict**: READY"), **ic), [backward])
    # A reviewed stage behind the jump: its review records are the brief's (M5), refused by name when rows are on the ledger.
    reviewed = at_stage(CREATED_STATE, "approval-handoff", done=ideation)
    check_("execute/backward_over_reviewed_stage", created(reviewed, **ic, **artifact("rough-mockups", "wireframes", "# W\n\n## Review\n- R-01 New: x\n")),
           [["execute", "--target", "team-formation", "--direction", "backward"]])
    check_("execute/backward_over_reviewed_stage_rows", created(reviewed, CREATED_AUDIT + block("REVIEW_COMPLETED", T(2), "**Stage**: rough-mockups", "**Verdict**: READY"), **ic),
           [["execute", "--target", "team-formation", "--direction", "backward"]])
    check_("execute/backward_mis_specified", created(), [["execute", "--target", "feasibility", "--direction", "backward"]])
    check_("execute/backward_leaves_plan_skipped", created(at_stage(CREATED_STATE, "practices-discovery", done=ideation + ["approval-handoff"], skipped=["reverse-engineering"], phase="INCEPTION")),
           [backward])
    operation_off = at_stage(CREATED_STATE, "requirements-analysis", done=ideation + ["approval-handoff"], phase="INCEPTION").replace("- **Operation**: Pending", "- **Operation**: Skipped")
    for slug in ("deployment-pipeline", "environment-provisioning", "deployment-execution", "observability-setup", "incident-response", "performance-validation", "feedback-optimization"):
        operation_off = operation_off.replace(f"- [ ] {slug} \u2014 EXECUTE", f"- [ ] {slug} \u2014 SKIP")
    check_("execute/backward_phase_without_execute", created(operation_off, **ic), [backward])
    check_("execute/backward_phase_boundary", created(at_stage(CREATED_STATE, "requirements-analysis", done=ideation + ["approval-handoff"], phase="INCEPTION"), **ic),
           [["execute", "--target", "market-research", "--direction", "backward"]])
    operation = at_stage(CREATED_STATE, "deployment-pipeline", done=ideation + ["approval-handoff", "practices-discovery", "requirements-analysis", "user-stories", "refined-mockups",
                                                                                 "domain-design", "units-generation", "contract-design", "delivery-planning", "functional-design",
                                                                                 "nfr-requirements", "nfr-design", "infrastructure-design", "code-generation", "build-and-test",
                                                                                 "ci-pipeline"], phase="OPERATION")
    per_unit = {f"{CREATED_DIR}/construction/api/functional-design/entities.md": "# api\n", f"{CREATED_DIR}/construction/web/code-generation/code-summary.md": "# web\n",
                f"{CREATED_DIR}/construction/web/functional-design/entities.md": "# web\n", f"{CREATED_DIR}/construction/web/functional-design/frontend-components.md": "# web\n",
                **artifact("units-generation", "unit-of-work", phase="inception")}
    # Units Generation off the plan: the unit directories still count when a stage-level file is not there.
    units_off = operation.replace("- [x] units-generation \u2014 EXECUTE", "- [S] units-generation \u2014 SKIP")
    check_("execute/backward_over_construction_dirs_without_units_generation", created(units_off, **per_unit), [["execute", "--target", "delivery-planning", "--direction", "backward"]])
    # Both a stage-level file and unit directories, with no DAG: the stage-level file wins.
    check_("execute/backward_over_construction_mixed", created(operation, **per_unit, **{f"{CREATED_DIR}/construction/functional-design/entities.md": "# all\n"}),
           [["execute", "--target", "delivery-planning", "--direction", "backward"]])
    # A DAG with unit kinds: a unit's kind picks which artifacts it produces (frontend-components for ui, not for a service).
    kinds = "# Dependencies\n\n```yaml\nunits:\n  - name: api\n    kind: service\n  - name: web\n    kind: ui\n```\n"
    check_("execute/backward_over_construction_kinds", created(operation, **per_unit, **{UNITS.replace(RECORD_DIR, CREATED_DIR): kinds}), [["execute", "--target", "delivery-planning", "--direction", "backward"]])
    check_("execute/backward_to_stage_with_optional", created(at_stage(CREATED_STATE, "scope-definition", done=["intent-capture", "market-research", "feasibility"]), **artifact("feasibility", "raid-log")),
           [["execute", "--target", "feasibility", "--direction", "backward"]])
    check_("execute/backward_over_construction_dirs", created(operation, **per_unit), [["execute", "--target", "units-generation", "--direction", "backward"]])
    check_("execute/backward_over_construction_dag", created(operation, **per_unit, **{UNITS.replace(RECORD_DIR, CREATED_DIR): A_UNITS}), [["execute", "--target", "units-generation", "--direction", "backward"]])
    # The files a jump lists for review are a stage's own outputs, laid out by the stage's marker alone: one of
    # the five stages that always run per unit, in a compiled graph that lost its marker, is listed at the stage.
    lost = [lambda tree: no_marker(tree, ["functional-design", "code-generation"])]
    check_("execute/backward_over_construction_dirs_no_marker", created(operation, **per_unit), [["execute", "--target", "units-generation", "--direction", "backward"]], edits=lost)
    check_("execute/backward_over_construction_dag_no_marker", created(operation, **per_unit, **{UNITS.replace(RECORD_DIR, CREATED_DIR): A_UNITS}),
           [["execute", "--target", "units-generation", "--direction", "backward"]], edits=lost)
    check_("execute/backward_over_construction_mixed_no_marker", created(operation, **per_unit, **{f"{CREATED_DIR}/construction/functional-design/entities.md": "# all\n"}),
           [["execute", "--target", "delivery-planning", "--direction", "backward"]], edits=lost)
    check_("execute/backward_over_construction_empty", created(operation), [["execute", "--target", "delivery-planning", "--direction", "backward"]])
    check_("execute/backward_over_construction_stage_level", created(operation.replace("- [x] units-generation \u2014 EXECUTE", "- [S] units-generation \u2014 SKIP"),
                                                                     **{f"{CREATED_DIR}/construction/functional-design/entities.md": "# all\n"}),
           [["execute", "--target", "delivery-planning", "--direction", "backward"]])
    brownfield = at_stage(CREATED_STATE.replace("- [ ] reverse-engineering \u2014 SKIP", "- [ ] reverse-engineering \u2014 EXECUTE"), "practices-discovery",
                          done=ideation + ["approval-handoff", "reverse-engineering"], phase="INCEPTION")
    codekb = {"aidlc/spaces/default/codekb/app/architecture.md": "# Known\n", "aidlc/spaces/default/codekb/app/business-overview.md": "# Known\n\n## Review\n- R-01\n"}
    check_("execute/backward_over_codekb", created(brownfield, **codekb), [["execute", "--target", "reverse-engineering", "--direction", "backward"]])
    check_("execute/backward_over_codekb_registered", created(brownfield, registry=CREATED_REGISTRY.replace('"status": "in-flight"', '"status": "in-flight",\n    "repos": ["app", "lib"]'), **codekb),
           [["execute", "--target", "reverse-engineering", "--direction", "backward"]])
    check_("execute/backward_over_codekb_duplicate_repos", created(brownfield, registry=CREATED_REGISTRY.replace('"status": "in-flight"', '"status": "in-flight",\n    "repos": ["app", "app"]'), **codekb),
           [["execute", "--target", "reverse-engineering", "--direction", "backward"]])
    check_("execute/backward_session", created(at_feasibility, **ic), [backward], {"SESSION_OVERRIDE": "s1"})
    check_("execute/backward_unit_stage", created(at_feasibility + "- **Unit Stage**: market-research\n- **Active Unit**: api\n", **ic), [backward, ["resolve", "--stage", "feasibility"]])
    check_("execute/backward_unit_stage_unknown", created(at_feasibility + "- **Unit Stage**: nope\n", **ic), [backward])
    check_("execute/redo", created(at_feasibility, **ic), [["execute", "--target", "feasibility", "--direction", "redo"], ["resolve", "--stage", "feasibility"]])
    check_("execute/redo_elsewhere", created(at_feasibility, **ic), [["execute", "--target", "intent-capture", "--direction", "redo"]])
    check_("execute/answer_mode", created(audit=CREATED_AUDIT + block("DECISION_RECORDED", T(1), "**Stage**: intent-capture", "**Decision**: How would you like to answer them?",
                                                                      "**Options**: Guide me, I'll edit the file, Chat") + block("QUESTION_ANSWERED", T(2), "**Stage**: intent-capture", "**Details**: Chat")),
           [forward])
    check_("execute/units", created(**ic), [["execute", "--target", "functional-design", "--direction", "forward", "--units", "api"],
                                            ["execute", "--target", "feasibility", "--direction", "forward", "--units", ""]])
    check_("execute/directive_marker", created(**ic, **{f"{GATE_DIR}/active-directive.json": json.dumps({"stage": "market-research"})}), [forward])
    check_("execute/no_state_file", created(**{f"{CREATED_DIR}/aidlc-state.md": None}), [forward])
    check_("execute/archived", created(CREATED_STATE.replace("- **Status**: Running", "- **Status**: Archived")), [forward])
    check_("reopen/usage", created(), [["reopen"], ["reopen", "--target", "functional-design"], ["reopen", "--units", "api"]])
    check_("reopen/by_name", created(), [["reopen", "--target", "functional-design", "--units", "api,web"], ["reopen", "--target", "intent-capture", "--units", "api", "--via", "redo"]])
    for marker in ("STOP_HOOK_PROBE", "ROUTE_CHECK"):
        check_(f"observer/{marker.lower()}", created(**ic), [forward, ["resolve", "--stage", "feasibility"]], {marker: "1"})


def transition_fuzz(count):
    """Random sequences of transitions on a created record, started at a random stage with random artifacts."""
    rng = random.Random(SEED + 3)
    with open(os.path.join(DIST, ".claude", "tools", "data", "stage-graph.json"), encoding="utf-8") as handle:
        graph = [stage for stage in json.load(handle) if stage["phase"] not in ("initialization", "construction")
                 and stage["slug"] != "reverse-engineering"]
    order = [stage["slug"] for stage in graph]
    for index in range(count):
        start = rng.randrange(0, len(order) - 1)
        current = graph[start]
        state = at_stage(GATE_STATE, current["slug"], done=order[:start], phase=current["phase"].upper())
        files = {}
        # Artifacts for the stages ahead, most of them, and a few behind.
        for stage in graph[start:start + 6] + rng.sample(graph[:start], min(start, 2)):
            if rng.random() < 0.8:
                for name in rng.sample(stage["produces"], rng.randrange(1, len(stage["produces"]) + 1)):
                    files.update(artifact(stage["slug"], name, rng.choice(["# A\n", "# B\r\nline\r\n", "", "x"]), phase=stage["phase"]))
        slugs = order[max(0, start - 1):start + 5]
        steps, cursor = [], start
        for _ in range(rng.randrange(3, 8)):
            slug = order[min(cursor, len(order) - 1)] if rng.random() < 0.7 else rng.choice(slugs)
            kind = rng.choice(["advance", "advance", "finalize", "skip", "route", "complete", "resume", "get",
                               "gate-start", "gate-start", "approve", "approve", "reject", "revise"])
            if kind == "gate-start":
                steps.append(["gate-start", slug] + (["--recovered"] if rng.random() < 0.2 else []) +
                             (["--artifacts", rng.choice(["a.md", "a.md,b.md", ""])] if rng.random() < 0.2 else []))
            elif kind == "approve":
                steps.append(["approve", slug] + (["--user-input", rng.choice(["Approve", "Approve and continue", "Request Changes", "...",
                                                                               "Accept as-is", "Approve (agent's default)", " "])] if rng.random() < 0.9 else []))
                cursor += 1
            elif kind == "reject":
                steps.append(["reject", slug] + rng.choice([["--feedback", "more"], ["--reason", "less"], ["--feedback=--x"], [], ["--feedback", "..."]]))
            elif kind == "revise":
                steps.append(["revise", slug] + (["--user-input", "x"] if rng.random() < 0.2 else []))
            elif kind == "advance":
                steps.append(["advance", slug] + ([rng.choice(slugs)] if rng.random() < 0.2 else []))
                cursor += 1
            elif kind == "finalize":
                steps.append(["finalize", slug])
                cursor += 1
            elif kind == "skip":
                steps.append(["skip", rng.choice(slugs)] + (["--reason", rng.choice(["not needed", " ", "x"])] if rng.random() < 0.6 else []))
            elif kind == "route":
                steps.append(["skip", slug, "--route", "--reason", rng.choice(["not this one", "new project"])])
                cursor += 1
            elif kind == "complete":
                steps.append(["complete-workflow", slug] + (["--reason", "enough"] if rng.random() < 0.5 else []))
            elif kind == "resume":
                steps.append(["resume"])
            else:
                steps.append(["get", rng.choice(["Current Stage", "Completed", "Status", "Next Stage"])])
        steps.append(["resume"])
        env = {"ALLOW_DIRECT_STATE_TRANSITIONS": "1", "SKIP_HUMAN_PRESENCE_GUARD": "1"}
        if rng.random() < 0.2:
            env["SKIP_ARTIFACT_GUARD"] = "1"
        runtime_check(str(index), created(state, **files), steps, env, group="fuzz/transitions", tool="state")


def jump_fuzz(count):
    """Random jumps on a created record started at a random stage with random artifacts and suffixes."""
    rng = random.Random(SEED + 4)
    with open(os.path.join(DIST, ".claude", "tools", "data", "stage-graph.json"), encoding="utf-8") as handle:
        graph = [stage for stage in json.load(handle) if stage["phase"] != "initialization"]
    order = [stage["slug"] for stage in graph]
    for index in range(count):
        start = rng.randrange(0, len(order))
        current = graph[start]
        state = at_stage(GATE_STATE, current["slug"], done=order[:start], phase=current["phase"].upper())
        for stage in rng.sample(graph, 3):
            if rng.random() < 0.5:
                flipped = "SKIP" if rng.random() < 0.5 else "EXECUTE"
                state = re.sub(rf"(- \[.\] {re.escape(stage['slug'])} \u2014 )(EXECUTE|SKIP)", rf"\g<1>{flipped}", state)
        if rng.random() < 0.3:
            state = state.replace(f"- [-] {current['slug']}", f"- [{rng.choice('?Rx ')}] {current['slug']}")
        files = {}
        for stage in rng.sample(graph, rng.randrange(0, 6)):
            for name in rng.sample(stage["produces"], rng.randrange(1, len(stage["produces"]) + 1)):
                text = rng.choice(["# A\n", "# B\n\n## Review\n- R-01 New: x\n", "", "```\n## Review\n```\n"])
                files.update(artifact(stage["slug"], name, text, phase=stage["phase"]))
        steps = []
        for _ in range(rng.randrange(1, 4)):
            target = rng.choice(order)
            if rng.random() < 0.3:
                steps.append(["resolve", rng.choice(["--stage", "--phase"]), rng.choice([target, rng.choice(["1", "2", "3", "4", "inception", "1.2"])])])
            else:
                direction = rng.choice(["forward", "backward", "redo", "forward", "backward"])
                steps.append(["execute", "--target", target, "--direction", direction])
        steps.append(["resolve", "--stage", rng.choice(order)])
        runtime_check(str(index), created(state, **files), steps, {}, group="fuzz/jump", tool="jump")


def runtime_checks():
    check_ = runtime_check
    record = {**RECORD_FILES, f"{RECORD_DIR}/aidlc-state.md": STATE}
    compiled = [["compile"], ["read", "intent-capture"], ["summary", "--json"], ["summary"]]

    # Usage.
    check_("usage/none", {}, [[]])
    check_("usage/unknown", {}, [["nope"]])
    check_("usage/fragment_fork", record, [["fragment-fork", "--slug", "one"]])
    check_("usage/fragment_merge", record, [["fragment-merge", "--slug", "one"]])
    check_("usage/read_no_slug", record, [["read"]])
    check_("usage/read_no_graph", record, [["read", "intent-capture"]])
    check_("usage/summary_no_graph", record, [["summary"]])

    # compile: where there is nothing to read yet.
    check_("compile/no_state", CLONE, [["compile"], ["read", "x"]])
    check_("compile/no_ledger", record, compiled)
    check_("compile/no_ledger_no_intent", {**CLONE, "aidlc/spaces/default/intents/aidlc-state.md": STATE}, compiled)
    check_("compile/no_workflow", {**record, SHARD: TITLE + STARTED_ROW(1, "intent-capture")}, compiled)
    check_("compile/state_without_scope", {**RECORD_FILES, SHARD: TITLE + WORKFLOW}, compiled)

    # compile: stages.
    check_("compile/a_workflow", {**record, SHARD: A_WORKFLOW}, compiled + [["read", "code-generation"],
                                                                           ["read", "nope"]])
    check_("compile/twice", {**record, SHARD: A_WORKFLOW}, [["compile"], ["compile"]])
    check_("compile/scope_from_ledger", {**RECORD_FILES, SHARD: A_WORKFLOW}, compiled)
    check_("compile/unknown_stage", {**record, SHARD: A_WORKFLOW + STARTED_ROW(11, "not-a-stage")}, compiled)
    check_("compile/restarted_stage",
           {**record, SHARD: A_WORKFLOW + STARTED_ROW(12, "intent-capture") + COMPLETED_ROW(13, "intent-capture") +
            COMPLETED_ROW(14, "intent-capture")}, compiled)
    check_("compile/second_workflow",
           {**record, SHARD: A_WORKFLOW + block("WORKFLOW_STARTED", T(20), "**Scope**: mvp") +
            STARTED_ROW(21, "market-research")}, compiled)
    check_("compile/single_stage_rows",
           {**record, SHARD: A_WORKFLOW + STARTED_ROW(15, "feasibility", "**Workflow**: single-stage:feasibility") +
            COMPLETED_ROW(16, "feasibility", "**Workflow**: single-stage:feasibility")}, compiled)
    check_("compile/same_instant",
           {**record, SHARD: TITLE + WORKFLOW + STARTED_ROW(1, "market-research") + STARTED_ROW(1, "intent-capture") +
            COMPLETED_ROW(1, "intent-capture") + COMPLETED_ROW(1, "market-research")}, compiled)
    check_("compile/label_without_value",
           {**record, SHARD: TITLE + WORKFLOW + block("STAGE_STARTED", T(1), "**Stage**:", "**Agent**:", "x")},
           compiled)
    check_("compile/two_shards",
           {**record, SHARD: A_WORKFLOW, f"{RECORD_DIR}/audit/desktop-ffffffffffff.md":
            TITLE + COMPLETED_ROW(11, "code-generation") + STARTED_ROW(12, "build-and-test")}, compiled)

    # compile: diaries, and the event an empty one is.
    diaries = {DIARY("ideation", "intent-capture"): A_DIARY,
               DIARY("inception", "requirements-analysis"): "# Memory\n\n## Interpretations\n\n## Deviations\n",
               DIARY("construction", "code-generation"): "## Tradeoffs\n"}
    check_("compile/diaries", {**record, SHARD: A_WORKFLOW, **diaries}, compiled + [["compile"], ["summary"]])
    check_("compile/empty_diary_again_after_restart",
           {**record, SHARD: A_WORKFLOW + block("MEMORY_EMPTY", T(9, 30), "**Stage**: requirements-analysis") +
            STARTED_ROW(11, "requirements-analysis") + COMPLETED_ROW(12, "requirements-analysis"), **diaries},
           [["compile"], ["compile"]])
    check_("compile/empty_diary_already_recorded",
           {**record, SHARD: A_WORKFLOW + block("MEMORY_EMPTY", T(9, 30), "**Stage**: requirements-analysis"),
            **diaries}, [["compile"]])
    # A clone that has no identity yet gets one from the compile that has an event to write, and from no other.
    no_clone = {k: v for k, v in record.items() if k not in CLONE}
    check_("compile/mints_identity", {**no_clone, SHARD: A_WORKFLOW, **diaries}, [["compile"], ["compile"]])
    check_("compile/mints_no_identity", {**no_clone, SHARD: A_WORKFLOW}, [["compile"]])
    check_("compile/diary_is_a_directory",
           {**record, SHARD: A_WORKFLOW, DIARY("ideation", "intent-capture") + "/x": "x"}, [["compile"]])

    # compile: the units' order.
    check_("compile/units", {**record, SHARD: A_WORKFLOW, UNITS: A_UNITS}, compiled)
    check_("compile/units_unreadable", {**record, SHARD: A_WORKFLOW, UNITS: A_UNITS.replace("[auth]", "[nobody]")},
           compiled)
    check_("compile/units_no_block", {**record, SHARD: A_WORKFLOW, UNITS: "# Dependencies\n"}, compiled)
    check_("compile/units_before_any_workflow", {**record, UNITS: A_UNITS}, compiled)

    # compile: sensors.
    sensors = (A_WORKFLOW + FIRED(2, 0, "intent-capture", "aaaaaaaa") + ENDED("PASSED", 2, 5, "aaaaaaaa") +
               FIRED(2, 1, "intent-capture", "bbbbbbbb", "types") +
               ENDED("FAILED", 2, 9, "bbbbbbbb", "**Detail path**: aidlc/x/detail.md") +
               FIRED(2, 2, "intent-capture", "cccccccc") + ENDED("BUDGET_OVERRIDE", 2, 3, "cccccccc") +
               FIRED(3, 0, "intent-capture", "dddddddd") +                     # closed stage, no ending
               FIRED(7, 0, "intent-capture", "eeeeeeee") +                     # outside its stage's window
               FIRED(10, 5, "code-generation", "ffffffff") +                   # open stage, old enough
               FIRED(12, 0, "code-generation", "99999999") +                   # open stage, the latest row
               FIRED(11, 30, "code-generation", "88888888") +                  # open stage, under a minute old
               FIRED(11, 0, "code-generation", "77777777") + ENDED("PASSED", 11, 1, "77777777") +
               ENDED("PASSED", 11, 2, "77777777") + ENDED("FAILED", 11, 1, "77777777", "**Detail path**: late") +
               FIRED(10, 6, "code-generation", "") + FIRED(10, 7, "code-generation", "66666666", ""))
    check_("compile/sensors", {**record, SHARD: sensors}, compiled + [["read", "code-generation"]])
    check_("compile/sensor_exactly_a_minute_old",
           {**record, SHARD: A_WORKFLOW + FIRED(11, 0, "code-generation", "aaaaaaaa") +
            FIRED(12, 0, "code-generation", "bbbbbbbb")}, compiled + [["read", "code-generation"]])

    # compile: what was learned at a gate.
    learned = lambda minute, stage, *fields: block("RULE_LEARNED", T(minute), f"**Stage**: {stage}", *fields)
    check_("compile/learnings",
           {**record, SHARD: A_WORKFLOW + learned(2, "intent-capture") +
            learned(3, "intent-capture", "**Source**: user_addition") +
            block("SENSOR_PROPOSED", T(4), "**Stage**: intent-capture", "**Source**: orchestrator") +
            learned(5, "intent-capture") + learned(7, "requirements-analysis", "**Source**: user_addition") +
            learned(11, "code-generation")}, compiled + [["read", "requirements-analysis"]])

    # compile: a stage built by several bolts.
    bolts = (A_WORKFLOW + FORKED(11, "web", "**Worktree path**: <project-dir>/wt/web") +
             FORKED(12, "api", "**Worktree path**: wt/api") + FORKED(13, "db") +
             MERGED(14, "api") + block("BOLT_FAILED", T(15), "**Bolt slug**: db") +
             FIRED(12, 30, "code-generation", "aaaaaaaa", "linter", "**Output path**: <project-dir>/wt/api/src/a.ts") +
             ENDED("PASSED", 12, 31, "aaaaaaaa") +
             FIRED(12, 40, "code-generation", "bbbbbbbb", "linter", "**Output path**: wt/web/b.ts") +
             FIRED(12, 50, "code-generation", "cccccccc", "linter", "**Output path**: src/main.ts") +
             ENDED("PASSED", 12, 51, "cccccccc") + FIRED(12, 55, "code-generation", "dddddddd"))
    check_("compile/bolts", {**record, SHARD: bolts}, compiled + [["read", "code-generation"]])
    check_("compile/bolts_all_merged",
           {**record, SHARD: A_WORKFLOW + FORKED(11, "web") + FORKED(12, "api") + MERGED(13, "web") +
            MERGED(14, "api") + COMPLETED_ROW(16, "code-generation")}, compiled + [["read", "code-generation"]])
    check_("compile/one_bolt", {**record, SHARD: A_WORKFLOW + FORKED(11, "web") + MERGED(13, "web")},
           compiled + [["read", "code-generation"]])
    check_("compile/bolts_outside_construction",
           {**record, SHARD: A_WORKFLOW + FORKED(2, "web") + FORKED(3, "api")}, compiled)
    check_("compile/bolt_forked_twice",
           {**record, SHARD: A_WORKFLOW + FORKED(11, "web", "**Worktree path**: /abs/one") + FORKED(12, "api") +
            FORKED(13, "web", "**Worktree path**: wt/../outside/./two/") + MERGED(12, "web")},
           compiled + [["read", "code-generation"]])

    # compile: timestamps as people leave them. Only one place reads a timestamp as a time: a sensor with no
    # ending in a stage that is still open. Everywhere else a timestamp is text, compared as text.
    stamped = lambda event, at, *fields: block(event, at, *fields)
    odd_times = (TITLE + stamped("WORKFLOW_STARTED", "2026-01-05", "**Scope**: bugfix") +
                 stamped("STAGE_STARTED", "2026-01-05T10:01", "**Stage**: intent-capture") +
                 stamped("SENSOR_FIRED", "yesterday", "**Stage slug**: intent-capture", "**Sensor ID**: linter",
                         "**Fire id**: aaaaaaaa") +
                 stamped("SENSOR_FIRED", "2026-01-05T10:02:00+02:00", "**Stage slug**: intent-capture",
                         "**Sensor ID**: linter", "**Fire id**: bbbbbbbb") +
                 stamped("SENSOR_PASSED", "2026-01-05T10:02:01+02:00", "**Fire id**: bbbbbbbb") +
                 stamped("STAGE_COMPLETED", "z-last", "**Stage**: intent-capture") +
                 stamped("STAGE_STARTED", "2026-01-05T10:06:00.500Z", "**Stage**: market-research"))
    check_("compile/timestamps_as_text", {**record, SHARD: odd_times}, compiled)
    orphan = lambda fired, latest="2026-01-05T10:30:00Z": {**record, SHARD: (
        A_WORKFLOW + stamped("SENSOR_FIRED", fired, "**Stage slug**: code-generation", "**Sensor ID**: linter",
                             "**Fire id**: aaaaaaaa") +
        stamped("SENSOR_FIRED", latest, "**Stage slug**: nope", "**Sensor ID**: linter", "**Fire id**: bbbbbbbb"))}
    check_("compile/orphan_with_zone", orphan("2026-01-05T12:11:00.250+02:00"), compiled)
    check_("compile/orphan_no_zone", orphan("2026-01-05T10:11:00"), [["compile"]])
    check_("compile/orphan_junk_after", orphan("2026-01-05T10:11:00Zzz"), [["compile"]])
    check_("compile/orphan_date_only", orphan("2026-01-05"), [["compile"]])
    check_("compile/orphan_latest_no_zone", orphan("2026-01-05T10:11:00Z", "2026-01-05T10:30:00"), [["compile"]])
    # A word where a timestamp goes is no date to anyone: the firing has no age, and is left out.
    check_("compile/orphan_is_a_word", orphan("yesterday"), compiled + [["read", "code-generation"]])
    check_("compile/orphan_latest_is_a_word", orphan("2026-01-05T10:11:00Z", "tomorrow"),
           compiled + [["read", "code-generation"]])
    check_("compile/orphan_word_against_unread", orphan("2026-01-05T10:11:00", "tomorrow"),
           compiled + [["read", "code-generation"]])
    check_("compile/orphan_leap_day", orphan("2026-02-29T10:11:00Z", "2026-03-05T10:30:00Z"), [["compile"]])
    check_("compile/orphan_long_fraction", orphan("2026-01-05T10:11:00.0674309079Z"), [["compile"]])

    # compile: the stage graph it places stages by.
    stage_graph = ".claude/tools/data/stage-graph.json"
    check_("compile/no_stage_graph", {**record, SHARD: A_WORKFLOW}, compiled, edits=[lambda t: t.remove(stage_graph)])
    check_("compile/no_stage_graph_no_workflow", record, compiled, edits=[lambda t: t.remove(stage_graph)])
    check_("compile/broken_stage_graph", {**record, SHARD: A_WORKFLOW}, compiled,
           edits=[lambda t: t.write(stage_graph, "{ nope")])

    def without_agents(tree):
        stages = tree.json(stage_graph)
        for stage in stages:
            if stage["slug"] == "requirements-analysis":
                del stage["lead_agent"]
            if stage["slug"] == "code-generation":
                stage["lead_agent"] = ""
            if stage["slug"] == "intent-capture":
                stage["enabled"] = False
        tree.write_json(stage_graph, stages)
    check_("compile/stage_graph_agents", {**record, SHARD: A_WORKFLOW}, compiled + [["read", "requirements-analysis"]],
           edits=[without_agents])

    # The stage graph as a person might leave it. What the compile wrote, two rows for one slug included, is
    # read as upstream reads it; what it could not have written is refused.
    def edited(change):
        def edit(tree):
            stages = tree.json(stage_graph)
            change(stages)
            tree.write_json(stage_graph, stages)
        return edit
    row_of = lambda stages, slug: next(stage for stage in stages if stage["slug"] == slug)
    check_("compile/stage_graph_slug_twice",
           {**record, SHARD: A_WORKFLOW, DIARY("operation", "intent-capture"): A_DIARY}, compiled,
           edits=[edited(lambda stages: stages.append({**row_of(stages, "intent-capture"), "phase": "operation",
                                                        "lead_agent": "the-later-row"}))])
    check_("compile/diary_under_a_phase_that_looks_absolute",
           {**record, SHARD: A_WORKFLOW, f"{RECORD_DIR}/abs-phase/intent-capture/memory.md": A_DIARY}, compiled,
           edits=[edited(lambda stages: row_of(stages, "intent-capture").update(phase="/abs-phase"))])
    check_("compile/stage_named_dot_dot",
           {**record, SHARD: A_WORKFLOW + STARTED_ROW(11, "..") + COMPLETED_ROW(12, ".."),
            f"{RECORD_DIR}/memory.md": "## Deviations\n"}, compiled,
           edits=[edited(lambda stages: stages.append({**row_of(stages, "requirements-analysis"), "slug": ".."}))])
    no_phase = edited(lambda stages: row_of(stages, "requirements-analysis").pop("phase"))
    phase_7 = edited(lambda stages: row_of(stages, "intent-capture").update(phase=7))
    hand_graph = {**record, f"{RECORD_DIR}/runtime-graph.json": json.dumps(
        {"workflow_id": T(0), "scope": "feature", "started_at": T(0), "stages": [
            {"stage_slug": slug, "completed_at": None, "sensor_firings": [], "outcome": "approved",
             "memory_breakdown": None, "learnings_captured": None}
            for slug in ("intent-capture", "requirements-analysis", "code-generation")]}) + "\n"}
    check_("compile/stage_graph_phase_missing", {**record, SHARD: A_WORKFLOW}, [["compile"]], edits=[no_phase])
    check_("compile/stage_graph_phase_missing_unused", {**record, SHARD: TITLE + WORKFLOW + STARTED_ROW(1, "intent-capture")},
           compiled, edits=[no_phase])
    check_("summary/stage_graph_phase_missing_is_unknown", hand_graph, [["summary", "--json"]], edits=[no_phase])
    check_("summary/stage_graph_phase_null_is_unknown", hand_graph, [["summary", "--json"]],
           edits=[edited(lambda stages: row_of(stages, "intent-capture").update(phase=None))])
    check_("summary/stage_graph_phase_not_text", hand_graph, [["summary", "--json"]], edits=[phase_7])
    check_("compile/stage_graph_null_row", {**record, SHARD: A_WORKFLOW}, [["compile"]],
           edits=[edited(lambda stages: stages.insert(3, None))])
    check_("summary/stage_graph_null_row", hand_graph, [["summary"]], edits=[edited(lambda stages: stages.append(None))])
    check_("compile/stage_graph_not_a_list", {**record, SHARD: A_WORKFLOW}, [["compile"]],
           edits=[lambda t: t.write(stage_graph, '{"stages": []}\n')])
    check_("compile/stage_graph_rows_that_are_not_stages", {**record, SHARD: A_WORKFLOW}, compiled,
           edits=[edited(lambda stages: stages.extend([7, "text", [], {}]))])
    check_("summary/phase_named_like_an_object_member", hand_graph, [["summary", "--json"]],
           edits=[edited(lambda stages: row_of(stages, "intent-capture").update(phase="constructor"))])

    # compile: what settles a tie, and what the ledger's "now" is.
    stamped_fire = lambda at, stage, fire, *fields: stamped("SENSOR_FIRED", at, f"**Stage slug**: {stage}",
                                                            "**Sensor ID**: linter", f"**Fire id**: {fire}", *fields)
    check_("compile/now_is_a_stage_start",
           {**record, SHARD: A_WORKFLOW + FIRED(10, 0, "code-generation", "aaaaaaaa") +
            STARTED_ROW(11, "feasibility", "**Workflow**: single-stage:feasibility") +
            FIRED(10, 45, "code-generation", "bbbbbbbb")}, compiled + [["read", "code-generation"]])
    check_("compile/now_is_an_ending",
           {**record, SHARD: A_WORKFLOW + FIRED(10, 30, "code-generation", "aaaaaaaa") +
            ENDED("FAILED", 11, 30, "zzzzzzzz")}, compiled + [["read", "code-generation"]])
    check_("compile/now_is_no_other_row",
           {**record, SHARD: A_WORKFLOW + FIRED(10, 0, "code-generation", "aaaaaaaa") +
            COMPLETED_ROW(11, "build-and-test") + FORKED(11, "web") + block("RULE_LEARNED", T(11), "**Stage**: x") +
            block("MEMORY_EMPTY", T(11), "**Stage**: intent-capture") + block("WORKFLOW_COMPLETED", T(11))},
           compiled + [["read", "code-generation"]])
    check_("compile/two_endings_at_one_instant",
           {**record, SHARD: A_WORKFLOW + FIRED(2, 0, "intent-capture", "aaaaaaaa") + ENDED("PASSED", 2, 5, "aaaaaaaa") +
            ENDED("FAILED", 2, 5, "aaaaaaaa", "**Detail path**: where") + FIRED(2, 1, "intent-capture", "bbbbbbbb") +
            ENDED("FAILED", 2, 6, "bbbbbbbb", "**Detail path**: where") + ENDED("BUDGET_OVERRIDE", 2, 6, "bbbbbbbb"),
            UNITS: A_UNITS.replace("[auth]", "[nobody]")},
           compiled + [["read", "intent-capture"]])
    check_("compile/empty_diary_recorded_at_the_approval",
           {**record, SHARD: A_WORKFLOW + block("MEMORY_EMPTY", T(9), "**Stage**: requirements-analysis"), **diaries},
           [["compile"]])
    check_("compile/bolts_at_the_edges",
           {**record, SHARD: A_WORKFLOW + FORKED(10, "web", "**Worktree path**: wt/web") + MERGED(10, "web") +
            FORKED(11, "api", "**Worktree path**: wt/api") + MERGED(12, "api") +
            block("BOLT_FAILED", T(12), "**Bolt slug**: api") + FORKED(13, "db", "**Worktree path**: wt/db") +
            # An old firing with no ending in a bolt still open, one in a bolt that closed, one under no bolt.
            FIRED(13, 10, "code-generation", "aaaaaaaa", "linter", "**Output path**: wt/db/a") +
            FIRED(11, 10, "code-generation", "bbbbbbbb", "linter", "**Output path**: wt/api/b") +
            FIRED(10, 10, "code-generation", "cccccccc", "linter", "**Output path**: src/c") +
            FIRED(15, 0, "code-generation", "dddddddd", "linter", "**Output path**: wt/db/d")},
           compiled + [["read", "code-generation"]])
    check_("compile/bolts_named_past_the_basic_plane",
           {**record, SHARD: A_WORKFLOW + FORKED(11, "\ue000") + FORKED(12, "\U0001F600") + FORKED(13, "z") +
            FORKED(14, "\uffff") + FORKED(15, "\U00010000")}, compiled + [["read", "code-generation"]])

    # compile: a file that is a directory where the compile reads or writes.
    check_("compile/graph_is_a_directory", {**record, SHARD: A_WORKFLOW, f"{RECORD_DIR}/runtime-graph.json/x": "x"},
           [["compile"], ["read", "intent-capture"], ["summary"]])
    check_("compile/state_is_a_directory", {**CLONE, "aidlc/spaces/default/intents/active-intent": "add-login-1a2b3c4d\n",
                                            f"{RECORD_DIR}/aidlc-state.md/x": "x", SHARD: A_WORKFLOW}, [["compile"]])
    check_("compile/units_is_a_directory", {**record, SHARD: A_WORKFLOW, UNITS + "/x": "x"}, [["compile"]])

    # compile: links and odd paths.
    def workspace_link(tree):
        shutil.move(tree.path("aidlc"), tree.path("moved"))
        os.symlink(tree.path("moved"), tree.path("aidlc"))
    check_("compile/workspace_is_a_link", {**record, SHARD: A_WORKFLOW}, [["compile"]], edits=[workspace_link])
    def under_a_backslash(tree):
        # Upstream's tool cannot be run from a path with a backslash (Bun takes it for a separator when it
        # looks for the script), so the tool stays where it is and the project it is pointed at moves. Focus
        # finds the stage graph in the project, so the installation goes along.
        shutil.copytree(tree.path(".claude"), tree.path("back\\slash/.claude"))
        shutil.move(tree.path("aidlc"), tree.path("back\\slash/aidlc"))
    # The recording keeps the workspace where an installed project has it, so this tree is not one it holds.
    UNRECORDED.add("runtime/compile/project_path_with_a_backslash")
    check_("compile/project_path_with_a_backslash", {**record, SHARD: A_WORKFLOW},
           [["compile", "--project-dir", "{P}/back\\slash"],
            ["read", "intent-capture", "--project-dir", "{P}/back\\slash"]], edits=[under_a_backslash])

    # compile: which record.
    other = "aidlc/spaces/payments/intents"
    check_("compile/other_space_no_intent",
           {**CLONE, "aidlc/active-space": "payments\n", f"{other}/aidlc-state.md": STATE,
            f"{other}/audit/laptop-abc123.md": A_WORKFLOW, f"{other}/ideation/intent-capture/memory.md": A_DIARY},
           compiled)
    check_("compile/two_records_no_cursor",
           {**CLONE, f"{RECORD_DIR}/aidlc-state.md": STATE, SHARD: A_WORKFLOW,
            "aidlc/spaces/default/intents/fix-logout-9f8e7d6c/aidlc-state.md": STATE}, compiled)
    check_("compile/crlf_shard", {**record, SHARD: A_WORKFLOW.replace("\n", "\r\n")}, compiled)

    # compile: what Focus does not do yet, and an observer.
    empty = {**record, SHARD: A_WORKFLOW, DIARY("inception", "requirements-analysis"): "## Deviations\n"}
    check_("compile/metrics_endpoint", empty, [["compile"]], {"METRICS_ENDPOINT": "http://127.0.0.1:9/x"})
    check_("compile/metrics_endpoint_nothing_to_send", {**record, SHARD: A_WORKFLOW}, [["compile"]],
           {"METRICS_ENDPOINT": "http://127.0.0.1:9/x"})
    pids = {"aidlc/.aidlc-sessions/pids/1.json": "{}\n"}
    check_("compile/sessions_by_process_unmapped", {**record, SHARD: A_WORKFLOW, **pids}, [["compile"]])
    check_("read/sessions_by_process_unmapped", {**record, **pids}, [["read", "x"]])
    check_("summary/sessions_by_process_unmapped", {**record, **pids}, [["summary"]])
    theirs = {**my_pid_entry(), "aidlc/spaces/default/intents/fix-logout-9f8e7d6c/aidlc-state.md": "# State\n",
              "aidlc/spaces/default/intents/fix-logout-9f8e7d6c/audit/laptop-abc123.md": A_WORKFLOW,
              "aidlc/.aidlc-sessions/s1.binding.json": '{"space":"default","intent":"fix-logout-9f8e7d6c","boundAt":"2026-01-05T10:00:00Z","source":"create"}'}
    check_("compile/sessions_by_process", {**record, SHARD: A_WORKFLOW, **theirs}, [["compile"]])
    check_("summary/sessions_by_process", {**record, **theirs}, [["compile"], ["summary"]])
    UNRECORDED.update(("runtime/compile/sessions_by_process", "runtime/summary/sessions_by_process"))
    check_("compile/delegated_worktree", {**record, SHARD: A_WORKFLOW, ".aidlc/worktree-meta.json": "{}\n"},
           [["compile"]])
    check_("compile/unit_claim", {**record, SHARD: A_WORKFLOW, "aidlc/.aidlc-unit-scope.json": "{}\n"}, [["compile"]])
    check_("usage/stray_argument", {**record, SHARD: A_WORKFLOW}, [["compile", "now"]])
    check_("usage/flag_twice", hand_graph, [["summary", "--json", "--json"]])
    check_("usage/flag_without_its_value", {**record, SHARD: A_WORKFLOW}, [["compile", "--project-dir"]])

    # Where the project is, said in other ways.
    a_project = {**record, SHARD: A_WORKFLOW}
    check_("project_dir/before_the_verb", a_project, [["--project-dir", "{P}", "compile"],
                                                      ["--project-dir", "{P}", "read", "intent-capture"]])
    check_("project_dir/relative", a_project, [["compile", "--project-dir", "."],
                                               ["read", "intent-capture", "--project-dir", "./"],
                                               ["summary", "--project-dir", "aidlc/.."]])
    check_("project_dir/trailing_slash", a_project, [["compile", "--project-dir", "{P}/"],
                                                     ["compile", "--project-dir", "{P}//"]])
    check_("project_dir/with_dots", a_project, [["compile", "--project-dir", "{P}/aidlc/../."],
                                                ["read", "intent-capture", "--project-dir", "{P}/nowhere/.."]])
    check_("project_dir/not_there", a_project, [["compile", "--project-dir", "{P}/nowhere"]])
    for marker in ("STOP_HOOK_PROBE", "ROUTE_CHECK"):
        probe = {marker: "1"}
        check_(f"observer/{marker.lower()}/compile", {**record, SHARD: A_WORKFLOW}, compiled, probe)
        check_(f"observer/{marker.lower()}/compile_empty_diary", empty, [["compile"]], probe)
        check_(f"observer/{marker.lower()}/compile_no_ledger", record, [["compile"]], probe)
        check_(f"observer/{marker.lower()}/compile_no_state", CLONE, [["compile"]], probe)
        check_(f"observer/{marker.lower()}/reads",
               {**record, f"{RECORD_DIR}/runtime-graph.json":
                '{"workflow_id":"w","scope":"s","started_at":"","stages":[]}\n'}, [["read", "x"], ["summary"]], probe)

    # read and summary on a graph someone else wrote.
    graph = f"{RECORD_DIR}/runtime-graph.json"
    check_("read/not_json", {**record, graph: "{ nope"}, [["read", "x"], ["summary"]])
    a_stage = {"stage_slug": "intent-capture", "started_at": T(1), "completed_at": T(5), "agent": "a",
               "memory_path": "m", "memory_entries": 2, "memory_breakdown": {"interpretations": 1, "deviations": 0,
                                                                             "tradeoffs": 1, "open_questions": 0},
               "sensor_firings": [{"id": "linter", "fire_id": "a", "result": "passed", "ts": T(2)},
                                  {"id": "linter", "fire_id": "b", "result": "something-new", "ts": T(2)}],
               "outcome": "approved", "learnings_captured": {"from_orchestrator": 2, "from_user_addition": 1}}
    a_graph = lambda *stages, **top: json.dumps({"workflow_id": T(0), "scope": "feature", "started_at": T(0),
                                                 "stages": list(stages), **top}, indent=2) + "\n"
    by_bolts = {**a_stage, "stage_slug": "code-generation", "completed_at": None, "memory_breakdown": None,
                "learnings_captured": None, "outcome": "pending",
                "instances": [{**a_stage, "bolt": "api", "completed_at": T(40), "outcome": "failed"},
                              {**a_stage, "bolt": "web", "completed_at": None, "outcome": "pending",
                               "memory_breakdown": None, "sensor_firings": []}]}
    check_("summary/hand_written",
           {**record, graph: a_graph(a_stage, by_bolts, {**a_stage, "stage_slug": "9", "completed_at": "",
                                                         "extra": {"2": 1, "1": 2, "b": [1.5, 1e21, -0.0]}},
                                     {**a_stage, "stage_slug": "not-in-the-stage-graph", "completed_at": T(41, 29),
                                      "memory_entries": None}, extra=True)},
           [["summary", "--json"], ["summary"], ["read", "9"], ["read", "code-generation"]])
    check_("summary/half_minutes",
           {**record, graph: a_graph({**a_stage, "completed_at": T(0, 30)})}, [["summary", "--json"]])
    check_("summary/ends_before_it_starts",
           {**record, graph: a_graph({**a_stage, "completed_at": "2026-01-05T09:00:00Z"})}, [["summary"]])
    check_("summary/no_stages", {**record, graph: a_graph()}, [["summary", "--json"], ["summary"], ["read", "x"]])
    # JSON that JavaScript reads and focus's reader does not.
    with_extra = lambda extra: a_graph(a_stage).replace('"scope": "feature"', f'"scope": "feature", "extra": {extra}')
    check_("read/json_number_out_of_range", {**record, graph: with_extra("1e400")}, [["read", "intent-capture"]])
    check_("read/json_half_pair_escape", {**record, graph: with_extra('"\\ud800"')}, [["read", "intent-capture"]])
    check_("read/json_nested_deeper_than_127", {**record, graph: with_extra("[" * 200 + "]" * 200)},
           [["read", "intent-capture"]])
    check_("read/json_nested_127_deep", {**record, graph: with_extra("[" * 120 + "]" * 120)}, [["read", "intent-capture"]])
    check_("summary/duration_of_a_word",
           {**record, graph: a_graph({**a_stage, "completed_at": "soon"})}, [["summary", "--json"], ["summary"]])
    check_("summary/duration_no_zone",
           {**record, graph: a_graph({**a_stage, "completed_at": "2026-01-05T10:05:00"})}, [["summary", "--json"]])
    check_("summary/duration_date_only",
           {**record, graph: a_graph({**a_stage, "completed_at": T(5)}).replace(f'"started_at": "{T(0)}",\n  "stages"',
                                                                               '"started_at": "2026-01-05",\n  "stages"')},
           [["summary", "--json"]])
    not_graphs = {
        "list": "[]", "null": "null", "no_stages": "{}", "stages_not_a_list": '{"stages": {}}',
        "null_stage": a_graph(None), "stage_without_slug": a_graph({"outcome": "approved", "sensor_firings": []}),
        "unknown_outcome": a_graph({**a_stage, "outcome": "odd"}),
        "no_firings": a_graph({k: v for k, v in a_stage.items() if k != "sensor_firings"}),
        "count_not_a_count": a_graph({**a_stage, "memory_entries": "2"}),
        "learnings_not_an_object": a_graph({**a_stage, "learnings_captured": True}),
        "breakdown_not_an_object": a_graph({**a_stage, "memory_breakdown": "none"}),
        # Each count is one a double holds exactly, and their sum is not.
        "counts_past_exact": a_graph(
            {**a_stage, "learnings_captured": {"from_orchestrator": 9007199254740991, "from_user_addition": 0}},
            {**a_stage, "stage_slug": "requirements-analysis",
             "learnings_captured": {"from_orchestrator": 1, "from_user_addition": 0}}),
    }
    # `read` looks at less of the graph than `summary` does, so less can be wrong with it.
    for name in ("list", "stages_not_a_list", "null_stage", "stage_without_slug", "unknown_outcome", "no_firings",
                 "count_not_a_count"):
        check_(f"read/not_a_graph_{name}", {**record, graph: not_graphs[name]}, [["read", "intent-capture"]])
    for name in ("null", "no_stages", "null_stage", "stage_without_slug", "unknown_outcome", "no_firings",
                 "count_not_a_count", "learnings_not_an_object", "breakdown_not_an_object", "counts_past_exact"):
        check_(f"summary/not_a_graph_{name}", {**record, graph: not_graphs[name]}, [["summary", "--json"]])
    check_("summary/completed_workflow",
           {**RECORD_FILES, SHARD: A_WORKFLOW, f"{RECORD_DIR}/aidlc-state.md":
            "# AI-DLC State\n\n- **Scope**: feature\n- **Status**: Completed\n\n## Stage Progress\n"
            "- [x] intent-capture — EXECUTE\n- [ ] requirements-analysis — EXECUTE\n"
            "- [S] user-stories — EXECUTE\n- [x] market-research — EXECUTE\n"
            "- [-] code-generation — SKIP: no\n- [x] made-up — EXECUTE\n"},
           [["compile"], ["summary", "--json"], ["summary"]])


# What the random ledgers turned out to hold, so that a run of the fuzz says what it compared.
GRAPHS = {}
GRAPHS_LOCK = __import__("threading").Lock()


def seen_in_graphs(files):
    with GRAPHS_LOCK:
        tally_graphs(files)


def tally_graphs(files):
    count = lambda what, by=1: GRAPHS.__setitem__(what, GRAPHS.get(what, 0) + by)
    for rel, text in files.items():
        if rel.endswith("/audit/laptop-abc123.md"):
            count("MEMORY_EMPTY events written",
                  sum(1 for found in re.finditer(r"\*\*Timestamp\*\*: (\S+)\r?\n\*\*Event\*\*: MEMORY_EMPTY", text)
                      if found.group(1) >= STARTED))
        if not rel.endswith("/runtime-graph.json"):
            continue
        graph = json.loads(text)
        count("graphs")
        count("graphs with no workflow", graph["workflow_id"] == "")
        count("graphs with the units' order", "bolt_dag" in graph)
        for stage in graph["stages"]:
            count("stages")
            count(f"stages {stage['outcome']}")
            count("stages with a diary", stage["memory_entries"] is not None)
            count("stages with learnings", bool(stage["learnings_captured"]) and
                  sum(stage["learnings_captured"].values()) > 0)
            count("stages built by bolts", "instances" in stage)
            for unit in [stage, *stage.get("instances", [])]:
                for firing in unit["sensor_firings"]:
                    count(f"firings {firing['result']}")
            for instance in stage.get("instances", []):
                count(f"bolts {instance['outcome']}")


def runtime_fuzz(count):
    """Random ledgers, diaries and units files; each compiled twice, summed up and read."""
    rng = random.Random(SEED + 2)
    with open(os.path.join(DIST, ".claude", "tools", "data", "stage-graph.json"), encoding="utf-8") as handle:
        phase_of = {stage["slug"]: stage["phase"] for stage in json.load(handle)}
    real = ["intent-capture", "market-research", "requirements-analysis", "units-generation", "functional-design",
            "code-generation", "build-and-test", "deployment-execution"]
    fires = ["aaaaaaaa", "bbbbbbbb", "cccccccc", "dddddddd", "eeeeeeee", ""]
    bolts = ["api", "web", "db", "Zeta", ""]
    paths = ["wt/api", "<project-dir>/wt/web", "<project-dir>", "/abs/db", "wt/api/../web/x.ts", "src/main.ts", "",
             "wt/apiary/x"]

    def at():
        roll = rng.random()
        if roll < 0.9:
            return T(rng.randrange(0, 30), rng.choice([0, 0, 0, 20, 40, 59]))
        if roll < 0.94:
            return f"2026-01-05T{rng.randrange(11, 14)}:{rng.randrange(60):02d}:00{rng.choice(['+01:00', '.250Z', '-00:30'])}"
        if roll < 0.97:
            return T(rng.randrange(0, 30))[:-1] + rng.choice([".5Z", ".000Z", "Z"])
        return rng.choice(["yesterday", "soon", "z-last", "-"])

    def row():
        stage = rng.choice(real * 3 + ["not-a-stage"])
        label = lambda name, value: rng.choice([f"**{name}**: {value}"] * 12 + [
            f"**{name}**:{value}", f"**{name}**:   {value}  ", f"- **{name}**: {value}", f"**{name}**:",
            f"**{name}**: {value}\n**{name}**: other", f" **{name}**: {value}"])
        roll = rng.random()
        if roll < 0.22:
            extra = rng.choice([[], [], [label("Agent", rng.choice(["a-agent", "", " spaced "]))],
                                [label("Workflow", "single-stage:" + stage)], [label("Workflow", "main")]])
            return block("STAGE_STARTED", at(), label("Stage", stage), *extra)
        if roll < 0.38:
            return block("STAGE_COMPLETED", at(), label("Stage", stage))
        if roll < 0.56:
            extra = [label("Output path", rng.choice(paths))] if rng.random() < 0.5 else []
            return block("SENSOR_FIRED", at(), label("Stage slug", stage),
                         label("Sensor ID", rng.choice(["linter", "types", ""])), label("Fire id", rng.choice(fires)),
                         *extra)
        if roll < 0.7:
            extra = [label("Detail path", rng.choice(["aidlc/x/detail.md", ""]))] if rng.random() < 0.5 else []
            return block(rng.choice(["SENSOR_PASSED", "SENSOR_FAILED", "SENSOR_FAILED", "SENSOR_BUDGET_OVERRIDE"]), at(),
                         label("Fire id", rng.choice(fires)), *extra)
        if roll < 0.78:
            extra = [label("Source", rng.choice(["user_addition", "orchestrator", ""]))] if rng.random() < 0.6 else []
            return block(rng.choice(["RULE_LEARNED", "SENSOR_PROPOSED"]), at(), label("Stage", stage), *extra)
        if roll < 0.9:
            extra = [label("Worktree path", rng.choice(paths))] if rng.random() < 0.6 else []
            return block(rng.choice(["STATE_FORKED", "STATE_FORKED", "STATE_MERGED", "BOLT_FAILED"]), at(),
                         label("Bolt slug", rng.choice(bolts)), *extra)
        if roll < 0.94:
            return block("MEMORY_EMPTY", at(), label("Stage", stage))
        if roll < 0.97:
            return block("WORKFLOW_STARTED", at(), *([label("Scope", rng.choice(["mvp", "", "bugfix"]))]
                                                    if rng.random() < 0.7 else []))
        return note("A decision", at(), "We chose.", "**Stage**: code-generation")

    diary_lines = ["## Interpretations", "## Deviations", "## Tradeoffs", "## Open questions", "## Notes", "- one", "- two",
                   "> quoted", "<!-- hidden -->", "```", "", "constructor", "plain"]
    def timeline():
        """A workflow as an engine would log it: stages one after another, each with what happens inside it."""
        clock = [0]

        def tick(most=90):
            clock[0] += rng.choice([0, 1, 5, 20, 61, rng.randrange(most + 1)])
            return T(clock[0] // 60 % 60, clock[0] % 60) if clock[0] < 3600 else f"2026-01-05T11:{clock[0] // 60 % 60:02d}:{clock[0] % 60:02d}Z"

        rows = [block("WORKFLOW_STARTED", tick(0), "**Scope**: bugfix")]
        fire = [0]
        for stage in rng.sample(real, rng.randrange(1, len(real) + 1)) + rng.sample(real, rng.randrange(0, 2)):
            agent = [f"**Agent**: {rng.choice(['a-agent', 'b-agent'])}"] if rng.random() < 0.4 else []
            rows.append(block("STAGE_STARTED", tick(), f"**Stage**: {stage}", *agent))
            forked = []
            if phase_of[stage] == "construction" and rng.random() < 0.7:
                for bolt in rng.sample(bolts[:4], rng.randrange(1, 4)):
                    worktree = rng.choice([f"wt/{bolt}", f"<project-dir>/wt/{bolt}", "", None])
                    rows.append(block("STATE_FORKED", tick(), f"**Bolt slug**: {bolt}",
                                      *([] if worktree is None else [f"**Worktree path**: {worktree}"])))
                    forked.append((bolt, worktree))
            for _ in range(rng.randrange(0, 5)):
                fire[0] += 1
                fire_id = f"{fire[0]:08x}"
                under = rng.choice(forked)[1] if forked and rng.random() < 0.7 else None
                output = [f"**Output path**: {under}/src/{fire_id}.ts"] if under else (
                    ["**Output path**: src/main.ts"] if rng.random() < 0.3 else [])
                rows.append(block("SENSOR_FIRED", tick(), f"**Stage slug**: {stage}",
                                  f"**Sensor ID**: {rng.choice(['linter', 'types', 'tests'])}", f"**Fire id**: {fire_id}",
                                  *output))
                for _ in range(rng.choice([0, 1, 1, 1, 1, 2])):
                    kind = rng.choice(["PASSED", "PASSED", "FAILED", "BUDGET_OVERRIDE"])
                    detail = ["**Detail path**: aidlc/x/detail.md"] if kind == "FAILED" and rng.random() < 0.7 else []
                    rows.append(block(f"SENSOR_{kind}", tick(30), f"**Fire id**: {fire_id}", *detail))
            for _ in range(rng.choice([0, 0, 1, 2])):
                source = rng.choice([[], ["**Source**: user_addition"], ["**Source**: orchestrator"]])
                rows.append(block(rng.choice(["RULE_LEARNED", "SENSOR_PROPOSED"]), tick(), f"**Stage**: {stage}", *source))
            for bolt, _ in forked:
                roll = rng.random()
                if roll < 0.6:
                    rows.append(block("STATE_MERGED", tick(), f"**Bolt slug**: {bolt}"))
                elif roll < 0.8:
                    rows.append(block("BOLT_FAILED", tick(), f"**Bolt slug**: {bolt}"))
            if rng.random() < 0.75:
                rows.append(block("STAGE_COMPLETED", tick(), f"**Stage**: {stage}"))
        return rows

    for index in range(count):
        rows = timeline() if rng.random() < 0.8 else []
        # And rows nobody would log: any event at any time, labels written every way.
        noise = [row() for _ in range(rng.randrange(0, 12 if rows else 40))]
        for one in noise:
            rows.insert(rng.randrange(len(rows) + 1), one)
        shards = [[], [], []]
        for one in rows:
            shards[rng.choice([0, 0, 0, 1, 2])].append(one)
        files = dict(RECORD_FILES)
        for rows_of, name in zip(shards, ("laptop-abc123", "desktop-ffffffffffff", "tablet-0123456789ab")):
            if rows_of:
                text = TITLE + "".join(rows_of)
                files[f"{RECORD_DIR}/audit/{name}.md"] = text.replace("\n", "\r\n") if rng.random() < 0.1 else text
        state = ["# AI-DLC State", ""]
        if rng.random() < 0.7:
            state.append(f"- **Scope**: {rng.choice(['feature', '', ' padded '])}")
        if rng.random() < 0.3:
            state += ["- **Status**: Completed", "", "## Stage Progress"]
            state += [f"- [{rng.choice('x x-S?R')}] {slug} — {rng.choice(['EXECUTE', 'EXECUTE', 'SKIP: no'])}"
                      for slug in rng.sample(real + ["made-up"], rng.randrange(0, 7))]
        if rng.random() < 0.95:
            files[f"{RECORD_DIR}/aidlc-state.md"] = "\n".join(state) + "\n"
        for slug in rng.sample(real, rng.randrange(0, 7)):
            files[DIARY(phase_of[slug], slug)] = rng.choice(["", "## Deviations\n", "# Memory\n\n## Interpretations\n\n"]) if (
                rng.random() < 0.4) else "\n".join(rng.choice(diary_lines) for _ in range(rng.randrange(0, 9))) + "\n"
        if rng.random() < 0.3:
            files[UNITS] = rng.choice([A_UNITS, A_UNITS, A_UNITS.replace("[auth]", "[web]"), "# none\n",
                                       A_UNITS.replace("kind: service", "kind: big")])
        runtime_check(str(index), files, [["compile"], ["compile"], ["summary", "--json"], ["summary"],
                                          ["read", rng.choice(real)]], group="fuzz/runtime", tool="runtime")


def audit_fuzz(count):
    """Random sequences of audit commands on a record that already has history, each ending in a history read."""
    rng = random.Random(SEED + 1)
    keys = ["Stage", "Stage", "Note", "Tool", "Bolt slug", "Details (raw)", "A b.c_(d)/e-f", "Timestamp", "Workflow",
            "x", "Z9", "Event", "1", "9x", "é", "a*b", " lead", "with\nnewline"]
    values = ["code-generation", "requirements-analysis", "", "  spaced  ", "a=b", "a\r\nb", "a\rb", "a\nb",
              "a\u2028b", "x\u0085y", "**Event**: HUMAN_TURN", "\n---\n## Forged\n**Event**: GATE_APPROVED",
              "é \U0001F600", "{P}/src/main.rs", "at {P}", "{P}x", "aidlc-state", "<project-dir>", "tab\there",
              "trailing\\", '"quoted"', "- bullet", "**Stage**: other"]
    events = [event for event in EVENT_TYPES if event.startswith(("STAGE_S", "ERROR", "SENSOR", "PHASE", "WORKFLOW"))]
    lines = ["plain", "", "**Event**: MY_NOTE", "**Event**: GATE_APPROVED", "- **Event**: ERROR_LOGGED", "**Event**:",
             "**Stage**: code-generation", "## Inner heading", "---", "**Timestamp**: 2026-01-01T00:00:00Z",
             "see {P}/x", "é \U0001F600", " indented", "\ttab"]

    def fields():
        return {rng.choice(keys if rng.random() < 0.25 else keys[:11]):
                rng.choice(values) for _ in range(rng.randrange(0, 4))}

    def step():
        roll = rng.random()
        if roll < 0.4:
            args = ["append", rng.choice(events) if rng.random() < 0.9 else rng.choice(EVENT_TYPES + ["NOPE"])]
            for key, value in fields().items():
                args += ["--field", f"{key}={value}"]
            return args
        if roll < 0.65:
            rows = [rng.choice([ROW(rng.choice(events), **fields())] * 8 + [
                ROW("GATE_APPROVED"), ROW("HUMAN_TURN"), {"eventType": rng.choice(events)},
                {"eventType": rng.choice(events), "fields": {"Note": 5}}, None, "text"])
                for _ in range(rng.randrange(1, 4))]
            return ["append-batch", json.dumps(rows, ensure_ascii=rng.random() < 0.5)]
        if roll < 0.9:
            heading = rng.choice(["A decision", "Why", "At {P}/x", "é", "## Twice", "two words  ", "tab\there"])
            body = rng.choice(["\\n", "\n", "\r\n"]).join(rng.choice(lines) for _ in range(rng.randrange(1, 5)))
            return ["append-raw", heading, body or "x"]
        args = ["history"]
        if rng.random() < 0.5:
            args += ["--stage", rng.choice(["code-generation", "requirements-analysis", "nope"])]
        if rng.random() < 0.5:
            args += ["--event", rng.choice(events + ["NOTE", "GATE_APPROVED"])]
        if rng.random() < 0.4:
            args += ["--limit", rng.choice(["1", "2", "5", "0", "x"])]
        return args

    for index in range(count):
        files = rng.choice([RECORD_FILES, HISTORY_FILES, HISTORY_FILES, CLONE])
        env = {"ALLOW_DIRECT_AUDIT_EVENTS": "1"} if rng.random() < 0.15 else None
        audit_check(str(index), files, [step() for _ in range(rng.randrange(2, 6))] + [["history"]], env=env,
                    group="fuzz/audit")


# --------------------------------------------------------------------------
# Markdown: a document's structure, line by line.
# --------------------------------------------------------------------------
#
# Not a command: the two analyses are asked through the oracle (upstream's
# `markdownBlocks` and `visibleMarkdownLines` by driver.ts, Focus's by
# `focus dev oracle`) about documents nobody would write on purpose: random
# nesting, unclosed constructs, stray characters. On documents written with
# care the two agree line for line, and those are in the vendored corpus.
# Here they are not expected to: upstream approximates the structure, and
# each line is judged with a third parser as referee (markdown_rule.py).

DEVIATIONS.update({
    "fuzz/markdown/commonmark":
        "upstream's analysis leaves the line unplaced, or places it otherwise than CommonMark does and the "
        "referee reads it as focus does; focus reads the structure from a parser",
    "fuzz/markdown/reading":
        "focus's parser reads the line otherwise than upstream's renderer and the referee do, or all three "
        "differ: corners of tables, of tabs in nested containers and of raw HTML that runs over lines, in "
        "documents nobody writes; counted, and refused above the ceiling",
})
# The share of documents that may hold a line only Focus reads its way. It is
# 1.6% at the hostility used here; a defect in Focus's analysis shows as
# several times that.
READING_CEILING = 0.05


def markdown_fuzz(count):
    rng = random.Random(SEED)
    write = markdown_docs.writer(rng, careful=False)
    cases = [{"id": f"markdown-{index:05d}", "kind": "markdown", "content": write()} for index in range(count)]
    asked = os.path.join(WORK, "markdown.ndjson")
    with open(asked, "w", encoding="utf-8") as handle:
        for case in cases:
            handle.write(json.dumps(case, ensure_ascii=False) + "\n")
    driver = os.path.join(os.path.dirname(os.path.abspath(__file__)), "driver.ts")
    theirs = subprocess.run([BUN, driver, UPSTREAM, asked], capture_output=True, text=True, encoding="utf-8")
    if theirs.returncode != 0:
        sys.exit(f"differential.py: the oracle driver failed (are the referee's packages installed? "
                 f"`bun install` in tools/oracle/referee)\n{theirs.stderr[-2000:]}")
    ours = subprocess.run([FOCUS, "dev", "oracle", asked], capture_output=True, text=True, encoding="utf-8", check=True)
    tally = markdown_rule.Tally()
    # Split on line feeds only: a document may hold other line separators, and JSON leaves them as they are.
    wanted, given = (out.rstrip("\n").split("\n") for out in (theirs.stdout, ours.stdout))
    if not len(cases) == len(wanted) == len(given):
        sys.exit(f"differential.py: {len(cases)} documents asked, {len(wanted)} answered by upstream, "
                 f"{len(given)} by focus")
    for case, want, got in zip(cases, wanted, given):
        want, got = json.loads(want), json.loads(got)
        if not case["id"] == want["id"] == got["id"]:
            sys.exit(f"differential.py: answers out of step at {case['id']}")
        tally.add(case, want["out"], got["out"])
    # A document is one check: different when something in it has no
    # explanation, a deviation when a line was judged to be one, else same.
    counted = {"same": 0, "deviation": 0, "DIFFERENT": 0, "reading": 0}
    for verdicts, failed in tally.held:
        if failed or "port" in verdicts:
            counted["DIFFERENT"] += 1
        elif verdicts & set(markdown_rule.READING + ("follows a reading",)):
            counted["reading"] += 1
        elif verdicts - {"same", "unplaced"}:
            counted["deviation"] += 1
        else:
            counted["same"] += 1
    over = counted["reading"] > READING_CEILING * len(cases)
    counted["DIFFERENT" if over else "deviation"] += counted.pop("reading")
    for verdict, documents in counted.items():
        results[verdict] += documents
    print(f"\nmarkdown: {tally.summary()}")
    for verdict, examples in tally.examples.items():
        if EXPLAIN or verdict == "port":
            for example in examples[:3]:
                print(f"           {verdict}: {example[:300]}")
    for failure in tally.failures[:5]:
        print(f"DIFFERENT fuzz/markdown {failure[:300]}")
    if tally.of("port"):
        print(f"DIFFERENT fuzz/markdown: {tally.of('port')} lines differ with nothing in the document's structure "
              f"to explain it")
    apart = tally.of("no text", "upstream alone", "repair column", "follows")
    if apart:
        print(f"deviation fuzz/markdown/commonmark ({apart} lines): {DEVIATIONS['fuzz/markdown/commonmark']}")
    reading = tally.of(*markdown_rule.READING, "follows a reading")
    if over:
        print(f"DIFFERENT fuzz/markdown/reading: more than {READING_CEILING:.0%} of the documents hold a line only "
              f"focus reads its way ({reading} lines)")
    elif reading:
        print(f"deviation fuzz/markdown/reading ({reading} lines): {DEVIATIONS['fuzz/markdown/reading']}")


try:
    for name, edits in COMPILE.items():
        compile_check(name, *edits)
    for name, edits in COMPILE_CHECKS.items():
        compile_check(name, *edits, args=("compile", "--check"))
    # A second compile changes nothing.
    if not ONLY or re.search(ONLY, "compile/twice"):
        twice = pair("compile/twice", with_record("spike"), grid_only)
        check("compile/twice_first", twice, ["compile"], files=True)
        check("compile/twice_second", twice, ["compile"], files=True)
        check("compile/twice_check", twice, ["compile", "--check"], files=True)
    verbs()
    audit_checks()
    runtime_checks()
    state_checks()
    utility_checks()
    intent_create_checks()
    transition_checks()
    gate_checks()
    park_checks()
    jump_checks()
    log_checks()
    status_checks()
    hook_checks()
    run_queued()
    if FUZZ:
        if not ONLY or not re.search(ONLY, "fuzz/audit"):
            fuzz(FUZZ)
        audit_fuzz(FUZZ // 3)
        runtime_fuzz(FUZZ // 4)
        transition_fuzz(FUZZ // 4)
        jump_fuzz(FUZZ // 6)
        run_queued(quiet=True)
    if MARKDOWN:
        markdown_fuzz(MARKDOWN)
finally:
    shutil.rmtree(WORK, ignore_errors=True)

if RECORD and not ONLY and not results["DIFFERENT"]:
    os.makedirs(RECORD, exist_ok=True)
    with open(os.path.join(RECORD, "trees.json"), "w", encoding="utf-8") as handle:
        json.dump(dict(sorted(RECORDED_TREES.items())), handle, indent=1, ensure_ascii=False, sort_keys=True)
        handle.write("\n")
    with open(os.path.join(RECORD, "checks.ndjson"), "w", encoding="utf-8") as handle:
        for name in sorted(RECORDED_CHECKS):
            handle.write(json.dumps(RECORDED_CHECKS[name], ensure_ascii=False, sort_keys=True) + "\n")
    print(f"recorded {len(RECORDED_CHECKS)} checks over {len(RECORDED_TREES)} trees into {RECORD}")

total = sum(results.values())
print(f"\n{total} checks: {results['same']} same, {results['deviation']} known deviation, "
      f"{results['DIFFERENT']} different")
if GRAPHS:
    print("fuzz/runtime compared: " + ", ".join(f"{count} {what}" for what, count in sorted(GRAPHS.items())))
for name, count in shapes.items():
    if count:
        print(f"{count} of the same after a difference of shape ({name}): {SHAPES[name]}")
sys.exit(1 if results["DIFFERENT"] else 0)
