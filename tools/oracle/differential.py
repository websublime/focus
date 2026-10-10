#!/usr/bin/env python3
"""Run upstream's engine and focus on the same project trees and compare.

    differential.py <upstream checkout with dist/ built> [focus binary]
                    [--fuzz N] [--seed S] [--only PATTERN] [--record DIR] [--explain]

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
FUZZ = int(option("--fuzz", "0"))
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
    r"verbs/bad_scope/grid_matched_.*":
        "with a scope file that does not load, upstream falls back to 'strict' inside a try/catch and prints a "
        "verdict about Guard Policy; focus reports the scope file",
}

# Differences of shape that are settled before comparing, and counted.
SHAPES = {
    "uncaught":
        "upstream lets a refusal thrown inside an audit verb reach the runtime, which prints a source excerpt "
        "and a stack trace around the sentence; focus prints the same sentence as {\"error\": ...}, like every "
        "other refusal of the command. The sentence is compared",
}

results = {"same": 0, "deviation": 0, "DIFFERENT": 0}
shapes = {name: 0 for name in SHAPES}
# A timestamp at or after this instant was written during the run.
STARTED = time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime(time.time() - 2))


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


def pair(name, *edits):
    """The same tree twice, one per engine in its own names, with `edits` applied to both."""
    trees = []
    for engine in ("ts", "rs"):
        root = os.path.join(WORK, name.replace("/", "_"), engine)
        shutil.copytree(DIST if engine == "ts" else focus_base(), root, symlinks=True)
        tree = Tree(root, mapped=engine == "rs")
        for edit in edits:
            edit(tree)
        trees.append(tree)
    return trees


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
    return files


def first_difference(a, b):
    a_lines, b_lines = a.split("\n"), b.split("\n")
    for index in range(max(len(a_lines), len(b_lines))):
        left = a_lines[index] if index < len(a_lines) else "<end>"
        right = b_lines[index] if index < len(b_lines) else "<end>"
        if left != right:
            return f"line {index + 1}\n             upstream: {left[:300]}\n             focus:    {right[:300]}"
    return "identical"


JOBS = []

# The part of an installed project the graph verbs read or write. Recording
# stores each tree as its difference from this subset of upstream's
# distribution, which refresh.sh vendors as fixtures/upstream/installed/claude.
BASE_DIRS = (".claude/aidlc-common/stages", ".claude/agents", ".claude/scopes", ".claude/sensors", "aidlc")
BASE_FILES = tuple(f".claude/tools/data/{name}"
                   for name in ("harness.json", "stage-graph.json", "scope-grid.json", "ars-priors.json"))
RECORDED_TREES = {}
RECORDED_CHECKS = {}
LARGE = 3000


def relevant_files(root):
    found = {}
    # A settings path can hold a directory where the file goes; walking a file finds nothing.
    for directory in (*BASE_DIRS, "p", "machine", "aidlc.settings.json", "aidlc.settings.local.json"):
        for folder, _, names in os.walk(os.path.join(root, directory)):
            for name in names:
                path = os.path.join(folder, name)
                rel = os.path.relpath(path, root)
                if os.path.islink(path) or "/.aidlc-" in f"/{rel}" and "composer-proposal" not in rel:
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


def overlay(tree):
    """`tree` as its difference from the base: {relative path: text, or None when removed}."""
    import hashlib
    if not _base_cache:
        for rel, path in relevant_files(DIST).items():
            with open(path, "rb") as handle:
                _base_cache[rel] = handle.read()
    now = {}
    for rel, path in relevant_files(tree.root).items():
        with open(path, "rb") as handle:
            now[rel] = handle.read()
    diff = {}
    for rel, data in now.items():
        if _base_cache.get(rel) != data:
            diff[rel] = data.decode("utf-8")
    for rel in _base_cache:
        if rel not in now:
            diff[rel] = None
    # Stored under Focus's names, like the base the replay starts from.
    diff = {to_focus(rel): text if text is None else to_focus(text) for rel, text in diff.items()}
    key = hashlib.sha256(json.dumps(diff, sort_keys=True).encode()).hexdigest()[:16]
    RECORDED_TREES[key] = diff
    return key


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

    with ThreadPoolExecutor(max_workers=os.cpu_count() or 2) as pool:
        list(pool.map(work, groups.values()))
    for index in sorted(lines):
        verdict, name, detail = lines[index]
        results[verdict] += 1
        if verdict != "same" or not quiet:
            print(f"{verdict:<9} {name:<46} {detail}")
    JOBS.clear()


def compare(name, trees, args, env, stdin, files):
    if files == "audit":
        return compare_audit(name, trees, args, env)
    ts, rs = trees
    outcome = {}
    started_from = overlay(ts) if RECORD and not has_links(ts.root) else None
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
    for key in sorted(set(outcome["ts"]) | set(outcome["rs"])):
        left, right = outcome["ts"].get(key, "<absent>"), outcome["rs"].get(key, "<absent>")
        if left != right:
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

    def on(tree, name, args, env=None, stdin=None):
        label = f"verbs/{tree}/{name}"
        if ONLY and not re.search(ONLY, label):
            return
        check(label, trees(tree), args, env, stdin)

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
    text = text.replace("Usage: aidlc-audit ", "Usage: <cli> audit ").replace("Usage: focus engine audit ",
                                                                             "Usage: <cli> audit ")
    for tool in ("orchestrate", "log"):
        suffix = " report" if tool == "orchestrate" else ""
        text = text.replace(f"aidlc-{tool}.ts{suffix}", f"<cli> {tool}{suffix}")
        text = text.replace(f"`focus engine {tool}{suffix}`", f"<cli> {tool}{suffix}")
    text = STAMP.sub(lambda found: "<now>" if found.group(0) >= STARTED else found.group(0), text)
    text = re.sub(r"(could not be opened: [^\n(]*\()[^\n]*?\)(?=\"\}|\n|$)", r"\1<runtime>)", text)
    return text


def uncaught(stderr):
    """The sentence of an error upstream let reach the runtime, or None when stderr is not one."""
    found = re.search(r"^error: (.*?)\n +at ", stderr, flags=re.M | re.S)
    if not found or "\nBun v" not in stderr:
        return None
    return found.group(1)


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
            # Focus's lock is a file in the workspace; upstream's is a directory elsewhere.
            if os.path.basename(rel).startswith(".focus-lock"):
                continue
            if os.path.islink(path):
                files[rel] = "<link>"
            elif not os.path.isfile(path):
                files[rel] = "<not a file>"
            else:
                with open(path, "rb") as handle:
                    files[rel] = say(handle.read().decode("utf-8", errors="replace"))
    return files


def minted(rel, text):
    """A path and content with a clone identity minted during the run written as placeholders."""
    rel = re.sub(r"/audit/[a-z0-9][a-z0-9-]*-[0-9a-f]{12}\.md$", "/audit/<host>-<token>.md", rel)
    if rel.endswith("/.focus-clone-id") and text is not None:
        text = re.sub(r"\A[0-9a-f]{12}\n[a-z0-9][a-z0-9-]*\n\Z", "<token>\n<host>\n", text)
    return rel, text


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


def run_audit(engine, tree, args, env):
    full = dict(os.environ)
    for key in list(full):
        if key.startswith(("AIDLC_", "FOCUS_")) or key == "CLAUDE_PROJECT_DIR":
            del full[key]
    # `{P}` in an argument is the project's own location.
    args = [arg.replace("{P}", tree.root) for arg in args]
    if engine == "ts":
        full.update({f"AIDLC_{k}": v for k, v in (env or {}).items()})
        command = [BUN, AUDIT_TOOL, *args, "--project-dir", tree.root]
    else:
        full.update({f"FOCUS_{k}": to_focus(v) for k, v in (env or {}).items()})
        command = [FOCUS, "engine", "audit", *map(to_focus, args), "--project-dir", tree.root, "--harness", "claude"]
    done = subprocess.run(command, cwd=tree.root, env=full, capture_output=True, text=True, timeout=120)
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


def compare_audit(name, trees, steps, env):
    ts, rs = trees
    outcome, shaped = {}, False
    started_from = None
    if RECORD and not has_links(ts.root) and name not in UNRECORDED and not name.startswith("fuzz/"):
        started_from = workspace_files(ts, "")
    for engine, tree in (("ts", ts), ("rs", rs)):
        say = to_focus if engine == "ts" else (lambda text: text)
        before = workspace_files(tree)
        exits, outs, errs = [], [], []
        for args in steps:
            code, out, err = run_audit(engine, tree, args, env)
            if engine == "ts" and code != 0:
                sentence = uncaught(err)
                if sentence is not None:
                    err = json.dumps({"error": sentence}, ensure_ascii=False, separators=(",", ":")) + "\n"
                    shaped = True
            exits.append(str(code))
            outs.append(say(normalise_audit(out, tree, engine)))
            errs.append(say(normalise_audit(err, tree, engine)))
        mark = (lambda parts: parts[0]) if len(steps) == 1 else (
            lambda parts: "".join(f"[{index}] {part}" if part.endswith("\n") or not part else f"[{index}] {part}\n"
                                  for index, part in enumerate(parts)))
        outcome[engine] = {"exit": ",".join(exits), "stdout": mark(outs), "stderr": mark(errs)}
        after = workspace_files(tree)
        for rel in sorted(set(before) | set(after)):
            if before.get(rel) == after.get(rel):
                continue
            text = after.get(rel)
            shown, text = minted(rel, text if text is None else normalise_audit(text, tree, engine))
            outcome[engine][shown] = "<removed>" if text is None else text
    problems = []
    for key in sorted(set(outcome["ts"]) | set(outcome["rs"])):
        left, right = outcome["ts"].get(key, "<absent>"), outcome["rs"].get(key, "<absent>")
        if left != right:
            problems.append(f"{key}: {first_difference(left, right)}")
    if not problems:
        verdict = "same"
        said = (outcome["ts"]["stderr"] or outcome["ts"]["stdout"]).strip().split("\n")[0][:110]
        detail = f"exit {outcome['ts']['exit'][:40]}" + (f": {said}" if said else "")
        if shaped:
            shapes["uncaught"] += 1
            detail += "  (upstream: uncaught)"
    elif any(re.fullmatch(pattern, name) for pattern in DEVIATIONS):
        verdict = "deviation"
        detail = next(reason for pattern, reason in DEVIATIONS.items() if re.fullmatch(pattern, name))
        if EXPLAIN:
            detail += "\n           " + "\n           ".join(problems)
    else:
        verdict, detail = "DIFFERENT", "\n           ".join(problems)
    if started_from is not None:
        import hashlib
        key = hashlib.sha256(json.dumps(started_from, sort_keys=True).encode()).hexdigest()[:16]
        RECORDED_TREES[key] = started_from
        RECORDED_CHECKS[name] = {
            "name": name, "tool": "audit", "base": "empty", "tree": key,
            "steps": [[to_focus(arg) for arg in args] for args in steps], "args": [],
            "env": {key_: to_focus(value) for key_, value in (env or {}).items()},
            "stdin": None, "writes": True,
            "deviation": detail if verdict == "deviation" else None,
            "expect": {key_: compact(value) for key_, value in outcome["ts"].items()},
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
    "audit/append/sessions_by_process":
        "upstream walks the process tree to find which conversation is calling; focus refuses and asks for the "
        "session, or for --space and --intent",
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
    check_("append/sessions_by_process", {**RECORD_FILES, "aidlc/.aidlc-sessions/pids/1.json": "{}\n"}, [APPEND])

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
    run_queued()
    if FUZZ:
        if not ONLY or not re.search(ONLY, "fuzz/audit"):
            fuzz(FUZZ)
        audit_fuzz(FUZZ // 3)
        run_queued(quiet=True)
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
for name, count in shapes.items():
    if count:
        print(f"{count} of the same after a difference of shape ({name}): {SHAPES[name]}")
sys.exit(1 if results["DIFFERENT"] else 0)
