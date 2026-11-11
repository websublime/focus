#!/usr/bin/env python3
"""Generate oracle cases.

    gen_cases.py <core dir> <seed> <scale> <patterns.json>  > cases.ndjson

`patterns.json` is what `patterns.ts` prints: upstream's case-insensitive
regular expressions. Cases are inputs only. The answers come from upstream (driver.ts) and from
focus (`focus dev oracle`), and are compared as text. Generation is seeded, so
a seed and a scale name one corpus exactly.

Two key names are never generated: `__proto__` and `constructor`. Upstream
handles both through JavaScript's prototype chain, by accident, and focus
deliberately does not reproduce that (see crates/focus-graph/src/stage_schema.rs).
"""
import glob
import json
import os
import random
import re
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import markdown_docs  # noqa: E402
from names import PHRASES  # noqa: E402

core, seed, scale = sys.argv[1], int(sys.argv[2]), int(sys.argv[3])
with open(sys.argv[4], encoding="utf-8") as listed:
    UPSTREAM_ICASE = json.load(listed)
rng = random.Random(seed)
FENCE = re.compile(r"^---\r?\n([\s\S]*?)\r?\n---")

# Characters that exercise the places JS and Rust disagree by default:
# whitespace classes, line terminators, quotes, YAML punctuation, non-ASCII.
TRICKY = [" ", " ", "\t", "\n", "\n", "\r", "\r\n", ":", ":", "-", "-", '"', "'", "[", "]", ",", "#", ">", "|",
          "{", "}", "a", "b", "k", "0", "1", "~", "_", ".", "\\", " ", " ", " ", "﻿",
          "\u0085", "　", "é", "\U0001F600"]
SCALARS = ["", '""', "''", '"x"', "'x'", ">", "|", ">-", "|-", "true", "false", "2", "0", "-3", "2.5", "two",
           "[a, b]", "[]", "x # note", '"', "'", "null", "~", "a: b", "  spaced  ", "é", "x ", "{a: b}"]
KEYS = ["slug", "number", "name", "plugin", "phase", "execution", "condition", "lead_agent", "mode", "for_each",
        "workspace_requires", "reviewer", "review_artifact", "reviewer_max_iterations", "review_class",
        "summary_confirmation", "inputs", "outputs"]
LIST_KEYS = ["support_agents", "produces", "requires_stage", "sensors", "scopes", "optional_produces",
             "required_sections"]
EXTRA_KEYS = ["on_failure", "blocks_on", "timeout", "retry", "bundle", "unknown_key", "when", "ars",
              "produces_kinds", "consumes"]
BLOCKS = [
    "when:\n  producer-in-plan: requirements",
    "when: {producer-in-plan: requirements}",
    "when: nope",
    "when:\n  other-predicate: x",
    "when:\n  producer-in-plan:   ",
    "ars:\n  targets: [iae, ve]\n  cost: 3\n  role: core\n  project_types: [brownfield]",
    "ars:\n  cost: null\n  targets: []",
    "ars:\n  targets: [iae, iae, zz]\n  cost: 9\n  role: boss\n  project_types: []",
    "ars:\n  cost: 2.5\n  extra: 1\n  targets:\n    - iae",
    "ars: scalar",
    "ars:\n  # comment\n\n  cost: ~ # none\n  targets: [a,,b]",
    "ars:\n  role: \"core\"\n  cost: 3.0\n  targets: [ua] # t",
    "produces_kinds:\n  requirements: [service, ui]",
    "produces_kinds:\n  requirements: []\n  Bad_Key: [ui]\n  missing-artifact: [robot]",
    "produces_kinds:\n  requirements: ui",
    "produces_kinds: nope",
    "consumes:\n  - artifact: a\n    required: true\n\n  - artifact: b\n    required: false",
    "consumes:\n  - artifact: Bad_Name\n    required: yes\n    conditional_on: sometimes",
    "consumes:\n  - required: true\n  - artifact: \"quoted\"\n    required: \"true\"",
    "consumes:\n  - artifact: a\n      deeper: x\n    required: true",
    "consumes: []",
    "consumes:\n  - just a string",
    "optional_produces:\n  - Bad_Name\n  - ok-name",
    "required_sections:\n  - Summary\n  - \"  \"",
    "for_each: unit-of-work\nproduces_kinds:\n  requirements: [service]\n  requirements-analysis-questions: [ui]",
]

cases = []


def add(kind, **fields):
    cases.append({"id": f"{kind}-{len(cases):05d}", "kind": kind, **fields})


def read(path):
    with open(path, encoding="utf-8") as fh:
        return fh.read()


def split(raw):
    m = FENCE.match(raw)
    return m.group(1) if m else None


def char_fuzz(text, edits):
    chars = list(text)
    for _ in range(edits):
        op = rng.choice("idr")
        pos = rng.randrange(len(chars) + 1) if chars else 0
        if op == "i" or not chars:
            chars.insert(pos, rng.choice(TRICKY))
        elif op == "d":
            chars.pop(min(pos, len(chars) - 1))
        else:
            chars[min(pos, len(chars) - 1)] = rng.choice(TRICKY)
    return "".join(chars)


def mutate_stage(block):
    lines = block.split("\n")
    choice = rng.randrange(16)
    if choice == 0:
        return "\r\n".join(lines)
    if choice == 1:
        return "\n".join(line.replace("  ", "\t", 1) for line in lines)
    if choice == 2:
        return "\n".join(line + rng.choice(["  ", "\t", "  ", ""]) for line in lines)
    if choice == 3 and len(lines) > 1:
        lines.pop(rng.randrange(len(lines)))
    elif choice == 4:
        idx = rng.randrange(len(lines))
        lines.insert(idx, lines[idx])
    elif choice == 5 and len(lines) > 1:
        a, b = rng.randrange(len(lines)), rng.randrange(len(lines))
        lines[a], lines[b] = lines[b], lines[a]
    elif choice == 6:
        lines.insert(rng.randrange(len(lines) + 1), rng.choice(["", "  ", "# comment", "\t"]))
    elif choice in (7, 8):
        key = rng.choice(KEYS)
        value = rng.choice(SCALARS)
        hit = [i for i, line in enumerate(lines) if line.startswith(key + ":")]
        if hit:
            lines[hit[0]] = f"{key}:{rng.choice([' ', '', '  ', chr(9)])}{value}"
        else:
            lines.insert(rng.randrange(len(lines) + 1), f"{key}: {value}")
    elif choice == 9:
        key = rng.choice(LIST_KEYS)
        form = rng.choice(["{k}: [a, b-c, \"d\"]", "{k}: []", "{k}: [a, b", "{k}:\n  - a\n  -b\n  - c", "{k}: scalar",
                           "{k}:\n  - \"q\"\n  - 'r'\n  - ", "{k}:\n- flush", "{k}: [a] # tail", "{k}: [a] tail",
                           "{k}:\n  - a\n\n  - b"])
        lines = [line for line in lines if not line.startswith(key + ":")]
        lines.insert(rng.randrange(len(lines) + 1), form.format(k=key))
    elif choice == 10:
        key = rng.choice(EXTRA_KEYS)
        lines.insert(rng.randrange(len(lines) + 1), f"{key}: {rng.choice(SCALARS)}")
    elif choice in (11, 12):
        block_text = rng.choice(BLOCKS)
        head = block_text.split(":", 1)[0].split("\n")[-1]
        lines = [line for line in lines if not line.startswith(head + ":")]
        lines.append(block_text)
    elif choice == 13:
        lines.insert(rng.randrange(len(lines) + 1),
                     rng.choice(["mode: pipeline", "mode: mob", "reviewer: aidlc-nobody-agent",
                                 "lead_agent: aidlc-ghost-agent", "support_agents: [aidlc-product-agent]",
                                 "mode: pipeline\nsupport_agents: [aidlc-product-agent, aidlc-product-agent]",
                                 "review_class: adversarial", "reviewer_max_iterations: 3",
                                 "review_artifact: traceability", "review_artifact: not-produced"]))
    else:
        return char_fuzz(block, rng.randrange(1, 6))
    return "\n".join(lines)


agents = sorted(os.path.basename(p)[:-3] for p in glob.glob(os.path.join(core, "agents", "*.md")))
stage_files = sorted(glob.glob(os.path.join(core, "aidlc-common", "stages", "*", "*.md")))
sensor_files = sorted(glob.glob(os.path.join(core, "sensors", "*.md")))
rule_files = sorted(glob.glob(os.path.join(core, "memory", "**", "*.md"), recursive=True))

# Every real stage file, whole, against the real agent roster.
for path in stage_files:
    add("stage", raw=read(path), agents=agents)

# Mutated stage frontmatter. Bodies are dropped: no reader looks past the fence.
for path in stage_files:
    block = split(read(path))
    for _ in range(scale):
        mutated = mutate_stage(block)
        for _ in range(rng.choice([0, 0, 1, 2])):
            mutated = mutate_stage(mutated)
        fields = {"raw": f"---\n{mutated}\n---\n"}
        if rng.random() < 0.8:
            fields["agents"] = agents
        add("stage", **fields)

# Documents that are barely stage files at all.
MINIMAL = ("slug: s\nphase: inception\nexecution: ALWAYS\ncondition: c\nlead_agent: orchestrator\n"
           "support_agents: []\nmode: inline\nproduces: []\nconsumes: []\nrequires_stage: []\ninputs: i\noutputs: o")
for raw in ["", "---", "---\n---\n", "---\n\n---\n", "no fence at all", "---\nslug: s\n", "﻿---\nslug: s\n---\n",
            "---\r\nslug: s\r\n---\r\n", f"---\n{MINIMAL}\n---\n", f"---\n{MINIMAL}\n---", f"\n---\n{MINIMAL}\n---\n",
            f"---\n{MINIMAL}\n--- trailing", f"---\n{MINIMAL}\n----\n"]:
    add("stage", raw=raw)
for _ in range(scale * 12):
    add("stage", raw=f"---\n{char_fuzz(MINIMAL, rng.randrange(1, 9))}\n---\n")
for block_text in BLOCKS:
    add("stage", raw=f"---\n{MINIMAL}\n{block_text}\n---\n")
    add("stage", raw=f"---\n{block_text}\n{MINIMAL}\n---\n")

# Sensor manifests: real, then mutated.
for path in sensor_files:
    stem = os.path.basename(path)[len("aidlc-"):-3]
    raw = read(path)
    add("sensor", raw=raw, filename_id=stem)
    add("sensor", raw=raw, filename_id=stem + "-other")
    block = split(raw)
    for _ in range(scale * 3):
        choice = rng.randrange(4)
        if choice == 0:
            mutated = char_fuzz(block, rng.randrange(1, 6))
        elif choice == 1:
            lines = block.split("\n")
            lines.pop(rng.randrange(len(lines)))
            mutated = "\n".join(lines)
        elif choice == 2:
            key = rng.choice(["id", "kind", "command", "default_severity", "description", "category", "fire_on",
                              "matches", "timeout_seconds"])
            lines = [line for line in block.split("\n") if not line.startswith(key + ":")]
            lines.insert(rng.randrange(len(lines) + 1), f"{key}: {rng.choice(SCALARS + ['12abc', 'gate', 'blocking'])}")
            mutated = "\n".join(lines)
        else:
            mutated = block.replace("\n", "\r\n")
        add("sensor", raw=rng.choice(["", "﻿"]) + f"---\n{mutated}\n---\n", filename_id=stem)

# Method files: the real ones carry no frontmatter; the rest is synthetic.
for path in rule_files:
    add("rule", raw=read(path))
RULE_VALUES = ["feedforward-only", "aidlc-linter", "linter", '"aidlc-x"', "", "active", "deprecated", "draft", "Active",
               "2026-02-28", "2024-02-29", "2026-02-29", "2026-13-01", "2026-00-10", "2026-04-31", "26-01-01",
               "2026-1-1", "0000-01-01", "9999-12-31", "2026-06-30 # note", ">"]
for _ in range(scale * 20):
    lines = [f"{key}:{rng.choice([' ', '', '  '])}{rng.choice(RULE_VALUES)}"
             for key in rng.sample(["pairing", "status", "stale_after", "other"], rng.randrange(0, 5))]
    raw = "---\n" + "\n".join(lines) + "\n---\nbody\n"
    if rng.random() < 0.3:
        raw = char_fuzz(raw, rng.randrange(1, 4))
    add("rule", raw=rng.choice(["", "", "﻿"]) + raw)

# The two primitive readers: on text built only from tricky characters, and on
# well-formed values with a few characters disturbed.
PRIMITIVE_SHAPES = ["{k}: value", "{k}: \"v w\"", "{k}: 'v'", "{k}:\nnext: 1", "{k}: >\n  folded", "{k}:   v   ",
                    "{k}:\n  - a\n  - b", "{k}:\n  - \"a\"\n  - 'b'\nnext: 1", "{k}: [a, b]", "{k}: [\"a,b\", c] # t",
                    "{k}:\n\t- a\n\t- b", "{k}:  \n  - a\n  -  b  ", "{k}:\r\n  - a\r\n  - b\r\n", "{k}: [a, 'b]",
                    "{k}:\n  - a\n\n  - b", "{k}: []", "{k}:\n  - - a\n  - b: c"]
for _ in range(scale * 120):
    key = rng.choice(["k", "key", "a"])
    lead = rng.choice(["", "", "x: 1\n", " ", "\n"])
    tail = rng.choice(["", "", "\nb: 2", "\n  - z", "\n"])
    if rng.random() < 0.5:
        text = f"{key}:" + "".join(rng.choice(TRICKY) for _ in range(rng.randrange(0, 14)))
    else:
        text = char_fuzz(rng.choice(PRIMITIVE_SHAPES).format(k=key), rng.randrange(0, 4))
    add(rng.choice(["scalar", "list"]), fm=f"{lead}{text}{tail}", key=key)

# Collation, over printable ASCII, where focus claims to reproduce
# localeCompare exactly. Half the pairs differ only by case or by one
# character, which is where the collation's levels matter.
PRINTABLE = "".join(chr(c) for c in range(0x20, 0x7F))
IDENT = "abcXYZ019-._+@/"
for _ in range(scale * 60):
    alphabet = rng.choice([PRINTABLE, IDENT, IDENT])
    a = "".join(rng.choice(alphabet) for _ in range(rng.randrange(0, 7)))
    roll = rng.random()
    if roll < 0.25:
        b = "".join(c.swapcase() if rng.random() < 0.4 else c for c in a)
    elif roll < 0.5 and a:
        at = rng.randrange(len(a))
        b = a[:at] + rng.choice(alphabet) + a[at + 1:]
    elif roll < 0.6:
        b = a + rng.choice(alphabet)
    else:
        b = "".join(rng.choice(alphabet) for _ in range(rng.randrange(0, 7)))
    add("collate", a=a, b=b)

# Numbers: parsing, printing, fixed-point rounding (ties included) and
# Math.round, on text a command line or a JSON file could carry.
NUMBER_TEXT = ["0", "-0", "1", "1.0", "0.30", "0.125", "0.375", "0.625", "0.875", "-0.125", "2.5", "-2.5", "0.5",
               "-0.5", "1.005", "0.995", "9.999", "20.499999999999996", "62.74999999999999", "0.49999999999999994",
               "1e21", "1e20", "1e-6", "1e-7", "1.5e22", "123456789012345680000", "5e-324", "1.7976931348623157e308",
               "", " ", " 12 ", "1.", ".5", "+.5e1", "-1e-2", "0x1f", "0b101", "0o17", "Infinity", "-Infinity", "abc",
               "1,5", "1e", ".", "+", "inf", "nan", "NaN", "1_000", "0x", "-0x1f", "1 2", "1e+", "--1", "0.1", "0.2",
               "0.7", "100", "1e3", "12345.6789", "0.000001234", "4.35", "4.345", "8.575", "1.45", "-1.45"]
for text in NUMBER_TEXT:
    for digits in (0, 2, 9):
        add("num", text=text, digits=digits)
for _ in range(scale * 40):
    roll = rng.random()
    if roll < 0.35:
        text = f"{rng.randrange(0, 101) / 100}"
    elif roll < 0.55:
        text = f"{rng.randrange(-4000, 4000) / rng.choice([8, 16, 32, 64, 1000])}"
    elif roll < 0.75:
        text = repr(rng.uniform(-1000, 1000))
    elif roll < 0.9:
        text = repr(rng.uniform(0, 1) * 10 ** rng.randrange(-12, 25))
    else:
        text = str(rng.randrange(-10**18, 10**18))
    add("num", text=text, digits=rng.choice([0, 1, 2, 2, 2, 3, 9]))

# Dependency cycles and topological order on small random graphs.
for _ in range(scale * 25):
    size = rng.randrange(1, 8)
    slugs = [f"s{i}" for i in range(size)]
    stages = []
    for index, slug in enumerate(slugs):
        deps = [rng.choice(slugs + ["outside"]) for _ in range(rng.choice([0, 0, 1, 1, 2, 3]))]
        stages.append({"slug": slug, "number": f"{rng.randrange(0, 3)}.{rng.randrange(1, 12)}",
                       "requires_stage": deps})
    rng.shuffle(stages)
    add("cycles", stages=stages)

# Composed-scope records: well-formed, then damaged one way at a time.
BEGIN = ("<!-- BEGIN aidlc composed-scope-grid: generated by `aidlc engine graph compile` \u2014 reshape the plan "
         "through /aidlc compose, not by editing here -->")
END = "<!-- END aidlc composed-scope-grid -->"
SCOPE_NAMES = ["spike", "Zeta+v2", "2024", "a.b_c@d", "x", "feature", "not a name", "-lead", "", "caf\u00e9"]
ACTIONS = ['"EXECUTE"', '"SKIP"', '"EXECUTE"', '"SKIP"', '"MAYBE"', "1", "null", "true", '["EXECUTE"]']


def make_record():
    name = rng.choice(SCOPE_NAMES[:5]) if rng.random() < 0.7 else rng.choice(SCOPE_NAMES)
    prose = rng.choice(["# Plan\n\nProse.\n", "# Plan\n", "", "Text\n\n\n\n", "No newline at end",
                        "```json\n{\"stages\": {\"x\": \"EXECUTE\"}}\n```\n", "## Stage Grid\n\n- a\n"])
    head = rng.choice(["---\nname: {n}\ndepth: Minimal\n---\n\n", "---\nname:   {n}  \n---\n", "---\ndepth: x\n---\n",
                       "---\nname: \"{n}\"\n---\n\n", "", "---\r\nname: {n}\r\n---\r\n"]).replace("{n}", name)
    keys = rng.sample(["code-generation", "build-and-test", "10", "2", "state-init", "a b", "A"], rng.randrange(0, 5))
    cells = ", ".join(f'"{k}": {rng.choice(ACTIONS[:4]) if rng.random() < 0.85 else rng.choice(ACTIONS)}' for k in keys)
    grid = rng.choice(['{{"stages": {{{c}}}}}', '{{\n  "stages": {{\n    {c}\n  }}\n}}', '{{"stages": {{{c}}}, "note": 1}}',
                       '{{"stages": [{c}]}}', '{{"cells": {{{c}}}}}', "{{ nope", "null", '"text"', "[]",
                       '{{"stages": null}}']
                      if rng.random() < 0.25 else ['{{"stages": {{{c}}}}}', '{{\n  "stages": {{\n    {c}\n  }}\n}}']
                      ).format(c=cells)
    fence = rng.choice(["```json\n{g}\n```", "```json\n{g}\n```", "```json  \n{g}\n```", "```\n{g}\n```",
                        "```json\n{g}```", "```JSON\n{g}\n```", "{g}", "```json\r\n{g}\r\n```"]).replace("{g}", grid)
    region = rng.choice(["{b}\n\n{f}\n\n{e}\n", "{b}\n\n{f}\n\n{e}\n", "{b}\n{f}\n{e}", "{b}{f}{e}",
                         "{e}\n{f}\n{b}\n", "{b}\n{f}\n", "{f}\n{e}\n", "{b}\n{f}\n{e}\n{b}\n{e}\n",
                         "{b}\n{f}\n{e}\n{e}\n", "{b}\n{b}\n{f}\n{e}\n", "{f}\n", "{b}\n\n{e}\n"]
                        if rng.random() < 0.3 else ["{b}\n\n{f}\n\n{e}\n"])
    region = region.replace("{b}", BEGIN).replace("{e}", END).replace("{f}", fence)
    return f"{head}{prose}{rng.choice(['', chr(10), chr(10) * 3])}{region}"


for _ in range(scale * 40):
    body = make_record()
    if rng.random() < 0.1:
        body = char_fuzz(body, rng.randrange(1, 3))
    add("record", body=body)

# Folding composed scopes back into a freshly transposed grid.
FOLD_NAMES = ["spike", "legacy", "Zeta+v2", "2024", "7", "feature", "bugfix", "not a name", "alpha", "a.b"]


def column():
    roll = rng.random()
    if roll < 0.75:
        return {"stages": {k: rng.choice(["EXECUTE", "SKIP"]) for k in rng.sample(["a", "b", "10", "2", "c"], 3)}}
    return rng.choice([{"stages": None}, {"stages": []}, {"cells": {}}, {"stages": {}, "note": 1}, "text", None, 3,
                       [], {"stages": "x"}])


for _ in range(scale * 40):
    records = {name: {k: rng.choice(["EXECUTE", "SKIP"]) for k in rng.sample(["a", "b", "10", "2", "c"], 3)}
               for name in rng.sample(FOLD_NAMES, rng.randrange(0, 4))}
    disk = {name: column() for name in rng.sample(FOLD_NAMES, rng.randrange(0, 5))}
    on_disk = rng.choice([json.dumps(disk)] * 6 + [None, "", "{ nope", "[]", "null", '"text"'])
    stock = rng.sample(["feature", "bugfix", "2024", "alpha"], rng.randrange(0, 4))
    installed = rng.sample(FOLD_NAMES, rng.randrange(0, len(FOLD_NAMES) + 1))
    fresh = {name: {"stages": {"a": "EXECUTE", "b": "SKIP"}} for name in sorted(stock)}
    add("foldback", records=records, on_disk=on_disk, stock=stock, installed=installed, fresh=fresh)

# A proposal's scopeSettings member.
SETTING_KEYS = ["sensors", "learnings", "summary_confirmation", "plan_approval", "collaborators", "review_cap"]
SETTING_VALUES = ["on", "off", "on", "off", "adversarial", "advisory", "none", "ON", "", 1, True, None, ["on"],
                  {"a": 1}, 0.5]
for raw in [None, [], "on", 3, {}, {key: "on" for key in SETTING_KEYS}]:
    add("settings", raw=raw)
for _ in range(scale * 30):
    raw = {}
    for key in rng.sample(SETTING_KEYS + ["extra", "Sensors", "10"], rng.randrange(0, 9)):
        if key == "review_cap" and rng.random() < 0.7:
            raw[key] = rng.choice(["adversarial", "advisory", "none"])
        elif rng.random() < 0.8:
            raw[key] = rng.choice(["on", "off"])
        else:
            raw[key] = rng.choice(SETTING_VALUES)
    add("settings", raw=raw)

# Guard Policy declarations in a memory file. "guard" cases are plain
# Markdown; "guardwild" cases add what hides text from a reader: fences, HTML
# blocks, comments and code spans that run over lines. One construct is left
# out, a code span opened after an escaped backtick (``a \` b ` ... ` c``):
# upstream's analysis does not see that span and focus does (it is a
# `markdown` deviation, declared in crates/focus-graph/src/guard_memory.rs).
MODES = ["Mode: strict", "- Mode: relaxed", "* **Mode**: off", "**Mode:** strict", "mode :   STRICT  ",
         "Mode: `off`", "- Mode: lenient", "Mode:", "Mode:\n  strict", "- Mode:\n  relaxed\n  wrapped",
         "Mode: strict\nMethodology: tdd", "  Mode: off", "Mode strict", "Modes: strict", "> Mode: strict",
         "1. Mode: strict", "+ Mode: strict", "- [ ] Mode: strict",
         "Mode: strict # really", "Ordering: x\nMode: off", "Mode:\n\n  strict", "- Mode: strict\n- Other: x",
         "Mode:\n   relaxed\n   - item", "MODE: Off", "Mode: *strict*", "Mode: _relaxed_", "Mode: off.",
         "Mode:   \n  \n", "Mode: strict\n  and more", " Mode: strict", "   - Mode: strict"]
PLAIN_BLOCKS = ["Some prose here.", "Two lines\nof prose.", "- a list item\n- another", "> a quote",
                "A `code span` and `another <tag>` here.", "| a | b |\n|---|---|\n| 1 | 2 |", "1. first\n2. second",
                "Text with *emphasis* and **strong**.", "A line with a trailing colon:", "key: value",
                "<!-- a comment -->", "<!-- Mode: strict, relaxed, or off. -->", "<!-- multi\nline\ncomment -->",
                "<!-- hides\n## Guard Policy\nMode: strict\n-->", "<!-- hides\nMode: strict\n--> after it",
                "<!-- c --> Mode: strict", "<!----> empty", "  indented two", "   indented three", "---",
                "Setext heading\n===", "### Deeper heading", "# Top heading", "\\* not a list"]
WILD_BLOCKS = ["```\ncode\nMode: strict\n```", "```md\n## Guard Policy\nMode: strict\n```", "~~~\nMode: off\n~~~",
               "    indented code\n    Mode: strict", "\tMode: strict", "<div>\nMode: strict\n</div>",
               "text <!-- inline\nMode: strict\n--> more", "a `span that\nMode: strict\nends` here",
               "<!-->\nMode: strict", "<details>\n\nMode: relaxed\n\n</details>", "``Mode: strict``",
               "text <b>bold</b>", "<!-- c --> <!-- d --> Mode: off", "-  list\n\n       Mode: strict",
               "<?php\nMode: strict\n?>", "a \\` b\nMode: strict\n\\` c", "  <!-- indented comment\nMode: off\n-->",
               "> quote\n> ```\n> Mode: strict\n> ```", "<![CDATA[\nMode: strict\n]]>", "``` unclosed fence", "Mode: strict <-- an arrow"]
HEADINGS = ["## Guard Policy", "## Guard Policy", "## Guard Policy", "## Change Control", "## Guard Policy  ",
            "##  Guard Policy", "## Guard policy", "### Guard Policy", " ## Guard Policy", "## Guard Policy:",
            "## Deployment", "## Other"]


def memory_file(wild):
    parts = []
    for _ in range(rng.randrange(1, 7)):
        roll = rng.random()
        if roll < 0.3:
            section = [rng.choice(HEADINGS)]
            for _ in range(rng.randrange(0, 4)):
                pool = rng.random()
                if pool < 0.45:
                    section.append(rng.choice(MODES))
                elif wild and pool < 0.65:
                    section.append(rng.choice(WILD_BLOCKS))
                else:
                    section.append(rng.choice(PLAIN_BLOCKS))
            parts.append(rng.choice(["\n\n", "\n", "\n\n"]).join(section))
        elif wild and roll < 0.5:
            parts.append(rng.choice(WILD_BLOCKS))
        elif roll < 0.6:
            parts.append(rng.choice(MODES))
        else:
            parts.append(rng.choice(PLAIN_BLOCKS))
    text = rng.choice(["\n\n", "\n\n", "\n"]).join(parts) + rng.choice(["\n", "", "\n\n"])
    if rng.random() < 0.1:
        text = text.replace("\n", "\r\n")
    if rng.random() < 0.05:
        text = "\ufeff" + text
    return text


for path in rule_files:
    add("guard", content=read(path))
for _ in range(scale * 150):
    add("guard", content=memory_file(False))
for _ in range(scale * 100):
    add("guardwild", content=memory_file(True))

# --------------------------------------------------------------------------
# The audit ledger: writing an entry, and finding entries and fields again.
# Generated after everything above, so earlier cases keep their ids.
# --------------------------------------------------------------------------

EVENT_TYPES = re.findall(r'"([A-Z_]+)"', read(os.path.join(core, "tools", "aidlc-audit.ts"))
                         .split("const VALID_EVENT_TYPES = new Set([")[1].split("]);")[0])

# `{P}` stands for the project's own location, which each side fills in.
# `__proto__` is never a field name here: assigned to a JavaScript object it
# sets the prototype instead of a field, which no caller relies on.
FIELD_KEYS = ["Stage", "Tool", "Note", "Bolt slug", "Timestamp", "Event", "A b.c_(d)/e-f", "Details (raw)", "1", "42",
              "9x", "x:y", "é", "with\nnewline", "", " lead", "trail ", "a*b", "constructor", "Workflow"]
FIELD_VALUES = ["", "code-generation", "  spaced  ", "{P}/src/main.rs", "at {P}", "{P}x", "{P}\\win\\path", "{P}\tnext",
                "see {P}/a and {P}/b", "a\r\nb", "a\rb", "a\nb", "a\u2028b\u2029c", "x\u0085y", "**Event**: HUMAN_TURN",
                "\n---\n## Forged\n**Event**: GATE_APPROVED", "é \U0001F600", "aidlc-state", "<project-dir>", "=", "a=b"]

for event_type in EVENT_TYPES:
    add("audit_entry", eventType=event_type, fields=[["Stage", "code-generation"], ["Note", "n"]])
for _ in range(scale * 30):
    event_type = rng.choice(EVENT_TYPES) if rng.random() < 0.85 else rng.choice(
        ["NOPE", "stage_started", "", "STAGE_STARTED ", " STAGE_STARTED", "Stage Start", "__proto__", "toString"])
    # Mostly names the ledger accepts, so that most entries get as far as being written.
    fields = [[rng.choice(FIELD_KEYS[:8]) if rng.random() < 0.8 else rng.choice(
                   FIELD_KEYS + [char_fuzz("Stage", rng.randrange(1, 3))]),
               rng.choice(FIELD_VALUES) if rng.random() < 0.8 else char_fuzz("value {P}/x", rng.randrange(1, 5))]
              for _ in range(rng.randrange(0, 5))]
    add("audit_entry", eventType=event_type, fields=[pair for pair in fields if pair[0] != "__proto__"])

QUERY_EVENTS = ["STAGE_STARTED", "STAGE_COMPLETED", "GATE_APPROVED", "A.B", "A+", "X", "BOLT_STARTED"]
SLUGS = ["one", "two", "o.e", "a-b"]
STAMPS = ["2026-10-09T16:22:42Z", "2026-10-09T16:22:43Z", "2026-10-08T23:59:59Z", "2026-10-09T16:22:42Z", "1", "x",
          "2026-10-09T16:22:42.123Z", "é"]


def ledger_block(asked=None, asked_slug=None):
    """One block, mostly as the engine writes it and sometimes not; mostly about the event being asked for."""
    event = asked if asked and rng.random() < 0.65 else rng.choice(QUERY_EVENTS)
    slug = asked_slug if asked_slug and rng.random() < 0.7 else rng.choice(SLUGS)
    lines = ["", f"## {event.title()}"]
    if rng.random() < 0.9:
        lines.append(f"**Timestamp**:{rng.choice([' ', ' ', '', chr(9), '  '])}{rng.choice(STAMPS)}{rng.choice(['', '', ' tail'])}")
    lines.append(rng.choice([f"**Event**: {event}", f"**Event**: {event}", f"**Event**:{event}", f"**Event**: {event}  ",
                             f"**Event**: {event}\t", f"**Event**:\n{event}", f"**Event**: {event}x", f"- **Event**: {event}",
                             f"**event**: {event}", f"**Event**: {event}\n**Event**: X"]))
    if rng.random() < 0.6:
        lines.append(rng.choice([f"**Bolt slug**: {slug}", f"**Bolt slug**: {slug}", f"**Bolt slug**:{slug} ",
                                 f"- **Bolt slug**: {slug}", f"**Bolt slug**: {slug}x"]))
    if rng.random() < 0.5:
        lines.append(rng.choice(["**Stage**: code-generation", "- **Stage**:  spaced  ", "**Stage**:", "**Note**: a\\nb",
                                 "**Stage**: first\n**Stage**: second", "**Details (raw)**: x", "plain line"]))
    lines.append("")
    return "\n".join(lines)


def ledger(asked, asked_slug):
    blocks = [ledger_block(asked, asked_slug) for _ in range(rng.randrange(0, 7))]
    text = "# AI-DLC Audit Log\n" + rng.choice(["\n---\n", "\n---\n", "\n---\n", "\n--- \n", "\n----\n", "---\n"]).join(blocks)
    roll = rng.random()
    if roll < 0.15:
        text = text.replace("\n", "\r\n")
    elif roll < 0.3:
        text = char_fuzz(text, rng.randrange(1, 6))
    return text


for _ in range(scale * 40):
    asked = rng.choice(QUERY_EVENTS)
    asked_slug = rng.choice(SLUGS) if rng.random() < 0.4 else None
    fields = {"audit": ledger(asked, asked_slug), "event": asked}
    if asked_slug:
        fields["slug"] = asked_slug
    add("audit_find", **fields)
for _ in range(scale * 30):
    block = ledger_block() if rng.random() < 0.8 else char_fuzz(ledger_block(), rng.randrange(1, 5))
    add("audit_field", block=block,
        name=rng.choice(["Event", "Event", "Timestamp", "Timestamp", "Bolt slug", "Stage", "Note", "Details (raw)",
                         "event", "", "Sta"]))

STATE_LINES = ["- **Scope**: bugfix", "- **Scope**:", "- **Scope**:   ", "- **Scope**:\tfeature\t", "**Scope**: bare",
               "  - **Scope**: indented", "- **Depth (x)**: standard", "- **Depth**: standard # note", "- **Scope**: second",
               "- **scope**: lower", "-  **Scope**: two spaces", "- **Scope** : spaced colon", "# heading", ""]
for _ in range(scale * 30):
    lines = [rng.choice(STATE_LINES) for _ in range(rng.randrange(0, 7))]
    content = rng.choice(["\n", "\n", "\r\n", "\r", "\u2028"]).join(lines)
    if rng.random() < 0.25:
        content = char_fuzz(content, rng.randrange(1, 5))
    add("bullet_field", content=content,
        field=rng.choice(["Scope", "Scope", "Scope", "Depth", "Depth (x)", "scope", "", "Sc.pe", "Scope**"]))

for _ in range(scale * 20):
    add("unsafe_line", value="".join(rng.choice(TRICKY + ["\x00", "\x1f", "\x7f", "\x20", "\x80", "word"])
                                     for _ in range(rng.randrange(0, 8))))

FIELD_ARGS = ["--field", "--field", "--field", "Stage=code-generation", "Note=a=b", "=empty-key", "novalue", "1=one",
              "42=answer", "Stage=again", "b=", "--field=inline", "--other", "x", "é=é", "constructor=c"]
for _ in range(scale * 20):
    args = []
    for _ in range(rng.randrange(0, 6)):
        if rng.random() < 0.75:
            args.append("--field")
        args.append(rng.choice(FIELD_ARGS))
    add("field_args", args=args)

# --------------------------------------------------------------------------
# Which record a command acts on: a project as a set of files, a selector, and
# what the environment says about the caller.
#
# Left out on purpose: backslashes in names (a separator on Windows only, so
# the answer depends on the platform replaying the corpus), a delegated
# worktree's marker and a process-to-session map (focus refuses both).
# --------------------------------------------------------------------------

RECORDS = ["add-login-1a2b3c4d", "fix-logout-9f8e7d6c", "260101-tidy", "undefined-abc", "x", "5-ab"]
STATUSES = ["in-flight", "complete", "archived", "archived", " Archived ", "ARCHIVED\n", "archive", ""]


def registry_row(record):
    slug, _, tail = record.rpartition("-")
    uuid = "00000000-0000-0000-0000-0000" + tail.rjust(8, "0")
    status = rng.choice(STATUSES)
    return rng.choice([
        {"uuid": uuid, "slug": slug, "status": status},
        {"uuid": uuid, "slug": slug, "status": status},
        {"uuid": uuid, "slug": slug, "dirName": record, "status": status},
        {"uuid": "unrelated", "slug": "renamed", "dirName": record, "status": status},
        {"uuid": uuid.upper(), "slug": slug, "status": status},
        {"uuid": "ffffffff-0000-0000-0000-00000000" + tail[-4:].rjust(4, "0"), "slug": slug, "status": status},
        {"uuid": uuid, "slug": slug},
        {"uuid": uuid, "slug": slug, "status": 1},
        {"uuid": 5, "slug": slug, "status": status},
        {"slug": slug, "status": status},
        {"uuid": uuid, "slug": 5, "status": status},
        {"uuid": uuid, "slug": [slug], "status": status},
        {"uuid": uuid, "slug": slug, "dirName": 0, "status": status},
        {"uuid": uuid, "slug": slug, "dirName": 7, "status": status},
        {"uuid": uuid, "status": status},
        None, 7, "text", [], True,
    ])


def selection_case():
    files = {}
    spaces = ["default"] + (["payments"] if rng.random() < 0.5 else [])
    held = {}
    for space in spaces:
        base = f"aidlc/spaces/{space}/intents"
        records = rng.sample(RECORDS, rng.choice([0, 1, 1, 1, 1, 2, 2, 3]))
        held[space] = records
        for record in records:
            name = "aidlc-state.md" if rng.random() < 0.9 else "notes.md"
            files[f"{base}/{record}/{name}"] = "state\n"
        roll = rng.random()
        if roll < 0.55:
            rows = [registry_row(record) for record in records if rng.random() < 0.85]
            if rng.random() < 0.2:
                rows.insert(rng.randrange(len(rows) + 1), registry_row(rng.choice(RECORDS)))
            files[f"{base}/intents.json"] = json.dumps(rows)
        elif roll < 0.65:
            files[f"{base}/intents.json"] = rng.choice(["not json", '{"uuid": "x"}', "[]", "null", "\ufeff[]"])
        roll = rng.random()
        if roll < 0.3 and records:
            files[f"{base}/active-intent"] = rng.choice(["{}", "{}\n", "  {}  \n"]).format(rng.choice(records))
        elif roll < 0.4:
            files[f"{base}/active-intent"] = rng.choice(["gone-00000000", "", "  \n", "two words", "."])
        elif roll < 0.47:
            files[f"{base}/active-intent"] = "../other"
            if rng.random() < 0.7:
                files[f"aidlc/spaces/{space}/other/aidlc-state.md"] = "state\n"
    if rng.random() < 0.45:
        files["aidlc/active-space"] = rng.choice(["payments", "payments\n", " payments ", "nope", "Bad Name", "default", ""])

    # Mostly a space that has a record, so that most bindings name a real one.
    space = rng.choice([name for name in spaces if held[name]] or spaces)
    record = rng.choice(held[space]) if held[space] else "nothing-00000000"
    if rng.random() < 0.45:
        binding = rng.choice([
            {"space": space, "intent": record, "boundAt": "2026-10-09T16:22:42Z", "source": "create"},
            {"space": space, "intent": record, "boundAt": "2026-10-09T16:22:42Z", "source": "switch"},
            {"space": space, "intent": record, "boundAt": "2026-10-09T16:22:42Z", "source": "cursor"},
            {"space": space, "intent": record, "boundAt": "2026-10-09T16:22:42Z"},
            {"space": space, "intent": record, "boundAt": "2026-10-09T16:22:42Z"},
            {"space": space, "intent": record, "boundAt": "2026-10-09T16:22:42Z", "source": "made-up"},
            {"space": space, "intent": None, "boundAt": "t", "source": "archive"},
            {"space": space, "intent": "gone-00000000", "boundAt": "t"},
            {"space": "Bad Name", "intent": record, "boundAt": "t"},
            {"space": space, "intent": record, "boundAt": ""},
            {"space": space, "intent": record},
            {"space": space, "boundAt": "t"},
            {"space": space, "intent": " " + record, "boundAt": "t"},
            {"space": space, "intent": 5, "boundAt": "t"},
            [space, record], "text", None,
        ])
        files["aidlc/.aidlc-sessions/s1.binding.json"] = rng.choice([json.dumps(binding)] * 9 + ["{ nope"])

    selector = {}
    if rng.random() < 0.35:
        selector["space"] = rng.choice(spaces + spaces + ["zz", "../x", ".", "a/b", ""])
    if rng.random() < 0.3:
        selector["intent"] = rng.choice(RECORDS[:3] + [record, "nope", "", "a/b", ".."])
    if rng.random() < (0.7 if "aidlc/.aidlc-sessions/s1.binding.json" in files else 0.2):
        selector["sessionId"] = rng.choice(["s1", "s1", "s1", "s1", "s2", "-bad", "has space", ""])
    fields = {"files": files, "selector": selector}
    if rng.random() < 0.25:
        fields["override"] = {"id": rng.choice(["s1", "s1", "s2", "bad id"]), "payload": rng.random() < 0.5}
    return fields


for _ in range(scale * 80):
    add("selection", **selection_case())

# --------------------------------------------------------------------------
# Several shards read as one ledger, and a record's ledger on disk.
#
# A shard is written here as the engine writes one (a title, then blocks that
# each end in a rule), with the departures a reader has to survive: notes,
# rows without a timestamp or an event, a row repeated in its own file, and
# files that are copies of one another, whole, cut short, grown or diverged.
#
# `{H}` in a path or in the clone file is the name of the machine answering.
# Left out on purpose: links (a platform's to make), a unit claim (focus
# refuses it), and backslashes in names.
# --------------------------------------------------------------------------

ROW_STAMPS = ["2026-10-09T16:22:42Z", "2026-10-09T16:22:42Z", "2026-10-09T16:22:43Z", "2026-10-09T16:23:00Z",
              "2026-10-08T23:59:59Z"]
NOTE_HEADINGS = ["## A note", "##  Spaced heading ", "## Decision", "##", "### Deeper", "##Tight", "## é"]
SHARD_NAMES = ["laptop-0123456789ab.md", "laptop-0123456789ab 2.md", "desktop-ffffffffffff.md", "old-name-abc123.md",
               "build-01-abc123.md", "copy of laptop.md", "Laptop-ABC.md", "notes.md", "a-b.md", "-x-1.md"]


def shard_row():
    """One block of a shard, without the rule that ends it."""
    stamp = rng.choice(ROW_STAMPS)
    event = rng.choice(QUERY_EVENTS)
    roll = rng.random()
    if roll < 0.6:
        lines = ["", f"## {event.title()}", f"**Timestamp**: {stamp}", f"**Event**: {event}"]
        if rng.random() < 0.5:
            lines.append(rng.choice(["**Stage**: code-generation", "**Bolt slug**: one", "**Note**: a\\nb"]))
    elif roll < 0.82:
        at = rng.choice([f"**Timestamp**: {stamp}", f"**Timestamp**: {stamp}", f"- **Timestamp**: {stamp}"])
        lines = rng.choice([
            ["", rng.choice(NOTE_HEADINGS), at, "first line", "", "second line  "],
            ["", rng.choice(NOTE_HEADINGS), at],
            ["", rng.choice(NOTE_HEADINGS), "before the stamp", at, "after it"],
            ["", at, rng.choice(NOTE_HEADINGS), "stamp above the heading"],
            ["", "no heading", at],
            ["", "## One", "## Two", at, "text"],
            ["", "## One", at, f"**Timestamp**: {stamp}", "a second stamp"],
        ])
    else:
        lines = rng.choice([
            ["", f"## {event.title()}", f"**Event**: {event}"],
            ["", f"## {event.title()}", "**Timestamp**:", f"**Event**: {event}"],
            ["", f"## {event.title()}", f"**Timestamp**: {stamp}", "**Event**:"],
            ["", f"## {event.title()}", f"**Timestamp**: {stamp}", "**Event**:   "],
            ["", "just prose"],
            [""],
        ])
    lines.append("")
    return "\n".join(lines)


def shard_text():
    rows = [shard_row() for _ in range(rng.randrange(0, 5))]
    if rows and rng.random() < 0.15:
        rows.append(rng.choice(rows))
    return "# AI-DLC Audit Log\n" + "".join(row + "\n---\n" for row in rows)


def shard_set(count):
    """`count` shards for one directory, as [name, content]: some independent, some copies of an earlier one."""
    shards = []
    for name in rng.sample(SHARD_NAMES, count):
        if shards and rng.random() < 0.6:
            source = rng.choice(shards)[1]
            rows = source.split("\n---\n")
            roll = rng.random()
            if roll < 0.3:
                content = source
            elif roll < 0.5:
                content = "\n---\n".join(rows[:rng.randrange(1, len(rows) + 1)]) + rng.choice(["\n---\n", ""])
            elif roll < 0.75:
                content = source + "".join(shard_row() + "\n---\n" for _ in range(rng.randrange(1, 3)))
            elif roll < 0.85:
                content = source.replace("\n", "\r\n")
            elif roll < 0.93:
                content = source.replace("**Timestamp**: 2", "**Timestamp**: 1", 1)
            else:
                content = "\n---\n".join(rows[:1] + rows[2:])
        else:
            content = shard_text()
        shards.append([name, content])
    return shards


for _ in range(scale * 60):
    shards = shard_set(rng.choice([1, 2, 2, 2, 3, 3, 4]))
    names = [name for name, _ in shards]
    prefix = rng.choice(["", "audit/", "/project/aidlc/spaces/default/intents/x/audit/"])
    add("shard_read",
        shards=[{"shard": prefix + name, "content": content} for name, content in shards],
        own=rng.choice([None, None, names[0], rng.choice(names), rng.choice(names), "elsewhere-000000000000.md"]))

LEDGER_HOSTS = ["laptop", "desktop", "old-name", "build-01"]
CLONE_ID = "aidlc/.aidlc-clone-id"


def ledger_case():
    files, mtimes = {}, {}
    token = rng.choice(["abc123", "0123456789ab"])
    roll = rng.random()
    if roll < 0.3:
        pass
    elif roll < 0.55:
        files[CLONE_ID] = f"{token}\n{rng.choice(LEDGER_HOSTS + ['{H}'])}\n"
    elif roll < 0.85:
        files[CLONE_ID] = rng.choice([f"{token}\n", f"{token}\n", token, f"{token}\r\n", f" {token} \n\n",
                                      f"{token}\nBad Host\n", f"{token}\n-bad\n", f"\ufeff{token}\n",
                                      f"{token}\n" + "h" * 49 + "\n"])
    else:
        files[CLONE_ID] = rng.choice(["", "\n", "ABC123\nlaptop\n", "a" * 33 + "\nlaptop\n", "two words\nlaptop\n",
                                      f"{token}\r\nlaptop\r\n", f" {token} \n laptop \nthird\n", "\nlaptop\n"])
    spaces = ["default"] + (["payments"] if rng.random() < 0.35 else [])
    held = {}
    for space in spaces:
        base = f"aidlc/spaces/{space}/intents"
        records = rng.sample(RECORDS[:3], rng.choice([0, 1, 1, 1, 2]))
        held[space] = records
        for record in records:
            files[f"{base}/{record}/aidlc-state.md"] = "state\n"
        if records and rng.random() < 0.8:
            files[f"{base}/active-intent"] = rng.choice(records)
        for audit in [f"{base}/audit"] + [f"{base}/{record}/audit" for record in records]:
            roll = rng.random()
            if roll < 0.2:
                continue
            if roll < 0.25:
                files[audit] = "a file where a directory goes\n"
                continue
            shards = shard_set(rng.choice([1, 1, 2, 2, 3]))
            if rng.random() < 0.55:
                # A shard this clone's token names, under some host.
                host = rng.choice(LEDGER_HOSTS + ["{H}", "{H}", "Bad_Host"])
                # Often a copy of a neighbour, and often the shorter of the two: being this clone's own
                # is then the only thing that makes it the one to read.
                content = shard_text()
                if rng.random() < 0.6:
                    content = rng.choice(shards)[1]
                    rows = content.split("\n---\n")
                    if len(rows) > 2 and rng.random() < 0.6:
                        content = "\n---\n".join(rows[:rng.randrange(1, len(rows) - 1)]) + "\n---\n"
                shards.append([f"{host}-{token}.md", content])
            for name, content in shards:
                files[f"{audit}/{name}"] = content
            if rng.random() < 0.12:
                files[f"{audit}/folder.md/inner.txt"] = "a directory named like a shard\n"
            if rng.random() < 0.2:
                files[f"{audit}/README.txt"] = "not a shard\n"
    if rng.random() < 0.3:
        files["aidlc/active-space"] = rng.choice(spaces)
    for rel in files:
        if rel.endswith(f"-{token}.md"):
            mtimes[rel] = 1760000000 + rng.choice([0, 0, 60, 120])
    selector = {}
    space = rng.choice(spaces)
    roll = rng.random()
    if roll < 0.2:
        selector["space"] = space
    elif roll < 0.35 and held[space]:
        selector["intent"] = rng.choice(held[space])
    elif roll < 0.45 and held[space]:
        selector = {"space": space, "intent": rng.choice(held[space])}
    elif roll < 0.5:
        selector["space"] = rng.choice(["zz", "payments"])
    fields = {"files": files, "selector": selector, "mintFirst": rng.random() < 0.5}
    if mtimes:
        fields["mtimes"] = mtimes
    return fields


for _ in range(scale * 50):
    add("ledger", **ledger_case())

# --------------------------------------------------------------------------
# Settings files: one file checked, and the machine's, the project's and one
# person's merged.
#
# Left out on purpose, because the two JSON parsers do not agree on them and
# no settings file needs them: numbers too large for a double, and strings
# with half of a surrogate pair. A `release-base-url` is only ever one the two
# engines read alike (focus reads one spelling of a location; see its test).
# `ca-bundle` is only ever `/...` or relative: what else is absolute depends on
# the platform replaying the corpus.
# --------------------------------------------------------------------------

BYPASSES = ["AIDLC_SKIP_ARTIFACT_GUARD", "AIDLC_SKIP_REVISION_BACKSTOP", "AIDLC_SKIP_SUMMARY_CONFIRMATION_GUARD",
            "AIDLC_SKIP_HUMAN_PRESENCE_GUARD", "AIDLC_DISABLE_ENSEMBLE_EVIDENCE", "AIDLC_DISABLE_PLAN_APPROVAL_GUARD",
            "AIDLC_DISABLE_REVIEWER_SCOPE_HOOK", "AIDLC_DISABLE_REVIEW_FREEZE_HOOK", "AIDLC_DISABLE_USAGE_TRACKING",
            "AIDLC_DISABLE_SENSORS", "AIDLC_DISABLE_LEARNINGS", "AIDLC_DISABLE_SUMMARY_CONFIRMATION"]
FLAG_VARIABLES = ["AWS_AIDLC_DEFAULT_SCOPE", "AIDLC_USE_SWARM", "AIDLC_HOOK_DEBUG", "AIDLC_SENSOR_TIMEOUT_MS",
                  "AIDLC_QUESTION_RETENTION_DAYS"]
RELEASE_URLS = ["https://example.com", "https://releases.example.com:8443/aidlc/", "https://example.com/a///",
                "https://example.com/a/b%20c", "http://localhost:8080", "http://127.0.0.1/x", "https://10.0.0.1",
                "http://example.com", "https://user@example.com", "https://user:secret@example.com",
                "https://example.com/?q=1", "https://example.com/#top", "ftp://example.com", "example.com", "https://",
                "https://example.com:99999", "https://1.2.3.4.5", "https://256.1.1.1", "", 5, None, True]
EFFORTS = ["low", "medium", "high", "xhigh", "max"]


# How likely each value of the file being generated is to be an invalid one. Zero for about half of the files,
# so that half get past every check and the rest mostly fail at one.
ODDS = [0.0]


def odd(valid, *invalid):
    """Mostly a valid value, sometimes one of the invalid ones."""
    return rng.choice(invalid) if invalid and rng.random() < ODDS[0] else valid


def flags_section():
    flags = {"schemaVersion": odd(1, 2, "1", None, 1.5)}
    if rng.random() < ODDS[0]:
        del flags["schemaVersion"]
    if rng.random() < 0.4:
        flags["defaultScope"] = odd(rng.choice(["feature", "bugfix", "x-1", "a"]), "Feature", "", "1st", 5, None,
                                    "has space")
    if rng.random() < 0.4:
        flags["swarm"] = odd(rng.choice([True, False]), "true", 1, None)
    if rng.random() < 0.3:
        flags["hookDebug"] = odd(rng.choice([True, False]), 0, "no")
    if rng.random() < 0.4:
        flags["sensorTimeoutMs"] = odd(rng.choice([1, 900, 60000, 5.0, 1e3]), 0, -5, 1.5, "900", None, True)
    if rng.random() < 0.3:
        flags["questionRetentionDays"] = odd(rng.choice([1, 30, 365]), 0, 2.5, "30")
    if rng.random() < 0.55:
        names = rng.sample(BYPASSES, rng.randrange(0, 5))
        if names and rng.random() < 0.3:
            names.append(names[0])
        flags["bypasses"] = odd(names, names + ["AIDLC_DISABLE_COLLABORATORS"], names + [5], "AIDLC_DISABLE_SENSORS",
                                None, {"AIDLC_DISABLE_SENSORS": True}, names + ["aidlc_disable_sensors"])
    if rng.random() < ODDS[0]:
        flags[rng.choice(["unknown", "Swarm", "1", "bypass"])] = True
    return flags


def groups_section():
    groups = {}
    for name in rng.sample(["deciding", "reviewing", "writing-up"], rng.randrange(0, 4)):
        groups[name] = odd({"effort": rng.choice(EFFORTS)}, {"effort": "extreme"}, {}, {"effort": "low", "more": 1},
                           "low", None, {"effort": 5})
    if rng.random() < ODDS[0]:
        groups[rng.choice(["thinking", "Deciding", "2"])] = {"effort": "low"}
    return groups


def models_section():
    models = {"schemaVersion": odd(1, 2, "1")}
    if rng.random() < 0.4:
        models["preset"] = odd(rng.choice(["thorough", "balanced", "minimal"]), "fast", 5, None)
    if rng.random() < 0.5:
        models["groups"] = odd(groups_section(), [], None, "x")
    if rng.random() < 0.6:
        agents = {}
        for name in rng.sample(["architect", "reviewer", "qa-lead", "2", "10", "constructor", "x1"], rng.randrange(0, 4)):
            agent = {}
            if rng.random() < 0.6:
                agent["effort"] = odd(rng.choice(EFFORTS), "extreme", 5, None)
            if rng.random() < 0.6:
                agent["model"] = odd(
                    {harness: odd(rng.choice(["model-a", " model-b ", "m"]), "", "  ", 5, None)
                     for harness in rng.sample(["claude", "codex", "copilot", "cursor", "kiro", "kiro-ide", "opencode"],
                                               rng.randrange(0, 3))},
                    {"vscode": "m"}, [], "m", None)
            if rng.random() < ODDS[0]:
                agent["temperature"] = 1
            agents[name] = odd(agent, None, "x", [])
        if rng.random() < ODDS[0]:
            agents[rng.choice(["Architect", "-lead", "a_b", ""])] = {"effort": "low"}
        models["agents"] = odd(agents, [], None)
    if rng.random() < 0.35:
        profiles = {}
        for name in rng.sample(["fast", "careful", "3", "night-shift"], rng.randrange(0, 3)):
            profiles[name] = odd({"groups": groups_section()}, {}, {"groups": groups_section(), "more": 1},
                                 {"groups": None}, None, "x")
        if rng.random() < ODDS[0]:
            profiles["Bad Name"] = {"groups": {}}
        models["profiles"] = odd(profiles, [], None)
    if rng.random() < ODDS[0]:
        models[rng.choice(["default", "model"])] = "x"
    return models


def settings_object(layer):
    ODDS[0] = rng.choice([0.0, 0.0, 0.0, 0.04, 0.04, 0.1])
    settings = {"schemaVersion": odd(1, 2, "1", None, 0)}
    if rng.random() < ODDS[0] / 2:
        del settings["schemaVersion"]
    if rng.random() < 0.2:
        settings["$schema"] = odd("./aidlc-settings.schema.json", "", "   ", 5, None)
    if rng.random() < 0.6:
        settings["flags"] = odd(flags_section(), None, [], "x", 5)
    if rng.random() < 0.4:
        settings["models"] = odd(models_section(), None, [], "x")
    if rng.random() < (0.6 if layer == "machine" else 0.05):
        if rng.random() < 0.5:
            settings["update-check"] = odd(rng.choice([True, False]), "false", 0, None)
        if rng.random() < 0.5:
            settings["offline"] = odd(rng.choice([True, False]), "yes")
        if rng.random() < 0.6:
            settings["release-base-url"] = rng.choice(RELEASE_URLS[:7] if rng.random() < 0.6 else RELEASE_URLS)
        if rng.random() < 0.4:
            settings["ca-bundle"] = rng.choice(["/etc/ssl/ca.pem", "/etc/ssl/ca.pem", "/", "relative/ca.pem", "", 5,
                                                None])
    if rng.random() < ODDS[0]:
        settings[rng.choice(["flag", "Models", "plugins", "7"])] = {}
    return settings


def settings_raw(layer):
    roll = rng.random()
    if roll < 0.04:
        return rng.choice(["", "{", "nope", "[]", "null", "5", '"x"', "﻿{\"schemaVersion\":1}", "{} extra",
                           '{"schemaVersion":1,}'])
    if roll < 0.16:
        return rng.choice(['{"schemaVersion":1.0}', '{"schemaVersion":1e0}', '{"schemaVersion":1,"schemaVersion":2}',
                           '{"schemaVersion":2,"schemaVersion":1}',
                           '{"schemaVersion":1,"flags":{"schemaVersion":1.0,"sensorTimeoutMs":5.0}}',
                           '{"schemaVersion":1,"flags":{"schemaVersion":1,"sensorTimeoutMs":1e3,"questionRetentionDays":2e1}}',
                           '{"schemaVersion":1,"flags":{"schemaVersion":1,"sensorTimeoutMs":1.5e0}}',
                           '{"schemaVersion":1,"flags":{"schemaVersion":1,"sensorTimeoutMs":9007199254740993}}',
                           '{\n  "schemaVersion": 1,\n  "flags": { "schemaVersion": 1, "swarm": true }\n}\n',
                           # One of each refusal a generated file rarely reaches.
                           '{"schemaVersion":1,"models":{"schemaVersion":1,"groups":{"deciding":{"effort":"low","more":1}}}}',
                           '{"schemaVersion":1,"models":{"schemaVersion":1,"groups":{"deciding":{"effort":"low"},"thinking":{"effort":"low"}}}}',
                           '{"schemaVersion":1,"models":{"schemaVersion":1,"profiles":{"fast":{"groups":{},"more":1}}}}',
                           '{"schemaVersion":1,"models":{"schemaVersion":1,"profiles":{"Fast":{"groups":{}}}}}',
                           '{"schemaVersion":1,"models":{"schemaVersion":1,"profiles":{"fast":{}}}}',
                           '{"schemaVersion":1,"models":{"schemaVersion":1,"agents":{"architect":{"effort":"low","temperature":1}}}}',
                           '{"schemaVersion":1,"models":{"schemaVersion":1,"agents":{"architect":{"model":{"vscode":"m"}}}}}',
                           '{"schemaVersion":1,"models":{"schemaVersion":1,"agents":{"architect":{"model":{"claude":"  "}}}}}',
                           '{"schemaVersion":1,"models":{"schemaVersion":1,"agents":{"architect":{},"reviewer":{"model":{}}}}}',
                           '{"schemaVersion":1,"models":{"schemaVersion":1,"agents":{"10":{"effort":"low"},"b":{"effort":"max"},"2":{"effort":"high"}},"profiles":{"z":{"groups":{}},"1":{"groups":{}}}}}',
                           '{"schemaVersion":1,"flags":{"schemaVersion":1,"defaultScope":"1st"}}',
                           '{"schemaVersion":1,"flags":{"schemaVersion":1,"sensorTimeoutMs":0}}',
                           '{"schemaVersion":1,"flags":{"schemaVersion":1,"questionRetentionDays":-1}}',
                           '{"schemaVersion":1,"flags":{"schemaVersion":1,"zeta":1,"alpha":2,"3":true}}',
                           '{"schemaVersion":1,"zeta":1,"alpha":2,"3":true}',
                           '{"schemaVersion":1,"$schema":" "}'])
    return json.dumps(settings_object(layer), ensure_ascii=rng.random() < 0.5)


for _ in range(scale * 100):
    layer = rng.choice(["machine", "machine", "project", "project", "local"])
    add("settings_file", raw=settings_raw(layer), layer=layer)


def valid_settings(layer):
    """A settings file that is accepted, with enough in it to merge."""
    settings = {"schemaVersion": 1}
    flags = {"schemaVersion": 1}
    if rng.random() < 0.5:
        flags["defaultScope"] = rng.choice(["feature", "bugfix", "poc"])
    if rng.random() < 0.5:
        flags["swarm"] = rng.choice([True, False])
    if rng.random() < 0.4:
        flags["hookDebug"] = rng.choice([True, False])
    if rng.random() < 0.4:
        flags["sensorTimeoutMs"] = rng.choice([900, 60000, 5.0])
    if rng.random() < 0.3:
        flags["questionRetentionDays"] = rng.choice([7, 30])
    if rng.random() < 0.6:
        flags["bypasses"] = rng.sample(BYPASSES, rng.randrange(0, 4))
    if rng.random() < 0.8:
        settings["flags"] = flags
    if rng.random() < 0.45:
        models = {"schemaVersion": 1}
        if rng.random() < 0.5:
            models["preset"] = rng.choice(["thorough", "balanced", "minimal"])
        if rng.random() < 0.6:
            models["groups"] = {name: {"effort": rng.choice(EFFORTS)}
                                for name in rng.sample(["deciding", "reviewing", "writing-up"], rng.randrange(1, 4))}
        if rng.random() < 0.6:
            models["agents"] = {
                name: rng.choice([{"effort": rng.choice(EFFORTS)},
                                  {"model": {rng.choice(["claude", "cursor", "copilot"]): rng.choice(["model-a", "m"])}},
                                  {"effort": rng.choice(EFFORTS), "model": {"claude": "model-a", "codex": "c"}}])
                for name in rng.sample(["architect", "reviewer", "2", "10", "qa-lead"], rng.randrange(1, 4))}
        if rng.random() < 0.3:
            models["profiles"] = {rng.choice(["fast", "careful"]): {"groups": {"deciding": {"effort": "low"}}}}
        settings["models"] = models
    if layer == "machine" and rng.random() < 0.6:
        settings.update(rng.choice([{"offline": True}, {"update-check": False},
                                    {"release-base-url": "https://releases.example.com/aidlc/"},
                                    {"ca-bundle": "/etc/ssl/ca.pem", "offline": False}]))
    return json.dumps(settings)


LAYER_FILES = {"machine": "machine/aidlc.settings.json", "project": "project/aidlc.settings.json",
               "local": "project/aidlc.settings.local.json"}

for _ in range(scale * 60):
    files = {}
    for layer, rel in LAYER_FILES.items():
        roll = rng.random()
        if roll < 0.3:
            continue
        if roll < 0.9:
            files[rel] = valid_settings(layer)
        elif roll < 0.96:
            files[rel] = settings_raw(layer)
        else:
            files[rel + "/inside.txt"] = "a directory where the file goes\n"
    asked = rng.sample(BYPASSES, 3) + rng.sample(FLAG_VARIABLES, 2) + ["AIDLC_DISABLE_COLLABORATORS"]
    env = {}
    if rng.random() < 0.4:
        env[rng.choice(asked)] = rng.choice(["1", "0", "", "true", "feature"])
    probe = rng.sample(["env AIDLC_DISABLE_SENSORS", "env ", "env", "AIDLC_DISABLE_SENSORS in aidlc.settings.json",
                        "AIDLC_X9_Y in /somewhere", "AIDLC_DISABLE_SENSORS in", "AIDLC_DISABLE_SENSORS  in x",
                        "aidlc_disable_sensors in x", "X AIDLC_DISABLE_SENSORS in x", "scope feature", "", "AIDLC_ in x"], 4)
    add("settings_layers", files=files, ask=asked, env=env, probe=probe)

# --- markdown: a document's structure, line by line ---------------------------
#
# Three sources: the documents upstream keeps to pin its own analysis
# (tests/fixtures/markdown-goldens), the Markdown it ships and reads (stage,
# agent, sensor, scope and memory files), and documents written here the way a
# careful author writes them. Documents written carelessly or with malice are
# compared live, not vendored (`differential.py --markdown`).


def markdown(content):
    # The corpus is stored in Focus's names. A phrase the map changes the
    # length of would move the columns upstream answered with, and a renamed
    # label would sort elsewhere in its list of labels.
    if any(theirs in content for theirs, _ in PHRASES) or re.search(r"\[[^\]\n]*aidlc[^\]\n]*\]:", content, re.I):
        return
    add("markdown", content=content)


# Written by hand: what the generated documents reach too rarely to pin, and
# one document for each way the two analyses part (the same documents as in
# crates/focus-markdown/tests/deviations.rs).
WRITTEN = [
    # A construct over three lines: the middle line is wholly inside it.
    "a `b\nmiddle\nc` d\n",
    "x <!-- a\nmiddle\nb --> y\n",
    "text <span\n  class=\"x\">s</span> more\n",
    "text <a\n  href=\"u\"\n  title=\"t\">link</a> more\n",
    # A list item three columns into a quote, and code four columns in.
    ">    - item\n>      more\n",
    ">     code\n> text\n",
    # A block of raw text ended on its first line, by another's closing tag, in upper case.
    "<pre> x </style>\nafter\n",
    "<pre>\ncode\n</style>\n\nafter\n",
    "<SCRIPT>\nx\n</SCRIPT>\nafter\n\n<style>\n</TextArea> y\nz\n",
    "- <pre>\n  </script>\n- <textarea>\n  x\n\n  </pre>\n\ntext\n",
    # One label defined three times, and twice with a title on its own line.
    "[a]: /1\n[a]: /2\n[a]: /3\n\n[a]\n",
    "[a]: /one\n[A]: /two\n  'title'\n\n[a]\n",
    # Blank lines after indented code are not code.
    "    code\n\ntext\n",
    "    code\n\n    more\n    \n\ntext\n",
    # A NUL before a code span: one unit in the source, three bytes as the parser reads it.
    "a\x00b `c` d\n",
    "\x00 <!-- c --> `x` \x00\n",
    # A code span in a table cell with an escaped pipe before, after and around the probe.
    "| a | b |\n| --- | --- |\n| `a\\|b` | `x\\|y\\|z` |\n| ` p\\|q ` | `\\|` |\n",
    # The same with the escaped pipe before the probe, where which probe is measured from shows.
    "| a | b |\n|---|---|\n| `\\|a` | `\\|a\\|` |\n| `a\\|b\\|c` | x `\\|` `q\\|` |\n| <!-- \\| --> `z` | t |\n",
    # A comment block, then on its last line a comment whose opening is `<!-->`.
    "<!-- a --> x <!--> y --> z\n",
    # Heading tags inside a line, and opening one.
    "text <h2 id=x>t</h2> and <h3>u</h3>\n\n<h2>t</h2> <h3>u</h3>\n",
    # Where the analyses part.
    "1. one\n\n11.\n\n> a\n>\n> b\n",
    "".join(f"| a{n} | b |\n| --- | --- |\n| `x{n}` | <!-- c --> |\n\n" for n in range(60))
    + "## Guard Policy\n\n<!--\nMode: strict\n-->\n",
    "> [Answer]:\n> ---\n",
    "<!x y>\nstill text\n",
    "[Answer]: <!--<!---->\n\n[Answer]\n",
    "a `` b\nMode: strict\n`` c\n",
    "a \\` b `\nMode: strict\n` c\n",
    "para\n    <div>\nmore\n",
    "para `x\n    <h2> a` b\n",
    "a\n\n\u00a0\n\nb\n",
    # A tight list item whose text has a line with no text of its own, and one with none at all.
    "- see [the plan](docs/plan.md\n  \"Plan\")\n- next\n",
    "- [a\n  ](u)\n- **bold\n  over lines**\n",
    "- ![](x)\n- [](y)\n",
    # Text of a tight item after a quote that holds only code.
    "- a\n  > ```\n  > code\n  > ```\n  b\n",
    "- a\n  >     code\n  b\n- c\n",
    # Letters that fold to ASCII ones are not them: `ſ` is no `s`.
    "<pre>\n</\u017ftyle>\nMode: strict\n</pre>\nafter\n",
    # A declaration the parser would end at a quote marker on the next line.
    "> a <!x ` b\n> Mode: strict\n",
    # Columns are counted in UTF-16 code units where upstream counts them.
    "| a |\n|---|\n| `x\\|\\|\u00e9\u00e9` | `\u00e9\\|x` |\n",
    # Form feed and vertical tab are characters, not white space.
    "a\n\u000c\nb\n",
    ">\u000c\n",
    "<pre\u000c>\nx\n</PRE>\nMode: strict\n",
    "[a]: /u\u000b\n\n[a]\n",
    "x\u000c`c` <b\u000cid=x>\n",
    # A list item holding only a definition, then a line of white space indented past it.
    "- [a]: b\n      \n",
    "> - [a]: /u\n\t\n",
    "- item\n  - [ref]: https://example.com\n        \n- next\n",
]
for document in WRITTEN:
    markdown(document)
# The same reader as a memory file's: what hides a `Mode:` line and what does not.
for document in [
    "## Guard Policy\n\n> - [a]: b\n\t\n\nMode: off\n",
    "## Guard Policy\n<pre>\n</\u017ftyle>\nMode: strict\n</pre>\n",
    "## Guard Policy\n<pre\u000c>\nMode: strict\n",
    "## Guard Policy\n- see [the plan](docs/plan.md\n  \"Mode: strict\")\n- Mode: off\n",
    "## Guard Policy\n- a\n  > ```\n  > x\n  > ```\n  Mode: relaxed\n",
    "## Guard Policy\r\nMode: strict\r",
]:
    add("guard", content=document)
goldens = json.loads(read(os.path.join(core, "..", "tests", "fixtures", "markdown-goldens", "corpus.json")))["documents"]
for name in sorted(goldens):
    markdown(goldens[name]["content"])
for part in ("aidlc-common/stages", "agents", "sensors", "scopes", "memory"):
    for path in sorted(glob.glob(os.path.join(core, part, "**", "*.md"), recursive=True)):
        markdown(read(path))
careful = markdown_docs.writer(rng, careful=True)
for _ in range(scale * 50):
    markdown(careful())

# --- icase: which characters a case-insensitive pattern takes for which -------
#
# Outside the Unicode modes JavaScript compares characters by their upper-case
# form, and never takes a character outside ASCII for one inside it. Every
# character of the Basic Multilingual Plane with another case is a pattern
# here, alone and as a class of one, and the text searched holds all of them.
# The characters focus's engine compares otherwise, and the ones focus puts in
# their place (crates/focus-js/src/re.rs), are not patterns: focus refuses a
# pattern that names one. They are in the text, where what matters is that no
# other pattern takes them.
MISFOLDED = ({0x131, 0x17F, 0x3F4, 0x1E9E, 0x1FD3, 0x1FE3, 0x2126, 0x212A, 0x212B, 0xFB05, 0xFB06}
             | set(range(0x1F80, 0x1FB0)) | {0x1FB3, 0x1FBC, 0x1FC3, 0x1FCC, 0x1FF3, 0x1FFC})
STAND_INS = {0x138, 0x2129}
CASED = [cp for cp in range(0x80, 0x20000)
         if not 0xD800 <= cp <= 0xDFFF and (chr(cp).upper() != chr(cp) or chr(cp).lower() != chr(cp))]
BMP = [cp for cp in CASED if cp < 0x10000]
LETTERS = list(range(0x41, 0x5B)) + list(range(0x61, 0x7B))
NAMED = [cp for cp in LETTERS + BMP if cp not in MISFOLDED | STAND_INS]
ICASE_TEXT = ("".join(chr(cp) for cp in range(0x20, 0x7F)) + "".join(chr(cp) for cp in BMP)
              + "ĸ℩ 日本 sſ kK iı Straße STRASSE")
ICASE_PATTERNS = ([f"\\u{cp:04x}" for cp in NAMED] + [f"[\\u{cp:04x}]" for cp in NAMED]
                  # Two in a row, which an engine may search for otherwise than one.
                  + [f"\\u{a:04x}\\u{b:04x}" for a, b in list(zip(NAMED, NAMED[1:]))[::3]]
                  + ["[a-z]", "[A-Z]", "[^a-z]", "[^A-Z0-9 -/]", r"\w", r"\W+", "[a-z]+", r"\S+", r".",
                     r"[\D]+", r"[^\D]+", r"[\S]+", r"[^\S]+", r"[\s\S]+", "[^\\s\"']+", r"[\w-]+", r"[^\w-]+",
                     r"[\d\s]+", r"[^\d\s]+", r"(s)\1", r"(k)\1", r"(i)\1", "strasse", r"straße",
                     r"\bs\b", r"s\B", "s", "S", "k", "K", "i", "I", r"[sS]", r"[^sS]", r"[ék]",
                     r"[^ék]+", r"[^ß]+", r"[^ωå]+", r"µ", r"σ", r"ǅ",
                     r"[αω]", r"<(script|style)\b[\s\S]*?(?:<\/\1\s*>|$)"])
for start in range(0, len(ICASE_PATTERNS), 200):
    add("icase", patterns=ICASE_PATTERNS[start:start + 200], hay=ICASE_TEXT)
# A backreference compares what a group took with the text, a third way to
# compare: every ASCII letter is followed here by every character with a
# case, and none may be taken for the letter.
add("icase", patterns=["(" + "|".join(chr(letter) for letter in range(0x61, 0x7B)) + r")\1", r"(s|k|i)\1"],
    hay="".join(chr(letter) + chr(cp) for letter in range(0x61, 0x7B) for cp in BMP))
# Upstream's own case-insensitive patterns, over that text and over lines where
# a letter focus's engine would misread stands where the pattern has its ASCII
# twin. One of them refused by focus would fail here, before its module is
# ported.
TWINS = "\n".join([
    "<script>x</\u017fcript> <STYLE>a</\u017fTYLE> </style>", "<H1> <h\u0131> <t\u0131tle>x</T\u0131TLE>",
    "eslint couldn\u2019t find an eslint.config", "ESLINT COULDN'T FIND A CONFIGURATION",
    "the human\u2019s deci\u017fion", "not a human decision \u3002", "conductor\u2019s default\u3002",
    "```J\u017fON\n{}\n```", "```JSON\n{}\n```", "Mode: \u017ftrict", "Mode: STRICT", "\u212aey: value",
    "\u017fkip the plan-approval", "please SKIP the plan approvals", "bun .\u212airo/tools/x.ts", "\u2126 \u212b \u1e9e",
])
for start in range(0, len(UPSTREAM_ICASE), 200):
    add("icase", patterns=UPSTREAM_ICASE[start:start + 200], hay=ICASE_TEXT + "\n" + TWINS)
# Beyond the Basic Multilingual Plane a character is two code units to
# JavaScript, and a code unit of a pair has no other case. focus refuses a
# pattern that names such a character; what is left to check is that no
# other pattern is troubled by them. None here consumes half a pair.
add("icase", patterns=["[a-z]+", r"\w+", r"\W+", r"[^a-z ]+", r"\S+", r"(az)\1", r"[\s\S]+"],
    hay="".join(chr(cp) for cp in CASED if cp >= 0x10000) + " az AZ azAZ")

# --- The runtime graph's readers: instants, diaries, units, checkboxes --------
#
# A generator of its own, so that what it draws moves none of the cases above.
rt = random.Random(seed * 7919 + 6)


def instant():
    """A timestamp of the one shape focus reads: an instant with its zone written out, on a real date."""
    year = rt.choice([0, 1, 99, 1600, 1900, 1969, 1970, 1999, 2000, 2024, 2026, 2100, 9999, rt.randrange(10000)])
    month = rt.randrange(1, 13)
    leap = year % 4 == 0 and (year % 100 != 0 or year % 400 == 0)
    last = [31, 29 if leap else 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31][month - 1]
    day = rt.choice([1, last, rt.randrange(1, last + 1)])
    text = f"{year:04d}-{month:02d}-{day:02d}T{rt.randrange(24):02d}:{rt.randrange(60):02d}"
    if rt.random() < 0.85:
        text += f":{rt.randrange(60):02d}"
        if rt.random() < 0.4:
            text += "." + "".join(rt.choice("0123456789") for _ in range(rt.choice([1, 2, 3, 3, 4, 6, 8, 9])))
    if rt.random() < 0.6:
        return text + "Z"
    return text + f"{rt.choice('+-')}{rt.choice([0, 1, 5, 12, 14, 23, rt.randrange(24)]):02d}:{rt.choice([0, 30, 45, 59, rt.randrange(60)]):02d}"


def not_an_instant(text):
    """`text` broken in a way that makes it no date to JavaScript either."""
    way = rt.randrange(11)
    if way == 0:
        return text[:5] + rt.choice(["00", "13", "19", "99"]) + text[7:]
    if way == 1:
        return text[:8] + rt.choice(["00", "32", "40", "99"]) + text[10:]
    if way == 2:
        return text[:14] + rt.choice(["60", "61", "99"]) + text[16:]
    if way == 3 and text[16:17] == ":":
        return text[:17] + rt.choice(["60", "61", "99"]) + text[19:]
    if way == 4:
        return text + rt.choice(["x", " ", "Z", "junk", ":00", "\u200b"])
    if way == 5:
        return rt.choice(["x", " ", "T", "+"]) + text
    if way == 6:
        return text[:4] + rt.choice(["/", ".", "", "\u2010"]) + text[5:]
    if way == 7 and text.endswith("Z"):
        return text[:-1] + rt.choice(["+24:00", "+00:60", "+1:00", "+01", "-99:99", "+01:0", " Z", "UTC", "\u2212" + "01:00"])
    if way == 8:
        return text[:11] + rt.choice(["25", "30", "99", "2"]) + text[13:]
    if way == 9:
        return text[:13] + rt.choice([".", "-", "", " "]) + text[14:]
    return text.replace(":", rt.choice(["", ";", "::"]), 1)


WORDS = ["yesterday", "tomorrow", "soon", "now", "today", "z-last", "Jan", "January", "Mon", "Monday", "GMT", "UTC",
         "Z", "T", "AM", "PM", "noon", "midnight", "never", "n/a", "TBD", "-", "--", ":", "::", "T::Z", "(", "()", "+",
         "\u00e9t\u00e9", "\u65e5\u672c", "\U0001F600", "\u0661\u0662\u0663", "\uff12\uff10\uff12\uff16", "\u2014",
         " ", "\t", "\u00a0", "Infinity", "NaN", "null", "undefined", "true"]
for _ in range(scale * 3):
    texts = [instant() for _ in range(60)]
    texts += [not_an_instant(instant()) for _ in range(20)]
    # No digit, no date: single words, and several strung together.
    texts += [rt.choice([" ", "-", ",", ", ", "/", ":", "T", ""]).join(rt.choice(WORDS) for _ in range(rt.randrange(1, 5)))
              for _ in range(20)]
    add("instant", texts=texts)
add("instant", texts=["2026-01-05T10:00:00Z", "2026-01-05T10:00:00.000Z", "2026-01-05T10:00Z", "1970-01-01T00:00:00Z",
                      "1969-12-31T23:59:59.999Z", "0000-01-01T00:00:00Z", "9999-12-31T23:59:59.999Z",
                      "2024-02-29T23:59:59-23:59", "2000-02-29T00:00:00+23:59", "1900-02-28T12:00:00.5+05:45",
                      "9999-12-31T23:59:59.999999999Z", "0000-01-01T00:00:00.999999999Z", "2026-06-30T23:59:59.9999-23:59",
                      "2026-01-05T10:00:00.000000001Z", "2026-01-05T10:00:00.099999999Z", "2026-01-05T10:00:00.0005Z",
                      "", "yesterday", "2026-13-01T00:00:00Z", "2026-01-32T00:00:00Z", "2026-01-05T23:60:00Z",
                      "2026-01-05T23:59:60Z", "2026-01-05T10:00:00+24:00", "2026-01-05T10:00:00.Z",
                      "2026-01-05T10:00:00Zjunk", "2026-01-05T10:00:00+01", "2026-1-5T10:00:00Z",
                      "2026-01-05T10:00:00 Z", "2026-01-05T10:00:00,5Z", "20260105T100000Z", "2026-W01"])

MEMORY_HEADINGS = ["## Interpretations", "## Deviations", "## Tradeoffs", "## Open questions"]
MEMORY_LINES = (MEMORY_HEADINGS * 3 + [
    "## Interpretations ", " ## Deviations", "## tradeoffs", "## Open Questions", "### Deviations", "##Tradeoffs",
    "## Notes", "## ", "##", "# Memory", "", "", "   ", "\t", "\u00a0", "\u3000", "\ufeff",
    "- 2026-01-05T10:01:00Z \u2014 read it as X; because Y", "- one", "- two", "  - nested", "wrapped onto a second line",
    "- three", "- four; with context", "* starred", "1. numbered", "plain words",
    "> quoted", "  > quoted, indented", ">", "<!-- hidden -->", "  <!-- hidden -->  ", "<!-- opened", "closed -->",
    "<!-- a --> and more", "<!-- a --><!-- b -->", "<!---->", "<!-- \u2028 -->", "<!-- a\r -->",
    "```", "```js", "``` ", " ```", "~~~", "````",
    "constructor", "__proto__", "hasOwnProperty", "isPrototypeOf", "propertyIsEnumerable", "toLocaleString",
    "toString", "valueOf", "__defineGetter__", "__defineSetter__", "__lookupGetter__", "__lookupSetter__",
    " constructor", "constructor ", "toString()", "total", "interpretations",
])
for _ in range(scale * 40):
    body = [rt.choice(MEMORY_LINES) for _ in range(rt.randrange(1, 30))]
    raw = rt.choice(["\n", "\n", "\r\n", "\r"]).join(body) + rt.choice(["", "\n", "\r\n"])
    if rt.random() < 0.1:
        raw = "\ufeff" + raw
    if rt.random() < 0.15:
        raw = char_fuzz(raw, rt.randrange(1, 4))
    add("memory_counts", raw=raw)
add("memory_counts", raw="")
add("memory_counts", raw="\n".join(f"## Deviations\n{name}\n- after {name}" for name in MEMORY_LINES[-17:-5]))

UNIT_NAMES = ["auth", "api", "web", "store", "a", "x1", "9lives", "with.dot", "under_score", "a-b", "Api", "API",
              "-lead", ".lead", "sp ace", "tab\tbed", "été", "\U0001F600", "x" * 64, "x" * 65,
              "\U0001F600" * 33, '"auth"', "'api'", '"', "''", "a#b", "auth # the first", "[auth]", "name", "units"]
UNIT_KIND_VALUES = ["service", "spec", "ui", "packaging", "library", '"ui"', "'spec'", "Service", "big", "", "ui # k"]


def deps_inline(names, careful):
    items = [rt.choice([name, name, f'"{name}"', f"'{name}'", f" {name} "]) for name in names]
    text = "[" + rt.choice([", ", ",", " , "]).join(items) + "]"
    way = rt.randrange(4 if careful else 12)
    if way == 0:
        return text + "  # why"
    if way == 1 and not names:
        return rt.choice(["", "[]", "[ ]"])
    if way == 2 and len(names) == 1:
        return names[0]
    if way == 4:
        return text[:-1]
    if way == 5:
        return text + " trailing"
    if way == 6:
        return text.replace("]", ",]")
    if way == 7:
        return rt.choice(["~", "null", "solo", "''"])
    if way == 8:
        return text.replace("[", '["a\\"]b", ', 1)
    if way == 9:
        return text.replace("[", "['it,s', ", 1)
    if way == 10:
        return text + "]"
    return text


def units_block(careful):
    names = rt.sample(UNIT_NAMES[:10], rt.randrange(1, 7))
    if not careful and rt.random() < 0.35:
        names[rt.randrange(len(names))] = rt.choice(UNIT_NAMES)
    lines = [rt.choice(["units:", "units:", "units :", "  units:", "units: "] if careful or rt.random() < 0.8 else
                       ["units: []", "Units:", "unit:", "units:  x", "units"])]
    for index, name in enumerate(names):
        pad = rt.choice(["  ", "  ", "", "    ", "\t"])
        dash = (["- name:", "- name:", "-  name :", "-\tname:", "- name:   "] if careful or rt.random() < 0.85 else
                ["-name:", "- Name:", "* name:"])
        lines.append(f"{pad}{rt.choice(dash)} {name}")
        earlier = names[:index]
        pool = earlier if careful or rt.random() < 0.6 else names[index:] + ["ghost"]
        deps = rt.sample(pool, min(len(pool), rt.randrange(0, 4))) if pool else []
        way = rt.randrange(8)
        if way <= 3:
            lines.append(f"{pad}  depends_on: {deps_inline(deps, careful)}")
        elif way == 4:
            lines.append(f"{pad}  depends_on:")
            lines += [f"{pad}    - {rt.choice([dep, repr(dep)])}" for dep in deps]
        if rt.random() < 0.3:
            kinds = UNIT_KIND_VALUES[:7] if careful else UNIT_KIND_VALUES
            lines.insert(len(lines) - rt.randrange(0, 2), f"{pad}  kind: {rt.choice(kinds)}")
        if not careful and rt.random() < 0.08:
            lines.append(rt.choice(["    note: free text", "  - stray", "random words", "    depends_on: [a]\n    kind: ui",
                                    "  # a comment", "---"]))
        if rt.random() < 0.15:
            lines.append(rt.choice(["", "   ", "\t"]))
    if not careful and rt.random() < 0.15:
        rt.shuffle(lines)
    return lines


for _ in range(scale * 50):
    careful = rt.random() < 0.6
    fence = rt.choice(["```yaml", "```yaml", "```yml", "```yaml ", "  ```yaml"] if careful or rt.random() < 0.8 else
                      ["```YAML", "```", "~~~yaml", "```yaml units", "````yaml"])
    body = ["# Dependencies", ""]
    if rt.random() < 0.2:
        body += ["```mermaid", "graph TD", "units: no", "```", ""]
    if rt.random() < 0.15:
        body += ["```yaml", "other: 1", rt.choice(["```", "```", "   ```  ", "````"]), ""]
    body += [fence, *units_block(careful)]
    if careful or rt.random() < 0.7:
        body.append(rt.choice(["```", "```", "  ```  ", "``` "] if careful else ["```", "````", "~~~", "```yaml"]))
    body += ["", "trailing prose"]
    text = rt.choice(["\n", "\n", "\n", "\r\n"]).join(body)
    if not careful and rt.random() < 0.25:
        text = char_fuzz(text, rt.randrange(1, 4))
    add("bolt_dag", body=text)
for body in ["", "```yaml\nunits:\n```", "```yaml\nunits\n:\n  - name: a\n```", "```yaml\nunits:\n  - name: a\n    depends_on: [b]\n  - name: b\n    depends_on: [a]\n```",
             "```yaml\nunits:\n  - name: a\n    depends_on: [a]\n```", "```yaml\nunits:\n  - name: c\n  - name: b\n    depends_on: [c]\n  - name: a\n  - name: d\n    depends_on: [a, b]\n```\r",
             "```yaml\nunits:\n  depends_on: [a]\n```", "```yaml\nunits:\n  kind: ui\n```", "```yaml\nunits:\n  - name: a\n  - name: a\n```",
             "```yaml\nunits:\n  - name: Api\n  - name: api\n```", "```yaml\nunits:\n  - name: a\n    - kind: ui\n```",
             "```yaml\nunits:\n  - name: a\n    depends_on: \"\"\n```", "```yaml\nunits:\n  - name: a\n    depends_on: ''\n```\n```yaml\nunits:\n  - name: b\n```",
             # Round in a circle: three units, two with one that is free, and a circle a level down.
             "```yaml\nunits:\n  - name: a\n    depends_on: [c]\n  - name: b\n    depends_on: [a]\n  - name: c\n    depends_on: [b]\n```",
             "```yaml\nunits:\n  - name: z\n  - name: a\n    depends_on: [b]\n  - name: b\n    depends_on: [a, z]\n```",
             "```yaml\nunits:\n  - name: a\n  - name: b\n    depends_on: [a, c]\n  - name: c\n    depends_on: [b]\n```",
             # Each way a name cannot be a unit's.
             "```yaml\nunits:\n  - name: a b\n```", "```yaml\nunits:\n  - name: " + "x" * 65 + "\n```",
             "```yaml\nunits:\n  - name: " + "\U0001F600" * 33 + "\n```", "```yaml\nunits:\n  - name: ''\n```",
             "```yaml\nunits:\n  - name: \"\n```", "```yaml\nunits:\n  - name: -a\n```",
             "```yaml\nunits: [a, b]\n```", "```yaml\nunits:\n  - name: a\n    kind: big\n```",
             # A document that ends in a carriage return, inside the block: the last line keeps it.
             "```yaml\nunits:\n  - name: a\n    depends_on: []\r", "```yaml\nunits:\n  - name: a\n  - name: b\n    depends_on: [a]\r",
             "```yaml\r\nunits:\r\n  - name: a\r\n    depends_on: []\r\n", "```yaml\nunits:\n  - name: a\r",
             # A backslash escapes inside double quotes only.
             "```yaml\nunits:\n  - name: a\n  - name: b\n    depends_on: ['a\\', a]\n```",
             "```yaml\nunits:\n  - name: a\n  - name: b\n    depends_on: [\"a\\\", a]\n```",
             "```yaml\nunits:\n  - name: a\n  - name: b\n    depends_on: [\"x\\\"]\", a]\n```",
             "```yaml\nunits:\n  - name: a\n    note: x\n```", "```yaml\nunits:\n  - name: a\n    depends_on: [ghost]\n```"]:
    add("bolt_dag", body=body)

BOX_LINES = ["- [x] intent-capture \u2014 EXECUTE", "- [ ] requirements-analysis \u2014 EXECUTE", "- [-] code-generation \u2014 EXECUTE",
             "- [?] build-and-test \u2014 EXECUTE", "- [R] user-stories \u2014 EXECUTE", "- [S] nfr-design \u2014 SKIP: not needed",
             "- [S] nfr-design \u2014 SKIP", "- [x] intent-capture \u2014 SKIP", "- [x] a\u2014EXECUTE", "- [x] a  \u2014  EXECUTE  ",
             "- [x] a \u2014 EXECUTED", "- [x] a \u2014 EXECUTE: now", "- [x] a \u2014 execute", "- [x] a \u2014", "- [x] a \u2014 ",
             "- [x] a - EXECUTE", "- [x] a \u2013 EXECUTE", "- [X] a \u2014 EXECUTE", "- [] a \u2014 EXECUTE", "- [xx] a \u2014 EXECUTE",
             " - [x] a \u2014 EXECUTE", "* [x] a \u2014 EXECUTE", "- [x]  a \u2014 EXECUTE", "- [x] a b \u2014 EXECUTE", "- [x] a\t\u2014\tSKIP",
             "- [x] a", "\u2014 EXECUTE", "- [x] \u2014 EXECUTE", "- [x] a\u00a0\u2014\u00a0EXECUTE", "- [x] \u00e9t\u00e9 \u2014 EXECUTE",
             "- **Status**: Completed", "## Stage Progress", "", "text", "- [x] a \u2014 EXECUTE \u2014 SKIP", "- [x] a \u2014 SKIPPED"]
for _ in range(scale * 30):
    lines = [rt.choice(BOX_LINES) for _ in range(rt.randrange(1, 14))]
    content = rt.choice(["\n", "\n", "\r\n", "\r"]).join(lines) + rt.choice(["", "\n"])
    if rt.random() < 0.15:
        content = char_fuzz(content, rt.randrange(1, 4))
    add("progress", content=content)

# --- M3: Guard Policy, fences and the lines they are read from -----------------------------------------
# A case is a state text, the scope files' defaults, the memory declarations and the environment; the
# answer is the policy, every fence, and the decision for each fence.
gp = random.Random(seed * 7919 + 7)
POLICY_WORDS = ["strict", "relaxed", "off"]
LABELS = ["set by you", "you", "set by a command", "from scope classic", "from scope feature", "from project.md",
          "from org.md", "not set", "", "from ", "set by  you", "FROM scope x", "from scope  wide "]
FENCE_NAMES = ["plan-approval", "review-freeze", "state-transition", "reviewer-scope"]


def policy_line(careful):
    word = gp.choice(POLICY_WORDS)
    if not careful and gp.random() < 0.3:
        word = gp.choice(["Strict", "OFF", "Relaxed", "`off`", "**strict**", "offx", "none", "sometimes", "", "strict relaxed",
                          "strict (", "off ()", "off (a (b))", " off ", "o\u0066f"])
    label = gp.choice(LABELS)
    shape = gp.random()
    if shape < 0.5:
        return f"{word} ({label})"
    if shape < 0.7:
        return word
    if shape < 0.8:
        return f"{word}  ({label})  "
    return f"{word} ({label}) extra" if not careful else f"{word} ({label})"


def guards_line(careful):
    picked = gp.sample(FENCE_NAMES + (["human-presence", "nope", "`plan-approval`", "Review-Freeze", "plan-approval"] if not careful else []),
                       gp.randrange(0, 4))
    if not picked:
        return gp.choice(["none", "None", "", "  ", "none (set by you)"])
    return gp.choice([", ", ",", " , "]).join(picked) + gp.choice(["", " (set by you)", " (x)", "  "])


def state_text(careful):
    lines = ["# Focus State", ""]
    if gp.random() < 0.9:
        lines.append(f"- **Scope**: {gp.choice(['feature', 'bugfix', 'classic', 'Feature', ' feature ', 'nope', '', 'enterprise'])}")
    for field in ["Guard Policy", "Change Control"]:
        if gp.random() < (0.6 if field == "Guard Policy" else 0.25):
            lines.append(f"- **{field}**: {policy_line(careful)}")
    for field in ["Guards Off", "Guards On"]:
        if gp.random() < 0.4:
            lines.append(f"- **{field}**: {guards_line(careful)}")
    if gp.random() < 0.3:
        lines.append("- **Test Strategy**: unit")
    if gp.random() < 0.2:
        lines.append("- **Review Override**: no")
    gp.shuffle(lines[2:]) if gp.random() < 0.3 else None
    lines += ["", "## Notes", "- **Guard Policy**: off (in a later section, still the second line)"] if gp.random() < 0.1 else []
    text = gp.choice(["\n", "\n", "\r\n"]).join(lines) + gp.choice(["", "\n"])
    if not careful and gp.random() < 0.1:
        text = char_fuzz(text, gp.randrange(1, 3))
    return text


for _ in range(scale * 60):
    careful = gp.random() < 0.5
    scopes = {}
    for name in gp.sample(["feature", "bugfix", "classic", "enterprise", "wide", "Feature"], gp.randrange(0, 5)):
        scopes[name] = gp.choice([None, None, "off", "strict", "relaxed"])
    memory = []
    for layer in ["org", "team", "project"]:
        if gp.random() < 0.3:
            memory.append({"layer": layer, "heading": gp.choice(["## Guard Policy", "## Guard Policy", "## Change Control"]),
                           "value": gp.choice(POLICY_WORDS + ([] if careful else ["Strict", "OFF", "maybe", "`relaxed`"]))})
    env = [name for name in ["AIDLC_DISABLE_PLAN_APPROVAL_GUARD", "AIDLC_DISABLE_REVIEW_FREEZE_HOOK", "AIDLC_DISABLE_REVIEWER_SCOPE_HOOK",
                             "AIDLC_SKIP_HUMAN_PRESENCE_GUARD", "AIDLC_UNATTENDED"] if gp.random() < 0.15]
    add("guard_policy", state=state_text(careful), scopes=scopes, memory=memory, env=env, tolerate=gp.random() < 0.5)

for _ in range(scale * 25):
    careful = gp.random() < 0.5
    fences = gp.sample(FENCE_NAMES + ["human-presence", "nope", "Plan-Approval"], gp.randrange(0, 5))
    line = gp.choice([policy_line(careful), guards_line(careful), policy_line(careful)])
    add("guards_line", line=line, fences=fences, content=state_text(careful))

# --------------------------------------------------------------------------
# The shell reader (M4): what a command may write, read as POSIX shell or as
# PowerShell, and Node's path functions it resolves with.
sh = random.Random(seed * 7919 + 8)

SH_PATHS = ["a", "b.txt", "dir/sub", "../x", "./y", "/abs/p", "/work/q r", "a b", "é.md", "x,y", "(z)", "[w]", "{v}",
            "C:\\win\\p", "~", "~/h", "$HOME/h", "${HOME}/h", "$PWD/p", "${PWD}", "$PWD", "$VAR/x", "*.log", "f?", "`cmd`",
            "-", "-x", "src/", ".", "..", "dest", "dir", "build", "a:b", ",c,", "of=d", "x\\ y", "../../../../up", "/", "//n/s/t"]
SH_DIRS = ["dest", "dir", "build", "a"]


def sh_word(word=None):
    """A path written with one of the shell's quotings."""
    word = SH_PATHS[sh.randrange(len(SH_PATHS))] if word is None else word
    style = sh.random()
    if style < 0.5:
        return word.replace(" ", "\\ ")
    if style < 0.7:
        return "'" + word.replace("'", "'\\''") + "'"
    if style < 0.9:
        return '"' + word.replace('"', '\\"') + '"'
    if len(word) > 2:
        cut = sh.randrange(1, len(word))
        return word[:cut].replace(" ", "\\ ") + "'" + word[cut:] + "'"
    return word


def sh_mutator():
    w = sh_word
    pick = sh.choice
    return pick([
        lambda: f"rm {pick(['-rf', '-r', '-f', '--', '-r --', '-i'])} {w()}",
        lambda: f"rm {w()} {w()}",
        lambda: f"cp {w()} {w()}",
        lambda: f"cp {pick(['-r', '-a', '-t', '--target-directory=' + w(), '-S .bak', '--suffix=.o'])} {w()} {w()}",
        lambda: f"cp {w()} {w()} {w()}",
        lambda: f"mv {w()} {w()}",
        lambda: f"mv {pick(['-f', '-t', '-n', '--target-directory=' + w()])} {w()} {w()}",
        lambda: f"install {pick(['-d', '-m 644', '-o root', '-t ' + w(), '--directory', '-m755', '-g', '--mode=600'])} {w()} {w()}",
        lambda: f"touch {pick(['', '-a', '-d \"2020-01-01\"', '-r ' + w(), '-t 202001010000', '--date=now', '-d'])} {w()}",
        lambda: f"tee {pick(['', '-a', '--append'])} {w()}",
        lambda: f"truncate {pick(['-s 0', '-s0', '--size=0', '-s', '-r ' + w()])} {w()}",
        lambda: f"unlink {w()}",
        lambda: f"sed {pick(['-i', '-i.bak', '--in-place', '-n', '-e x -i', '-i -e', '-ie', '-l 5 -i', '--expression=s/a/b/ -i', '-f'])} {pick(['s/a/b/', chr(39) + 's/x/y/' + chr(39), ''])} {w()} {pick(['', w()])}",
        lambda: f"perl {pick(['-pi', '-i.bak -pe', '-e', '-i', '-p -i -e', '-Mstrict -i', '-I lib -pi', '-F, -i'])} {pick([chr(39) + 's/a/b/' + chr(39), 'x', ''])} {w()}",
        lambda: f"find {pick(['', '-H', '-L -D opt', '-D', '-O2', '-P'])} {w()} {pick(['-name x -delete', '-delete', '-fprint ' + w(), '-fprint0 ' + w(), '-fls ' + w(), '-fprintf ' + w() + ' %p', '-print', '! -name a -delete', '( -name a ) -delete'])}",
        lambda: f"find -delete {w()}",
        lambda: f"dd if={w()} of={w()} bs=1",
        lambda: f"dd of={w()} if={w()}",
        lambda: f"rsync {pick(['-a', '-av', '--remove-source-files', '--remove-source-files -a'])} {w()} {w()}",
        lambda: f"{pick(['shred', 'rmdir', 'del', 'erase', 'rd', 'move', 'rename', 'ren'])} {pick(['', '-p', '/q', '-f'])} {w()} {pick(['', w()])}",
        lambda: f"{pick(['remove-item', 'ri', 'cli', 'move-item', 'mi', 'rename-item', 'rni', 'clear-item'])} {pick(['-Path:' + w(), '-LiteralPath=' + w(), '--path:' + w(), '-Recurse', '-Destination:' + w(), '-NewName=' + w()])} {pick(['', w()])}",
        lambda: f"copy-item {w()} {pick(['-Destination:' + w(), '-Destination=' + w(), w(), ''])}",
        lambda: f"{pick(['set-content', 'add-content', 'out-file', 'tee-object', 'new-item', 'set-item', 'clear-content'])} {pick(['-Path', '-LiteralPath', '-FilePath', '-Value', '-Name', '-Variable', '-Path:', '-pa', '-Encoding', '--', '-Weird', '-Force', '-Confirm:'])} {w()} {pick(['', w(), '-Value x', '-Force', '-Variable v', '-Name ' + w()])}",
    ])()


def sh_plain():
    w = sh_word
    pick = sh.choice
    return pick([
        lambda: "ls -la",
        lambda: f"cat {w()}",
        lambda: f"grep -r x {w()}",
        lambda: "git status",
        lambda: f"echo {w()}",
        lambda: "make test",
        lambda: f"python3 {w()}",
        lambda: "true",
        lambda: f"{pick(['RM', 'Rm', '/bin/rm', './rm', 'rm.exe', 'C:\\\\tools\\\\rm.EXE', 'bin\\\\cp.cmd'])} {w()}",
        lambda: f"{pick(['}', 'fi', 'done', 'esac'])}",
        lambda: "",
    ])()


def sh_redirected(inner):
    w = sh_word
    pick = sh.choice
    return pick([
        lambda: f"{inner} > {w()}",
        lambda: f"{inner} >> {w()}",
        lambda: f"{inner} >{w()}",
        lambda: f"{inner} 2> {w()}",
        lambda: f"{inner} &> {w()}",
        lambda: f"{inner} >| {w()}",
        lambda: f"{inner} >&2",
        lambda: f"{inner} 2>&1",
        lambda: f"{inner} 1>&-",
        lambda: f"{inner} >& {w()}",
        lambda: f"{inner} >&{w()}",
        lambda: f"{inner} < {w()}",
        lambda: f"{inner} > /dev/null 2>&1",
        lambda: f"{inner} 3>&1 1>&2 2>&3",
        lambda: f"{inner} 2>{w()} >{w()}",
        lambda: f"{inner} > {w()} 2>&1",
        lambda: f"{inner} >",
        lambda: f"{inner} > '{w()}",
        lambda: f"{inner}>{w()}",
        lambda: f"{inner} 2 > {w()}",
        lambda: f"{inner} >> ~/{w()}",
    ])()


def sh_wrapped(inner):
    w = sh_word
    pick = sh.choice
    return pick([
        lambda: f"sudo {pick(['', '-u root', '-E', '--preserve-env=HOME', '-n', '--weird', '-u', '-Hn', '-S -k', '--', '-g adm -u x', '-D ' + w()])} {inner}",
        lambda: f"env {pick(['', '-i', 'FOO=1', 'PATH=/x', '-u PATH', '--unset=PATHEXT', '--chdir=' + w(), '-C ' + w(), '-0', '-iv', '--', '-uPATH', '-S', '--split-string=' + inner, '-x', '-a name', '--default-signal'])} {inner}",
        lambda: f"env -S {sh_word(inner)} {w()}",
        lambda: f"env -S'{inner}'",
        lambda: f"{pick(['PATH=/x', 'FOO=bar', 'PATHEXT=.exe', 'A=1 B=2', 'path=x'])} {inner}",
        lambda: f"command {pick(['', '-p', '-v', '-V', '--', '-pv', '-x', '-p --'])} {inner}",
        lambda: f"builtin {pick(['', '--', '-x'])} {inner}",
        lambda: f"exec {pick(['', '-a name', '-c', '-cl', '-x', '-a'])} {inner}",
        lambda: f"nohup {pick(['', '--help', '-x'])} {inner}",
        lambda: f"nice {pick(['', '-n 5', '-5', '--adjustment=3', '-n', '--adjustment 2', '-x'])} {inner}",
        lambda: f"ionice {pick(['-c 3', '-c3 -n7', '-t', '--class=idle', '-p 1'])} {inner}",
        lambda: f"stdbuf {pick(['-o0', '-oL -eL', '--output=0', '-i'])} {inner}",
        lambda: f"setsid {pick(['', '-f', '-w', '--fork', '-x'])} {inner}",
        lambda: f"doas {pick(['', '-u x', '-n', '-C ' + w(), '-x'])} {inner}",
        lambda: f"xargs {pick(['', '-0', '-I{}', '-n1', '--max-args=2', '-P 4', '-r', '-0rn 1', '--null', '-x', '-I', '-d \\n', '-e', '-l', '--replace=X'])} {inner}",
        lambda: f"time {pick(['', '-p', '-f x', '-o ' + w(), '--format=x', '-v', '-x'])} {inner}",
        lambda: f"timeout {pick(['5', '-k 3 5', '--signal=TERM 10s', '-s KILL 1', '--foreground 2', '', '-x 5', '-k'])} {inner}",
        lambda: f"unbuffer {pick(['', '-p', '-x'])} {inner}",
        lambda: f"{pick(['busybox', 'toybox'])} {pick(['', '-x', ''])} {inner}",
        lambda: f"{pick(['exec', 'sudo', 'nice'])}",
        lambda: f"sudo env -i nice -n 3 {inner}",
    ])()


def sh_control(inner, other):
    pick = sh.choice
    return pick([
        lambda: f"if {inner}; then {other}; fi",
        lambda: f"if true; then {inner}; else {other}; fi",
        lambda: f"while {inner}; do {other}; done",
        lambda: f"until false; do {inner}; done",
        lambda: f"for x in a b; do {inner}; done",
        lambda: f"{{ {inner}; }}",
        lambda: f"( {inner} )",
        lambda: f"! {inner}",
        lambda: f"case x in a) {inner};; esac",
        lambda: f"then {inner}",
        lambda: f"elif {inner}; then {other}",
        lambda: f"select x in a; do {inner}; done",
        lambda: f"{{ FOO=1 {inner}; }}",
    ])()


def sh_cd():
    w = sh_word
    pick = sh.choice
    return pick([
        lambda: f"cd {w()}",
        lambda: "cd",
        lambda: "cd -",
        lambda: f"cd -- {w()}",
        lambda: f"cd 2>/dev/null {w()}",
        lambda: f"cd {w()} 2>&1",
        lambda: f"pushd {w()}",
        lambda: f"pushd -n {w()}",
        lambda: "pushd +1",
        lambda: "pushd",
        lambda: "popd",
        lambda: f"chdir {w()}",
        lambda: "cd ~",
        lambda: "cd $HOME",
        lambda: "cd \"$X\"",
        lambda: f"cd -L {w()}",
        lambda: "cd +1",
        lambda: "cd -- -",
        lambda: f"CD {w()}",
        lambda: f"set-location {w()}",
        lambda: f"sl {w()}",
        lambda: f"cd {w()} >{w()}",
        lambda: "cd -P",
        lambda: f"pushd -- {w()}",
        lambda: "pushd -1",
        lambda: "cd ~/",
        lambda: "cd '~'",
    ])()


def sh_segment(depth=0):
    roll = sh.random()
    if roll < 0.35:
        inner = sh_mutator()
    elif roll < 0.45:
        inner = sh_plain()
    elif roll < 0.6:
        inner = sh_cd()
    elif roll < 0.8 and depth < 2:
        inner = sh_wrapped(sh_segment(depth + 1))
    elif roll < 0.9 and depth < 2:
        inner = sh_control(sh_segment(depth + 1), sh_segment(depth + 1))
    else:
        inner = sh_redirected(sh_mutator() if sh.random() < 0.5 else sh_plain())
    if sh.random() < 0.3:
        inner = sh_redirected(inner)
    return inner


def sh_command():
    parts = [sh_segment() for _ in range(sh.choice([1, 1, 2, 2, 3, 4]))]
    out = parts[0]
    for part in parts[1:]:
        out += sh.choice([" ; ", "; ", " && ", " || ", " | ", " & ", "\n", ";", "&&", "|"]) + part
    if sh.random() < 0.08:
        out = char_fuzz(out, sh.randrange(1, 4))
    return out


PS_PATHS = ["a", "b.txt", "dir\\sub", "..\\x", ".\\y", "C:\\abs\\p", "D:q", "\\\\srv\\share\\f", "a b", "é.md", "x,y",
            "~", "~\\h", "$HOME\\h", "${HOME}/h", "$pwd\\p", "${PWD}", "$PWD/p", "$env:TEMP\\x", "$x", "$x.y", "${x}", "$(pwd)",
            "@(a)", "(Get-Location)", "[string]'t'", "it''s", "-", "+", "--", "-x", ".", "..", "/", "dest", "dir", "build",
            "-Path", "-Value", "a`.txt", "$null", "nul", "/dev/null", "{v}"]


def ps_word(word=None):
    word = PS_PATHS[sh.randrange(len(PS_PATHS))] if word is None else word
    style = sh.random()
    if style < 0.45:
        return word
    if style < 0.65:
        return "'" + word.replace("'", "''") + "'"
    if style < 0.8:
        return '"' + word.replace('"', '""') + '"'
    if style < 0.87:
        return sh.choice(["‘", "‚"]) + word + sh.choice(["’", "‛", "‘"])
    if style < 0.94:
        return sh.choice(["“", "„"]) + word + sh.choice(["”", "“"])
    return "(" + sh.choice(["'", '"']) + word + sh.choice(["'", '"']) + ")"


def ps_param(name, value=None):
    """A parameter written one of PowerShell's ways."""
    dash = sh.choice(["-", "-", "-", "–", "—", "―"])
    style = sh.random()
    if value is None:
        return f"{dash}{name}"
    if style < 0.5:
        return f"{dash}{name} {value}"
    if style < 0.7:
        return f"{dash}{name}:{value}"
    if style < 0.8:
        return f"{dash}{name}: {value}"
    if style < 0.9:
        return f"{dash}{name[:sh.randrange(2, len(name) + 1)]} {value}"
    return f"{dash}{name}={value}"


def ps_cmdlet():
    w = ps_word
    pick = sh.choice
    name = pick(["Set-Content", "sc", "Add-Content", "ac", "Clear-Content", "clc", "Out-File", "Tee-Object", "tee",
                 "New-Item", "ni", "Set-Item", "si", "set-content", "SET-CONTENT"])
    args = []
    for _ in range(sh.randrange(0, 4)):
        args.append(pick([
            lambda: ps_param("Path", w()), lambda: ps_param("LiteralPath", w()), lambda: ps_param("FilePath", w()),
            lambda: ps_param("Value", w()), lambda: ps_param("Encoding", "utf8"), lambda: ps_param("Force"),
            lambda: ps_param("Confirm", "$false"), lambda: ps_param("WhatIf"), lambda: ps_param("Variable", w()),
            lambda: ps_param("Name", w()), lambda: ps_param("ItemType", "File"), lambda: ps_param("Weird", w()),
            lambda: ps_param("ea", "0"), lambda: ps_param("Path", w() + "," + w()), lambda: w(),
            lambda: w() + ", " + w(), lambda: w() + " ,", lambda: ", " + w(), lambda: "--", lambda: ps_param("pa", w()),
            lambda: ps_param("Path", "(" + ps_cmdlet() + ")"), lambda: "$(" + ps_cmdlet() + ")", lambda: ps_param("Value", "@(" + w() + ")"),
            lambda: ps_param("Path") + ":", lambda: ps_param("Force") + ": " + w(), lambda: "-`Path", lambda: ps_param("lp", w()),
            lambda: ps_param("PSPath", w()), lambda: ps_param("NoClobber"), lambda: ps_param("Append"), lambda: ps_param("Stream", "x"),
        ])())
    return name + "".join(" " + arg for arg in args)


def ps_native():
    w = ps_word
    pick = sh.choice
    return pick([
        lambda: f"rm {pick(['', '-r -fo', '-Recurse', '-Path', '-LiteralPath', '-rf'])} {w()}",
        lambda: f"{pick(['del', 'ri', 'erase', 'rd', 'rmdir', 'Remove-Item', 'remove-item', 'cli', 'Clear-Item'])} {pick(['', '-Recurse', '-Path:', '-LiteralPath=', '-Force'])} {w()}",
        lambda: f"{pick(['Copy-Item', 'cp', 'copy', 'cpi'])} {w()} {pick([ps_param('Destination', w()), w(), '', '-t ' + w(), '-r'])}",
        lambda: f"{pick(['Move-Item', 'mv', 'move', 'mi'])} {pick([ps_param('Path', w()), w()])} {pick([ps_param('Destination', w()), w(), ''])}",
        lambda: f"{pick(['Rename-Item', 'ren', 'rni'])} {w()} {pick([ps_param('NewName', w()), w()])}",
        lambda: f"{pick(['Get-ChildItem', 'gci', 'dir', 'ls', 'Get-Item'])} {w()} | {pick(['Remove-Item', 'ri', 'rm -Force', 'Copy-Item -Destination ' + w(), 'Move-Item ' + w(), 'Set-Content -Value x', 'Set-Content ' + w(), 'Out-File ' + w(), 'rename-item -NewName ' + w(), 'Copy-Item', 'ri -Path ' + w(), 'del -LiteralPath:' + w(), 'Remove-Item -Confirm'])}",
        lambda: f"{pick(['echo', 'Write-Output', 'cat'])} {w()} | {pick(['Out-File ' + w(), 'tee ' + w(), 'tee -Variable v', 'Tee-Object -FilePath ' + w(), 'Set-Content ' + w(), 'Add-Content -Path ' + w()])}",
        lambda: f"{pick(['sed', 'touch', 'tee', 'dd', 'find', 'install', 'truncate'])} {pick(['-i', '-a', 'of=' + w(), '-delete', '-s 0'])} {w()}",
        lambda: f"{pick(['Get-Content', 'cat', 'type', 'git status', 'echo'])} {w()}",
    ])()


def ps_redirected(inner):
    w = ps_word
    pick = sh.choice
    return pick([
        lambda: f"{inner} > {w()}",
        lambda: f"{inner} >> {w()}",
        lambda: f"{inner} >{w()}",
        lambda: f"{inner} 2> {w()}",
        lambda: f"{inner} 2>&1",
        lambda: f"{inner} *> $null",
        lambda: f"{inner} 6> {w()}",
        lambda: f"{inner} > {w()} 2>&1",
        lambda: f"{inner} 3>&2",
        lambda: f"{inner} > \"$dir\\{w()}\"",
        lambda: f"{inner} >",
        lambda: f"{inner} 2>> {w()}",
        lambda: f"{inner} < {w()}",
        lambda: f"{inner} *>&1",
    ])()


def ps_statement(inner, other):
    w = ps_word
    pick = sh.choice
    return pick([
        lambda: f"$x = {inner}",
        lambda: f"$x += {inner}",
        lambda: f"$x.y = {inner}",
        lambda: f"[string]$x={inner}",
        lambda: f"$a[0] = {inner}",
        lambda: f"${{x}} = {inner}",
        lambda: f"$x ??= {inner}",
        lambda: f"$x=$y={inner}",
        lambda: f"& '{pick(['rm', 'Set-Content', 'cmd'])}' {w()}",
        lambda: f"& $cmd {w()}",
        lambda: f". .\\script.ps1 {w()}",
        lambda: f"{inner}; {other}",
        lambda: f"{inner} && {other}",
        lambda: f"{inner} || {other}",
        lambda: f"{inner} &",
        lambda: f"{inner}\n{other}",
        lambda: f"{inner} `\n  {other}",
        lambda: f"{inner} # {other}",
        lambda: f"<# {other} #> {inner}",
        lambda: f"{inner} --% {other} -Path raw",
        lambda: f"if ($x) {{ {inner} }}",
        lambda: f"foreach ($f in gci) {{ {inner} }}",
        lambda: f"1..3 | % {{ {inner} }}",
        lambda: f"{inner} |\n {other}",
        lambda: f"{inner} ; ; {other}",
        lambda: f"{inner} 2 = {other}",
        lambda: f"3.5 {inner}",
        lambda: f"{inner}\r\n{other}",
        lambda: f"{inner} `\r\n{other}",
        lambda: f"@{{a=1}} {inner}",
        lambda: f"{{ {inner} }}.Invoke()",
        lambda: f"({inner})",
        lambda: f"\"$({inner})\"",
        lambda: f"'{inner}'",
    ])()


def ps_cd():
    w = ps_word
    pick = sh.choice
    return pick([
        lambda: f"cd {w()}",
        lambda: f"Set-Location {ps_param('Path', w())}",
        lambda: f"sl {ps_param('LiteralPath', w())}",
        lambda: f"pushd {w()}",
        lambda: f"Push-Location {w()}",
        lambda: "cd ~",
        lambda: "cd $HOME\\x",
        lambda: "cd ${HOME}/x",
        lambda: "cd -",
        lambda: "cd +",
        lambda: "cd",
        lambda: "Set-Location",
        lambda: "cd \\",
        lambda: "cd C:\\x",
        lambda: "cd ..\\y",
        lambda: f"chdir {w()}",
        lambda: f"cd {w()}, {w()}",
        lambda: f"Set-Location -StackName s {w()}",
        lambda: f"cd -PassThru {w()}",
        lambda: "cd 'a b'",
        lambda: "cd \"$x\\y\"",
    ])()


def ps_segment(depth=0):
    roll = sh.random()
    if roll < 0.35:
        inner = ps_cmdlet()
    elif roll < 0.55:
        inner = ps_native()
    elif roll < 0.65:
        inner = ps_cd()
    elif roll < 0.85 and depth < 2:
        inner = ps_statement(ps_segment(depth + 1), ps_segment(depth + 1))
    else:
        inner = ps_redirected(ps_cmdlet() if sh.random() < 0.5 else ps_native())
    if sh.random() < 0.25:
        inner = ps_redirected(inner)
    return inner


def ps_command():
    parts = [ps_segment() for _ in range(sh.choice([1, 1, 2, 2, 3]))]
    out = parts[0]
    for part in parts[1:]:
        out += sh.choice(["; ", " ; ", " && ", " || ", " | ", " & ", "\n", ";", "\r\n"]) + part
    if sh.random() < 0.08:
        out = char_fuzz(out, sh.randrange(1, 4))
    return out


# Lines the random mix reaches rarely: a data-driven in-place edit, a
# fileless cmdlet parameter attached in a POSIX reading, a quoted backtick
# inside a group, a pipeline continued on the next line, an escaped quote in
# a PowerShell string, the stop-parsing token, a pushd stack operand.
SH_FIXED = [
    ("posix", "xargs sed --in-place 's/a/b/'"), ("posix", "xargs -0 perl --in-place -pe x"), ("posix", "find . | xargs sed -i"),
    ("posix", "tee-object -Variable:v"), ("posix", "tee-object -Variable:v f"), ("posix", "tee-object -Var v -- -Path"),
    ("posix", "cat a | xargs -I{} find {} -fprint out"), ("posix", "pushd +2 && echo x > y"), ("posix", "cd +1; touch z"),
    ("posix", "sed --in-place=.bak s/a/b/ f g"), ("posix", "install -d a b c"), ("posix", "cp -t d"), ("posix", "mv -- -t a b"),
    ("powershell", "Set-Content (\"a`\"b\") x"), ("powershell", "ni (\"q`\"\") -Name f"), ("powershell", "gci x |\n Remove-Item"),
    ("powershell", "gci x |\n\n ri -Force; ls |\r\n del"), ("powershell", "echo a | \n Out-File"), ("powershell", "sc \"it`\"s\" v"),
    ("powershell", "rm --% -Path 'a b' c"), ("powershell", "Set-Content --% x y"), ("powershell", "$x = $y = $z = sc a b"),
    ("powershell", "[int]$n = 3; [string]$s = Out-File f"), ("powershell", "cp a -Destination b; cp a b; cp -r a b"),
    ("powershell", "Tee-Object -Variable v; Tee-Object -Variable:v -FilePath f; tee -Var v -- x"),
    ("powershell", "Get-Item a | Set-Content -Value x; Get-Item a | Set-Content -Path p -Value x; dir | ni -Name n"),
    ("powershell", "ri -Path:a,b -Force:$false c"), ("powershell", "Remove-Item -Confirm: $false d"), ("powershell", "Remove-Item -Confirm:$false -- -Path"),
    ("powershell", "Set-Content -Path a -Path b"), ("powershell", "Out-File -FilePath a -LiteralPath b"), ("powershell", "sc -Weird x y z"),
    ("powershell", "& { Set-Content a b }; $(ni c)"), ("powershell", "cd (Join-Path a b); Set-Location \"$env:TEMP\\x\""),
    ("powershell", "pushd -; cd +; Set-Location -Path -"), ("powershell", "\"$(rm a)\" > b"), ("powershell", "Set-Content 'a' > b 2>&1 *>&1"),
    ("powershell", "sc $(echo `)) x; ni (a`)) -Name n"), ("powershell", "Remove-Item { a`} } b; rm @(x`)y) z"),
]
for shell_, command in SH_FIXED:
    if shell_ == "posix":
        add("shell_invocations", command=command)
    add("shell_targets", command=command, cwd="{P}/w", home="/home/me", shell=shell_, dirs=["d", "dest"])

for _ in range(scale * 100):
    add("shell_invocations", command=sh_command())

for _ in range(scale * 130):
    powershell = sh.random() < 0.4
    add("shell_targets", command=ps_command() if powershell else sh_command(), cwd="{P}/w", home="/home/me",
        shell="powershell" if powershell else "posix", dirs=sh.sample(SH_DIRS, sh.randrange(0, 3)))

for _ in range(scale * 15):
    tool = sh.choice(["Bash", "Write", "Edit", "MultiEdit", "NotebookEdit", "Read", "Glob", "bash"])
    if tool == "Bash":
        powershell = sh.random() < 0.3
        value = sh.choice([{"command": ps_command() if powershell else sh_command()}, {"command": 1}, {}, None, {"cmd": "rm x"}])
    else:
        powershell = False
        value = {}
        for key in sh.sample(["file_path", "notebook_path", "path", "paths", "content", "old_string"], sh.randrange(0, 4)):
            value[key] = sh.choice(["a", "/abs/b", "", 3, None, ["x", "", 2, "y"], "é", "C:\\w"]) if key != "paths" else sh.choice([["x", "", 2, "y"], "nope", [], None])
        if sh.random() < 0.1:
            value = None
    add("write_targets", tool=tool, input=value, cwd="{P}/w", home="/home/me", shell="powershell" if powershell else "posix", dirs=[])

NP_PARTS = ["", ".", "..", "a", "a/b", "/a", "//a", "///a//b/", "a/../..", "../../../x", "./", "a/.", "C:", "C:\\", "C:x", "c:\\A\\..\\b",
            "\\\\srv\\share", "\\\\srv\\share\\f\\..\\g", "\\\\?\\C:\\x", "\\\\.\\PHYSICALDRIVE0", "//srv/share/f", "\\", "/", "D:..\\y",
            "a\\b", "x/..\\y", "C:/x", "CON", "nul:", "a:b", "..\\..", "é/ü", "a b/c d", "\\\\\\a", "//", "a//", "C:a/b", "\\\\srv", "\\\\srv\\"]

# Joins whose first part is one separator, which Node keeps from reading as
# a UNC root, and drive-relative resolves against a drive the cwd is not on.
NP_FIXED = [
    ("win32", "join", ["/", "a", "b"]), ("win32", "join", ["\\", "x", "y"]), ("win32", "join", ["//", "a", "b"]), ("win32", "join", ["/", "/a", "b"]),
    ("win32", "join", ["\\\\srv", "share", "f"]), ("win32", "join", ["//srv", "share"]), ("win32", "join", ["C:", "a"]), ("win32", "join", ["C:/", "..", ".."]),
    ("win32", "resolve", ["C:\\w", "D:x"]), ("win32", "resolve", ["D:\\a", "d:b"]), ("win32", "resolve", ["\\\\s\\sh\\x", "\\\\S\\SH\\y"]),
    ("win32", "resolve", ["x", "C:", "y"]), ("win32", "resolve", ["\\x", "C:y"]), ("win32", "normalize", ["a\\..\\..\\..\\b"]), ("posix", "normalize", ["a/../../../b"]),
    ("win32", "basename", ["C:\\"]), ("win32", "basename", ["C:a"]), ("win32", "basename", ["\\\\srv\\share\\"]), ("posix", "basename", ["/a/b.txt//"]),
    ("win32", "normalize", ["\\\\.\\X\\.."]), ("win32", "normalize", ["\\\\?\\UNC\\s\\sh\\f"]), ("win32", "resolve", ["\\\\.\\PHYSICALDRIVE0"]),
    ("win32", "relative", ["C:\\foo\\bar", "C:\\foo\\bar\\baz"]), ("win32", "relative", ["C:\\foo\\bar", "C:\\foo"]), ("win32", "relative", ["C:\\", "C:\\foo"]),
    ("win32", "relative", ["C:\\foo\\bar", "C:\\"]), ("win32", "relative", ["C:\\a\\b", "D:\\a"]), ("win32", "relative", ["c:\\A\\b", "C:\\a\\C"]),
    ("win32", "relative", ["\\\\s\\sh\\a", "\\\\s\\sh\\b"]), ("win32", "relative", ["C:\\İ\\x", "C:\\i̇\\y"]), ("win32", "relative", ["C:\\a", "C:\\ab"]),
    ("posix", "relative", ["/a/b/c", "/a/d"]), ("posix", "relative", ["/", "/foo"]), ("posix", "relative", ["/foo/bar", "/"]), ("posix", "relative", ["/a", "/ab"]),
    ("posix", "dirname", ["//a"]), ("win32", "dirname", ["\\\\s\\sh\\f"]), ("win32", "dirname", ["\\\\s\\sh"]), ("win32", "dirname", ["C:a\\b"]), ("win32", "dirname", ["C:"]),
]
for flavour, op, args in NP_FIXED:
    add("node_path", flavour=flavour, op=op, args=args, cwd="/work/w")

for _ in range(scale * 70):
    flavour = sh.choice(["posix", "win32"])
    op = sh.choice(["resolve", "normalize", "isAbsolute", "join", "basename", "dirname", "relative"])
    count = sh.randrange(0, 4) if op in ("resolve", "join") else 2 if op == "relative" else 1
    args = [sh.choice(NP_PARTS) if sh.random() < 0.9 else char_fuzz(sh.choice(NP_PARTS), 2) for _ in range(count)]
    add("node_path", flavour=flavour, op=op, args=args, cwd=sh.choice(["/work/w", "/", "/a/b/c", "/tmp"]))

# --------------------------------------------------------------------------
# Runtime integrity (M4, slice 2): what a tool call may not touch.
# --------------------------------------------------------------------------
# engine_command: the engine's own invocations as the recompile hook and the relay read them off a shell command.

ec = random.Random(seed * 7919 + 10)
EC_HARNESS = [".claude", ".kiro", ".codex", ".aidlc", ".cursor", ".gemini", ".claude2"]
EC_TOOLS = ["aidlc-state", "aidlc-jump", "aidlc-bolt", "aidlc-unit", "aidlc-utility", "aidlc-orchestrate", "aidlc-runtime",
            "aidlc-log", "aidlc-worktree", "aidlc", "aidlc-swarm", "aidlcx", "x"]
EC_NOUNS = ["state", "state", "jump", "bolt", "unit", "recompose", "status", "doctor", "version", "help", "scope", "config", "report", "report",
            "orchestrate", "orchestrate", "next", "next", "next", "runtime", "swarm", "intent", "space", "workspace", "sensor", "gen",
            "continue", "park", "utility"]
EC_VERBS = ["approve", "advance", "report", "compile", "change", "set", "next", "resolve", "execute", "create", "help", "--resume",
            "--status", "--json", "list", ""]
EC_GLOBALS = ["--json", "--quiet", "--no-color", "--yes", "--offline", "--verbose", "--project-dir", "--project-dir=/p",
              "--aidlc-attempt-id", "--aidlc-attempt-id=a1", "--", "-x", "--phase", "ideation"]
EC_PATH_PREFIX = ["", "./", "/p/", "\\srv\\share\\", "C:/p/", "\"/p q/", "'/p q/", "$HOME/", "~/", "../", "a/b/"]
EC_WRAP = ["", "env ", "env -- ", "env -i ", "env X=1 Y=2 ", "command ", "command -- ", "command -v ", "exec ", "exec -a x ", "X=1 ", "X=1 Y=2 ",
           "sudo ", "sh -c ", "node ", "bash "]
EC_PRELUDE = ["", "cd /p && ", "cd -- /p && ", "cd /p q && ", "cd 'C:\\p' && ", "cd \\\\srv\\share && ", "cd rel && ", "cd ~ && ", "pushd /p && ",
              "cd /p; ", "cd /p || "]
EC_TAIL = ["", " 2>&1", " 2>&1 ", " | cat", " && echo done", " > out", " ;", " &", " #c", " $(x)", " `x`", " \\\n x"]


def ec_dispatcher():
    harness = ec.choice(EC_HARNESS)
    prefix = ec.choice(EC_PATH_PREFIX)
    quoted = ec.choice(["", "\"", "'"])
    sep = ec.choice(["/", "/", "\\"])
    name = ec.choice(["aidlc.ts", "aidlc.ts", "aidlc", "aidlc.js", "aidlc-state.ts"])
    path = f"{prefix}{harness}{sep}tools{sep}{name}"
    if quoted:
        path = f"{quoted}{path}{quoted}"
    bun = ec.choice(["bun", "bun", "/usr/bin/bun", "bunx", "BUN", "bun\t"])
    return f"{bun} {path}"


def ec_tool():
    harness = ec.choice(EC_HARNESS)
    tool = ec.choice(EC_TOOLS)
    sep = ec.choice(["/", "/", "\\"])
    return f"{ec.choice(['bun ', 'bun ', 'node ', '', '/opt/bun '])}{ec.choice(EC_PATH_PREFIX)}{harness}{sep}tools{sep}{tool}.ts"


def ec_native():
    return ec.choice(["aidlc"] * 7 + ["/usr/local/bin/aidlc", "./aidlc", "aidlc.exe", "aidlcx", "AIDLC"])


def ec_args():
    words = []
    if ec.random() < 0.5:
        words.append("engine")
    words.append(ec.choice(EC_NOUNS))
    for _ in range(ec.randrange(0, 4)):
        words.append(ec.choice(EC_VERBS + EC_VERBS + EC_GLOBALS + ["a", "intent-capture", "\"two words\"", "'q'", "x=y"]))
    return " ".join(w for w in words if w)


def ec_pick(options, plain):
    """One of `options`, the empty one with probability `plain`."""
    return "" if ec.random() < plain else ec.choice(options)


def ec_command():
    head = ec.choice([ec_dispatcher(), ec_dispatcher(), ec_tool(), ec_tool(), ec_native(), ec_native(), ec_native(), "echo", "ls -la", "git status"])
    text = f"{ec_pick(EC_PRELUDE, 0.6)}{ec_pick(EC_WRAP, 0.6)}{head} {ec_args()}{ec_pick(EC_TAIL, 0.6)}"
    if ec.random() < 0.15:
        text = text.replace(" ", ec.choice(["  ", "\t", " "]), 1)
    if ec.random() < 0.1:
        text = ec.choice(["", " ", "\n", "x\0y", "aidlc 'unterminated", 'aidlc "a$b"', "aidlc a\\ b", "aidlc (x)", "aidlc *"])
    return text


EC_FIXED = [
    "bun .claude/tools/aidlc.ts engine state approve x", "bun '/p q/.claude/tools/aidlc.ts' engine orchestrate help",
    'bun "/p/.kiro\\tools\\aidlc.ts" engine runtime compile', "bun .claude/tools/aidlc.ts engine state advance && bun .claude/tools/aidlc-runtime.ts compile",
    "aidlc engine runtime compile", "echo 'aidlc engine runtime compile'", "aidlc state approve; aidlc engine runtime compile",
    "bun .claude/tools/aidlc-utility.ts status", "bun .claude/tools/aidlc-orchestrate.ts report --result done", "bun .claude/tools/aidlc-orchestrate.ts next",
    "aidlc next --resume && aidlc report", "aidlc next report", "aidlc engine orchestrate report", "aidlc scope change", "aidlc config set x y",
    "aidlc config get x", "aidlc intent create", "aidlc workspace status", "aidlc unit", "aidlc recompose", "aidlcx state", "xaidlc state",
    "cd /p && aidlc next", "cd -- /p && bun .claude/tools/aidlc.ts engine orchestrate continue 2>&1", "cd rel && aidlc next",
    "cd /p && cd /q && aidlc next", "env X=1 aidlc report", "env -i aidlc park", "env -- aidlc next", "command aidlc next", "command -v aidlc next",
    "exec -- aidlc next", "exec -x aidlc next", "X=1 bun .claude/tools/aidlc-orchestrate.ts next", "node .claude/tools/aidlc.ts engine orchestrate next",
    "bun .gemini/tools/aidlc.ts engine orchestrate next", "bun aidlc.ts engine orchestrate next", "aidlc.ts engine orchestrate next",
    "aidlc --project-dir /p next", "aidlc --project-dir", "aidlc --project-dir=/p engine orchestrate next --aidlc-attempt-id a1",
    "aidlc engine orchestrate next --aidlc-attempt-id", "aidlc orchestrate --json next", "aidlc -- next", "aidlc next -- --json",
    "bun .claude/tools/aidlc-orchestrate.ts --json next", "bun .claude/tools/aidlc-orchestrate.ts --project-dir /p next",
    "aidlc next | cat", "aidlc next > out", "aidlc next; echo", "aidlc 'next'", 'aidlc "next"', 'aidlc "ne\\"xt"', "aidlc ne\\xt", "aidlc ~/x",
    "aidlc x~", "aidlc $(x)", "aidlc `x`", "aidlc #x", "aidlc next\n", "aidlc\tnext", "  aidlc next  ", "aidlc next 2>&1", "aidlc next 2>&1 2>&1",
    "aidlc next\t2>&1", "", "   ", "cd /p &&", "&& aidlc next", "cd /p && && aidlc next", "cd '/p q' && aidlc next", "cd \"C:\\p\" && aidlc next",
    "cd 'C:/p' && aidlc next", "cd '\\\\srv\\share' && aidlc next", "cd '\\\\srv' && aidlc next", "cd /p -- && aidlc next", "cd -- -- /p && aidlc next",
    "bun .claude/tools/aidlc.ts engine", "bun .claude/tools/aidlc.ts", "aidlc", "aidlc engine", "aidlc engine orchestrate",
    "aidlc orchestrate next", "aidlc continue", "aidlc park", "aidlc swarm x", "aidlc bolt create", "bun .claude/tools/aidlc-swarm.ts next",
    "bun .claude/tools/aidlc-state.ts next", "bun .claude/tools/aidlc-jump.ts resolve", "sudo aidlc next", "sh -c 'aidlc next'", "bash aidlc next",
]
for command in EC_FIXED:
    add("engine_command", command=command)
for _ in range(scale * 60):
    add("engine_command", command=ec_command())


ri = random.Random(seed * 7919 + 9)
HARNESS_JSON = json.dumps({"schemaVersion": 1, "distribution": "claude", "name": "claude", "harnessDir": ".claude"})
RECORD = "aidlc/spaces/default/intents/260105-add-login"
RI_PROTECTED = [".aidlc-sessions/sess.binding.json", "aidlc/.aidlc-sessions/plan-approval/bypass-x", f"{RECORD}/.aidlc-engine/gate-words/s.json",
                f"{RECORD}/audit/laptop-abc123.md", "aidlc/spaces/default/intents/audit/x.md", "aidlc/spaces/other/intents/x/audit/y.md",
                ".claude/hooks/aidlc-review-freeze.ts", ".claude/hooks/x.ts", ".claude/hooks", ".claude/tools/aidlc-lib.ts", ".claude/tools/aidlc.ts",
                ".claude/tools", ".claude/settings.json", ".claude/hooks.json", ".claude/bin/aidlc", ".claude/bin", ".claude",
                ".cursor/hooks/aidlc-x.ts", ".cursor/hooks.json", ".aidlc/tools/aidlc-utility.ts", ".kiro/agents/aidlc-x.json", ".kiro/agents",
                ".codex/tools/aidlc-state.ts", ".github/hooks/aidlc.json", ".opencode/plugin/aidlc-opencode-adapter.ts", ".AIDLC-SESSIONS/x",
                "AIDLC/Spaces/Default/Intents/X/Audit/y.md"]
RI_ALLOWED = ["src/audit/x.md", "notes.md", "src/main.ts", "aidlc/spaces/default/intents/x/ideation/intent.md", f"{RECORD}/.aidlc-engine/engine-touch",
              f"{RECORD}/aidlc-state.md", ".claude/rules/x.md", ".claude/agents/aidlc-x.md", ".claude/tools/data/x.json", ".claude/settings.local.json",
              ".cursor/settings.json", "audit/x.md", "my-audit/y", "aidlc/spaces/default/memory/org.md", ".kiro/steering/x.md", ".claude/skills/x/SKILL.md",
              "aidlc/spaces/default/intents/audit", "docs/aidlc-sessions.md", "hooks/aidlc-x.ts", "tools/aidlc.ts"]
RI_HOOK_WORDS = ["hooks/aidlc-review-freeze.ts", ".claude/hooks/aidlc-state-transition-guard.ts", "./hooks/aidlc-x.ts", "hooks/aidlc-x",
                 "aidlc-record-human-turn", "aidlc-guard-switch.ts", "./aidlc-cursor-adapter.ts", "x/aidlc-codex-adapter.ts", "hooks/aidlc_x.ts",
                 "hooks/aidlc-x.js", "hooks/other.ts", "src/hooks/aidlc-x.ts"]
RI_ENV = ["AIDLC_SESSION_OVERRIDE", "AIDLC_UNATTENDED", "AIDLC_SKIP_ARTIFACT_GUARD", "AIDLC_DISABLE_SENSORS", "CODEX_THREAD_ID",
          "AIDLC_INTERNAL_HUMAN_TURN_TOKEN", "AIDLC_ALLOW_DIRECT_AUDIT_EVENTS", "aidlc_unattended", "AIDLC_UNATTENDEDX", "AIDLC_PROJECT_DIR", "FOO"]


def ri_path():
    return ri.choice(RI_PROTECTED + RI_ALLOWED + RI_ALLOWED)


def ri_quote(word):
    style = ri.random()
    if style < 0.6:
        return word.replace(" ", "\\ ")
    if style < 0.8:
        return "'" + word + "'"
    return '"' + word + '"'


def ri_bash():
    w = lambda: ri_quote(ri_path())  # noqa: E731
    h = lambda: ri.choice(RI_HOOK_WORDS)  # noqa: E731
    e = lambda: ri.choice(RI_ENV)  # noqa: E731
    pick = ri.choice
    return pick([
        lambda: f"echo x > {w()}", lambda: f"echo x >> {w()}", lambda: f"cat {w()}", lambda: f"rm -rf {w()}", lambda: f"mv {w()} {w()}",
        lambda: f"cp {w()} {w()}", lambda: f"mkdir -p {w()}", lambda: f"touch {w()}", lambda: f"sed -i s/a/b/ {w()}", lambda: f"tee -a {w()} < x",
        lambda: f"ls {w()}", lambda: f"rmdir {w()}", lambda: f"install -d {w()}", lambda: f"dd of={w()}", lambda: f"find {w()} -delete",
        lambda: f"cd {ri_quote(RECORD)} && echo y >> audit/z.md", lambda: f"cd {RECORD}; cat audit/z.md", lambda: f"P={RECORD}/audit/z.md; echo >> $P",
        lambda: f"P={RECORD}/audit/z.md; echo >> \"$P\"; P=notes.md", lambda: "cd $D && echo >> audit/x", lambda: "echo >> \"$X\"",
        lambda: "echo >> \"$X\" # audit", lambda: f"X={RECORD}; echo >> $X/audit/y.md", lambda: f"X={RECORD}; echo >> ${{X}}/audit/y.md",
        lambda: "echo >> aud\"\"it/x", lambda: f"cd {RECORD} && echo >> aud''it/x", lambda: "echo >> $(printf au)dit/x", lambda: "echo >> \\audit\\x",
        lambda: "pushd aidlc/spaces/default/intents/x; echo >> audit/y; popd", lambda: f"cd ..; echo >> {RECORD}/audit/y", lambda: f"cd $HOME; echo >> {RECORD}/audit/y",
        lambda: f"A=aidlc; B=spaces; echo >> $A/$B/default/intents/x/audit/y", lambda: "A+=/audit; echo >> $A/x", lambda: f"export R={RECORD}; echo >> $R/audit/y",
        lambda: "echo >> src/audit/x.log", lambda: "echo >> $Y/src/audit/x", lambda: "cd src && echo >> audit/x", lambda: "echo >> audit/x",
        lambda: f"{e()}=1 aidlc engine state get", lambda: f"export {e()}=1", lambda: f"read {e()}", lambda: f"declare -x {e()}=1", lambda: f"printf -v {e()} 1",
        lambda: f"$env:{e()} = 1", lambda: f"${{env:{e()}}} += 1", lambda: f"setx {e()} 1", lambda: f"Set-Item env:{e()} 1", lambda: f"New-Item -Name {e()} -Path env:",
        lambda: f"[Environment]::SetEnvironmentVariable(\"{e()}\", 1)", lambda: f"@{{ \"{e()}\" = 1 }}", lambda: f"echo {e()}=1", lambda: f"test {e()}==1",
        lambda: f"FOO={e()}=1", lambda: f"ls --local {e()}", lambda: f"cat AppData\\Local\\{e()}", lambda: f"${{x:-read}} {e()}", lambda: f"local {e()}",
        lambda: f"cd $(dirname {e()})", lambda: f"typeset -i {e()}=3", lambda: f"mapfile {e()} < x", lambda: f"echo ${{{e()}:=1}}",
        lambda: f"bun {h()}", lambda: f"bun run {h()}", lambda: f"node {h()} --flag", lambda: f"node -r {h()} app.js", lambda: f"bun --preload={h()} x.ts",
        lambda: f"bun --preload {h()} x.ts", lambda: f"node --import={h()} x", lambda: f"bun -r{h()} x", lambda: f"tsx {h()}", lambda: f"deno run {h()}",
        lambda: f"bun --check {h()}", lambda: f"node -c {h()}", lambda: f"deno -c {h()}", lambda: f"bun -- {h()}", lambda: f"bun -e x -- {h()}",
        lambda: f"bun --cwd x {h()}", lambda: f"node --config {h()}", lambda: "aidlc engine hook foo", lambda: "aidlc engine adapter", lambda: "aidlc engine state get",
        lambda: "aidlc --json engine $X", lambda: "aidlc engine $(cat x)", lambda: "aidlc $X", lambda: "aidlc --project-dir x engine hook", lambda: "aidlc --project-dir engine hook",
        lambda: "aidlc.ts engine hook", lambda: "./aidlc engine *", lambda: "aidlc --internal-aidlc-record-human-turn", lambda: "aidlc engine state --internal-aidlc-record-human-turn",
        lambda: "C:\\\\tools\\\\aidlc.exe engine hook", lambda: "aidlcx engine hook", lambda: "AIDLC engine hook", lambda: "aidlc --quiet --yes engine adapter",
        lambda: f"bun -e \"require('{h()}')\"", lambda: f"node --eval=\"import('{h()}')\"", lambda: f"bun -p 'require(\"{h()}\")'", lambda: f"node -pe 'require(\"{h()}\")'",
        lambda: f"node -e 'x' {h()}", lambda: f"bun -e", lambda: f"python -c \"import subprocess; subprocess.run(['bun','{h()}'])\"", lambda: f"python3 -c 'import os; os.system(\"bun {h()}\")'",
        lambda: f"python3 -ic \"__import__('subprocess').run(['bun','{h()}'])\"", lambda: f"python -c 'eval(\"exec(\\\"import os\\\")\")'",
        lambda: f"sh -c 'bun {h()}'", lambda: f"bash -lc \"{e()}=1 aidlc\"", lambda: f"zsh -c 'cd {RECORD} && echo >> audit/x'", lambda: f"bash -c \"echo >> .aidlc-sessions/x\"",
        lambda: f"bash -c 'echo .aidlc-sessions'", lambda: f"sh -c cat {h()}", lambda: f"bash -x {h()}", lambda: f"bash -s < {h()}",
        lambda: f"eval \"bun {h()}\"", lambda: f"eval bun {h()}", lambda: f"alias r='bun {h()}'", lambda: f"alias r=\"ls\"; r", lambda: f"$(bun {h()})",
        lambda: f"x=$(bun {h()})", lambda: f"`aidlc engine hook`", lambda: f"echo \"$(echo bun) {h()}\"", lambda: f"echo `echo .aidlc-sessions`",
        lambda: f"cat <<EOF > run.sh\\nbun {h()}\\nEOF", lambda: f"cat <<'EOF' > run.sh\\nbun {h()}\\nEOF", lambda: f"cat <<EOF > notes.md\\nbun {h()}\\nEOF",
        lambda: f"bash <<'EOF'\\nbun {h()}\\nEOF", lambda: f"bash <<EOF\\nbun {h()}\\nEOF", lambda: f"python3 - <<EOF\\nimport os; os.system(\"bun {h()}\")\\nEOF",
        lambda: f"node <<EOF\\nrequire('{h()}')\\nEOF", lambda: f"cat <<-EOF > x.py\\n\\timport subprocess; subprocess.run(['bun','{h()}'])\\n\\tEOF",
        lambda: f"cat <<EOF | bash\\nbun {h()}\\nEOF", lambda: f"cat <<EOF 3> x.sh\\nbun {h()}\\nEOF", lambda: f"bash 3<<EOF\\nbun {h()}\\nEOF",
        lambda: f"cat <<EOF\\n{e()}=1\\nEOF", lambda: f"cat <<EOF > .aidlc-sessions/x\\nhi\\nEOF", lambda: f"echo a <<EOF; bash\\nbun {h()}\\nEOF",
        lambda: f"cat <<EOF > a.sh && echo >> audit/x\\nhi\\nEOF", lambda: f"X=$(cat <<EOF\\nbun {h()}\\nEOF\\n)", lambda: f"cat <<EOF >> run.sh\\n$(bun {h()})\\nEOF",
        lambda: "./run.sh", lambda: "./run.sh arg", lambda: "bash run.sh", lambda: "sh ./run.sh", lambda: "bun script.ts", lambda: "python x.py", lambda: "python3 x.py",
        lambda: "node script.js", lambda: "bun run script.ts", lambda: "./script.ts", lambda: "./tool", lambda: "bun .claude/tools/aidlc-lib.ts x", lambda: "bun .claude/tools/aidlc-other.ts",
        lambda: "bun .claude/tools/aidlc.ts engine hook x", lambda: "bun core/tools/aidlc-lib.ts", lambda: "bun big.ts", lambda: "bun missing.ts",
        lambda: "sudo bun hooks/aidlc-x.ts", lambda: "env -S 'bun hooks/aidlc-x.ts'", lambda: "xargs bun hooks/aidlc-x.ts", lambda: "timeout 5 aidlc engine hook",
        lambda: f"au\\\\\\ndit() {{ :; }}; echo >> {RECORD}/au\\\\\\ndit/x", lambda: f"echo >> {RECORD}/audit/x\\\\\\n.md", lambda: f"git add {RECORD}/audit/x.md",
        lambda: f"cat {RECORD}/audit/x.md | grep y", lambda: f"ls {RECORD}/audit/", lambda: f"rm {RECORD}/audit/x.md", lambda: f"echo >> {RECORD}/.aidlc-engine/x",
        lambda: f"echo {{ > x; echo }} >> {RECORD}/audit/y", lambda: f"( echo >> {RECORD}/audit/y )", lambda: f"{{ echo >> {RECORD}/audit/y; }}", lambda: f"echo ${{ echo >> {RECORD}/audit/y; }}",
        lambda: "echo > ${X}", lambda: "echo \"${X}\" > y", lambda: "echo >> '$X'", lambda: "echo >> \"$PWD/audit/x\"", lambda: f"cd {RECORD}; echo >> \"$PWD/audit/x\"",
        lambda: "mkdir .aidlc-sessions", lambda: "mkdir -p aidlc/spaces/default/intents/x/audit", lambda: "mkdir -p src/audit", lambda: "mkdir .claude/hooks",
        lambda: "rm -rf .claude", lambda: "rm -r .", lambda: "rm -rf ..", lambda: "rm -rf /", lambda: "mv .claude .bak", lambda: "rm .claude/rules/x.md", lambda: "rm .claude/skills",
        lambda: "rm -rf .cursor/hooks", lambda: "rmdir .kiro", lambda: "mv .github/hooks/aidlc.json x", lambda: "rm .opencode", lambda: "rm -rf .aidlc",
        lambda: "ls -la", lambda: "git status", lambda: "cargo test", lambda: "echo hello", lambda: "",
    ])()


RI_JS = [
    'import x from "./hooks/aidlc-x.ts";', 'import "hooks/aidlc-x";', 'import type { T } from "./hooks/aidlc-x.ts";', 'import { type T } from "./hooks/aidlc-x.ts";',
    'import type, { y } from "./hooks/aidlc-x.ts";', 'export { a } from "./hooks/aidlc-x.ts";', 'export const a = 1;', 'const m = await import("./hooks/aidlc-x.ts");',
    'require("hooks/aidlc-guard-switch");', 'const h = "hooks/aidlc-x.ts"; require(h);', 'const h = "hooks/aidlc-x.ts";', 'let h; h = "hooks/aidlc-x.ts"; require(h);',
    'const { createRequire } = require("module"); createRequire(import.meta.url)("hooks/aidlc-x");', 'eval("require(\'hooks/aidlc-x\')");', 'const e = eval; e("require(\'hooks/aidlc-x\')");',
    'new Function("a", "require(\'hooks/aidlc-x\')");', 'Function("require(\'hooks/aidlc-x\')")();', 'globalThis.eval("require(\'hooks/aidlc-x\')");',
    'const vm = require("vm"); vm.runInThisContext("require(\'hooks/aidlc-x\')");', 'import { runInNewContext as r } from "node:vm"; r("require(\'hooks/aidlc-x\')");',
    'const cp = require("child_process"); cp.exec("bun hooks/aidlc-x.ts");', 'import { execSync } from "node:child_process"; execSync("aidlc engine hook");',
    'import * as cp from "child_process"; cp.spawn("bun", ["hooks/aidlc-x.ts"]);', 'Bun.spawn(["bun", "hooks/aidlc-x.ts"]);', 'Bun.spawnSync({ cmd: ["bun", "hooks/aidlc-x.ts"] });',
    'Deno.run({ cmd: ["bun", "hooks/aidlc-x.ts"] });', 'new Deno.Command("bun", { args: ["hooks/aidlc-x.ts"] });', 'spawn("bun", ["hooks/aidlc-x.ts"]);',
    'const { spawn } = require("child_process"); spawn("bun", ["hooks/aidlc-x.ts"]);', 'const { spawn: s } = require("child_process"); s(process.execPath, ["hooks/aidlc-x.ts"]);',
    'const cp = require("child_process"); const cmd = "bun hooks/aidlc-x.ts"; cp.execSync(cmd);', 'const cmd = `bun hooks/aidlc-x.ts`; require("child_process").exec(cmd);',
    'const cmd = `bun ${x}`; require("child_process").exec(cmd);', '// bun hooks/aidlc-x.ts', '/* require("hooks/aidlc-x") */', 'const s = "bun hooks/aidlc-x.ts";',
    'const doc = { example: "bun hooks/aidlc-x.ts" };', 'console.log("hooks/aidlc-x.ts");', 'const re = /hooks\\/aidlc-x/;', 'execSync.call(null, "bun hooks/aidlc-x.ts");',
    'execSync.apply(null, ["bun hooks/aidlc-x.ts"]);', 'const f = execSync.bind(null); f("bun hooks/aidlc-x.ts");', 'function run() { require("hooks/aidlc-x"); }',
    'const run = () => require("hooks/aidlc-x");', 'const run = (require) => require("hooks/aidlc-x");', 'function run(require) { require("hooks/aidlc-x"); }',
    'const require = () => {}; require("hooks/aidlc-x");', 'let eval = 1;', 'globalThis.eval = () => {}; eval("require(\'hooks/aidlc-x\')");',
    'const cp = require("child_process"); cp.exec = () => {}; cp.exec("bun hooks/aidlc-x.ts");', 'for (const require of []) require("hooks/aidlc-x");',
    'import("hooks/aidlc-x")', 'await import(`hooks/${x}`)', 'const p = process.env.AIDLC_SESSION_OVERRIDE;', 'process.env.AIDLC_UNATTENDED = "1";',
    'echo "x" > .aidlc-sessions/y', 'sh -c "bun hooks/aidlc-x.ts"', 'const t = `bun hooks/aidlc-x.ts`', 'bun hooks/aidlc-x.ts', 'aidlc engine hook; const x = 1',
    'node hooks/aidlc-x.ts', 'const bun = 1; bun hooks/aidlc-x.ts', 'class X { eval() { require("hooks/aidlc-x") } }', 'if (x) { require("hooks/aidlc-x") }',
    'const x = (require("hooks/aidlc-x"));', 'const [a] = [require("hooks/aidlc-x")];', 'import { exec } from "child_process"\nexec("bun hooks/aidlc-x.ts")',
    'import { exec } from "child_process";\nconst c = "bun hooks/aidlc-x.ts";\nexec(c)', 'const path = "hooks/aidlc-x.ts";\nconst m = await import(path);',
    'import("./hooks/aidlc-x.ts").then(m => m)', 'const body = "require(\'hooks/aidlc-x\')"; eval(body);', 'eval(`require("hooks/aidlc-x")`);', 'eval(x);',
    'subprocess.run(["bun", "hooks/aidlc-x.ts"])', 'import subprocess\nsubprocess.run(["bun", "hooks/aidlc-x.ts"])', 'import subprocess as sp\nsp.call(["aidlc", "engine", "hook"])',
    'from subprocess import run\nrun(["bun", "hooks/aidlc-x.ts"])', 'from subprocess import run as r\nr(args=["bun", "hooks/aidlc-x.ts"])', 'import os\nos.system("bun hooks/aidlc-x.ts")',
    'import os\nos.popen("aidlc engine adapter")', '__import__("subprocess").run(["bun", "hooks/aidlc-x.ts"])', 'exec("import os; os.system(\'bun hooks/aidlc-x.ts\')")',
    'eval("__import__(\'os\').system(\'bun hooks/aidlc-x.ts\')")', 'import builtins\nbuiltins.exec("import os; os.system(\'aidlc engine hook\')")',
    'def f():\n    import subprocess\n    subprocess.run(["bun", "hooks/aidlc-x.ts"])', 'def f(subprocess):\n    subprocess.run(["bun", "hooks/aidlc-x.ts"])',
    'def f():\n    subprocess = None\n    subprocess.run(["bun", "hooks/aidlc-x.ts"])', 'class C:\n    def m(self):\n        import os\n        os.system("bun hooks/aidlc-x.ts")',
    '# os.system("bun hooks/aidlc-x.ts")', 's = """\nbun hooks/aidlc-x.ts\n"""', "x = 'bun hooks/aidlc-x.ts'", 'import os\nos.system(cmd)', 'cmd = "bun hooks/aidlc-x.ts"\nimport os\nos.system(cmd)',
    'if True: import os; os.system("bun hooks/aidlc-x.ts")', 'import os as o\no.system("bun hooks/aidlc-x.ts")', 'from os import system\nsystem("bun hooks/aidlc-x.ts")',
    'from os import *\nsystem("bun hooks/aidlc-x.ts")', 'subprocess.Popen("bun hooks/aidlc-x.ts")', 'import subprocess\nsubprocess.check_output(["bun", "--", "hooks/aidlc-x.ts"])',
    'bun hooks/aidlc-x.ts\necho done', '#!/bin/sh\nbun hooks/aidlc-x.ts', '#!/usr/bin/env bun\nrequire("hooks/aidlc-x")', '#!/usr/bin/env python3\nimport os; os.system("aidlc engine hook")',
    'AIDLC_UNATTENDED=1 aidlc engine state get', 'export AIDLC_SESSION_OVERRIDE=x', 'echo >> aidlc/spaces/default/intents/x/audit/y.md', 'cd aidlc/spaces/default/intents/x && echo >> audit/y.md',
    'echo hi', '', 'const x = 1;\n', 'print("hi")',
    'const s = `${require("hooks/aidlc-x")}`;', 'const c = `bun ${x}`; require("child_process").exec(c);', 'const r = /"/; require("hooks/aidlc-x");',
    'const r = x / 2 / 3; require("hooks/aidlc-x");', 'import { default as cp } from "node:child_process"; cp.exec("bun hooks/aidlc-x.ts");',
    'const cp = await import("node:child_process"); cp.exec("bun hooks/aidlc-x.ts");', 'import { execSync } from "child_process"; const f = execSync.bind(null); f("bun hooks/aidlc-x.ts");',
    'import { execSync } from "child_process"; execSync.call(null, "bun hooks/aidlc-x.ts");', 'import { execSync } from "child_process"; execSync.apply(null, ["bun hooks/aidlc-x.ts"]);',
    'import type cp from "node:child_process"; cp.exec("bun hooks/aidlc-x.ts");', 'import cp from "node:child_process"; cp.exec("bun hooks/aidlc-x.ts");',
    'import * as cp from "node:child_process"; cp.default.exec("bun hooks/aidlc-x.ts");', 'function require() {}\nrequire("hooks/aidlc-x");', 'let eval;\neval("require(\'hooks/aidlc-x\')");',
    'for (const require of []) { require("hooks/aidlc-x") }', 'for (let eval of []) { eval("require(\'hooks/aidlc-x\')") }', 'while (x) { require("hooks/aidlc-x") }',
    'function f(require) { require("hooks/aidlc-x") }', 'const f = (eval) => { eval("require(\'hooks/aidlc-x\')") };', 'const f = ({ require }) => require("hooks/aidlc-x");',
    'const { exec: run } = require("child_process"); run("bun hooks/aidlc-x.ts");', 'const { execPath } = process; const { spawn } = require("child_process"); spawn(execPath, ["hooks/aidlc-x.ts"]);',
    'const { spawn } = require("child_process"); spawn(process.execPath, ["hooks/aidlc-x.ts"]);', 'globalThis.require = () => {}; require("hooks/aidlc-x");', 'global.eval = 1; eval("require(\'hooks/aidlc-x\')");',
    'const vm = require("node:vm"); new vm.Script("require(\'hooks/aidlc-x\')");', 'const { compileFunction } = require("vm"); compileFunction("require(\'hooks/aidlc-x\')");',
    'eval("eval(\'require(\\\\\'hooks/aidlc-x\\\\\')\')");', 'const cp = require("child_process"); eval("cp.exec(\'bun hooks/aidlc-x.ts\')");', '(0, eval)("cp.exec(\'bun hooks/aidlc-x.ts\')"); const cp = require("child_process");',
    'const cp = require("child_process"); (0, eval)("cp.exec(\'bun hooks/aidlc-x.ts\')");', 'const cp = require("child_process"); eval.call(null, "cp.exec(\'bun hooks/aidlc-x.ts\')");',
    'new Function("cp", "cp.exec(\'bun hooks/aidlc-x.ts\')");', 'new Function("require", "require(\'hooks/aidlc-x\')");', 'new Function("x", "require(\'hooks/aidlc-x\')");',
    'import subprocess\ndef f(subprocess):\n    subprocess.run(["bun", "hooks/aidlc-x.ts"])', 'import subprocess\ndef f():\n    subprocess = None\n    subprocess.run(["bun", "hooks/aidlc-x.ts"])',
    'import subprocess\ndef f():\n    subprocess.run(["bun", "hooks/aidlc-x.ts"])', 'import subprocess\ndef f():\n    import subprocess\n    subprocess.run(["bun", "hooks/aidlc-x.ts"])',
    'import subprocess\ndef f():\n    from os import system as subprocess\n    subprocess.run(["bun", "hooks/aidlc-x.ts"])', 'import subprocess\nclass C:\n    subprocess = 1\n    subprocess.run(["bun", "hooks/aidlc-x.ts"])',
    'import subprocess\ndef f(): subprocess.run(["bun", "hooks/aidlc-x.ts"])', 'import subprocess\ndef f(): x = 1\nsubprocess.run(["bun", "hooks/aidlc-x.ts"])',
    'import subprocess\ndef f():\n    pass\nsubprocess.run(["bun", "hooks/aidlc-x.ts"])', 'import subprocess\nif x:\n    subprocess.run(["bun", "hooks/aidlc-x.ts"])',
    'import os\nexec("os.system(\'bun hooks/aidlc-x.ts\')")', 'exec("import os\\nos.system(\'bun hooks/aidlc-x.ts\')")', 'import os\neval("os.system(\'bun hooks/aidlc-x.ts\')")',
    'import subprocess\nsubprocess.run(args=["bun", "hooks/aidlc-x.ts"])', 'import subprocess\nsubprocess.run(["bun", "run", "hooks/aidlc-x.ts"])', 'import subprocess\nsubprocess.run("bun hooks/aidlc-x.ts")',
    'import subprocess\nsubprocess.run(["aidlc", "engine", "hook"])',
    'const cp = require("child_process"); cp.exec = () => {}; cp["exec"]("bun hooks/aidlc-x.ts");', 'import type { exec } from "node:child_process"; exec("bun hooks/aidlc-x.ts");',
    'import subprocess\ndef f():\n    subprocess.run(["bun", "hooks/aidlc-x.ts"])\n    import subprocess', 'import subprocess\nsubprocess.run(["aidlc", "engine", x])', 'import subprocess\nsubprocess.run(["aidlc", "engine", "state", "get"])',
]


def ri_content():
    text = ri.choice(RI_JS)
    if ri.random() < 0.3:
        text = ri.choice(RI_JS) + "\n" + text
    if ri.random() < 0.1:
        text = char_fuzz(text, ri.randrange(1, 3))
    return text


RI_TARGETS = ["script.ts", "x.py", "run.sh", "notes.md", "a.js", "tool", "x.mjs", "README.txt", "doc.rst", "x.ipynb", "src/x.ts", ".claude/rules/x.md",
              "core/hooks/aidlc-x.ts", "docs/x.ts", "tests/t.ts", ".claude/tools/aidlc-lib.ts", RECORD + "/x.ts", "big.ts"]


def ri_files():
    files = {}
    if ri.random() < 0.7:
        files[".claude/tools/data/harness.json"] = HARNESS_JSON
    for name, text in ri.sample([
        ("run.sh", ri.choice(["bun hooks/aidlc-x.ts", "#!/bin/sh\necho hi", "echo >> " + RECORD + "/audit/x", "AIDLC_UNATTENDED=1 aidlc"])),
        ("script.ts", ri.choice(['import "./hooks/aidlc-x.ts";', 'console.log(1)', 'require("hooks/aidlc-x")', '// require("hooks/aidlc-x")'])),
        ("script.js", 'require("hooks/aidlc-x")'), ("x.py", ri.choice(['import os\nos.system("bun hooks/aidlc-x.ts")', 'print(1)'])),
        ("tool", ri.choice(["bun hooks/aidlc-x.ts", "#!/usr/bin/env bun\nrequire('hooks/aidlc-x')", "#!/usr/bin/env python3\nimport os\nos.system('aidlc engine hook')"])),
        ("hooks/aidlc-x.ts", "export {}"), ("big.ts", "@bytes:1048577"), ("a.js", ri.choice(["/* x */", "/* require('hooks/aidlc-x') */", "// require('hooks/aidlc-x')\n", "const a = 1;"])),
        (".claude/tools/aidlc-lib.ts", 'require("hooks/aidlc-x")'), (".claude/tools/aidlc-other.ts", 'require("hooks/aidlc-x")'),
        ("core/tools/aidlc-lib.ts", 'require("hooks/aidlc-x")'), ("scripts/package.ts", "export {}"), ("src/x.ts", "const a = 1;"),
        (".aidlc-sessions/x", "1"), (RECORD + "/audit/laptop-abc123.md", "# AI-DLC Audit Log\n"), ("notes.md", "bun hooks/aidlc-x.ts"),
    ], ri.randrange(0, 7)):
        files[name] = text
    for name in ri.sample(["aidlc/spaces/default/intents/x/audit/", ".claude/hooks/", "src/audit/", ".cursor/", ".kiro/agents/", "sub/"], ri.randrange(0, 3)):
        files[name] = None
    return files


def ri_links():
    out = []
    if ri.random() < 0.2:
        out.append(ri.choice([["link", RECORD + "/audit"], ["cl", ".claude"], ["rec", RECORD], ["s", ".aidlc-sessions"], ["h", ".claude/hooks"], ["n", "notes"], ["loop", "loop"]]))
    return out


def ri_case(input_, files=None, links=None, exec_=None, env=None, cwd="{P}", project=""):
    input_ = dict(input_)
    input_.setdefault("cwd", cwd)
    add("runtime_integrity", input=input_, files=files if files is not None else ri_files(), links=links if links is not None else ri_links(),
        exec=exec_ if exec_ is not None else (["run.sh", "tool"] if ri.random() < 0.5 else []), env=env or {}, project=project)


for _ in range(scale * 90):
    command = ri_bash() if ri.random() < 0.8 else sh_command()
    if ri.random() < 0.2:
        command = command + ri.choice(["; ", " && ", " | ", "\n"]) + ri_bash()
    powershell = ri.random() < 0.08
    input_ = {"tool_name": "Bash", "tool_input": {"command": command}}
    if powershell:
        input_["aidlc_shell"] = "powershell"
    ri_case(input_, cwd=ri.choice(["{P}", "{P}", "{P}/sub", "{P}/" + RECORD]), project=ri.choice(["", "", "sub"]))

for _ in range(scale * 60):
    tool = ri.choice(["Write", "Write", "Edit", "MultiEdit", "NotebookEdit", "Read", "bash"])
    target = ri.choice(RI_TARGETS + RI_PROTECTED)
    if tool == "Write":
        input_ = {"tool_name": tool, "tool_input": {"file_path": target, "content": ri_content()}}
    elif tool == "NotebookEdit":
        input_ = {"tool_name": tool, "tool_input": {"notebook_path": target, "new_source": ri_content(), "cell_id": "c1"}}
    elif tool == "Edit":
        input_ = {"tool_name": tool, "tool_input": {"file_path": target, "old_string": ri.choice(["", "/* ", "// ", "const a = 1;", "x", "require"]), "new_string": ri_content()}}
        if ri.random() < 0.3:
            input_["tool_input"]["replace_all"] = ri.choice([True, False, "yes"])
    elif tool == "MultiEdit":
        edits = []
        for _ in range(ri.randrange(0, 4)):
            edit = {"old_string": ri.choice(["", "/* ", "// ", "const a = 1;", "x", " */"]), "new_string": ri_content()}
            if ri.random() < 0.3:
                edit["file_path"] = ri.choice(RI_TARGETS)
            if ri.random() < 0.2:
                edit = ri.choice([edit, "nope", None, {"new_string": 3}])
            edits.append(edit)
        input_ = {"tool_name": tool, "tool_input": {"file_path": target, "edits": edits}}
    else:
        input_ = {"tool_name": tool, "tool_input": {"file_path": target, "content": ri_content()}}
    ri_case(input_, cwd=ri.choice(["{P}", "{P}", "{P}/sub"]), project=ri.choice(["", "", "sub"]))

# The edges the random mix reaches rarely.
RI_FIXED = [
    {"tool_name": "Bash", "tool_input": {"command": 1}}, {"tool_name": "Bash", "tool_input": {}}, {"tool_name": "Bash"}, {"tool_name": "Write", "tool_input": {"file_path": ".aidlc-sessions/x"}},
    {"tool_name": "Write", "tool_input": {"file_path": "x.ts", "content": 3}}, {"tool_name": "MultiEdit", "tool_input": {"edits": "nope", "file_path": "x.ts"}},
    {"tool_name": "MultiEdit", "tool_input": {"edits": [{"file_path": ".claude/hooks/x", "new_string": ""}]}}, {"tool_name": "Edit", "tool_input": {"file_path": "a.js", "old_string": "/* ", "new_string": ""}},
    {"tool_name": "Edit", "tool_input": {"file_path": "a.js", "old_string": "", "new_string": "require('hooks/aidlc-x')", "replace_all": True}},
    {"tool_name": "Edit", "tool_input": {"file_path": "empty.js", "old_string": "", "new_string": "require('hooks/aidlc-x')"}},
    {"tool_name": "Edit", "tool_input": {"file_path": "empty.js", "old_string": "", "new_string": "require('hooks/aidlc-x')", "replace_all": True}},
    {"tool_name": "Write", "tool_input": {"file_path": "core/hooks/aidlc-new.ts", "content": 'import "./aidlc-record-human-turn.ts";'}},
    {"tool_name": "Write", "tool_input": {"file_path": "docs/x.md", "content": 'require("hooks/aidlc-x")'}},
    {"tool_name": "NotebookEdit", "tool_input": {"notebook_path": "n.md", "new_source": 'require("hooks/aidlc-x")'}},
    {"tool_name": "Write", "tool_input": {"file_path": "run.sh", "content": "echo >> " + RECORD + "/audit/x"}},
    {"tool_name": "Bash", "tool_input": {"command": "cat link/x; echo >> link/y.md"}}, {"tool_name": "Bash", "tool_input": {"command": "rm -rf cl"}},
    {"tool_name": "Bash", "tool_input": {"command": "echo >> cl/hooks/aidlc-x.ts"}}, {"tool_name": "Bash", "tool_input": {"command": "echo >> rec/audit/x"}},
    {"tool_name": "Bash", "tool_input": {"command": "echo >> s/x"}}, {"tool_name": "Bash", "tool_input": {"command": "echo >> h/x"}},
    {"tool_name": "Bash", "tool_input": {"command": "echo >> loop/x"}}, {"tool_name": "Bash", "tool_input": {"command": "echo >> link/new/deeper.md"}},
    {"tool_name": "Bash", "tool_input": {"command": "Set-Content -Path .aidlc-sessions\\x -Value 1"}, "aidlc_shell": "powershell"},
    {"tool_name": "Bash", "tool_input": {"command": "sc .aidlc-sessions\\x 1"}, "aidlc_shell": "powershell"},
    {"tool_name": "Bash", "tool_input": {"command": "Out-File -FilePath " + RECORD.replace("/", "\\") + "\\audit\\x.md"}, "aidlc_shell": "powershell"},
    {"tool_name": "Bash", "tool_input": {"command": "Set-Content .claude\\settings.json x"}, "aidlc_shell": "posix"},
    {"tool_name": "Bash", "tool_input": {"command": "$env:AIDLC_UNATTENDED = 1"}, "aidlc_shell": "powershell"},
    {"tool_name": "Bash", "tool_input": {"command": "bun .claude/tools/aidlc-lib.ts"}}, {"tool_name": "Bash", "tool_input": {"command": "bun .claude/tools/aidlc-other.ts"}},
    {"tool_name": "Bash", "tool_input": {"command": "bun core/tools/aidlc-lib.ts"}}, {"tool_name": "Bash", "tool_input": {"command": "bun big.ts"}},
    {"tool_name": "Bash", "tool_input": {"command": "bun tool"}}, {"tool_name": "Bash", "tool_input": {"command": "./tool"}}, {"tool_name": "Bash", "tool_input": {"command": "tool"}},
    {"tool_name": "Bash", "tool_input": {"command": "cat <<EOF > tool\nbun hooks/aidlc-x.ts\nEOF"}}, {"tool_name": "Bash", "tool_input": {"command": "cat <<EOF > run.sh\n#!/bin/bash\nbun hooks/aidlc-x.ts\nEOF"}},
    {"tool_name": "Bash", "tool_input": {"command": "cat <<EOF > a.js\n#!/usr/bin/env python\nimport os\nos.system('bun hooks/aidlc-x.ts')\nEOF"}},
]
for input_ in RI_FIXED:
    files = ri_files()
    files.update({"a.js": "/* require('hooks/aidlc-x') */", "empty.js": "", "run.sh": "bun hooks/aidlc-x.ts", "tool": "bun hooks/aidlc-x.ts", ".claude/tools/aidlc-lib.ts": 'require("hooks/aidlc-x")',
                  ".claude/tools/aidlc-other.ts": 'require("hooks/aidlc-x")', "core/tools/aidlc-lib.ts": 'require("hooks/aidlc-x")', "scripts/package.ts": "export {}",
                  "big.ts": "@bytes:1048577", ".claude/tools/data/harness.json": HARNESS_JSON, "hooks/aidlc-x.ts": "export {}", "notes/x": "1"})
    ri_case(input_, files=files, links=[["link", RECORD + "/audit"], ["cl", ".claude"], ["rec", RECORD], ["s", ".aidlc-sessions"], ["h", ".claude/hooks"], ["loop", "loop"]],
            exec_=["tool"], cwd="{P}")
for input_ in RI_FIXED[:6]:
    ri_case(input_, files={".claude/tools/data/harness.json": HARNESS_JSON}, links=[], exec_=[], env={"AIDLC_HARNESS_DIR": ".cursor"}, cwd="{P}/sub", project="")

# Every source snippet written to a script, and the shell lines that reach
# one branch each, so that no branch depends on the random draw.
RI_STANDARD_FILES = {"a.js": "/* require('hooks/aidlc-x') */", "empty.js": "", "run.sh": "bun hooks/aidlc-x.ts", "tool": "bun hooks/aidlc-x.ts",
                     ".claude/tools/aidlc-lib.ts": 'require("hooks/aidlc-x")', ".claude/tools/aidlc-other.ts": 'require("hooks/aidlc-x")',
                     "core/tools/aidlc-lib.ts": 'require("hooks/aidlc-x")', "scripts/package.ts": "export {}", "big.ts": "@bytes:1048577",
                     ".claude/tools/data/harness.json": HARNESS_JSON, "hooks/aidlc-x.ts": "export {}", "x.py": "import os\nos.system('bun hooks/aidlc-x.ts')",
                     "script.ts": 'import "./hooks/aidlc-x.ts";', "plain.sh": "echo hi", "exec-plain": "#!/bin/sh\necho hi", "src/x.ts": "const a = 1;",
                     RECORD + "/audit/laptop-abc123.md": "# AI-DLC Audit Log\n", "config": "k=v", "real-other/hooks/x": "1", "other-tools/tools/aidlc.ts": "1"}
def ri_python(snippet):
    return re.match(r"(?:import os|import subprocess|from |def |class C|print\(|__import__|subprocess\.|exec\(\"import|eval\(\"__import__|import builtins|if True:|cmd = |s = \"\"\"|x = 'bun|#!/usr/bin/env python|#.*os\.system)", snippet) is not None


for snippet in RI_JS:
    python = ri_python(snippet)
    target = "x.py" if python else "script.ts"
    ri_case({"tool_name": "Write", "tool_input": {"file_path": target, "content": snippet}}, files=dict(RI_STANDARD_FILES), links=[], exec_=["tool", "exec-plain"])
    if not python:
        ri_case({"tool_name": "Write", "tool_input": {"file_path": "run.sh", "content": snippet}}, files=dict(RI_STANDARD_FILES), links=[], exec_=["tool"])
RI_FIXED_BASH = [
    "bun --check hooks/aidlc-x.ts", "node -c hooks/aidlc-x.ts", "deno -c hooks/aidlc-x.ts", "bun -e 'x' hooks/aidlc-x.ts", "bun -e x -- hooks/aidlc-x.ts",
    "bun --check -- hooks/aidlc-x.ts", "bun big.ts", "bun .claude/tools/aidlc-lib.ts", "bun .claude/tools/aidlc-other.ts", "bun core/tools/aidlc-lib.ts",
    "x=$(printf %s bun) ; $x hooks/aidlc-x.ts", "$(printf %s bun) hooks/aidlc-x.ts", "$(echo bun) hooks/aidlc-x.ts", "$(printf '%s' bun hooks/aidlc-x.ts)",
    "\"$(printf %s bun hooks/aidlc-x.ts)\"", "echo \"$(printf %s .aidlc-sessions)/x\" > y", "echo x > \"$(printf %s .aidlc-sessions)/y\"", "echo x > `printf %s .aidlc-sessions`/y",
    "bash -s < run.sh", "bash -es < run.sh", "cat run.sh | bash -s", "cat run.sh | bash", "cat run.sh | bash -", "bash <<'EOF'\nbun hooks/aidlc-x.ts\nEOF",
    "bash <<EOF\nbun hooks/aidlc-x.ts\nEOF", "bash <<EOF\n$(bun hooks/aidlc-x.ts)\nEOF", "bash <<'EOF'\n$(bun hooks/aidlc-x.ts)\nEOF", "cat <<EOF\n$(bun hooks/aidlc-x.ts)\nEOF",
    "cat <<EOF | bash\nbun hooks/aidlc-x.ts\nEOF", "cat <<EOF || bash\nbun hooks/aidlc-x.ts\nEOF", "cat <<EOF && bash\nbun hooks/aidlc-x.ts\nEOF", "cat <<EOF |& bash\nbun hooks/aidlc-x.ts\nEOF",
    "cat <<EOF | tee x | bash\nbun hooks/aidlc-x.ts\nEOF", "true; cat <<EOF | bash\nbun hooks/aidlc-x.ts\nEOF", "cat <<EOF | bash; true\nbun hooks/aidlc-x.ts\nEOF",
    "cat <<EOF 3> x.sh\nbun hooks/aidlc-x.ts\nEOF", "bash 3<<EOF\nbun hooks/aidlc-x.ts\nEOF", "bash 0<<EOF\nbun hooks/aidlc-x.ts\nEOF", "cat <<EOF > tool\nbun hooks/aidlc-x.ts\nEOF",
    "cat <<EOF > exec-plain\nbun hooks/aidlc-x.ts\nEOF", "cat <<EOF > plain.sh\nbun hooks/aidlc-x.ts\nEOF", "cat <<EOF > newfile\nbun hooks/aidlc-x.ts\nEOF",
    "cat <<EOF > newfile\n#!/bin/bash\nbun hooks/aidlc-x.ts\nEOF", "cat <<EOF > newfile.js\n#!/bin/sh\nbun hooks/aidlc-x.ts\nEOF", "cat <<EOF > x.py\nimport os; os.system('bun hooks/aidlc-x.ts')\nEOF",
    "cat <<EOF > a.js\nrequire('hooks/aidlc-x')\nEOF", "cat <<EOF > notes.md\nrequire('hooks/aidlc-x')\nEOF", "cat <<-EOF > run.sh\n\tbun hooks/aidlc-x.ts\n\tEOF",
    "cat <<\"EOF\" > run.sh\nbun hooks/aidlc-x.ts\nEOF", "cat <<EOF > run.sh\n${X}\nEOF", "cat <<EOF > run.sh\n$(echo >> .aidlc-sessions/x)\nEOF",
    "cat <<'EOF' > run.sh\n$(echo >> .aidlc-sessions/x)\nEOF", "cat <<EOF > run.sh\n`echo >> .aidlc-sessions/x`\nEOF", "# bun hooks/aidlc-x.ts", "echo; # bun hooks/aidlc-x.ts",
    "echo '#' bun hooks/aidlc-x.ts", "echo \"# bun hooks/aidlc-x.ts\"", "x#bun hooks/aidlc-x.ts", "echo >> " + RECORD + "/au\\\ndit/x", "echo >> " + RECORD + "/au\\\r\ndit/x",
    "echo >> " + RECORD + "/audit/x\\\n.md", "cd " + RECORD + "; echo >> ${ x; }/audit", "echo >> ${X:-audit}/y", "echo >> ${X}/y # audit", "echo >> $X/y", "echo >> $X/audit/y",
    "cd $X; echo >> audit/y", "cd $X; echo >> y", "cd " + RECORD + "; cd $X; echo >> audit/y", "pushd -n x; echo >> " + RECORD + "/audit/y", "cd -; echo >> audit/y",
    "A=aidlc/spaces; A+=/default/intents/x/audit/y; echo >> $A", "A=x; A+=/audit; echo >> $A/y", "A=x; A+=$B; echo >> $A/audit/y", "A='$B'; echo >> $A/audit",
    "A=\"$B\"; echo >> $A/audit/y", "A=au\\dit; echo >> " + RECORD + "/$A/y", "A='aud'; echo >> " + RECORD + "/${A}it/y", "export A=" + RECORD + "; echo >> $A/audit/y",
    "declare -x A=" + RECORD + "; echo >> $A/audit/y", "local A=" + RECORD + "; echo >> $A/audit/y", "readonly A=" + RECORD + "; echo >> ${A}/audit/y", "(A=" + RECORD + "); echo >> $A/audit/y",
    "echo >> \\audit\\x", "echo >> x\\Audit\\y", "echo >> 'a\\audit\\b'", "mkdir .aidlc-sessions", "mkdir -p " + RECORD + "/audit", "mkdir src/audit", "mkdir .claude/hooks",
    "mkdir link/x", "eval 'bun hooks/aidlc-x.ts'", "eval bun hooks/aidlc-x.ts", "eval \"echo >> .aidlc-sessions/x\"", "alias r='bun hooks/aidlc-x.ts'", "alias r=ls r2='aidlc engine hook'",
    "alias r", "rm -rf .claude", "rm -rf .claude/rules", "rm -rf .", "rm -rf ..", "mv .claude x", "mv .claude/hooks x", "rmdir .claude/hooks", "rm cl", "rm -rf cl/", "rm -rf h", "mv rec x",
    "rm link/x", "rm -rf .github/hooks", "rm .github/hooks/aidlc.json", "rm -rf .opencode/plugin", "rm -rf .kiro/agents", "rm .claude/settings.json", "rm .cursor/settings.json",
    "rm .claude/hooks.json", "rm .claude/bin", "rm .claude/bin/aidlc", "rm .claude/tools/aidlc.ts", "rm .claude/tools/data/x", "rm .claude/tools",
    "echo >> cl/hooks/x", "echo >> cl/settings.json", "echo >> cl/rules/x.md", "echo >> h/x.ts", "echo >> rec/audit/x", "echo >> rec/x", "echo >> s/x", "echo >> link/x", "echo >> link/new/x",
    "echo >> loop/x", "echo >> missing/.aidlc-sessions/x", "echo >> .AIDLC-sessions/x", "echo >> .aidlc-sessionsx/x", "echo >> x/.aidlc-sessions", "echo >> .aidlc-plan-approval/x",
    "echo >> " + RECORD + "/.aidlc-engine/gate-words/x", "echo >> " + RECORD + "/.aidlc-engine/gate-words", "echo >> " + RECORD + "/.aidlc-engine/other", "echo >> " + RECORD + "/.aidlc-engine//gate-words/x",
    "AIDLC_UNATTENDED=1", "AIDLC_UNATTENDED==1", "x=AIDLC_UNATTENDED=1", "echo AIDLC_UNATTENDED", "export AIDLC_UNATTENDED", "export FOO AIDLC_UNATTENDED", "read -r AIDLC_SESSION_OVERRIDE",
    "printf -v AIDLC_UNATTENDED 1", "printf AIDLC_UNATTENDED", "ls --local AIDLC_UNATTENDED", "cat AppData\\Local\\AIDLC_UNATTENDED", "cat ~/local/AIDLC_UNATTENDED", "x/read AIDLC_UNATTENDED",
    "${x:-read} AIDLC_UNATTENDED", "$env:AIDLC_UNATTENDED = 1", "$env:AIDLC_UNATTENDED=1", "${env:AIDLC_UNATTENDED} += 1", "$x = $env:AIDLC_UNATTENDED", "Set-Item env:AIDLC_UNATTENDED 1",
    "Set-Item env:\\AIDLC_UNATTENDED 1", "x env:AIDLC_UNATTENDED", "$env:AIDLC_UNATTENDED", "New-Item -Name AIDLC_UNATTENDED -Path env:", "Rename-Item x -NewName:AIDLC_UNATTENDED",
    "[Environment]::SetEnvironmentVariable('AIDLC_UNATTENDED', 1)", "$psi.Environment.Add(\"AIDLC_UNATTENDED\", 1)", "setx AIDLC_UNATTENDED 1", "setx.exe /M AIDLC_UNATTENDED 1",
    "echo setx AIDLC_UNATTENDED", "@{ AIDLC_UNATTENDED = 1 }", "@{ \"AIDLC_UNATTENDED\" = 1 }", "AIDLC_UNATTENDED+=1", "AIDLC_UNATTENDED:=1", "AIDLC_UNATTENDED ??= 1",
    "${AIDLC_UNATTENDED:=1}", "echo ${AIDLC_UNATTENDED:-1}", "aidlc_unattended=1", "AIDLC_SKIP_ARTIFACT_GUARD=1 true", "AIDLC_DISABLE_SUMMARY_CONFIRMATION=1", "AIDLC_PROJECT_DIR=x",
    "aidlc engine hook", "aidlc engine adapter", "aidlc engine hooks", "aidlc engine", "aidlc", "aidlc engine $X", "aidlc engine 'a b'", "aidlc engine a*", "aidlc $X engine",
    "aidlc --json engine hook", "aidlc --project-dir x engine adapter", "aidlc --project-dir engine", "aidlc --project-dir --json engine hook", "aidlc --verbose --quiet", "aidlc engine $(x)",
    "aidlc --internal-aidlc-record-human-turn", "aidlc engine state get --internal-aidlc-record-human-turn", "aidlc.ts engine hook", "AIDLC.TS engine hook", "./aidlc", "x/aidlc engine hook",
    "aidlc.exe engine hook", "aidlc.cmd engine adapter", "aidlc.bat engine hook", "aidlc.sh engine hook", "aidlcx engine hook", "bun hooks/aidlc-x.ts", "bun hooks/aidlc-x", "bun hooks/aidlc-x.js",
    "bun ./hooks/aidlc-x.ts", "bun x/hooks/aidlc-x.ts", "bun hooks/aidlc_x.ts", "bun hooks/other.ts", "bun aidlc-cursor-adapter.ts", "bun x/aidlc-kiro-adapter.ts", "bun aidlc-codex-adapter.js",
    "node -r hooks/aidlc-x app.js", "node -rhooks/aidlc-x app.js", "node --require hooks/aidlc-x app.js", "node --require=hooks/aidlc-x app.js", "node --import hooks/aidlc-x app.js",
    "node --loader hooks/aidlc-x app.js", "node --experimental-loader=hooks/aidlc-x", "bun --preload hooks/aidlc-x x", "bun -r script.ts x", "bun -r plain.sh x", "node -r", "bun --preload",
    "bun --cwd .claude x", "bun --config hooks/aidlc-x.ts", "bun --env-file hooks/aidlc-x.ts x", "bun --conditions x hooks/aidlc-x.ts", "bun run hooks/aidlc-x.ts", "tsx run hooks/aidlc-x.ts",
    "node run hooks/aidlc-x.ts", "deno run -A hooks/aidlc-x.ts", "bun run", "bun -- hooks/aidlc-x.ts", "bun -- -x", "bun --", "bun --help hooks/aidlc-x.ts", "bun -e", "node -e",
    "bun -e \"require('hooks/aidlc-x')\"", "bun -e\"require('hooks/aidlc-x')\"", "bun --eval=\"require('hooks/aidlc-x')\"", "bun --eval \"require('hooks/aidlc-x')\"", "bun --eval=",
    "bun -p \"require('hooks/aidlc-x')\"", "bun --print \"require('hooks/aidlc-x')\"", "node -pe \"require('hooks/aidlc-x')\"", "node -ie \"require('hooks/aidlc-x')\"",
    "node -ipe \"require('hooks/aidlc-x')\"", "node -iep \"require('hooks/aidlc-x')\"", "node -e \"x\" -e \"require('hooks/aidlc-x')\"", "node -e \"x\" hooks/aidlc-x.ts",
    "bun -e \"echo .aidlc-sessions\"", "node -e \"'.aidlc-sessions'\"", "node -e \"'x/.aidlc-sessions/y'\"", "node -e \"'.aidlc-sessionsx'\"", "node -e '.aidlc-sessions'",
    "python -c \"import os; os.system('bun hooks/aidlc-x.ts')\"", "python3 -c 'import os; os.system(\"bun hooks/aidlc-x.ts\")'", "python -c", "python3.12 -c \"print(1)\"", "python2 -ic x",
    "python -uc \"import os; os.system('bun hooks/aidlc-x.ts')\"", "python -c\"import os; os.system('bun hooks/aidlc-x.ts')\"", "python x.py", "python -u x.py", "python -m x", "python -- x.py",
    "python -c x x.py", "python -c x -- x.py", "python -- x.py extra", "python script.ts", "python run.sh", "python tool", "python plain.sh", "python missing.py",
    "sh -c 'bun hooks/aidlc-x.ts'", "sh -c \"bun hooks/aidlc-x.ts\"", "sh -c", "sh -xc 'bun hooks/aidlc-x.ts'", "sh -cx 'bun hooks/aidlc-x.ts'", "bash -lc \"AIDLC_UNATTENDED=1 x\"",
    "sh -c '.aidlc-sessions'", "sh -c 'echo .aidlc-sessions'", "sh run.sh", "sh -x run.sh", "sh -e run.sh arg", "sh plain.sh", "sh -- run.sh", "sh - < run.sh", "bash /dev/stdin < run.sh",
    "bash -c 'bash -c \"bun hooks/aidlc-x.ts\"'", "sh -c \"sh -c 'sh -c \\\"bun hooks/aidlc-x.ts\\\"'\"", "./run.sh", "./plain.sh", "./tool", "./exec-plain", "./missing.sh", "run.sh",
    "bash run.sh", "bun script.ts", "bun src/x.ts", "node a.js", "./script.ts", "bun missing.ts", "sudo bun hooks/aidlc-x.ts", "sudo -u x bun hooks/aidlc-x.ts", "env -i bun hooks/aidlc-x.ts",
    "env -S 'bun hooks/aidlc-x.ts'", "xargs bun hooks/aidlc-x.ts", "timeout 5 aidlc engine hook", "nohup aidlc engine adapter", "command bun hooks/aidlc-x.ts", "exec bun hooks/aidlc-x.ts",
    "if true; then bun hooks/aidlc-x.ts; fi", "for x in a; do aidlc engine hook; done", "{ bun hooks/aidlc-x.ts; }", "( aidlc engine hook )", "! bun hooks/aidlc-x.ts", "x || bun hooks/aidlc-x.ts",
    "x | bun hooks/aidlc-x.ts", "x & bun hooks/aidlc-x.ts", "x\nbun hooks/aidlc-x.ts", "echo 'bun hooks/aidlc-x.ts'", "echo \"bun hooks/aidlc-x.ts\"", "echo bun hooks/aidlc-x.ts",
    "cat hooks/aidlc-x.ts", "git add hooks/aidlc-x.ts", "echo $(cat <<EOF\nbun hooks/aidlc-x.ts\nEOF\n)", "x=`bun hooks/aidlc-x.ts`", "x=\"`bun hooks/aidlc-x.ts`\"", "x='`bun hooks/aidlc-x.ts`'",
    "echo \"$(aidlc engine hook)\"", "echo '$(aidlc engine hook)'", "echo $(echo $(bun hooks/aidlc-x.ts))", "echo $(echo \"$(bun hooks/aidlc-x.ts)\")", "echo `echo \\`bun hooks/aidlc-x.ts\\``",
    "echo $( bun hooks/aidlc-x.ts", "echo `bun hooks/aidlc-x.ts", "echo ${ bun hooks/aidlc-x.ts; }", "echo ${| bun hooks/aidlc-x.ts; }", "echo ${x} bun hooks/aidlc-x.ts", "echo {bun,hooks/aidlc-x.ts}",
]
RI_FIXED_BASH += [
    "bash -s <<EOF\nbun hooks/aidlc-x.ts\nEOF", "bash -es <<EOF\nbun hooks/aidlc-x.ts\nEOF", "bash -s x <<EOF\nbun hooks/aidlc-x.ts\nEOF", "bash x <<EOF\nbun hooks/aidlc-x.ts\nEOF",
    "bash -- <<EOF\nbun hooks/aidlc-x.ts\nEOF", "bash -- - <<EOF\nbun hooks/aidlc-x.ts\nEOF", "bash -- x <<EOF\nbun hooks/aidlc-x.ts\nEOF", "bash -c x <<EOF\nbun hooks/aidlc-x.ts\nEOF",
    "bash --version <<EOF\nbun hooks/aidlc-x.ts\nEOF", "bash /dev/stdin <<EOF\nbun hooks/aidlc-x.ts\nEOF", "bash - <<EOF\nbun hooks/aidlc-x.ts\nEOF", "bun - <<EOF\nrequire('hooks/aidlc-x')\nEOF",
    "bun run - <<EOF\nrequire('hooks/aidlc-x')\nEOF", "node --check <<EOF\nrequire('hooks/aidlc-x')\nEOF", "node -r x <<EOF\nrequire('hooks/aidlc-x')\nEOF", "node -rx <<EOF\nrequire('hooks/aidlc-x')\nEOF",
    "node --cwd x <<EOF\nrequire('hooks/aidlc-x')\nEOF", "python - <<EOF\nimport os; os.system('bun hooks/aidlc-x.ts')\nEOF", "python3 -u - <<EOF\nimport os; os.system('bun hooks/aidlc-x.ts')\nEOF",
    "python3 -c x <<EOF\nimport os; os.system('bun hooks/aidlc-x.ts')\nEOF", "cat <<EOF > config\nbun hooks/aidlc-x.ts\nEOF", "cat <<EOF > tool\n#!/bin/sh\nAIDLC_UNATTENDED=1 x\nEOF",
    "cat <<EOF > newfile.js\n#!/bin/sh\nAIDLC_UNATTENDED=1 x\nEOF", "cat <<EOF > tool\nAIDLC_UNATTENDED=1 x\nEOF", "cat <<EOF > exec-plain\nAIDLC_UNATTENDED=1 x\nEOF", "cat <<EOF > x.py\nAIDLC_UNATTENDED=1 x\nEOF",
    "cat <<EOF > a.js\nAIDLC_UNATTENDED=1 x\nEOF", "cat <<EOF > run.sh\nAIDLC_UNATTENDED=1 x\nEOF", "echo x # bun hooks/aidlc-x.ts; bun hooks/aidlc-x.ts", "echo x #bun hooks/aidlc-x.ts; bun hooks/aidlc-x.ts",
    "echo x '#' bun; bun hooks/aidlc-x.ts", "echo x#; bun hooks/aidlc-x.ts", "echo x; # bun\nbun hooks/aidlc-x.ts", "pushd -n x; echo >> audit/y", "pushd x; echo >> audit/y", "cd -; echo >> audit/y",
    "cd ~; echo >> audit/y", "cd \"$X\"; echo >> audit/y", "B=x; A='$B'; echo >> $A/audit/y", "B=x; A=\"$B\"; echo >> $A/audit/y", "B=x; A=$B; echo >> $A/audit/y", "A='x'y; echo >> $A/audit/z",
    "A=\"x\"'y'; echo >> $A/audit/z", "A='x; echo >> $A/audit/z", "A=\"x; echo >> $A/audit/z", "A=x\\ y; echo >> $A/audit/z", "A=x; A+=\"$B\"; echo >> $A/audit/z", "A=x; A+='$'; echo >> $A/audit/z",
    "echo >> .other/hooks/x", "echo >> real-other/hooks/x", "echo >> other-tools/tools/aidlc.ts", "rm -rf .other", "bun .other/tools/aidlc-lib.ts", "echo >> .cursor/hooks/x",
    "echo >> link/x", "echo >> link/new/deeper", "cat link/x", "rm link/x", "echo >> rec/audit/x", "cd rec; echo >> audit/x", "cd link; echo >> x", "cd link && echo >> x", "cd rec/audit; echo >> x",
    "cd " + RECORD + "/audit; echo >> x", "cd " + RECORD + "; cd audit; echo >> x", "cd " + RECORD + "; cd \"$(printf %s audit)\"; echo >> x", "cd aidlc; cd spaces/default/intents/x; echo >> audit/y",
    "cd aidlc/spaces/default/intents/x/audit/..; echo >> audit/y", "cd src; cd ../" + RECORD + "; echo >> audit/y", "cd ..; echo >> " + RECORD + "/audit/y", "cd /; echo >> {P}/" + RECORD + "/audit/y",
    "echo >> /" + RECORD + "/audit/y", "echo >> /tmp/x/" + RECORD + "/audit/y", "echo >> ./" + RECORD + "/audit/y", "echo >> " + RECORD + "/audit", "echo >> " + RECORD + "/audit/", "echo >> " + RECORD + "/auditx/y",
    "echo >> aidlc/spaces/default/intents/audit/y", "echo >> aidlc/spaces/default/intents/audit", "echo >> aidlc/spaces/default/audit/y", "echo >> aidlc/spaces/x/y/intents/z/audit/w", "echo >> aidlc/spaces/x/intents/z/w/audit/v",
    "echo >> aidlc//spaces/x/intents/z/audit/w", "echo >> AIDLC/spaces/x/intents/z/AUDIT/w", "echo >> aidlc\\spaces\\x\\intents\\z\\audit\\w", "echo >> 'aidlc\\spaces\\x\\intents\\z\\audit\\w'",
]
for command in RI_FIXED_BASH:
    ri_case({"tool_name": "Bash", "tool_input": {"command": command}}, files=dict(RI_STANDARD_FILES), links=[["link", RECORD + "/audit"], ["cl", ".claude"], ["rec", RECORD], ["s", ".aidlc-sessions"], ["h", ".claude/hooks"], ["loop", "loop"], [".other", "real-other"]],
            exec_=["tool", "exec-plain"], cwd="{P}")
for command in ["echo >> .other/hooks/x", "echo >> real-other/hooks/x", "rm -rf .other", "echo >> .other/x.md", "echo >> other-tools/tools/aidlc.ts", "rm other-tools/tools", "echo >> .claude/hooks/x"]:
    for env in [{"AIDLC_HARNESS_DIR": ".other"}, {"AIDLC_HARNESS_DIR": "other-tools"}, {}]:
        ri_case({"tool_name": "Bash", "tool_input": {"command": command}}, files=dict(RI_STANDARD_FILES), links=[[".other", "real-other"]], exec_=[], env=env, cwd="{P}")
        ri_case({"tool_name": "Bash", "tool_input": {"command": command}}, files=dict(RI_STANDARD_FILES), links=[], exec_=[], env=env, cwd="{P}", project="other-tools")
for command in RI_FIXED_BASH[::5]:
    ri_case({"tool_name": "Bash", "tool_input": {"command": command}}, files=dict(RI_STANDARD_FILES), links=[], exec_=["tool", "exec-plain"], cwd="{P}/" + RECORD)

# --- reviewer_scope: the per-unit reviewer read-scope matcher -----------------
#
# A dispatch for one unit, a record root at `/p/r` (the parent of the dispatch
# file's engine directory), a cwd, and one tool call whose strings may name the
# current unit, a sibling, a wildcard over units, an exempt file, or nothing in
# construction/ at all; and shell commands with the searches the matcher reads.
rs = random.Random(seed * 7919 + 9)
RS_ROOT = "/p/r"
RS_UNITS = ["U01", "u02", "auth", "Payment-Api", "x y", "ünit"]
RS_FILES = ["design.md", "functional-design/functional-spec.md", "nfr-design/security-design.md", "notes/README.md", "a.txt", "src/main.rs", ""]
RS_CONSTRUCTION = ["construction", "Construction", "CONSTRUCTION", "constr*", "c?nstruction", "con[s]truction", "{construction,other}", "*", "c*", "constructio", "construction2", "x*y"]
RS_OUTSIDE = ["README.md", "ideation/intent-capture/intent-statement.md", "inception/domain-design/components.md", "src/lib.rs", ".", "..", "../..", "/etc/hosts", "/p", "/p/r", "/p/r/ideation", "/p/q/construction/U01/design.md", "C:\\p\\construction\\U01\\d.md", "construction.md", "constructions/U01/x", "a/construction", "~/construction/U01"]


def rs_unit_segment(unit):
    return rs.choice([unit, unit, unit.lower(), unit.upper(), rs.choice(RS_UNITS), rs.choice(RS_UNITS), "*", "U0?", "{U01,u02}", "[Uu]01", "", ".", "..", "sib ling"])


def rs_construction_path(unit, absolute=None):
    comps = []
    if absolute is None:
        absolute = rs.random() < 0.4
    head = rs.choice([RS_ROOT, RS_ROOT + "/", "/p/r/./", "/p/r/x/..", "/other/construction/..", "/p"]) if absolute else rs.choice(["", "./", "../", "../../", "r/", "./x/../", "a/../"])
    comps.append(rs.choice(RS_CONSTRUCTION))
    seg = rs_unit_segment(unit)
    if seg != "":
        comps.append(seg)
    if rs.random() < 0.3:
        comps.append(rs.choice(["..", ".", rs.choice(RS_UNITS), "../" + rs.choice(RS_UNITS)]))
    tail = rs.choice(RS_FILES)
    if tail:
        comps.append(tail)
    text = (head.rstrip("/") + "/" if head else "") + "/".join(comps)
    if rs.random() < 0.1:
        text = text.replace("/", "\\", rs.randrange(1, 3))
    if rs.random() < 0.05:
        text = text + "/"
    return text


def rs_path(unit):
    r = rs.random()
    if r < 0.6:
        return rs_construction_path(unit)
    if r < 0.9:
        return rs.choice(RS_OUTSIDE)
    return rs.choice(["", " ", "/", "//", "construction", "construction/", "/construction", "./construction/./", "CONSTRUCTION/"])


def rs_exempt(unit):
    out = []
    for _ in range(rs.choice([0, 0, 1, 1, 2, 3])):
        out.append(rs.choice([rs_construction_path(unit), rs_construction_path(unit, absolute=True), rs.choice(RS_OUTSIDE), "construction/" + rs.choice(RS_UNITS) + "/" + rs.choice(RS_FILES[:3])]))
    return out


RS_SEARCH_OPTS = ["-r", "-R", "-rn", "-n", "-i", "-e", "-f", "-f-", "-e-x", "--recursive", "--include=*.md", "--include", "-d", "-d recurse", "--directories=recurse", "--", "-nefoo", "-A", "-A2", "-A 2", "--max-count=1", "--exclude-from", "--exclude-from=x"]
RS_RG_OPTS = ["-n", "-i", "-g", "-g=*.md", "--glob", "--glob=construction/U01/**", "--iglob", "--files", "-f", "-f-", "-e", "-e x", "--type", "-tmd", "-t md", "--", "-rn", "--max-depth", "--max-depth=1", "-g", "--replace", "-rfoo"]


def rs_command(unit):
    def operand():
        return rs.choice([rs_path(unit), rs_path(unit), rs.choice(RS_UNITS), "pattern", "'quoted construction/" + rs.choice(RS_UNITS) + "/x'", '"dq construction/' + rs.choice(RS_UNITS) + '"', "-", "construction\\/" + rs.choice(RS_UNITS), "$VAR/construction/U01", "x\\ y"])
    def segment():
        kind = rs.choice(["grep", "egrep", "fgrep", "rg", "ripgrep", "find", "ls", "cat", "less", "head", "tail", "cd", "cd", "echo", "sed", "git", "/usr/bin/grep", "./rg", "awk", "python3"])
        words = [kind]
        n = rs.choice([0, 1, 1, 2, 2, 3, 4])
        for _ in range(n):
            if kind in ("grep", "egrep", "fgrep", "/usr/bin/grep") and rs.random() < 0.5:
                words.append(rs.choice(RS_SEARCH_OPTS))
            elif kind in ("rg", "ripgrep", "./rg") and rs.random() < 0.5:
                words.append(rs.choice(RS_RG_OPTS))
            elif kind == "find" and rs.random() < 0.3:
                words.append(rs.choice(["-name", "x", "!", "(", ")", "-type", "f", "-print"]))
            elif rs.random() < 0.2:
                words.append(rs.choice(["-l", "-la", "--help", "-", "--", "-n1"]))
            else:
                words.append(operand())
        return " ".join(words)
    parts = [segment() for _ in range(rs.choice([1, 1, 1, 2, 2, 3]))]
    out = parts[0]
    for part in parts[1:]:
        out += rs.choice([" | ", " || ", " && ", "; ", " & ", "|", ";", " ( ", " ) "]) + part
    if rs.random() < 0.05:
        out = char_fuzz(out, rs.randrange(1, 3))
    return out


def rs_tool_input(tool, unit):
    if tool == "Bash":
        return rs.choice([{"command": rs_command(unit)}, {"command": rs_command(unit)}, {"command": ""}, {"command": 3}, {}])
    if tool in ("Read", "NotebookRead", "Edit", "MultiEdit", "Write", "NotebookEdit"):
        value = {}
        for key in rs.sample(["file_path", "notebook_path", "path", "paths", "content"], rs.randrange(0, 3)):
            value[key] = rs.choice([rs_path(unit), 3, None, ""]) if key != "paths" else rs.choice([[rs_path(unit), rs_path(unit)], [rs_path(unit), 2, ""], "nope", []])
        return value
    if tool == "LS":
        return rs.choice([{"path": rs_path(unit)}, {}, {"path": ""}])
    if tool == "Glob":
        return {k: v for k, v in [("pattern", rs.choice([rs_path(unit), "**/*.md", "construction/" + unit + "/**", "construction/*/design.md", "*.rs", ""])), ("path", rs.choice([rs_path(unit), None, None, RS_ROOT, "/p/r/construction/" + unit]))] if v is not None}
    if tool == "Grep":
        return {k: v for k, v in [("pattern", rs.choice(["construction/U01", "TODO", "x"])), ("glob", rs.choice([None, None, rs_path(unit), "*.md", "construction/" + unit + "/**"])), ("path", rs.choice([rs_path(unit), None, None, RS_ROOT, "/p/r/construction/" + unit, "."]))] if v is not None}
    return {"x": 1}


RS_CWDS = [RS_ROOT, RS_ROOT + "/construction/{U}", RS_ROOT + "/construction/U01", "/p", "/tmp/x", "/", None, "", "/p/r/ideation"]
for _ in range(scale * 120):
    unit = rs.choice(RS_UNITS)
    tool = rs.choice(["Bash", "Bash", "Read", "NotebookRead", "Edit", "MultiEdit", "Write", "NotebookEdit", "LS", "Glob", "Glob", "Grep", "Grep", "Task", "bash"])
    cwd = rs.choice(RS_CWDS)
    if cwd is not None:
        cwd = cwd.replace("{U}", unit)
    context = {}
    if rs.random() < 0.9:
        context["record_root"] = rs.choice([RS_ROOT, RS_ROOT, RS_ROOT + "/", "/p/r/./", "/p/r/x/.."])
    if cwd is not None:
        context["cwd"] = cwd
    add("reviewer_scope", tool=tool, input=rs_tool_input(tool, unit), unit=unit, exempt=rs_exempt(unit), context=context)

# The cases the random draw rarely makes: an exempt file read lexically and resolved, an exempt entry
# with a wildcard, a wildcard segment in a glob that would otherwise constrain the search, escapes
# inside double quotes, `||` against `|`, the pipe feeding a filter, and the search options' values.
RS_FIXED = [
    ("Read", {"file_path": "construction/U02/design.md"}, ["construction/U02/design.md"]),
    ("Read", {"file_path": "/p/r/construction/U02/design.md"}, ["construction/U02/design.md"]),
    ("Read", {"file_path": "/p/r/construction/U02/design.md"}, ["/p/r/construction/U02/design.md"]),
    ("Read", {"file_path": "../U02/design.md"}, ["/p/r/construction/U02/design.md"]),
    ("Read", {"file_path": "construction/U02/other.md"}, ["construction/U02/design.md"]),
    ("Read", {"file_path": "construction/*/design.md"}, ["construction/*/design.md"]),
    ("Read", {"file_path": "construction/U0?/design.md"}, ["construction/U0?/design.md"]),
    ("LS", {"path": "/p/r/construction/U02"}, ["/p/r/construction/U02/design.md"]),
    ("Glob", {"pattern": "construction/U01/**/*.md"}, []),
    ("Glob", {"pattern": "construction/U0?/**/*.md"}, []),
    ("Glob", {"pattern": "construction/U02/**/*.md"}, ["construction/U02/x.md"]),
    ("Grep", {"pattern": "x", "glob": "construction/U01/*.md"}, []),
    ("Grep", {"pattern": "x", "glob": "construction/*/*.md"}, []),
    ("Bash", {"command": 'cat "construction/U\\"02/design.md"'}, []),
    ("Bash", {"command": 'cat "construction/U02\\/design.md"'}, []),
    ("Bash", {"command": "cat 'construction/U02/design.md'"}, []),
    ("Bash", {"command": "cat construction/U\\ 02/design.md"}, []),
    ("Bash", {"command": "cat x || grep -r TODO"}, []),
    ("Bash", {"command": "cat x || grep TODO"}, []),
    ("Bash", {"command": "cat x | grep -d skip TODO"}, []),
    ("Bash", {"command": "cat x | grep -d recurse TODO"}, []),
    ("Bash", {"command": "cd construction/U01 && cat ../U02/x"}, []),
    ("Bash", {"command": "cd construction/U01; cd ..; cat U02/x"}, []),
    ("Bash", {"command": "cd construction/U01/sub && cat ../../U02/x"}, []),
    ("Bash", {"command": "cd construction/U01/sub; cd deeper; ls ../../../U02"}, []),
    ("Read", {"file_path": "../U02/design.md"}, ["../U02/design.md"]),
    ("Read", {"file_path": "/p/r/construction/U02/design.md"}, ["../U02/design.md"]),
    ("Grep", {"pattern": "x", "glob": "construction/*/x.md"}, ["construction/*/x.md"]),
    ("Glob", {"pattern": "construction/*/x.md"}, ["construction/*/x.md"]),
    ("Bash", {"command": "cat x | grep -r TODO"}, []),
    ("Bash", {"command": "cat x | grep TODO"}, []),
    ("Bash", {"command": "cat x | rg TODO"}, []),
    ("Bash", {"command": "cat x | rg --files"}, []),
    ("Bash", {"command": "cat x | rg -f - "}, []),
    ("Bash", {"command": "cat x | rg -f patterns.txt"}, []),
    ("Bash", {"command": "cat x && grep TODO"}, []),
    ("Bash", {"command": "cat x ; rg TODO"}, []),
    ("Bash", {"command": "grep -d recurse TODO"}, []),
    ("Bash", {"command": "grep -d skip TODO"}, []),
    ("Bash", {"command": "grep --directories=recurse TODO"}, []),
    ("Bash", {"command": "grep -e TODO -e FIXME"}, []),
    ("Bash", {"command": "grep -e TODO construction/U02"}, []),
    ("Bash", {"command": "grep TODO construction/U02"}, []),
    ("Bash", {"command": "grep -nefoo construction/U02"}, []),
    ("Bash", {"command": "grep -A2 TODO construction/U02"}, []),
    ("Bash", {"command": "grep -A 2 TODO construction/U02"}, []),
    ("Bash", {"command": "grep --include=*.md -r TODO"}, []),
    ("Bash", {"command": "grep --include *.md -r TODO construction/U02"}, []),
    ("Bash", {"command": "rg -g construction/U01/** TODO"}, []),
    ("Bash", {"command": "rg -g construction/U02/** TODO"}, []),
    ("Bash", {"command": "rg --glob=construction/U01/** TODO"}, []),
    ("Bash", {"command": "rg -gconstruction/U01/** TODO"}, []),
    ("Bash", {"command": "rg --files construction/U02"}, []),
    ("Bash", {"command": "rg --files"}, []),
    ("Bash", {"command": "rg -tmd TODO"}, []),
    ("Bash", {"command": "rg -t md TODO construction/U02"}, []),
    ("Bash", {"command": "rg TODO -- construction/U02"}, []),
    ("Bash", {"command": "rg -- -x construction/U02"}, []),
    ("Bash", {"command": "find"}, []),
    ("Bash", {"command": "find -name x"}, []),
    ("Bash", {"command": "find construction/U02 -name x"}, []),
    ("Bash", {"command": "find . -name x"}, []),
    ("Bash", {"command": "ls"}, []),
    ("Bash", {"command": "ls -la"}, []),
    ("Bash", {"command": "ls construction/U02"}, []),
    ("Bash", {"command": "cd construction/U02 && ls"}, []),
    ("Bash", {"command": "cd - && cat construction/U02/x"}, []),
    ("Bash", {"command": "cd -P construction/U02 && ls"}, []),
    ("Bash", {"command": "cd .. && ls construction"}, []),
    ("Bash", {"command": "cd ../U02 && ls"}, []),
    ("Bash", {"command": "echo construction/U02/x"}, []),
    ("Bash", {"command": "echo .. ; echo -"}, []),
    ("Bash", {"command": "git log -- construction/U02"}, []),
    ("Bash", {"command": "sed -n 1p construction/U02/x"}, []),
    ("Bash", {"command": "cat $(ls construction/U02)"}, []),
    ("Bash", {"command": "( cat construction/U02/x )"}, []),
    ("Bash", {"command": "cat construction/U02/x & ls"}, []),
    ("Bash", {"command": "cat CONSTRUCTION/u02/X"}, []),
    ("Bash", {"command": "cat constr*/U02/x"}, []),
    ("Bash", {"command": "cat c*/U02/x"}, []),
    ("Bash", {"command": "cat */U02/x"}, []),
    ("Bash", {"command": "cat x*y/U02/x"}, []),
    ("Bash", {"command": "cat con[s]truction/U02/x"}, []),
    ("Bash", {"command": "cat {construction,other}/U02/x"}, []),
    ("Bash", {"command": "cat construction/../U02/x"}, []),
    ("Bash", {"command": "cat construction/U01/../U02/x"}, []),
    ("Bash", {"command": "cat construction/U01/./x"}, []),
    ("Bash", {"command": "cat construction//U02/x"}, []),
    ("Bash", {"command": "cat construction/"}, []),
    ("Bash", {"command": "cat construction"}, []),
    ("Bash", {"command": "cat /p/r/construction"}, []),
    ("Bash", {"command": "ls /p/r"}, []),
    ("Bash", {"command": "ls /p"}, []),
    ("Bash", {"command": "cat /p/r/x"}, []),
]
for tool, value, exempt in RS_FIXED:
    for cwd in ("/p/r/construction/U01", "/p/r", "/tmp/x"):
        add("reviewer_scope", tool=tool, input=value, unit="U01", exempt=exempt, context={"record_root": RS_ROOT, "cwd": cwd})
    add("reviewer_scope", tool=tool, input=value, unit="U01", exempt=exempt, context={})

RS_RECORDS = ['{"reviewer": "aidlc-architecture-reviewer-agent", "stage": "nfr-design", "unit": "U01", "exempt": ["a", "b"]}',
              '{"reviewer": "r", "stage": "", "unit": "u", "exempt": []}', '{"reviewer": "", "stage": "s", "unit": "u", "exempt": []}',
              '{"reviewer": "r", "stage": "s", "unit": "", "exempt": []}', '{"reviewer": "r", "unit": "u", "exempt": []}', '{"reviewer": "r", "stage": "s", "unit": "u"}',
              '{"reviewer": "r", "stage": "s", "unit": "u", "exempt": ["a", 1]}', '{"reviewer": "r", "stage": "s", "unit": "u", "exempt": "a"}', '{"reviewer": 1, "stage": "s", "unit": "u", "exempt": []}',
              '[]', 'null', '"x"', '{nope', '', '{"reviewer": "r", "stage": "s", "unit": "u", "exempt": [], "extra": true}', '{"reviewer": "r", "stage": 2, "unit": "u", "exempt": []}']
for raw in RS_RECORDS:
    add("reviewer_dispatch", raw=raw)


# The state-transition guard's three readers of a shell command: direct
# transitions, lifecycle boundaries and what a delegated agent may not run.
st = random.Random(seed * 7919 + 13)
ST_STATE_VERBS = ["set", "checkbox", "advance", "finalize", "complete-workflow", "gate-start", "approve", "reject", "revise", "skip", "park",
                  "refresh-unit-progress", "fold-unit-merge", "set-skeleton-stance", "sync-unit-scope-stage", "unit", "acknowledge-compaction",
                  "reuse-artifact", "practices-event", "fork", "merge", "unpark", "get", "count", "resume", "lookup", "init", "set-status", "status", "x"]
ST_SCRIPTS = {
    "aidlc-state.ts": ST_STATE_VERBS,
    "aidlc-orchestrate.ts": ["next", "continue", "report", "park", "wait", "team-board", "help"],
    "aidlc-jump.ts": ["execute", "reopen", "resolve", "x"],
    "aidlc-unit.ts": ["adopt", "claim", "release", "participate", "publish", "pin", "gate", "land", "status", "merge-status"],
    "aidlc-bolt.ts": ["start", "complete", "fail", "abort", "set-autonomy", "checkpoint", "swarm-checkpoint", "dispatch-event", "hold-merge", "release-merge", "list"],
    "aidlc-swarm.ts": ["prepare", "check", "finalize", "status"],
    "aidlc-log.ts": ["decision", "answer", "review", "link", "answers"],
    "aidlc-learnings.ts": ["persist", "surface"],
    "aidlc-runtime.ts": ["fragment-fork", "fragment-merge", "read", "summary", "compile"],
    "aidlc-testing-posture.ts": ["begin", "fingerprint", "resolve", "render", "verify", "brief", "reply", "restore", "x"],
    "aidlc-worktree.ts": ["purge", "list", "create"],
    "aidlc-audit.ts": ["audit-fork", "audit-merge", "history", "fork"],
    "aidlc-plugin.ts": ["sync", "list"],
    "aidlc-machine-config.ts": ["get", "set", ""],
    "aidlc-utility.ts": ["scope-change", "scope-save", "config-change", "recompose", "intent-create", "state-init", "space-create", "reclassify",
                         "select-plugins", "plugin-sync", "upgrade", "claim", "release", "participate", "set-status", "status", "help", "intent", "space"],
    "aidlc-graph.ts": ["topo", "export"],
    "other.ts": ["x"],
}
ST_DISPATCH_WORDS = ["next", "continue", "report", "park", "--resume", "--scope", "--claim", "--release", "scope-change", "scope-save", "config-change",
                     "compose", "recompose", "init", "scope change", "scope save", "scope x", "orchestrate next", "orchestrate wait", "intent create",
                     "intent list", "intent switch a", "intent switch", "intent switch a b", "intent archive a", "intent archive --json", "intent unarchive a",
                     "intent add-repo r", "intent rename a", "intent show", "intent help", "intent --json", "intent --all", "intent x", "intent x --flag",
                     "intent x y", "space create a", "space create", "space switch a", "space switch a b", "space list", "space x", "space-create a",
                     "space-create", "space-create a b", "jump execute", "jump resolve", "config set a b", "config global", "config get a",
                     "workspace reclassify", "workspace status", "plugin select", "plugin select a", "plugin select --json", "plugin sync", "plugin list",
                     "unit adopt", "unit status", "bolt start", "swarm prepare", "log decision", "learnings persist", "runtime fragment-fork",
                     "testing-posture begin", "testing-posture brief", "worktree purge", "audit fork", "audit history", "state approve", "state set x",
                     "state init", "state set-status", "state get", "state unpark", "status", "help", "", "x y"]
ST_GLOBALS = ["--json", "--quiet", "--no-color", "--yes", "--offline", "--verbose", "--project-dir /p", "--project-dir=/p", "--other"]
ST_NAMESPACE = ["", "", "engine ", "system ", "engines "]
ST_DISPATCHER = ["aidlc", "aidlc", "./aidlc", "/usr/local/bin/aidlc", "aidlc.exe", ".claude/bin/aidlc", "bun .claude/tools/aidlc.ts", "bun aidlc.ts",
                 "bun run .claude/tools/aidlc.ts", "bun --cwd /p aidlc.ts", "bun -e aidlc.ts", "bunx aidlc", "aidlcx", "xaidlc", 'bun "/p q/aidlc.ts"',
                 "bun '/p q/aidlc.ts'", "bun.exe aidlc.ts", "AIDLC", "bun .claude\\tools\\aidlc.ts", "/usr/bin/bun .claude/tools/aidlc.ts"]
ST_BUN = ["bun", "bun", "bun run", "bun --cwd /p", "bun -r x", "bun --preload x", "bun --define a=b", "bun --config=x", "bun --", "bun run --", "bun -e",
          "bun --print", "bun --quiet", "/usr/bin/bun", "bun.exe", "node", "", "bun --cwd"]
ST_SCRIPT_PATH = ["{s}", ".claude/tools/{s}", "./{s}", "/p/.claude/tools/{s}", '".claude/tools/{s}"', "'.claude/tools/{s}'", ".kiro\\tools\\{s}",
                  "~/x/{s}", "$HOME/{s}", "x{s}", "{s}x"]
ST_WRAP = ["", "", "", "time ", "time -p ", "time -f %e ", "time -o out ", "time -f ", "command ", "command -p ", "command -v ", "command -V ", "command -- ",
           "command -x ", "exec ", "exec -c ", "exec -a name ", "exec -l ", "exec -x ", "exec -a ", "exec -- ", "env ", "env -i ", "env -u X ", "env --unset=X ",
           "env -C /p ", "env -S ", "env -S 'bun x.ts' ", "env '-Sbun aidlc-state.ts approve' ", "env --split-string='aidlc next' ", "env -S '$X' ",
           "env -0 ", "env --null ", "env --help ", "env -P /p ", "env --debug ", "env --block-signal=INT ", "env -v ", "env -Z ", "env - ", "env X=1 ",
           "env X=1 -- ", "nice ", "nice -n 5 ", "nice -n ", "nice -n x ", "nice -5 ", "nice --5 ", "nice --adjustment=3 ", "nice --adjustment 3 ", "nice -x ",
           "nice --help ", "nohup ", "nohup -- ", "nohup -x ", "nohup --version ", "sudo ", "X=1 ", "X=1 Y=2 ", "if ", "! ", "while ", "then ", "do ", "else ",
           "elif ", "until ", "2>/dev/null ", "> out ", "< in ", "2>&1 ", ">out ", "2> err ", "command 2>/dev/null ", "env > out -i ", "exec 3>&1 ",
           "time -f %e 2>err ", "eval ", "eval -- ", "sh -c ", "bash -c ", "zsh -lc ", "bash -lc ", "sh -o x -c ", "bash --rcfile r -c ", "sh -x ", "dash -c ",
           "ksh -c ", "ash -c ", "fish -c ", "sh -- ", "sh -c -- ", "sh.exe -c ", "bash -ec ", "bash -", "exec sh -c ", "command exec env nice nohup "]
ST_SEP = [" && ", " || ", "; ", " | ", "\n", " & ", " |& ", " ;; ", " ( ", " { ", " } ", " ) ", "\n\n", " &&\n", " \\\n ", " # c\n"]
ST_PRELUDE = ["", "", "cd /p && ", "echo hi; ", "X=1\n", "X='aidlc next'\n", "CMD=aidlc\n", "CMD='bun aidlc-state.ts'\n", "V=\"a b\"\n", "X=1 Y=2\n",
              "X=$(echo 1)\n", "X=1; Y=2; ", "export X=1\n", "set -e\n", "ls # aidlc next\n", "# aidlc next\n", "true &&\n", "echo \"$(aidlc next)\"; ",
              "echo '$(aidlc next)'; ", "f() { aidlc next; }\n", "function f { aidlc next; }\n", "function f() { aidlc next; }\n", "f () {\n aidlc next\n}\n",
              "f() { echo x; }; ", "cat <<EOF\naidlc next\nEOF\n", "cat <<'EOF'\n$(aidlc next)\nEOF\n", "cat <<EOF\n$(aidlc next)\nEOF\n",
              "cat <<-EOF\n\t$(aidlc next)\n\tEOF\n", "cat <<\"EOF\"\n`aidlc next`\nEOF\n", "cat <<EOF\n`aidlc next`\nEOF\n", "cat <<EOF\naidlc next\n",
              "cat <<EOF <<X\na\nEOF\n$(aidlc next)\nX\n", "echo `aidlc next`; ", "echo $(aidlc next); ", "echo \"$(echo $(aidlc next))\"; ",
              "echo ${X:-aidlc next}; ", "echo ${X} ", "echo $X; "]
ST_TAIL = ["", "", " 2>&1", " | cat", " > out", " &", " #c", " \\\n x", " $(x)", " `x`", " \"$(aidlc next)\"", " '$(aidlc next)'", " ; aidlc next",
           " && bun aidlc-state.ts approve", " | aidlc approve", " || aidlc engine state set x", "\naidlc next", "\n$CMD next", "\n$X", "\n${CMD} next",
           "\n\"$CMD\" next", "\n$CMD", "\neval \"$X\"", "\neval $X", "\neval aidlc next", "\neval 'aidlc next'", "\neval -- aidlc next", "\neval \\$X",
           "\neval 'echo `x`'", "\nsh -c \"$X\"", "\nsh -c $X", "\nsh -c '$X'", "\nsh -c 'aidlc next'", "\nsh -c \"aidlc next\"", "\nsh -c 'aidlc $V'",
           "\nsh -c -- 'aidlc next'", "\nbash -lc 'aidlc next'", "\nbash -o pipefail -c 'aidlc next'", "\nsh -x 'aidlc next'", "\nsh 'aidlc next'",
           "\n$(aidlc next)", "\n`aidlc next`", "\n\"`aidlc next`\"", "\n'`aidlc next`'", "\nx=$(aidlc next)", "\nx=`aidlc next`", "\nx=$(aidlc next) y", "\n{ aidlc next; }",
           "\n( aidlc next )", "\nif aidlc next; then echo; fi", "\n! aidlc next", "\nfoo=bar aidlc next", "\n[ -f x ] && aidlc next", "\necho aidlc next",
           "\necho 'aidlc engine state approve'", "\n'aidlc' next", "\n\"aidlc\" 'next'", "\naidlc ne\\xt", "\naidlc $'next'", "\naidlc $\"next\"",
           "\naidlc\tnext", "\n aidlc  next ", "\naidlc\\\n next", "\naidlc next\\", "\n\u00e9 aidlc next", "\n\U0001F600 aidlc next", "\necho \U0001F600; aidlc next",
           "\necho '\U0001F600'; aidlc next", "\nf() { \U0001F600; }; aidlc next", "\nf() { x; }\U0001F600 aidlc next", "\ncat <<E\n\U0001F600\nE\naidlc next",
           "\necho \"\U0001F600 $(aidlc next)\"", "\necho \"\U0001F600\" $(aidlc next)", "\necho `\U0001F600` $(aidlc next)", "\necho \"\U0001F600\"; aidlc approve"]


def st_script_call():
    script = st.choice(list(ST_SCRIPTS))
    verb = st.choice(ST_SCRIPTS[script] + ["", "--help"])
    bun = st.choice(ST_BUN)
    path = st.choice(ST_SCRIPT_PATH).format(s=script)
    words = [bun, path]
    for _ in range(st.randrange(0, 3)):
        words.append(st.choice(ST_GLOBALS + ["--stage x", "--result done", "--", "a", "--k=v", "--stage"]))
    if st.random() < 0.3:
        words.insert(2 + st.randrange(0, len(words) - 1), verb)
    else:
        words.append(verb)
    return " ".join(w for w in words if w)


def st_dispatcher_call():
    head = st.choice(ST_DISPATCHER)
    words = [head]
    for _ in range(st.randrange(0, 3)):
        words.append(st.choice(ST_GLOBALS))
    words.append(st.choice(ST_NAMESPACE) + st.choice(ST_DISPATCH_WORDS))
    for _ in range(st.randrange(0, 2)):
        words.append(st.choice(ST_GLOBALS + ["--stage x", "--", "a"]))
    return " ".join(w for w in words if w)


def st_native_state():
    return f"{st.choice(ST_DISPATCHER)} {st.choice(['engine ', 'engine ', '', 'system '])}{st.choice(['state', 'orchestrate', 'jump', 'unit'])} {st.choice(ST_STATE_VERBS + ['report', 'park', 'execute', 'reopen', 'next'])}"


def st_head():
    return st.choice([st_script_call(), st_script_call(), st_dispatcher_call(), st_dispatcher_call(), st_native_state(), st_native_state(),
                      "echo x", "ls -la", "git status", "cat f", "$CMD next", "${CMD} next", "\"$CMD\" next", "$X", "$(cmd) next", "`cmd` next", "a$X next"])


def st_segment(depth=0):
    text = st.choice(ST_WRAP) + st_head()
    if depth < 3 and st.random() < 0.35:
        inner = st_segment(depth + 1)
        text = st.choice([
            f"{text}{st.choice(ST_SEP)}{inner}",
            f"sh -c '{inner}'",
            f"sh -c \"{inner}\"",
            f"eval '{inner}'",
            f"eval \"{inner}\"",
            f"eval {inner}",
            f"echo $({inner})",
            f"echo `{inner}`",
            f"echo \"$({inner})\"",
            f"x=$({inner})",
            f"cat <<EOF\n$({inner})\nEOF",
            f"cat <<'EOF'\n$({inner})\nEOF",
            f"f() {{ {inner}; }}",
            f"{{ {inner}; }}",
            f"( {inner} )",
            f"if {inner}; then :; fi",
            f"env -S '{inner}'",
            f"time {inner}",
            f"nohup {inner} &",
            f"X='{inner}'\n$X",
            f"X='{inner}'\nsh -c \"$X\"",
            f"X='{inner}'\neval \"$X\"",
            f"X=\"{inner}\"\n$X",
        ])
    return text


def st_command():
    text = f"{st.choice(ST_PRELUDE)}{st_segment()}{st.choice(ST_TAIL)}"
    if st.random() < 0.1:
        text = text.replace(" ", st.choice(["  ", "\t", " \\\n ", "\r\n", "\n"]), 1)
    if st.random() < 0.05:
        text = st.choice(["", " ", "\n", "\x00", "aidlc 'unterminated", 'aidlc "a$b', "aidlc `x", "aidlc $(x", "f() {", "cat <<EOF", "'", "\"", "\\", "eval", "sh -c", "env -S"])
    return text


ST_FIXED = [
    "bun aidlc-state.ts approve", "bun .claude/tools/aidlc-state.ts set x", "bun run aidlc-state.ts advance", "bun aidlc-state.ts get", "bun aidlc-state.ts unpark",
    "bun aidlc-state.ts --project-dir /p approve", "bun aidlc-state.ts --stage x approve", "bun aidlc-state.ts --stage=x approve", "bun aidlc-state.ts -- approve",
    "bun aidlc-state.ts --json approve", "bun aidlc-state.ts approve --stage x", "aidlc engine state approve", "aidlc engine state get", "./aidlc engine state set x",
    "aidlc.exe engine state skip", "aidlc state approve", "aidlc engine state  approve", "aidlc\tengine state approve", "/p/aidlc engine state park",
    '"/p q/aidlc" engine state park', "'/p/aidlc.exe' engine state park", "aidlc system state approve", "aidlc engine state approvex", "aidlc engine state approve-x",
    "aidlc engine orchestrate report", "aidlc engine orchestrate park", "aidlc engine jump execute", "aidlc engine jump reopen", "aidlc engine jump resolve",
    "bun aidlc-orchestrate.ts report --stage x", "bun aidlc-orchestrate.ts next", "bun aidlc-orchestrate.ts park", "bun aidlc-jump.ts execute", "bun aidlc-jump.ts reopen",
    "aidlc next", "aidlc report", "aidlc --resume", "aidlc --scope", "aidlc --claim", "aidlc --json next", "aidlc --quiet --yes park", "aidlc --other next",
    "aidlc -- next", "aidlc --project-dir /p next", "aidlc --project-dir=/p next", "aidlc engine next", "aidlc system continue", "aidlc engines next",
    "aidlc plugin select", "aidlc plugin select a", "aidlc plugin select --json", "aidlc plugin select -- a", "aidlc plugin sync", "aidlc plugin list",
    "aidlc intent create", "aidlc intent create --help", "aidlc intent switch a", "aidlc intent switch", "aidlc intent switch a b", "aidlc intent archive a",
    "aidlc intent archive --json", "aidlc intent archive", "aidlc intent unarchive a", "aidlc intent list", "aidlc intent add-repo r", "aidlc intent rename a",
    "aidlc intent show", "aidlc intent birth", "aidlc intent help", "aidlc intent -h", "aidlc intent --json", "aidlc intent --all", "aidlc intent x",
    "aidlc intent x --flag", "aidlc intent x y", "aidlc intent x y --flag", "aidlc space create a", "aidlc space create", "aidlc space create a b",
    "aidlc space switch a", "aidlc space switch", "aidlc space list", "aidlc space x", "aidlc space x y", "aidlc space --all", "aidlc space-create a",
    "aidlc space-create", "aidlc space-create a b", "aidlc space-create a --help", "aidlc intent", "aidlc space", "aidlc", "aidlc engine", "aidlc engine state",
    "aidlc state init", "aidlc state set-status", "aidlc state status", "aidlc state set-skeleton-stance", "aidlc engine state fold-unit-merge",
    "bun aidlc-utility.ts scope-change", "bun aidlc-utility.ts --json scope-change", "bun aidlc-utility.ts select-plugins", "bun aidlc-utility.ts select-plugins a",
    "bun aidlc-utility.ts select-plugins --json", "bun aidlc-utility.ts intent create", "bun aidlc-utility.ts intent switch a", "bun aidlc-utility.ts intent list",
    "bun aidlc-utility.ts space-create a", "bun aidlc-utility.ts space-create", "bun aidlc-utility.ts status", "bun aidlc-utility.ts --stage x upgrade",
    "bun aidlc-utility.ts --stage=x upgrade", "bun aidlc-utility.ts -- upgrade", "bun aidlc-utility.ts upgrade --stage x",
    "bun aidlc-testing-posture.ts begin", "bun aidlc-testing-posture.ts --stage x begin", "bun aidlc-testing-posture.ts brief begin", "bun aidlc-testing-posture.ts begin brief",
    "bun aidlc-testing-posture.ts x begin", "bun aidlc-testing-posture.ts fingerprint", "bun aidlc-testing-posture.ts resolve", "bun aidlc-testing-posture.ts",
    "bun aidlc-machine-config.ts", "bun aidlc-machine-config.ts get", "bun .claude/tools/aidlc-machine-config.ts set x y", "aidlc-machine-config.ts",
    "bun aidlc-audit.ts audit-fork", "bun aidlc-audit.ts fork", "aidlc audit fork", "aidlc audit audit-fork", "bun aidlc-plugin.ts sync", "aidlc plugin sync",
    "bun aidlc-worktree.ts purge", "bun aidlc-worktree.ts create", "bun aidlc-graph.ts topo", "bun other.ts approve", "bun aidlc-state.tsx approve",
    "bun xaidlc-state.ts approve", "bun ./aidlc-state.ts approve", "bun /p/.claude/tools/aidlc-state.ts approve", "bun .kiro\\tools\\aidlc-state.ts approve",
    "bun 'a b/aidlc-state.ts' approve", 'bun "a b/aidlc-state.ts" approve', "bun -e aidlc-state.ts approve", "bun --print aidlc-state.ts approve",
    "bun --cwd /p aidlc-state.ts approve", "bun --cwd=/p aidlc-state.ts approve", "bun --cwd aidlc-state.ts approve", "bun -r x aidlc-state.ts approve",
    "bun --quiet aidlc-state.ts approve", "bun -- aidlc-state.ts approve", "bun run -- aidlc-state.ts approve", "bun run --cwd /p aidlc-state.ts approve",
    "bun run", "bun", "bun.exe aidlc-state.ts approve", "/usr/bin/bun aidlc-state.ts approve", "node aidlc-state.ts approve", "aidlc-state.ts approve",
    "./aidlc-state.ts approve", "time bun aidlc-state.ts approve", "time -p aidlc next", "time -f %e aidlc next", "time -f %e -o out aidlc next", "time -f",
    "time -o", "time X=1 aidlc next", "time 2>err aidlc next", "command aidlc next", "command -p aidlc next", "command -pp aidlc next", "command -v aidlc next",
    "command -pv aidlc next", "command -V aidlc", "command -- aidlc next", "command -x aidlc next", "command 2>err aidlc next", "command X=1 aidlc next",
    "exec aidlc next", "exec -c aidlc next", "exec -cl aidlc next", "exec -a name aidlc next", "exec -a name", "exec -a", "exec -x aidlc next", "exec -- aidlc next",
    "exec 3>&1 aidlc next", "exec X=1 aidlc next", "env aidlc next", "env -i aidlc next", "env X=1 aidlc next", "env X=1 Y=2 aidlc next", "env -- aidlc next",
    "env -u X aidlc next", "env -uX aidlc next", "env --unset=X aidlc next", "env --unset X aidlc next", "env -u", "env -C /p aidlc next", "env -C/p aidlc next",
    "env --chdir=/p aidlc next", "env --chdir /p aidlc next", "env -P /p aidlc next", "env -P", "env -S aidlc next", "env -S 'aidlc next'", "env '-Saidlc next'",
    "env -S'aidlc next'", "env --split-string='aidlc next'", "env --split-string 'aidlc next'", "env -S 'aidlc $X'", "env -S 'aidlc `x`'", "env -S 'aidlc #x'",
    "env -S 'aidlc \\x'", "env -S", "env -S ''", "env -0 aidlc next", "env --null aidlc next", "env --help aidlc next", "env --version", "env --debug aidlc next",
    "env --list-signal-handling aidlc next", "env --block-signal=INT aidlc next", "env --block-signal aidlc next", "env --default-signal aidlc next",
    "env --ignore-signal aidlc next", "env --ignore-environment aidlc next", "env -v aidlc next", "env -iv aidlc next", "env -vi aidlc next", "env - aidlc next",
    "env -Z aidlc next", "env --other aidlc next", "env -i -- aidlc next", "env 2>err aidlc next", "env X=1 2>err -i aidlc next", "env", "env -i", "env X=1",
    "nice aidlc next", "nice -n 5 aidlc next", "nice -n -5 aidlc next", "nice -n +5 aidlc next", "nice -n x aidlc next", "nice -n", "nice -n5 aidlc next",
    "nice -5 aidlc next", "nice --5 aidlc next", "nice -n+5 aidlc next", "nice --adjustment=3 aidlc next", "nice --adjustment 3 aidlc next", "nice --adjustment aidlc next",
    "nice -x aidlc next", "nice --help", "nice --version aidlc next", "nice -- aidlc next", "nice 2>err aidlc next", "nice", "nohup aidlc next", "nohup -- aidlc next",
    "nohup -x aidlc next", "nohup --help", "nohup --version", "nohup", "nohup aidlc next &", "nohup bun aidlc-state.ts approve > /dev/null 2>&1 &",
    "sudo aidlc next", "sudo -u x aidlc next", "X=1 aidlc next", "X=1 Y=2 aidlc next", "X=1 2>err aidlc next", "2>err X=1 aidlc next", "if aidlc next; then :; fi",
    "! aidlc next", "while aidlc next; do :; done", "until aidlc next; do :; done", "if true; then aidlc next; fi", "if true; then\n aidlc next\nfi",
    "if true\nthen aidlc next\nfi", "else aidlc next", "elif aidlc next", "do aidlc next", "> out aidlc next", "< in aidlc next", ">out aidlc next", "2>&1 aidlc next",
    "2> err aidlc next", ">> out aidlc next", "<<< x aidlc next", "<> f aidlc next", "3>&- aidlc next", "&> out aidlc next", "X=1 > out Y=2 aidlc next",
    "eval aidlc next", "eval 'aidlc next'", "eval \"aidlc next\"", "eval -- aidlc next", "eval -- 'aidlc next'", "eval", "eval ''", "eval $X", "eval \"$X\"",
    "eval '$X'", "eval \\$X", "eval 'echo `x`'", "eval 'echo \\x'", "eval \"echo \\$x\"", "eval 'echo x'", "eval echo x", "X='aidlc next'\neval \"$X\"",
    "X='aidlc next'\neval $X", "X='aidlc next'; eval \"$X\"", "X='aidlc next' && eval \"$X\"", "X='aidlc next' eval \"$X\"", "X=\"aidlc next\"\neval \"$X\"",
    "X='aidlc'\neval \"$X next\"", "X='echo'\neval \"$X\"", "X='ls'\neval \"$X\"", "eval \"$(aidlc next)\"", "eval `aidlc next`", "eval \"$X\" 2>&1",
    "sh -c 'aidlc next'", "sh -c \"aidlc next\"", "sh -c aidlc next", "bash -c 'aidlc next'", "zsh -c 'aidlc next'", "ksh -c 'aidlc next'", "ash -c 'aidlc next'",
    "dash -c 'aidlc next'", "fish -c 'aidlc next'", "sh.exe -c 'aidlc next'", "bash.exe -c 'aidlc next'", "/bin/sh -c 'aidlc next'", "sh -lc 'aidlc next'",
    "sh -cl 'aidlc next'", "bash -ec 'aidlc next'", "bash -xc 'aidlc next'", "sh -c -- 'aidlc next'", "sh -- -c 'aidlc next'", "sh -x 'aidlc next'", "sh 'aidlc next'",
    "sh -o pipefail -c 'aidlc next'", "sh -o -c 'aidlc next'", "bash --rcfile r -c 'aidlc next'", "bash --init-file r -c 'aidlc next'", "bash --norc -c 'aidlc next'",
    "bash -O x -c 'aidlc next'", "bash +O x -c 'aidlc next'", "bash +o x -c 'aidlc next'", "sh -c", "sh -c ''", "sh -c $X", "sh -c \"$X\"", "sh -c '$X'",
    "sh -c 'aidlc $X'", "sh -c \"aidlc $X\"", "sh -c 'echo $X'", "sh -c 'aidlc \\$X'", "X='aidlc next'\nsh -c \"$X\"", "X='aidlc next'\nsh -c $X",
    "X='aidlc next'\nsh -c '$X'", "X='aidlc next'; sh -c \"$X\"", "X='ls'\nsh -c \"$X\"", "X='echo $Y'\nsh -c \"$X\"", "X='aidlc next'\nsh -c \"${X}\"",
    "X='aidlc next'\nsh -c \"$X\" 2>&1", "sh -c 'sh -c \"aidlc next\"'", "sh -c \"sh -c 'aidlc next'\"", "sh -c 'echo `aidlc next`'", "sh -c 'echo $(aidlc next)'",
    "sh -c 'f() { aidlc next; }'", "sh -c 'cat <<E\naidlc next\nE'", "sh -c 'X=1 aidlc next'", "sh -c 'env aidlc next'", "sh -c 'eval aidlc next'",
    "sh -c 'eval \"$X\"'", "echo x | sh", "echo 'aidlc next' | sh", "sh < script", "sh script", "sh -c 'echo' 'aidlc next'", "sh -c 'echo' aidlc next",
    "$CMD next", "${CMD} next", "\"$CMD\" next", "'$CMD' next", "$CMD", "$X", "${X}", "$1 next", "$CMD$X next", "a$X next", "$(cmd) next", "`cmd` next",
    "CMD=aidlc\n$CMD next", "CMD=aidlc; $CMD next", "CMD=aidlc && $CMD next", "CMD=aidlc || $CMD next", "CMD=aidlc | $CMD next", "CMD=aidlc\n${CMD} next",
    "CMD=aidlc\n\"$CMD\" next", "CMD='aidlc'\n$CMD next", "CMD=\"aidlc\"\n$CMD next", "CMD='bun aidlc-state.ts'\n$CMD approve", "CMD='bun aidlc-state.ts'\n\"$CMD\" approve",
    "CMD=''\n$CMD next", "CMD='a b'\n$CMD next", "CMD=aidlc X=1\n$CMD next", "CMD=aidlc echo\n$CMD next", "CMD=aidlc\nCMD=ls\n$CMD next", "CMD=ls\nCMD=aidlc\n$CMD next",
    "CMD=aidlc\n$CMD2 next", "CMD=aidlc\n$CMD $X", "CMD=aidlc\n$CMD next $X", "CMD=$(x)\n$CMD next", "CMD=`x`\n$CMD next", "CMD=$X\n$CMD next",
    "CMD=aidlc\nX=1 $CMD next", "CMD=aidlc\nenv $CMD next", "CMD=aidlc\ntime $CMD next", "CMD=aidlc\neval $CMD next", "CMD=aidlc\nsh -c \"$CMD next\"",
    "CMD=aidlc\necho $($CMD next)", "CMD=aidlc\nf() { $CMD next; }", "CMD=aidlc\n{ $CMD next; }", "CMD=aidlc\n( $CMD next )", "export CMD=aidlc\n$CMD next",
    "local CMD=aidlc\n$CMD next", "CMD=aidlc; export CMD\n$CMD next", "readonly CMD=aidlc\n$CMD next", "CMD=aidlc\n\n$CMD next", "CMD=aidlc # c\n$CMD next",
    "CMD=aidlc\n# $CMD next", "CMD=aidlc\n$CMD next # c", "CMD=aidlc\n$CMD next #c", "CMD=aidlc\n$CMD next#c", "CMD=aidlc;#c\n$CMD next", "CMD=aidlc\n$CMD ne\\xt",
    "echo $(aidlc next)", "echo `aidlc next`", "echo \"$(aidlc next)\"", "echo '$(aidlc next)'", "echo '`aidlc next`'", "echo \"`aidlc next`\"", "echo \\$(aidlc next)",
    "echo `$(bun aidlc-state.ts approve x)`", "echo `$(aidlc engine state approve x)`", "echo \"`$(aidlc engine state approve x)`\"", "echo `$(aidlc engine orchestrate report)`",
    "echo `(aidlc engine state approve x)`", "echo `; aidlc engine state approve x`", "echo `a; bun aidlc-state.ts approve x`", "echo \"`a; bun aidlc-state.ts approve x`\"",
    "echo \"\\$(aidlc engine state approve x)\"", "echo \"\\$(bun aidlc-state.ts approve x)\"", "echo \"a\\\\$(aidlc engine state approve x)\"", "echo \"\\$(aidlc engine orchestrate report)\"",
    "aidlc intent switch -- --json", "aidlc intent switch a -- --json", "aidlc space create a -- --quiet", "aidlc space-create a -- --yes", "aidlc -- --json next", "aidlc intent -- --json",
    "echo \\`aidlc next\\`", "echo $(echo $(aidlc next))", "echo $(echo `aidlc next`)", "echo `echo $(aidlc next)`", "echo $(aidlc next", "echo `aidlc next",
    "echo $(echo ')' aidlc next)", "echo $(echo \")\"; aidlc next)", "echo $(echo \\); aidlc next)", "echo $(echo '(' ; aidlc next)", "echo $((1+2)); aidlc next",
    "echo $((1+2)) aidlc next", "x=$(aidlc next)", "x=`aidlc next`", "x=$(aidlc next) y", "x=\"$(aidlc next)\"", "x='$(aidlc next)'", "x=$(aidlc next); echo $x",
    "echo ${X:-aidlc next}", "echo ${X:-aidlc; next}", "echo ${X}; aidlc next", "echo ${X aidlc next", "echo ${X:-$(aidlc next)}", "echo ${#X}; aidlc next",
    "echo $X; aidlc next", "echo $(aidlc next) 2>&1", "echo \"$(echo \"$(aidlc next)\")\"", "echo \"$(echo \\\"$(aidlc next)\\\")\"", "echo \"\\$(aidlc next)\"",
    "echo \"a\\\\$(aidlc next)\"", "echo 'a'$(aidlc next)", "echo 'a\\'$(aidlc next)", "echo \"a\\\"$(aidlc next)\"", "echo \"a\\\"\" $(aidlc next)",
    "echo `echo \\`aidlc next\\``", "echo `aidlc \\` next`", "echo `aidlc 'next'`", "echo `aidlc \"next\"`", "echo `aidlc ')' next`", "echo $(aidlc ')' next)",
    "echo $(aidlc ') next)", "echo $(aidlc \") next)", "echo $(aidlc '\\' next)", "echo $(aidlc \"\\\\\" next)", "echo $(aidlc \\) next)", "echo $(aidlc \\\\) next",
    "echo $(aidlc `)` next)", "echo $(aidlc $(x) next)", "echo $(aidlc (x) next)", "echo $(aidlc ((x)) next)",
    "f() { aidlc next; }", "f() { aidlc next; }; f", "f() { aidlc next; }\nf", "function f { aidlc next; }", "function f() { aidlc next; }", "function f () { aidlc next; }",
    "f () { aidlc next; }", "f ( ) { aidlc next; }", "f()\n{\n aidlc next\n}", "f() {\n aidlc next\n}\naidlc approve", "f() { aidlc next; }; aidlc approve",
    "f() { aidlc next; } ; aidlc approve", "f() { aidlc next; }\n\naidlc approve", "f() { echo '}'; aidlc next; }", "f() { echo \"}\"; aidlc next; }",
    "f() { echo \\}; aidlc next; }", "f() { echo '{'; aidlc next; }", "f() { { aidlc next; }; }", "f() { aidlc next", "f() { aidlc next; }}", "f() { aidlc next; } }",
    "f() { aidlc next; }\n}", "x; f() { aidlc next; }", "x;f() { aidlc next; }", "x && f() { aidlc next; }", "x | f() { aidlc next; }", "echo f() { aidlc next; }",
    "f() { aidlc next; }; g() { aidlc approve; }", "f() { aidlc next; } g() { aidlc approve; }", "f() { g() { aidlc next; }; }", "function { aidlc next; }",
    "function f { aidlc next; }\nfunction g { aidlc approve; }", "f(){ aidlc next; }", "f ()\n{\n aidlc next\n}", "function  f  { aidlc next; }", "function\tf { aidlc next; }",
    "f() { aidlc next; }; $(aidlc approve)", "f() { $(aidlc next); }", "f() { echo \"$(aidlc next)\"; }", "f() { eval 'aidlc next'; }", "f() { sh -c 'aidlc next'; }",
    "f() { cat <<E\naidlc next\nE\n}", "f() { \"$(aidlc next)\"; }", "f() { aidlc next; }\u00e9 aidlc approve", "f() { aidlc next; }\n\u00e9 aidlc approve",
    "f() { '\u00e9'; }; aidlc approve", "\u00e9() { aidlc next; }", "f\u00e9() { aidlc next; }", "f() { aidlc next; } \U0001F600", "\U0001F600; f() { aidlc next; }",
    "echo \U0001F600; f() { aidlc next; }; aidlc approve", "echo \U0001F600\U0001F600; f() { aidlc next; }; aidlc approve", "f() { echo \U0001F600; aidlc next; }; aidlc approve",
    "f() { echo \U0001F600; }; aidlc approve", "f() { echo \U0001F600\U0001F600\U0001F600; }; aidlc approve", "f() { echo '\U0001F600'; }; aidlc approve",
    "f() { echo \U0001F600; }\naidlc approve", "f() { echo \U0001F600; }; g() { aidlc approve; }", "f() { echo \U0001F600; }; echo $(aidlc approve)",
    "echo \U0001F600 $(aidlc next)", "echo \"\U0001F600\" $(aidlc next)", "echo \"\U0001F600 $(aidlc next)\"", "echo '\U0001F600' $(aidlc next)", "echo \U0001F600; aidlc next",
    "echo \U0001F600\U0001F600 `aidlc next`", "echo `\U0001F600` $(aidlc next)", "echo `echo \U0001F600` `aidlc next`", "echo $(echo \U0001F600) $(aidlc next)",
    "echo \"\U0001F600\"; aidlc approve", "echo '\U0001F600'; aidlc approve", "echo \U0001F600; aidlc approve", "echo \"\U0001F600;\" aidlc approve", "echo '\U0001F600|' aidlc approve",
    "echo \"\U0001F600\n\" aidlc approve", "echo \"a\nb\U0001F600\"; aidlc approve", "echo \"\U0001F600\n$(aidlc next)\"", "echo \"$(aidlc next)\U0001F600\"",
    "cat <<E\n\U0001F600\nE\naidlc approve", "cat <<E\n\U0001F600 $(aidlc next)\nE", "cat <<'E'\n\U0001F600 $(aidlc next)\nE", "cat <<E\n\U0001F600\n$(aidlc next)\nE",
    "cat <<EOF\naidlc next\nEOF", "cat <<EOF\naidlc next\nEOF\naidlc approve", "cat <<EOF\n$(aidlc next)\nEOF", "cat <<'EOF'\n$(aidlc next)\nEOF", "cat <<\"EOF\"\n$(aidlc next)\nEOF",
    "cat <<-EOF\n\t$(aidlc next)\n\tEOF", "cat <<-EOF\n\t$(aidlc next)\nEOF\naidlc approve", "cat <<-'EOF'\n\t$(aidlc next)\n\tEOF", "cat <<EOF\n`aidlc next`\nEOF",
    "cat <<EOF\naidlc next", "cat <<EOF\n$(aidlc next)", "cat <<EOF\n$(aidlc next)\nEOF ", "cat <<EOF\n$(aidlc next)\n EOF\nEOF", "cat <<EOF\n$(aidlc next)\nEOFX\nEOF",
    "cat << EOF\n$(aidlc next)\nEOF", "cat <<  EOF\n$(aidlc next)\nEOF", "cat <<EOF <<X\na\nEOF\n$(aidlc next)\nX", "cat <<EOF <<'X'\na\nEOF\n$(aidlc next)\nX",
    "cat <<'EOF' <<X\n$(aidlc approve)\nEOF\n$(aidlc next)\nX", "cat <<EOF; aidlc approve\nx\nEOF", "cat <<EOF | aidlc approve\nx\nEOF", "cat <<EOF\nx\nEOF\n$(aidlc next)",
    "cat <<EOF\n$(echo $(aidlc next))\nEOF", "cat <<EOF\n$(echo '$(aidlc next)')\nEOF", "cat <<EOF\n\"$(aidlc next)\"\nEOF", "cat <<EOF\n'$(aidlc next)'\nEOF",
    "cat <<EOF\n\\$(aidlc next)\nEOF", "cat <<EOF\n$(aidlc next\nEOF", "cat <<EOF\n$(aidlc ')' next)\nEOF", "cat <<EOF\naidlc $(next)\nEOF", "cat <<EOF\nf() { aidlc next; }\nEOF",
    "cat <<E1\n$(cat <<E2\n$(aidlc next)\nE2\n)\nE1", "cat <<1EOF\n$(aidlc next)\n1EOF", "cat <<_E\n$(aidlc next)\n_E", "cat <<E-X\n$(aidlc next)\nE-X", "cat <<'E X'\n$(aidlc next)\nE X",
    "cat <<\"E X\"\n$(aidlc next)\nE X", "cat <<E\n\n$(aidlc next)\nE", "echo <<E aidlc approve\nx\nE", "x <<E\naidlc approve\nE\naidlc next", "<<E cat\n$(aidlc next)\nE",
    "cat <<<x; aidlc approve", "cat <<< $(aidlc next)", "cat <<<\"$(aidlc next)\"", "cat <<E\r\n$(aidlc next)\r\nE", "cat <<E\n$(aidlc next)\r\nE\r\n",
    "{ aidlc next; }", "( aidlc next )", "(aidlc next)", "{aidlc next; }", "{ aidlc next }", "x; { aidlc next; }", "x || ( aidlc next )", "(( aidlc next ))",
    "[ -f x ] && aidlc next", "[[ -f x ]] && aidlc next", "test -f x && aidlc next", "true; aidlc next", "true & aidlc next", "true |& aidlc next", "true |aidlc next",
    "true| aidlc next", "true ;; aidlc next", "true &&& aidlc next", "true ||| aidlc next", "true;;aidlc next", "true&aidlc next", "x #; aidlc next", "x # aidlc next\naidlc approve",
    "x #aidlc next", "x#aidlc next", "x # aidlc next", "x;# aidlc next", "x;#aidlc next\naidlc approve", "x &# aidlc next", "x (# aidlc next", "x {# aidlc next", "x }# aidlc next",
    "x )# aidlc next", "# aidlc next", "#aidlc next", " # aidlc next", "\t# aidlc next", "#\naidlc next", "# x\n# y\naidlc next", "x # 'aidlc next\naidlc approve",
    "echo 'x # aidlc next'", "echo \"x # aidlc next\"", "echo x\\# aidlc next", "echo x# aidlc next", "echo x #aidlc next", "x$# aidlc next", "x${#X} aidlc next",
    "echo 'a;b' aidlc next", "echo 'a' ; aidlc next", "echo \"a;b\"; aidlc next", "echo \"a|b\" | aidlc next", "echo 'a\nb'; aidlc next", "echo 'a\nb'\naidlc next",
    "echo \"a\nb\"\naidlc approve", "echo 'a;\nb; aidlc next'", "echo 'a\n'; aidlc approve", "echo 'a\\'; aidlc next", "echo \"a\\\"; aidlc next\"", "echo \"a\\\\\"; aidlc next",
    "echo 'a\\\\'; aidlc next", "echo \"a\\\\\\\"; aidlc next\"", "echo \"$(echo ')')\"; aidlc approve", "echo \"$(echo ')'\"; aidlc approve", "echo \"$(\"; aidlc approve",
    "echo \"\\$(x; aidlc approve)\"", "echo \"$(x; aidlc approve)\"", "echo \"$(x)\"; aidlc approve", "echo \"$(x) ; aidlc approve\"", "echo \"$(x); aidlc approve\"",
    "echo \"$(echo $(x); aidlc approve)\"", "echo \"$(echo $(x)); aidlc approve\"", "echo '$(x); aidlc approve'", "echo `x; aidlc approve`", "echo \"`x; aidlc approve`\"",
    "echo 'unterminated; aidlc approve", "echo \"unterminated; aidlc approve", "echo `unterminated; aidlc approve", "echo 'a' 'b; aidlc approve", "echo 'a'; aidlc approve'",
    "echo \"a\"; aidlc approve\"", "echo 'a\"b'; aidlc approve", "echo \"a'b\"; aidlc approve", "echo \"a`b\"; aidlc approve", "echo 'a`b'; aidlc approve",
    "echo `a'b`; aidlc approve", "echo `a\"b`; aidlc approve", "echo 'a\\'b'; aidlc approve", "echo $'a\\'b'; aidlc approve", "echo $'aidlc'; aidlc approve", "echo $\"a\"; aidlc approve",
    "'aidlc' next", "\"aidlc\" next", "'aidlc' 'next'", "\"aidlc\" \"next\"", "aidlc 'ne'xt", "aidlc ne'x't", "aidlc \"next\"", "aidlc ne\\xt",
    "aidlc $'next'", "aidlc $\"next\"", "aidlc $next", "aidlc ${next}", "aidlc 'next", "aidlc \"next", "aidlc next'", "aidlc next\"", "aidlc 'a b' next", "aidlc\\ next",
    "aidlc\tnext", "aidlc\nnext", "aidlc\\\nnext", "aidlc \\\n next", "aidlc\\\n next", "aidlc\\\n\\\n next", " aidlc next ", "  aidlc  next  ", "aidlc next\\",
    "aidlc next \\", "aidlc\u00a0next", "aidlc\u2003next", "aidlc\ufeffnext", "aidlc\u200bnext", "aidlc\u00e9 next", "\u00e9 aidlc next", "\u00e9; aidlc next",
    "aidlc next\u00e9", "aidlc nex\u00e9t", "\U0001F600 aidlc next", "\U0001F600; aidlc next", "aidlc \U0001F600 next", "aidlc next \U0001F600", "'\U0001F600' aidlc next",
    "'\U0001F600'; aidlc next", "\"\U0001F600\"; aidlc next", "`\U0001F600`; aidlc next", "$(\U0001F600); aidlc next", "x=\U0001F600; aidlc next", "x=\U0001F600\naidlc next",
    "x='\U0001F600'\n$x next", "x=\U0001F600 aidlc next", "echo \U0001F600 | aidlc next", "echo \U0001F600 && aidlc next", "echo \U0001F600\naidlc next", "echo \U0001F600 \\\n aidlc next",
    "echo \U0001F600 #\naidlc next", "echo '\U0001F600' # aidlc next", "echo '\U0001F600\n'; aidlc next", "echo \"\U0001F600\n\"; aidlc approve", "echo \"\U0001F600\n;\"; aidlc approve",
    "echo \"\U0001F600\U0001F600\n;\" aidlc approve", "echo \"\U0001F600\n\" ; aidlc approve", "echo \"\U0001F600\n\"; aidlc approve ; x",
    "echo \"a\U0001F600\n;\"; aidlc approve", "echo \"\U0001F600\U0001F600\U0001F600\n\";aidlc approve", "echo \"\U0001F600\n\";aidlc approve;", "echo \"\U0001F600\";aidlc approve",
    "echo \"\n\U0001F600\";aidlc approve", "echo \"\n\U0001F600\" ; aidlc approve", "echo \"x\n\U0001F600\"; aidlc approve", "echo \"\n\" \U0001F600; aidlc approve",
    "echo \U0001F600; echo \"\n\"; aidlc approve", "echo \"\U0001F600\" \"\n\"; aidlc approve", "echo \"\U0001F600\n\" `aidlc next`", "echo \"\U0001F600\n\" $(aidlc next)",
    "echo \"\n\U0001F600\" `aidlc next`", "echo `\U0001F600\n`; aidlc approve", "echo `\n\U0001F600`; aidlc approve", "echo '\n\U0001F600'; aidlc approve",
    "echo '\U0001F600\n'; aidlc approve", "echo '\U0001F600\n;' aidlc approve", "echo \"$(aidlc next)\" \U0001F600", "echo \"\U0001F600$(aidlc next)\"",
    "echo \"a\U0001F600$(aidlc next)\"", "echo \"\U0001F600$(aidlc next)\" ; x", "echo \"\U0001F600\" \"$(aidlc next)\"", "echo \U0001F600\"$(aidlc next)\"",
    "echo \"\U0001F600\"\"$(aidlc next)\"", "echo \"\U0001F600\"'$(aidlc next)'", "echo '\U0001F600'\"$(aidlc next)\"", "echo \"\U0001F600\\\"$(aidlc next)\"",
    "echo \"\U0001F600\\$(aidlc next)\"", "echo \"\U0001F600\" \\$(aidlc next)", "echo \"\U0001F600\" $(aidlc next) \U0001F600", "echo $(aidlc next) \"\U0001F600\"",
    "echo $(\U0001F600 aidlc next)", "echo $(\U0001F600; aidlc next)", "echo $(echo \U0001F600; aidlc next)", "echo `\U0001F600; aidlc next`", "echo $(echo '\U0001F600'; aidlc next)",
    "echo $(echo \"\U0001F600\"; aidlc next)", "echo $(echo \U0001F600) $(aidlc approve)", "echo `echo \U0001F600` $(aidlc approve)", "echo \"\U0001F600\" `echo x` $(aidlc approve)",
    "echo \"$(echo \U0001F600)\" $(aidlc approve)", "echo \"$(echo \U0001F600) $(aidlc approve)\"", "echo \"\U0001F600 $(echo x) $(aidlc approve)\"",
    "echo \"\U0001F600 `echo x` `aidlc approve`\"", "echo \"\U0001F600\" ; echo `aidlc approve`", "echo \"\U0001F600\" ; echo $(aidlc approve) ; x",
    "echo \"\U0001F600\" ; echo \"$(aidlc approve)\"", "echo \"\U0001F600\" \"$(echo \"$(aidlc approve)\")\"", "echo \"\U0001F600\" $(echo $(aidlc approve))",
    "x=\"\U0001F600\" $(aidlc approve)", "x=\"\U0001F600\"; echo $(aidlc approve)", "x=\"\U0001F600\"; $(aidlc approve)", "x=\"\U0001F600\"\n$(aidlc approve)",
    "x=\U0001F600; $(aidlc approve)", "\U0001F600=x; $(aidlc approve)", "echo \"\U0001F600\" $(aidlc approve) \"$(aidlc next)\"", "echo \"\U0001F600\" `aidlc approve` `aidlc next`",
    "echo \"\U0001F600\" $(aidlc approve) `aidlc next`", "echo \"\U0001F600\" `aidlc approve` $(aidlc next)", "echo $(aidlc approve) \"\U0001F600\" `aidlc next`",
    "echo `aidlc approve` \"\U0001F600\" $(aidlc next)", "echo $(echo \U0001F600 $(aidlc approve)) `aidlc next`", "echo $(\U0001F600) $(aidlc approve) `aidlc next`",
    "echo `\U0001F600` $(aidlc approve) `aidlc next`", "echo \"$(\U0001F600)\" $(aidlc approve)", "echo \"`\U0001F600`\" $(aidlc approve)", "echo '`\U0001F600`' $(aidlc approve)",
    "echo \\\U0001F600 $(aidlc approve)", "echo \\\U0001F600; aidlc approve", "echo \"\\\U0001F600\"; aidlc approve", "echo \"\\\U0001F600$(aidlc approve)\"",
    "echo '\\\U0001F600'; aidlc approve", "echo `\\\U0001F600`; aidlc approve", "echo $(\\\U0001F600); aidlc approve", "echo \"$(\\\U0001F600)\"; aidlc approve",
    "echo \"\U0001F600\\\"\"; aidlc approve", "echo \"\U0001F600\\\\\"; aidlc approve", "echo \"\U0001F600\\\\\\\"\"; aidlc approve", "echo '\U0001F600\\'; aidlc approve",
    "echo `\U0001F600\\``; aidlc approve", "echo \"\U0001F600\" \\\n aidlc approve", "echo \U0001F600 \\\n; aidlc approve", "echo \"\U0001F600\" ${X}; aidlc approve",
    "echo \"\U0001F600\" ${X; aidlc approve", "echo \"\U0001F600\" ${X}} aidlc approve", "echo ${\U0001F600}; aidlc approve", "echo ${X:-\U0001F600}; aidlc approve",
    "echo ${X:-\U0001F600;} aidlc approve", "echo \"${X:-\U0001F600}\"; aidlc approve", "echo \"\U0001F600\" # x\naidlc approve", "echo \"\U0001F600\" #x; aidlc approve",
    "echo \U0001F600 # x\n# y\naidlc approve", "echo \U0001F600 #\n\naidlc approve", "echo '\U0001F600' #\n\U0001F600 aidlc approve", "echo \U0001F600;#\naidlc approve",
    "echo \"\U0001F600\";#\naidlc approve", "echo \U0001F600 #", "echo \U0001F600 # aidlc approve", "echo \"\U0001F600 # aidlc approve\"", "echo \"\U0001F600\" \"# aidlc approve\"",
    "echo \"\U0001F600\"\"# aidlc approve\"", "echo \"\U0001F600\"# aidlc approve", "echo \"\U0001F600\" #", "echo \"\U0001F600\"\n#\naidlc approve", "echo \"\U0001F600\\\n\"; aidlc approve",
    "echo \U0001F600\\\naidlc approve", "echo \U0001F600 \\\n aidlc approve", "echo \"\U0001F600\" \\\n aidlc approve", "echo \U0001F600\\\n; aidlc approve", "\\\U0001F600 aidlc approve",
    "\\\U0001F600; aidlc approve", "\\\U0001F600\naidlc approve", "\\\U0001F600 && aidlc approve", "\\\U0001F600 | aidlc approve", "\\\U0001F600 & aidlc approve",
    "a\\\U0001F600 aidlc approve", "a\\\U0001F600; aidlc approve", "a\\\U0001F600\naidlc approve", "'\U0001F600' \\\U0001F600; aidlc approve", "\"\U0001F600\" \\\U0001F600; aidlc approve",
    "\"\\\U0001F600\" \\\U0001F600; aidlc approve", "\\\U0001F600 \"\U0001F600\"; aidlc approve", "\\\U0001F600 '\U0001F600'; aidlc approve", "\\\U0001F600\\\U0001F600; aidlc approve",
    "\\\U0001F600 \\\U0001F600; aidlc approve", "\\\U0001F600 \\\U0001F600 \\\U0001F600; aidlc approve", "x \\\U0001F600; aidlc approve", "x \\\U0001F600 $(aidlc approve)",
    "x \\\U0001F600 `aidlc approve`", "x \\\U0001F600 \"$(aidlc approve)\"", "x \\\U0001F600 '$(aidlc approve)'", "x \\\U0001F600 # aidlc approve", "x \\\U0001F600 #\naidlc approve",
    "x \\\U0001F600 ${X}; aidlc approve", "x \\\U0001F600 ${X; aidlc approve", "x=\\\U0001F600; aidlc approve", "x=\\\U0001F600\naidlc approve", "x=\\\U0001F600 aidlc approve",
    "\\\U0001F600=x aidlc approve", "f() { \\\U0001F600; }; aidlc approve", "f() { \\\U0001F600; aidlc next; }", "cat <<E\n\\\U0001F600\nE\naidlc approve", "cat <<E\n\\\U0001F600 $(aidlc approve)\nE",
    "eval \\\U0001F600 aidlc approve", "eval '\\\U0001F600 aidlc approve'", "eval \"\\\U0001F600 aidlc approve\"", "sh -c '\\\U0001F600 aidlc approve'", "sh -c \"\\\U0001F600 aidlc approve\"",
    "sh -c '\U0001F600; aidlc approve'", "sh -c \"\U0001F600; aidlc approve\"", "sh -c \"echo \U0001F600; aidlc approve\"", "sh -c 'echo \"\U0001F600\"; aidlc approve'",
    "sh -c 'echo \"\U0001F600\n\"; aidlc approve'", "eval 'echo \"\U0001F600\n\"; aidlc approve'", "eval \"echo '\U0001F600'; aidlc approve\"", "eval \"echo \\\"\U0001F600\\\"; aidlc approve\"",
    "env -S '\U0001F600 aidlc approve'", "env -S 'echo \U0001F600; aidlc approve'", "env -S '\\\U0001F600 aidlc approve'", "env -S'\U0001F600' aidlc approve", "env '-S\U0001F600' aidlc approve",
    "env -S'aidlc approve' \U0001F600", "env -S\U0001F600 aidlc approve", "X=\U0001F600 env -S 'aidlc approve'", "time \U0001F600 aidlc approve", "time -f \U0001F600 aidlc approve",
    "command \U0001F600 aidlc approve", "exec -a \U0001F600 aidlc approve", "nice -n \U0001F600 aidlc approve", "nohup \U0001F600 aidlc approve", "\U0001F600=1 aidlc approve",
    "\U0001F600 \U0001F600 \U0001F600 aidlc approve", "\U0001F600\U0001F600\U0001F600 aidlc approve", "\U0001F600\U0001F600\U0001F600; aidlc approve", "\U0001F600\U0001F600\U0001F600\naidlc approve",
    "\U0001F600\U0001F600\U0001F600 $(aidlc approve)", "\U0001F600\U0001F600\U0001F600 `aidlc approve`", "\U0001F600\U0001F600\U0001F600 \"$(aidlc approve)\"",
    "\U0001F600\U0001F600\U0001F600 '$(aidlc approve)'", "\U0001F600 \"$(aidlc approve)\"", "\U0001F600 '$(aidlc approve)'", "\U0001F600 `aidlc approve`", "\U0001F600 $(aidlc approve)",
    "\U0001F600$(aidlc approve)", "\U0001F600`aidlc approve`", "\U0001F600\"$(aidlc approve)\"", "\U0001F600'$(aidlc approve)'", "\"\U0001F600\"$(aidlc approve)",
    "'\U0001F600'$(aidlc approve)", "`\U0001F600`$(aidlc approve)", "$(\U0001F600)$(aidlc approve)", "$(\U0001F600) $(aidlc approve)", "\"$(\U0001F600)\" $(aidlc approve)",
    "x=$(\U0001F600) $(aidlc approve)", "x=$(\U0001F600); $(aidlc approve)", "x=$(\U0001F600)\n$(aidlc approve)", "x=`\U0001F600`; $(aidlc approve)", "x=\"$(\U0001F600)\"; $(aidlc approve)",
    "x='$(\U0001F600)'; $(aidlc approve)", "x=\"`\U0001F600`\"; $(aidlc approve)", "x='`\U0001F600`'; $(aidlc approve)", "echo $(echo $(\U0001F600)) $(aidlc approve)",
    "echo $(echo `\U0001F600`) $(aidlc approve)", "echo `echo $(\U0001F600)` $(aidlc approve)", "echo \"$(echo \"\U0001F600\")\" $(aidlc approve)", "echo \"$(echo '\U0001F600')\" $(aidlc approve)",
    "echo \"$(echo `\U0001F600`)\" $(aidlc approve)", "echo \"$(\U0001F600 $(aidlc approve))\"", "echo \"$(\U0001F600; $(aidlc approve))\"", "echo \"$(echo \U0001F600; aidlc approve)\"",
    "echo \"$(echo \"\U0001F600\"; aidlc approve)\"", "echo \"$(echo '\U0001F600'; aidlc approve)\"", "echo \"$(echo `\U0001F600`; aidlc approve)\"", "echo \"`echo \U0001F600; aidlc approve`\"",
    "echo \"`echo \"\U0001F600\"; aidlc approve`\"", "echo \"`echo '\U0001F600'; aidlc approve`\"", "echo `echo \"\U0001F600\"; aidlc approve`", "echo `echo '\U0001F600'; aidlc approve`",
    "echo `echo \\\"\U0001F600\\\"; aidlc approve`", "echo `echo \\\U0001F600; aidlc approve`", "echo `\\\U0001F600 aidlc approve`", "echo $(\\\U0001F600 aidlc approve)",
    "echo \"$(\\\U0001F600 aidlc approve)\"", "echo \"`\\\U0001F600 aidlc approve`\"", "echo '$(\\\U0001F600 aidlc approve)'", "echo '`\\\U0001F600 aidlc approve`'",
    "aidlc next\n\U0001F600", "aidlc next \U0001F600 \U0001F600", "aidlc next; \U0001F600", "aidlc next && \U0001F600", "aidlc next | \U0001F600", "aidlc approve \"\U0001F600\"",
    "aidlc approve '\U0001F600'", "aidlc approve `\U0001F600`", "aidlc approve $(\U0001F600)", "aidlc approve \"$(\U0001F600)\"", "aidlc approve ${\U0001F600}", "aidlc approve \\\U0001F600",
    "aidlc approve # \U0001F600", "aidlc approve #\U0001F600", "aidlc approve\n# \U0001F600", "aidlc approve <<E\n\U0001F600\nE", "aidlc approve <<'E'\n\U0001F600\nE",
    "aidlc approve <<E\n$(\U0001F600)\nE", "aidlc approve <<E\n`\U0001F600`\nE", "aidlc approve <<-E\n\t\U0001F600\n\tE", "aidlc approve <<E\n\U0001F600\nE\n\U0001F600",
    "aidlc approve <<E\n\U0001F600\nE\n$(\U0001F600)", "aidlc approve <<E\n\U0001F600\nE\n`\U0001F600`", "aidlc approve <<E\n\U0001F600\nE\n\"$(\U0001F600)\"",
    "\U0001F600 <<E\n$(aidlc approve)\nE", "\U0001F600 <<'E'\n$(aidlc approve)\nE", "\U0001F600 <<E\n`aidlc approve`\nE", "\U0001F600 <<-E\n\t$(aidlc approve)\n\tE",
    "\U0001F600 <<E\n$(aidlc approve)\nE\naidlc next", "\U0001F600 <<E\n\U0001F600\nE\n$(aidlc approve)", "\U0001F600 <<E\n\U0001F600\nE\n`aidlc approve`",
    "echo \U0001F600 <<E\n$(aidlc approve)\nE", "echo \"\U0001F600\" <<E\n$(aidlc approve)\nE", "echo '\U0001F600' <<E\n$(aidlc approve)\nE", "echo `\U0001F600` <<E\n$(aidlc approve)\nE",
    "echo $(\U0001F600) <<E\n$(aidlc approve)\nE", "cat <<\U0001F600\n$(aidlc approve)\n\U0001F600", "cat <<'\U0001F600'\n$(aidlc approve)\n\U0001F600", "cat <<\"\U0001F600\"\n$(aidlc approve)\n\U0001F600",
    "cat <<E\U0001F600\n$(aidlc approve)\nE\U0001F600", "cat <<'E\U0001F600'\n$(aidlc approve)\nE\U0001F600", "cat <<E\n\U0001F600\n$(aidlc approve)\nE",
    "cat <<E\n\U0001F600 \U0001F600\n$(aidlc approve)\nE", "cat <<E\n\U0001F600\n\U0001F600\n$(aidlc approve)\nE", "cat <<E\n$(aidlc approve)\n\U0001F600\nE",
    "cat <<E\n$(aidlc approve) \U0001F600\nE", "cat <<E\n\U0001F600 $(aidlc approve) \U0001F600\nE", "cat <<E\n\"\U0001F600\" $(aidlc approve)\nE", "cat <<E\n'\U0001F600' $(aidlc approve)\nE",
    "cat <<E\n`\U0001F600` $(aidlc approve)\nE", "cat <<E\n$(\U0001F600) $(aidlc approve)\nE", "cat <<E\n\\\U0001F600 $(aidlc approve)\nE", "cat <<E\n\U0001F600\\ $(aidlc approve)\nE",
    "cat <<E\n\U0001F600\\\n$(aidlc approve)\nE", "cat <<E\n\U0001F600\n\\\n$(aidlc approve)\nE", "cat <<E\n\U0001F600 \"$(aidlc approve)\"\nE", "cat <<E\n\U0001F600 '$(aidlc approve)'\nE",
    "cat <<E\n\U0001F600 \"$(\U0001F600 aidlc approve)\"\nE", "cat <<E\n\U0001F600 $(\U0001F600 aidlc approve)\nE", "cat <<E\n\U0001F600 `\U0001F600 aidlc approve`\nE",
    "cat <<E\n\U0001F600 $(echo \U0001F600; aidlc approve)\nE", "cat <<E\n\U0001F600 $(echo \"\U0001F600\"; aidlc approve)\nE", "cat <<E\n\U0001F600 `echo \U0001F600; aidlc approve`\nE",
    "cat <<E\n\U0001F600 $(echo $(\U0001F600); aidlc approve)\nE", "cat <<E\n\U0001F600 $(echo `\U0001F600`; aidlc approve)\nE", "cat <<E\n\U0001F600 $(echo '\U0001F600'; aidlc approve)\nE",
    "cat <<E\n\U0001F600 $(f() { \U0001F600; }; aidlc approve)\nE", "cat <<E\n\U0001F600 $(X=\U0001F600; aidlc approve)\nE", "cat <<E\n\U0001F600 $(X=\U0001F600 aidlc approve)\nE",
    "cat <<E\n\U0001F600 $(\U0001F600=1 aidlc approve)\nE", "cat <<E\n\U0001F600 $(time \U0001F600 aidlc approve)\nE", "cat <<E\n\U0001F600 $(env -S '\U0001F600 aidlc approve')\nE",
    "cat <<E\n\U0001F600 $(eval '\U0001F600 aidlc approve')\nE", "cat <<E\n\U0001F600 $(sh -c '\U0001F600 aidlc approve')\nE", "cat <<E\n\U0001F600 $(cat <<X\n\U0001F600 $(aidlc approve)\nX\n)\nE",
    "cat <<E\n\U0001F600 $(cat <<'X'\n\U0001F600 $(aidlc approve)\nX\n)\nE", "cat <<E\n\U0001F600 $(cat <<X\n\U0001F600\nX\n$(aidlc approve))\nE",
    "\U0001F600() { aidlc next; }", "\U0001F600() { aidlc next; }; aidlc approve", "\U0001F600 () { aidlc next; }; aidlc approve", "function \U0001F600 { aidlc next; }; aidlc approve",
    "f\U0001F600() { aidlc next; }; aidlc approve", "\U0001F600f() { aidlc next; }; aidlc approve", "f() { aidlc next; }\U0001F600; aidlc approve", "f() { aidlc next; } \U0001F600; aidlc approve",
    "f() { aidlc next; }; \U0001F600 aidlc approve", "f() { aidlc next; }; \U0001F600; aidlc approve", "f() { aidlc next; }\n\U0001F600 aidlc approve", "f() { aidlc next; }\n\U0001F600; aidlc approve",
    "f() { \U0001F600 aidlc next; }; aidlc approve", "f() { \U0001F600; aidlc next; }; aidlc approve", "f() { aidlc next; \U0001F600; }; aidlc approve", "f() { aidlc next \U0001F600; }; aidlc approve",
    "f() { echo \U0001F600; aidlc next; }; aidlc approve", "f() { echo \"\U0001F600\"; aidlc next; }; aidlc approve", "f() { echo '\U0001F600'; aidlc next; }; aidlc approve",
    "f() { echo `\U0001F600`; aidlc next; }; aidlc approve", "f() { echo $(\U0001F600); aidlc next; }; aidlc approve", "f() { echo \"$(\U0001F600)\"; aidlc next; }; aidlc approve",
    "f() { echo \\\U0001F600; aidlc next; }; aidlc approve", "f() { echo \U0001F600\U0001F600; aidlc next; }; aidlc approve", "f() { echo \U0001F600 \U0001F600; aidlc next; }; aidlc approve",
    "f() { echo \U0001F600; }; g() { aidlc next; }; aidlc approve", "f() { echo \U0001F600; }; g() { echo \U0001F600; aidlc next; }; aidlc approve",
    "f() { echo \U0001F600; }\ng() { aidlc next; }\naidlc approve", "f() { echo \U0001F600; } ; g() { aidlc next; } ; aidlc approve", "f() { echo \U0001F600; }; aidlc approve; g() { aidlc next; }",
    "f() { echo \U0001F600; }; $(aidlc approve)", "f() { echo \U0001F600; }; `aidlc approve`", "f() { echo \U0001F600; }; \"$(aidlc approve)\"", "f() { echo \U0001F600; }; echo $(aidlc approve)",
    "f() { echo \U0001F600; }; echo `aidlc approve`", "f() { echo \U0001F600; }; echo \"$(aidlc approve)\"", "f() { echo \U0001F600; }; echo '$(aidlc approve)'",
    "f() { echo \U0001F600; }; X='aidlc approve'; $X", "f() { echo \U0001F600; }; X='aidlc approve'; eval \"$X\"", "f() { echo \U0001F600; }; X='aidlc approve'; sh -c \"$X\"",
    "f() { echo \U0001F600; }; eval 'aidlc approve'", "f() { echo \U0001F600; }; sh -c 'aidlc approve'", "f() { echo \U0001F600; }; env -S 'aidlc approve'",
    "f() { echo \U0001F600; }; time aidlc approve", "f() { echo \U0001F600; }; cat <<E\n$(aidlc approve)\nE", "f() { echo \U0001F600; }; cat <<'E'\n$(aidlc approve)\nE",
    "f() { echo \U0001F600; }; { aidlc approve; }", "f() { echo \U0001F600; }; ( aidlc approve )", "f() { echo \U0001F600; }; if aidlc approve; then :; fi",
    "f() { echo \U0001F600; }; ! aidlc approve", "f() { echo \U0001F600; }; X=1 aidlc approve", "f() { echo \U0001F600; }; \U0001F600 aidlc approve", "f() { echo \U0001F600; }; \U0001F600; aidlc approve",
    "f() { echo \U0001F600; }; echo \U0001F600; aidlc approve", "f() { echo \U0001F600; }; echo \"\U0001F600\"; aidlc approve", "f() { echo \U0001F600; }; echo '\U0001F600'; aidlc approve",
    "f() { echo \U0001F600; }; echo \"\U0001F600\n\"; aidlc approve", "f() { echo \U0001F600; }; echo \"\U0001F600\n\" $(aidlc approve)", "f() { echo \U0001F600; }; echo \"\U0001F600$(aidlc approve)\"",
    "f() { echo \U0001F600; }; echo \"\U0001F600\" $(aidlc approve)", "f() { echo \U0001F600; }; echo `\U0001F600` $(aidlc approve)", "f() { echo \U0001F600; }; echo $(\U0001F600) $(aidlc approve)",
    "f() { echo \U0001F600; }; f() { echo \U0001F600; }; aidlc approve", "f() { echo \U0001F600; }; f() { aidlc next; }; aidlc approve", "f() { echo \U0001F600; }; f() { echo \U0001F600; aidlc next; }; aidlc approve",
    "f() { echo \U0001F600\U0001F600; }; f() { echo \U0001F600; }; aidlc approve", "f() { echo \U0001F600; }; f() { echo \U0001F600\U0001F600; }; aidlc approve",
    "f() { echo \U0001F600; }; echo \U0001F600; f() { echo \U0001F600; }; aidlc approve", "f() { echo \U0001F600; }; echo \"\U0001F600\"; f() { echo \U0001F600; }; aidlc approve",
    "f() { echo \U0001F600; }; echo \"\U0001F600\n\"; f() { echo \U0001F600; }; aidlc approve", "f() { echo \U0001F600; }\nf() { echo \U0001F600; }\naidlc approve",
    "f() { echo \U0001F600; }\n\nf() { echo \U0001F600; }\n\naidlc approve", "f() {\n echo \U0001F600\n}\nf() {\n echo \U0001F600\n}\naidlc approve",
    "function f {\n echo \U0001F600\n}\nfunction g {\n echo \U0001F600\n}\naidlc approve", "f() { echo \U0001F600; }; g() { echo \U0001F600; }; h() { echo \U0001F600; }; aidlc approve",
    "f() { echo \U0001F600; }; g() { echo \U0001F600; }; h() { aidlc next; }; aidlc approve", "f() { echo \U0001F600; }; g() { echo \U0001F600; }; h() { echo \U0001F600; aidlc next; }; aidlc approve",
    "f() { echo \U0001F600; }; g() { echo \U0001F600; }; h() { echo \U0001F600; }; i() { aidlc next; }; aidlc approve",
    "f() { echo \U0001F600; }; g() { echo \U0001F600; }; h() { echo \U0001F600; }; i() { echo \U0001F600; aidlc next; }; aidlc approve",
    "f() { echo \U0001F600\U0001F600\U0001F600\U0001F600; }; g() { aidlc next; }; aidlc approve", "f() { echo \U0001F600\U0001F600\U0001F600\U0001F600; }; g() { echo \U0001F600; aidlc next; }; aidlc approve",
    "f() { echo \U0001F600\U0001F600\U0001F600\U0001F600; }; echo \U0001F600; g() { aidlc next; }; aidlc approve", "f() { echo \U0001F600; }; echo \U0001F600\U0001F600\U0001F600\U0001F600; g() { aidlc next; }; aidlc approve",
    "f() { echo \U0001F600; }; echo \U0001F600\U0001F600\U0001F600\U0001F600; g() { echo \U0001F600; aidlc next; }; aidlc approve",
    "echo \U0001F600\U0001F600\U0001F600\U0001F600; f() { aidlc next; }; aidlc approve", "echo \U0001F600\U0001F600\U0001F600\U0001F600; f() { echo \U0001F600; aidlc next; }; aidlc approve",
    "echo \U0001F600\U0001F600\U0001F600\U0001F600; f() { echo \U0001F600; }; aidlc approve", "echo \U0001F600\U0001F600\U0001F600\U0001F600; f() { echo \U0001F600; }; g() { aidlc next; }; aidlc approve",
    "echo \U0001F600\U0001F600\U0001F600\U0001F600\U0001F600\U0001F600\U0001F600\U0001F600; f() { aidlc next; }; aidlc approve",
    "echo \U0001F600\U0001F600\U0001F600\U0001F600\U0001F600\U0001F600\U0001F600\U0001F600; f() { echo x; }; aidlc approve",
    "echo \U0001F600\U0001F600\U0001F600\U0001F600\U0001F600\U0001F600\U0001F600\U0001F600; f() { echo x; }; g() { aidlc next; }; aidlc approve",
    "echo \U0001F600\U0001F600\U0001F600\U0001F600\U0001F600\U0001F600\U0001F600\U0001F600; f() { echo x; }; g() { echo y; }; aidlc approve",
    "echo \U0001F600\U0001F600\U0001F600\U0001F600\U0001F600\U0001F600\U0001F600\U0001F600; f() { echo x; }; g() { echo y; }; h() { aidlc next; }; aidlc approve",
    "echo \U0001F600\U0001F600\U0001F600\U0001F600\U0001F600\U0001F600\U0001F600\U0001F600\U0001F600\U0001F600\U0001F600\U0001F600; f() { echo x; }; g() { aidlc next; }; aidlc approve",
    "echo \U0001F600\U0001F600\U0001F600\U0001F600\U0001F600\U0001F600\U0001F600\U0001F600\U0001F600\U0001F600\U0001F600\U0001F600; f() { echo x; }; aidlc approve",
    "echo \U0001F600\U0001F600\U0001F600\U0001F600\U0001F600\U0001F600\U0001F600\U0001F600\U0001F600\U0001F600\U0001F600\U0001F600; aidlc approve",
    "echo \U0001F600\U0001F600\U0001F600\U0001F600\U0001F600\U0001F600\U0001F600\U0001F600\U0001F600\U0001F600\U0001F600\U0001F600 $(aidlc approve)",
    "echo \U0001F600\U0001F600\U0001F600\U0001F600\U0001F600\U0001F600\U0001F600\U0001F600\U0001F600\U0001F600\U0001F600\U0001F600 `aidlc approve`",
    "echo \U0001F600\U0001F600\U0001F600\U0001F600\U0001F600\U0001F600\U0001F600\U0001F600\U0001F600\U0001F600\U0001F600\U0001F600 \"$(aidlc approve)\"",
    "echo \"\U0001F600\U0001F600\U0001F600\U0001F600\U0001F600\U0001F600\U0001F600\U0001F600\U0001F600\U0001F600\U0001F600\U0001F600\n\"; aidlc approve",
    "echo \"\U0001F600\U0001F600\U0001F600\U0001F600\U0001F600\U0001F600\U0001F600\U0001F600\U0001F600\U0001F600\U0001F600\U0001F600\n;\" aidlc approve",
]
for command in ST_FIXED:
    add("state_transition", command=command)
# Every table entry once, as a script verb and as a dispatcher route, so a verb dropped from a table is seen.
for script, verbs in ST_SCRIPTS.items():
    for verb in verbs:
        add("state_transition", command=f"bun .claude/tools/{script} {verb}")
for words in ST_DISPATCH_WORDS:
    add("state_transition", command=f"aidlc {words}")
    add("state_transition", command=f"aidlc engine {words}")
for verb in ST_STATE_VERBS:
    add("state_transition", command=f"aidlc state {verb}")
for _ in range(scale * 150):
    add("state_transition", command=st_command())


ST_DIRECT_HEAD = ["bun aidlc-state.ts", "bun .claude/tools/aidlc-state.ts", "bun run aidlc-state.ts", "bun '/p q/aidlc-state.ts'", 'bun "/p/aidlc-state.ts"',
                  "/usr/bin/bun aidlc-state.ts", "bun.exe aidlc-state.ts", "bun .kiro\\tools\\aidlc-state.ts", "bun x/aidlc-state.ts", "bun xaidlc-state.ts",
                  "bun aidlc-state.tsx", "bun -e aidlc-state.ts", "bun --cwd /p aidlc-state.ts", "node aidlc-state.ts", "aidlc-state.ts", "bunx aidlc-state.ts",
                  "aidlc engine state", "./aidlc engine state", "/usr/local/bin/aidlc engine state", "aidlc.exe engine state", '"/p q/aidlc" engine state',
                  "'/p/aidlc.exe' engine state", "aidlc state", "aidlc system state", "aidlc engine  state", "aidlc\tengine\tstate", "xaidlc engine state",
                  "aidlcx engine state", "aidlc engine statex", "aidlc --json engine state", "aidlc engine --json state", "aidlc engine state --json",
                  "bun aidlc.ts engine state", "bun .claude/tools/aidlc.ts engine state", "bun aidlc.ts state", "bun aidlc-orchestrate.ts", "aidlc engine orchestrate",
                  "aidlc orchestrate", "bun aidlc-jump.ts", "aidlc engine jump", "aidlc jump", "bun .claude/tools/aidlc-orchestrate.ts", "./aidlc engine jump"]
ST_DIRECT_VERB = ST_STATE_VERBS + ["report", "park", "execute", "reopen", "next", "resolve", "wait", "approve x", "approve --stage x", "--stage x approve",
                                   "approve;", "approve&&", "approvex", "approve-x", "approve_x", "Approve", "APPROVE", "approve\n", "approve\t", " approve"]
ST_DIRECT_PRE = ["", "", "", "cd /p && ", "X=1 ", "X=1 Y=2 ", "env ", "env -i ", "env X=1 ", "command ", "exec ", "time ", "nice ", "nohup ", "sudo ", "if ", "! ",
                 "echo x; ", "echo x && ", "echo x || ", "echo x | ", "echo x & ", "echo x\n", "(", "{ ", "( ", "echo '", 'echo "', "echo `", "echo $(", "echo \"$(",
                 "echo '$(", "x=$(", "x=`", "f() { ", "function f { ", "cat <<E\n", "cat <<'E'\n", "cat <<E\n$(", "cat <<'E'\n$(", "# ", "x # ", "eval ", "eval '",
                 "sh -c '", 'sh -c "', "sh -c ", "env -S '", "X='", 'X="', "2>&1 ", "> out ", "\t", "  ", "\n", "\\\n", "\u00e9 ", "\U0001F600 ", "echo \U0001F600; ",
                 "echo '\U0001F600'; ", "echo \"\U0001F600\"; ", "f() { \U0001F600; }; ", "cat <<E\n\U0001F600\nE\n", "echo \"\U0001F600\n\"; ", "echo \"\U0001F600\" $("]
ST_DIRECT_POST = ["", "", "", " --stage x", " --result done", " x", " 2>&1", " | cat", " > out", " &", " ;", " # c", "\n", " && echo", "'", '"', "`", ")", "\"", "')",
                  "\")", "; }", "; }; f", "\nE", "\nE\n", ")\nE", "'\n$X", "\"\n$X", "'\neval \"$X\"", "'\nsh -c \"$X\"", "' x", " \U0001F600", "; \U0001F600",
                  " \"\U0001F600\"", "' \U0001F600", ") \U0001F600", "; }; aidlc approve", "'; aidlc approve", "\"; aidlc approve", "); aidlc approve", ")\nE\naidlc approve"]


def st_direct():
    return f"{st.choice(ST_DIRECT_PRE)}{st.choice(ST_DIRECT_HEAD)} {st.choice(ST_DIRECT_VERB)}{st.choice(ST_DIRECT_POST)}"


for _ in range(scale * 80):
    add("state_transition", command=st_direct())


# The switch a person types in chat (parseTypedGuardSwitchRequest) and Kiro's word splitter (splitKiroCommandArgs).
ts = random.Random(seed * 7919 + 17)
TS_PLAIN = ["skip plan approval", "Skip the plan approvals", "please skip plan approval", "stop plan approval", "drop plan approval", "disable plan approval",
            "turn off plan approval", "switch off plan-approval", "no more plan approvals", "no plan approval", "turn plan approval off", "switch the plan approvals off",
            "plan approval off", "plan-approvals off", "don't ask me to approve plans", "do not ask to approve the plan", "stop asking me to approve each code plan",
            "dont ask to approve every plan", "skip plan approval for this work", "skip plan approval on the rest of this piece of work", "skip plan approval for this task, please",
            "skip plan approval please", "skip plan approval for this thing", "skip plan approval now", "guard policy relaxed", "guard-policy off", "change control relaxed",
            "change-control off", "Guard Policy Relaxed", "guard policy strict", "guard policy on", "guard policy relaxed now", "guard policy  relaxed", "plan approval",
            "approve the plan", "skip the plan", "no", "", "   ", "aidlc", "aidlc on the login page", "aidlcx next", "/aidlcx next", "the aidlc tool", "hello"]
TS_FLAGS = ["--guard-policy off", "--guard-policy relaxed", "--guard-policy strict", "--guard-policy on", "--guard-policy OFF", "--GUARD-POLICY off", "--change-control relaxed",
            "--change-control off", "--plan-approval off", "--plan-approval on", "--guard.plan-approval off", "--guard.plan-approval on", "--guard.review-freeze off",
            "--guard.review-freeze on", "--guard.state-transition off", "--guard.reviewer-scope off", "--guard.human-presence off", "--guard.nope off",
            "--summary-confirmation off", "--summary-confirmation on", "--sensors off", "--sensors on", "--learnings off", "--learnings on", "--depth minimal", "--depth",
            "--test-strategy tdd", "--review none", "--skip x", "--add y", "--project-type brownfield", "--collaborators on", "--space s", "--intent i", "--scope bugfix",
            "--nope", "--nope value", "--help", "--json", "--", "--guard-policy", "--guard-policy ''", '--guard-policy ""', "--guard-policy --plan-approval off",
            "--plan-approval", "--sensors", "--space", "--scope", "--skip"]
TS_WORDS = ["", "", "approve the plan", "add the export", "1", "use postgres", "add a --help flag to the reverser", "fix --nope now", "x", "the plan is fine -- really"]
TS_ENTRY = ["/aidlc", "/aidlc", "$aidlc", "aidlc", "/AIDLC", "/Aidlc", " /aidlc", "/aidlc-runner", "/aidlcx", "x /aidlc"]
TS_TAIL = ["", "", "", ".", "?", "!", "...", ",", ";", ":", " ?", "?!"]
TS_COMMANDS = ["next", "report --stage x", "intent switch a", "intent create a --guard-policy off", "space create s --plan-approval off", "space-create s --guard-policy off",
               "intent --guard-policy off", "space list --plan-approval off", "config set guard-policy off", "config set guard-policy", "config set", "config set x y z",
               "config set change-control relaxed", "config set guard.review-freeze off", "config set plan-approval off", "config set depth minimal", "config set --guard-policy off",
               "CONFIG SET guard-policy off", "config set guard-policy off extra", "config set guard-policy off --space s", "status", "park"]


def ts_prompt():
    kind = ts.random()
    if kind < 0.25:
        return ts.choice(TS_PLAIN) + ts.choice(TS_TAIL)
    if kind < 0.35:
        return f"{ts.choice(TS_ENTRY)} {ts.choice(TS_COMMANDS)}{ts.choice(TS_TAIL)}"
    parts = [ts.choice(TS_ENTRY)]
    for _ in range(ts.randrange(0, 4)):
        parts.append(ts.choice(TS_FLAGS))
    if ts.random() < 0.3:
        parts.append("--")
    words = ts.choice(TS_WORDS)
    if words:
        parts.append(words)
    if ts.random() < 0.2:
        parts.append(ts.choice(TS_FLAGS))
    text = " ".join(parts) + ts.choice(TS_TAIL)
    if ts.random() < 0.1:
        text = text.replace(" ", ts.choice(["  ", "\t", "\n", " "]), 1)
    if ts.random() < 0.08:
        text = text.replace("off", ts.choice(['"off"', "'off'", "of\\f", "off\\ x", "o'ff"]), 1)
    return text


for _ in range(scale * 120):
    add("typed_switch", prompt=ts_prompt(), words_answer=ts.random() < 0.3)
TS_FIXED = ["/aidlc --space a --space b --guard-policy off", "/aidlc --intent a --intent b --plan-approval off", "/aidlc --scope x --scope y --guard-policy off",
            "/aidlc --plan-approval off --plan-approval on", "/aidlc --summary-confirmation off --summary-confirmation on", "/aidlc --guard.review-freeze off --guard.review-freeze on",
            "/aidlc --plan-approval on --plan-approval off", "/aidlc --guard-policy off --guard-policy strict", "/aidlc --guard-policy strict --guard-policy off",
            "/aidlc config set guard-policy", "/aidlc config set", "/aidlc config set guard-policy off x", "/aidlc --guard.plan-approval off add the export",
            "/aidlc --plan-approval off --guard.plan-approval on add x", "/aidlc --nope --plan-approval off", "/aidlc --nope v --nope w --plan-approval off"]
for prompt in TS_FIXED:
    for words_answer in (False, True):
        add("typed_switch", prompt=prompt, words_answer=words_answer)
for prompt in TS_PLAIN + [f"/aidlc {c}" for c in TS_COMMANDS] + [f"/aidlc {f}" for f in TS_FLAGS] + [f"/aidlc {f} add the export" for f in TS_FLAGS] + [f"/aidlc {f} -- add the export" for f in TS_FLAGS]:
    for words_answer in (False, True):
        add("typed_switch", prompt=prompt, words_answer=words_answer)

B = "\\"
Q = '"'
KA_FIXED = ["", " ", "a b  c", "a 'b c' d", 'a "b c" d', "a 'b", 'a "b', f"a{B} b", f"a{B};b", f"a{B}'b", f"a{B}{Q}b", f"'a{B}'b'", f"{Q}a{B}{Q}b{Q}", f"'a{B}{Q}b'",
            f"a{B}x", f"a{B}{B}b", f"C:{B}Users{B}x y", f"C:{B}x{B} y", f"'C:{B}x'", f"x{B} y C:{B}z", "users' files", "it's fine", "'it's' fine", "rock 'n' roll", "a' b",
            "a 'b' 'c", "'a'b", "a'b'c", "''", '""', "'", '"', f"--output a{B} b --export", f"--output 'a{B}' --export", f"--output 'a{B}'b'", f"--output a{B} b",
            f"--output 'a{B}' x", f"--output {Q}a{B}{Q} --output", f"--export a{B} b --export", B, f"a{B}", f"a {B}", f"{B} a", f"{B}{B}", "a\tb", "a\nb", "a b", "a b",
            "é 'café' été's", "l'été", "\U0001F600 '\U0001F600' \U0001F600's", "\U0001F600'\U0001F600", "a'\U0001F600", "\U0001F600'a", f"{B}\U0001F600",
            f"'{B}\U0001F600'", f"--output {B}\U0001F600 --export", "x; y", f"x{B}; y", "'x; y'", "a=b c", "--k=v w", f"{Q}a b{Q} 'c d' e{B} f g", "'a' 'b'", "'a''b'",
            f"'a'{Q}b{Q}", f"a{Q}b{Q}c", f"{Q}{Q} ''", "''a", f"a{Q}{Q}"]


def ka_raw():
    pieces = ["a", "bc", "--output", "--export", "'", '"', "\\", " ", "  ", ";", "'s", "n'", "C:", "\\x", "it's", "\u00e9", "\U0001F600", "\t", "--k", "=v"]
    return "".join(ts.choice(pieces) for _ in range(ts.randrange(1, 9)))


for raw in KA_FIXED:
    add("kiro_args", raw=raw)
for _ in range(scale * 60):
    add("kiro_args", raw=ka_raw())


# The readers of a prompt-submit payload that aidlc-lib exports: the host envelope, the host's fixed sentences, an
# adapter's word on the turn, an empty question box, and Claude Code's transcript.
tr = random.Random(seed * 7919 + 19)
HEADERS = ["[CURRENT USER REQUEST -- respond to this]\n", "[CURRENT USER REQUEST — respond to this]\r\n", "[CURRENT USER REQUEST -- respond to this]\r\n",
           "[CURRENT USER REQUEST - respond to this]\n", "[current user request -- respond to this]\n", "[CURRENT USER REQUEST -- respond to this]", " [CURRENT USER REQUEST -- respond to this]\n"]
for prompt in ["", "hello", "a\n" + HEADERS[0] + "b", HEADERS[0] + "b", "a" + HEADERS[0], HEADERS[0], "a\n" + HEADERS[1] + "b\n" + HEADERS[0] + "c", HEADERS[3] + "x",
               HEADERS[4] + "x", HEADERS[5] + "x", HEADERS[6] + "x", "\U0001F600 " + HEADERS[0] + "\U0001F600", HEADERS[0] + HEADERS[0], "x\n" + HEADERS[2] + "\n\ny"]:
    add("host_envelope", prompt=prompt)
for _ in range(scale * 10):
    parts = [tr.choice(["a", "b\n", "memory\n", "\U0001F600", "", "Approve Plan\n"]) + tr.choice(HEADERS + ["", ""]) for _ in range(tr.randrange(1, 5))]
    add("host_envelope", prompt="".join(parts) + tr.choice(["", "the turn", "x\ny"]))

NOTICE = "<task-notification>\nsomething finished\n</task-notification>"
for prompt in ["", " ", NOTICE, NOTICE + "\n", "\n" + NOTICE, "  " + NOTICE + "  ", NOTICE.replace("\n", "\r\n"), NOTICE + " x", "x " + NOTICE,
               "<task-notification></task-notification>", "<task-notification>\n</task-notification>", "<task-notification>\n\n</task-notification>",
               "<task-notification>\nx\n</task-notification>\n<task-notification>\ny\n</task-notification>", "<Task-Notification>\nx\n</Task-Notification>",
               "<task-notification>\nx\n</task-notification", "<original_user_request>\nVerbatim user messages that led to this workflow, oldest first.", "[SYSTEM] Sub-agent synthesis: x",
               "use postgres", "<task-notification>\n\U0001F600\n</task-notification>"]:
    add("host_template", prompt=prompt)

for origin in [None, "host", 1, [], {}, {"kind": "person"}, {"kind": "person", "source": "record"}, {"kind": "person", "source": "template"}, {"kind": "person", "source": 1},
               {"kind": "host"}, {"kind": "host", "reason": "x"}, {"kind": "host", "reason": "x", "source": "record"}, {"kind": "host", "reason": " x ", "source": "template"},
               {"kind": "host", "reason": "", "source": "record"}, {"kind": "host", "reason": "   ", "source": "record"}, {"kind": "host", "reason": 1, "source": "record"},
               {"kind": "host", "reason": "x", "source": "other"}, {"kind": "host", "reason": "x", "source": None}, {"kind": "Host", "reason": "x", "source": "record"},
               {"kind": "host", "reason": "r" * 250, "source": "record"}, {"kind": "host", "reason": " " + "r" * 199 + "x ", "source": "record"}, {"kind": "host", "reason": "\U0001F600" * 150, "source": "record"},
               {"kind": "host", "reason": "x", "source": "record", "extra": 1}, {"kind": None}, {"source": "record"}]:
    add("supplied_origin", origin=origin)

for response in [None, "", "x", "{}", '{"answers":{}}', '{"answers": {}}', '{"answers":{}} ', '{"answers":{},"x":1}', '{"answers":[]}', '{"answers":null}', '{"answers":{"a":1}}',
                 "[]", "null", "1", "{nope", {}, {"answers": {}}, {"answers": {"a": "b"}}, {"answers": []}, {"answers": None}, {"answers": {}, "x": 1}, {"x": {}}, [], [{"answers": {}}], 1, True]:
    add("picker_empty", response=response)

def row(**fields):
    return json.dumps(fields)
def rows(*entries):
    return "\n".join(entries) + "\n"
U = {"type": "user"}
sys_row = row(type="user", promptId="p1", promptSource="system", turnOrigin="task_notification", message={"role": "user", "content": [{"type": "text", "text": "use postgres"}]})
hum_row = row(type="user", promptId="p1", turnOrigin="human", message={"role": "user", "content": "use postgres"})
TR_FIXED = [
    ("chat.jsonl", rows(hum_row, sys_row), "{P}/chat.jsonl", "p1", "use postgres"),
    ("chat.jsonl", rows(sys_row, hum_row), "{P}/chat.jsonl", "p1", "use postgres"),
    ("chat.jsonl", rows(sys_row), "{P}/chat.jsonl", "p2", "use postgres"),
    ("chat.jsonl", rows(sys_row), "{P}/chat.jsonl", None, "use postgres"),
    ("chat.jsonl", rows(sys_row), "{P}/chat.jsonl", "", "use postgres"),
    ("chat.jsonl", rows(sys_row), "{P}/chat.jsonl", " p1 ", "use postgres"),
    ("chat.jsonl", rows(sys_row), "{P}/chat.jsonl", 1, "use postgres"),
    ("chat.jsonl", rows(sys_row), "{P}/chat.jsonl", "p1", " use postgres "),
    ("chat.jsonl", rows(sys_row), "{P}/chat.jsonl", "p1", "use postgres!"),
    ("chat.jsonl", rows(sys_row), "{P}/chat.jsonl", "p1", ""),
    ("chat.jsonl", rows(sys_row), "{P}/chat.jsonl", "p1", "   "),
    ("chat.jsonl", rows(sys_row), "{P}/chat.txt", "p1", "use postgres"),
    ("chat.jsonl", rows(sys_row), "{P}/nope.jsonl", "p1", "use postgres"),
    ("chat.jsonl", rows(sys_row), None, "p1", "use postgres"),
    ("chat.jsonl", rows(sys_row), 1, "p1", "use postgres"),
    ("chat.jsonl", rows(sys_row), "{P}", "p1", "use postgres"),
    ("chat.jsonl", "", "{P}/chat.jsonl", "p1", "use postgres"),
    ("chat.jsonl", "{nope\n", "{P}/chat.jsonl", "p1", "use postgres"),
    ("chat.jsonl", "{nope\n\n" + sys_row + "\n", "{P}/chat.jsonl", "p1", "use postgres"),
    ("chat.jsonl", "[1]\nnull\n\"x\"\n" + sys_row, "{P}/chat.jsonl", "p1", "use postgres"),
    ("chat.jsonl", sys_row, "{P}/chat.jsonl", "p1", "use postgres"),
    ("chat.jsonl", rows(row(type="assistant", promptId="p1", promptSource="system", message={"content": "use postgres"})), "{P}/chat.jsonl", "p1", "use postgres"),
    ("chat.jsonl", rows(row(type="user", promptId="p1", origin={"kind": "task-notification"}, content="use postgres")), "{P}/chat.jsonl", "p1", "use postgres"),
    ("chat.jsonl", rows(row(type="user", promptId="p1", origin={"kind": "other"}, content="use postgres")), "{P}/chat.jsonl", "p1", "use postgres"),
    ("chat.jsonl", rows(row(type="user", promptId="p1", origin="task-notification", content="use postgres")), "{P}/chat.jsonl", "p1", "use postgres"),
    ("chat.jsonl", rows(row(type="user", promptId="p1", turnOrigin="sdk", content="use postgres")), "{P}/chat.jsonl", "p1", "use postgres"),
    ("chat.jsonl", rows(row(type="user", promptId="p1", turnOrigin="task_notification", content="use postgres")), "{P}/chat.jsonl", "p1", "use postgres"),
    ("chat.jsonl", rows(row(type="user", promptId="p1", promptSource="system", content="use postgres")), "{P}/chat.jsonl", "p1", "use postgres"),
    ("chat.jsonl", rows(row(type="user", promptId="p1", promptSource="system", origin={"kind": "x"}, content="use postgres")), "{P}/chat.jsonl", "p1", "use postgres"),
    ("chat.jsonl", rows(row(type="user", promptId="p1", promptSource="system", turnOrigin=1, origin={"kind": "task-notification"}, content="use postgres")), "{P}/chat.jsonl", "p1", "use postgres"),
    ("chat.jsonl", rows(row(type="user", promptId="p1", promptSource="user", content="use postgres")), "{P}/chat.jsonl", "p1", "use postgres"),
    ("chat.jsonl", rows(row(type="user", promptId="p1", content="use postgres")), "{P}/chat.jsonl", "p1", "use postgres"),
    ("chat.jsonl", rows(row(type="user", promptId="p1", message={"content": [{"type": "text", "text": "use "}, {"type": "text", "text": "postgres"}]})), "{P}/chat.jsonl", "p1", "use postgres"),
    ("chat.jsonl", rows(row(type="user", promptId="p1", message={"content": [{"type": "image"}, {"type": "text", "text": "use postgres"}]})), "{P}/chat.jsonl", "p1", "use postgres"),
    ("chat.jsonl", rows(row(type="user", promptId="p1", message={"content": [{"type": "text", "text": 1}]})), "{P}/chat.jsonl", "p1", "use postgres"),
    ("chat.jsonl", rows(row(type="user", promptId="p1", message={"content": 1})), "{P}/chat.jsonl", "p1", "use postgres"),
    ("chat.jsonl", rows(row(type="user", promptId="p1", message="use postgres")), "{P}/chat.jsonl", "p1", "use postgres"),
    ("chat.jsonl", rows(row(type="user", promptId="p1", message=None, content="use postgres")), "{P}/chat.jsonl", "p1", "use postgres"),
    ("chat.jsonl", rows(row(type="user", promptId="p1", message={"content": " use postgres\n"})), "{P}/chat.jsonl", "p1", "use postgres"),
    ("chat.jsonl", rows(row(type="user", promptId="p1", promptSource="system", content="use postgres"), row(type="user", promptId="p1", content="use postgres")), "{P}/chat.jsonl", "p1", "use postgres"),
    ("chat.jsonl", rows(row(type="user", promptId="p1", content="use postgres"), row(type="user", promptId="p1", promptSource="system", content="use postgres")), "{P}/chat.jsonl", "p1", "use postgres"),
    ("chat.jsonl", rows(row(type="user", promptId="p1", promptSource="system", content="use postgres"), row(type="user", promptId="p2", content="use postgres")), "{P}/chat.jsonl", "p1", "use postgres"),
    ("chat.jsonl", rows(row(type="user", promptId=1, promptSource="system", content="use postgres")), "{P}/chat.jsonl", "p1", "use postgres"),
    ("chat.jsonl", rows(row(type="user", promptId=1, promptSource="system", content="use postgres")), "{P}/chat.jsonl", None, "use postgres"),
    ("chat.jsonl", rows(row(type="user", promptId="p1", promptSource="system", content="\U0001F600 use postgres")), "{P}/chat.jsonl", "p1", "\U0001F600 use postgres"),
    ("chat.jsonl", "x" * 2000000 + "\n" + sys_row + "\n", "{P}/chat.jsonl", "p1", "use postgres"),
    ("chat.jsonl", sys_row + "\n" + "x" * 2000000 + "\n", "{P}/chat.jsonl", "p1", "use postgres"),
    ("chat.jsonl", "\n".join([row(type="user", promptId="p1", content=f"m{n}") for n in range(3000)] + [sys_row]) + "\n", "{P}/chat.jsonl", "p1", "use postgres"),
    ("sub/dir/chat.jsonl", rows(sys_row), "{P}/sub/dir/chat.jsonl", "p1", "use postgres"),
    ("chat.txt", rows(sys_row), "{P}/chat.txt", "p1", "use postgres"),
    ("chat.JSONL", rows(sys_row), "{P}/chat.JSONL", "p1", "use postgres"),
    ("chat.jsonl.bak", rows(sys_row), "{P}/chat.jsonl.bak", "p1", "use postgres"),
]
for file, text, path, prompt_id, prompt in TR_FIXED:
    add("transcript_origin", file=file, rows=text, transcript_path=path, prompt_id=prompt_id, prompt=prompt)

# --- The Plan Approval guard's pure readers (M4, slice 4e-i) ---------------------
#
# guard_operation: a guard's recovery operations, rendered, matched and worded.
# plan_dispatch: a developer handoff judged against the evidence gathered per target.
# plan_reason: the refusals the guard prints, with their briefs.
# contract_markers, questions_file, review_appendix: what the guard reads of
# the Testing Posture contract (the handoff's contract lines, the questions
# file's Plan Approval answer, the plan's terminal review appendix).

GO_FENCES = ["plan-approval", "review-freeze", "state-transition", "reviewer-scope"]
GO_IDENTS = ["a", "code-generation", "auth", "x.y", "a_b", "9", "A-Z", "unit-1", "s" * 256, "s" * 257, "", "-a", ".a",
             "a b", "a/b", "ünit", "a\tb", "CodeGen"]
GO_KINDS = ["restart-stage", "abort-bolt", "lower-fence", "record-unit-completion", "reopen-unit", "review-advisory"]


def go_ident():
    return rng.choice(GO_IDENTS)


def go_operation(valid=True):
    kind = rng.choice(GO_KINDS)
    ident = (lambda: rng.choice(GO_IDENTS[:8])) if valid else go_ident
    if kind == "restart-stage":
        op = {"kind": kind, "stage": ident()}
    elif kind == "abort-bolt":
        op = {"kind": kind, "unit": ident(), "slug": ident()}
    elif kind == "lower-fence":
        op = {"kind": kind, "fence": rng.choice(GO_FENCES if valid else GO_FENCES + ["human-presence", "", "guard.plan-approval", 1])}
    elif kind == "review-advisory":
        op = {"kind": kind}
    else:
        op = {"kind": kind, "stage": ident(), "unit": ident()}
    if not valid and rng.random() < 0.3:
        choice = rng.random()
        if choice < 0.3:
            op["extra"] = 1
        elif choice < 0.6 and len(op) > 1:
            op.pop(rng.choice([k for k in op if k != "kind"]))
        elif choice < 0.8:
            op["kind"] = rng.choice(["restart", "lower_fence", "", 1, None])
        else:
            op = rng.choice([None, [], "restart-stage", 1, {"kind": kind, "stage": 1}])
    return op


def go_render(op, mode, harness, shell):
    # The rendering as upstream spells it, for commands that must match exactly.
    if not isinstance(op, dict) or op.get("kind") not in GO_KINDS:
        return "x"

    def q(w):
        w = re.sub(r"[\x00-\x1f\x7f]", "?", str(w))
        if re.fullmatch(r"[A-Za-z0-9_./:@%+=,-]+", w):
            return w
        return "'" + (w.replace("'", "''") if shell == "powershell" else w.replace("'", "'\"'\"'")) + "'"

    k = op["kind"]
    if k == "restart-stage":
        route, args, source = "orchestrate", ["next", "--stage", op.get("stage", "")], None
    elif k == "abort-bolt":
        route, args, source = "bolt", ["abort", "--name", op.get("unit", ""), "--slug", op.get("slug", ""), "--reason", "stale review recovery exhausted", "--discard"], None
    elif k == "lower-fence":
        key = f"guard.{op.get('fence', '')}"
        route, args, source = "config", ["set", key, "off"], ("utility", ["config-change", f"--{key}", "off"])
    elif k == "record-unit-completion":
        route, args, source = "state", ["unit", "complete", "--stage", op.get("stage", ""), "--unit", op.get("unit", "")], None
    elif k == "reopen-unit":
        route, args, source = "jump", ["reopen", "--target", op.get("stage", ""), "--units", op.get("unit", "")], None
    else:
        route, args, source = "config", ["set", "review", "advisory"], ("utility", ["config-change", "--review", "advisory"])
    if mode == "source" and source:
        route, args = source
    prefix = f"aidlc engine {q(route)}" if mode == "native" else f"bun {q(f'{harness}/tools/aidlc-{route}.ts')}"
    return f"{prefix} " + " ".join(q(a) for a in args)


GO_REMEDIES = ["present-approval-gate", "request-review", "start-recovery-review", "apply-repairs-then-request", "record-verdict",
               "retry-pending", "request-changes", "finish-revision", "redo-jump", "restore-or-jump", "restart-stage", "redo-unit-step",
               "reopen-unit-step", "review-advisory-gate", "change-scope", "restore-scope", "abort-bolt", "record-unit-completion",
               "repair-source-boundary", "reconfirm-summary", "unset-unattended", "lower-fence", "nope", "restart_stage", ""]
GO_OWN_REMEDIES = {"restart-stage": ["restart-stage", "redo-jump", "restore-or-jump"], "abort-bolt": ["abort-bolt"], "lower-fence": ["lower-fence"],
                   "record-unit-completion": ["record-unit-completion"], "reopen-unit": ["reopen-unit-step"], "review-advisory": ["review-advisory-gate"]}

for _ in range(scale * 40):
    op = go_operation(valid=rng.random() < 0.7)
    add("guard_operation", op="parse", operation=op)
    add("guard_operation", op="invocation", operation=op)
for _ in range(scale * 40):
    op = go_operation(valid=rng.random() < 0.85)
    mode, shell = rng.choice(["native", "source"]), rng.choice(["posix", "powershell"])
    harness = rng.choice([".claude", ".cursor", ".aidlc", ".a-b_c.d", "claude", ".", "./x", "", ".claude/x"])
    add("guard_operation", op="render", operation=op, mode=mode, harness=harness, shell=shell)
for _ in range(scale * 50):
    op = go_operation(valid=rng.random() < 0.85)
    mode, shell = rng.choice(["native", "source"]), rng.choice(["posix", "powershell"])
    harness = rng.choice([".claude", ".cursor", ".aidlc", ".a-b_c.d"])
    command = go_render(op, mode, harness, shell)
    edit = rng.random()
    if edit < 0.15:
        command += rng.choice([" ", " && echo", " --x", "\n", ";"])
    elif edit < 0.3:
        command = rng.choice(["cd x && ", "sudo ", " ", "bun ", "aidlc "]) + command
    elif edit < 0.4:
        command = command.replace("aidlc engine", "aidlc.exe engine").replace("aidlc-", "aidlc_")
    elif edit < 0.5:
        command = command.upper() if rng.random() < 0.5 else command.replace("'", '"')
    add("guard_operation", op="matches_command", operation=op, command=command)
for _ in range(scale * 40):
    op = go_operation(valid=rng.random() < 0.85)
    stage = rng.choice(GO_IDENTS[:8])
    unit = rng.choice(GO_IDENTS[:8] + [None])
    if isinstance(op, dict) and rng.random() < 0.5:
        stage = op.get("stage", stage)
        unit = op.get("unit", unit)
    remedy = rng.choice(GO_OWN_REMEDIES.get(op.get("kind"), GO_REMEDIES) if isinstance(op, dict) and rng.random() < 0.5 else GO_REMEDIES)
    add("guard_operation", op="matches_remedy", operation=op, remedy=remedy, stage=stage, unit=unit)
for _ in range(scale * 30):
    left = go_operation(valid=rng.random() < 0.8)
    right = left if rng.random() < 0.4 else go_operation(valid=rng.random() < 0.8)
    if isinstance(right, dict) and rng.random() < 0.2:
        right = {**right, "extra": 1}
    case = {}
    if rng.random() < 0.9:
        case["left"] = left
    if rng.random() < 0.9:
        case["right"] = right
    add("guard_operation", op="same", **case)
GO_CONTINUATIONS = [
    "aidlc engine jump execute --target {s} --direction {d} --scope {c}",
    "aidlc.exe engine jump execute --target {s} --direction {d} --scope {c}",
    "bun {h}/tools/aidlc-jump.ts execute --target {s} --direction {d} --scope {c}",
    "bun {h}/tools/aidlc-jump.ts  execute --target {s} --direction {d} --scope {c}",
    "aidlc engine jump execute --target {s} --direction {d} --scope {c} ",
    "aidlc engine jump execute --target {s} --direction {d} --scope {c} --x",
    "aidlc engine jump execute --target {s} --scope {c} --direction {d}",
    "aidlc engine jump reopen --target {s} --direction {d} --scope {c}",
    "aidlc jump execute --target {s} --direction {d} --scope {c}",
    "cd x && aidlc engine jump execute --target {s} --direction {d} --scope {c}",
    "aidlc engine jump execute --target {s} --direction {d} --scope {c} | cat",
    "bun {h}/tools/aidlc.ts engine jump execute --target {s} --direction {d} --scope {c}",
    "",
    "aidlc",
]
for _ in range(scale * 30):
    harness = rng.choice([".claude", ".cursor", None])
    ident = (lambda: rng.choice(GO_IDENTS[:8])) if rng.random() < 0.7 else go_ident
    command = rng.choice(GO_CONTINUATIONS).format(
        s=ident(), d=rng.choice(["redo", "backward", "redo", "backward", "forward", "Redo", ""]), c=ident(),
        h=rng.choice([".claude", ".cursor", ".x"]))
    add("guard_operation", op="continuation", command=command, harness=harness)
GO_ENGINE_ARGS = [
    ["engine", "config", "set", "guard.{f}", "off"],
    ["engine", "config", "set", "guard.{f}", "on"],
    ["engine", "config", "set", "guard.{f}", "off", "--yes"],
    ["engine", "config", "set", "{f}", "off"],
    ["engine", "utility", "config-change", "--guard.{f}", "off"],
    ["engine", "utility", "config-change", "--guard.{f}", "on"],
    ["engine", "utility", "config-change", "--guard-policy", "off"],
    ["engine", "bolt", "abort", "--name", "{u}", "--slug", "{s}", "--reason", "stale review recovery exhausted", "--discard"],
    ["engine", "bolt", "abort", "--name", "{u}", "--slug", "{s}", "--reason", "stale review recovery exhausted", "--keep"],
    ["engine", "bolt", "abort", "--name", "{u}", "--slug", "{s}", "--reason", "stale review recovery exhausted", "--discard", "--x"],
    ["engine", "bolt", "abort", "--name", "{u}", "--slug", "{s}", "--reason", "other", "--discard"],
    ["engine", "bolt", "abort", "--slug", "{s}", "--name", "{u}", "--reason", "stale review recovery exhausted", "--discard"],
    ["engine", "bolt", "abort", "--name", "{u}", "--slug", "{s}", "--discard"],
    ["engine", "orchestrate", "next", "--stage", "{s}"],
    ["engine", "state", "unit", "complete", "--stage", "{s}", "--unit", "{u}"],
    ["engine", "jump", "reopen", "--target", "{s}", "--units", "{u}"],
    ["engine", "config", "set", "review", "advisory"],
    ["engine", "utility", "config-change", "--review", "advisory"],
    ["config", "set", "guard.{f}", "off"],
    ["engine"],
    [],
]
for _ in range(scale * 40):
    fence, unit, stage = rng.choice(GO_FENCES * 3 + ["human-presence", "x"]), rng.choice(GO_IDENTS[:8] * 3 + GO_IDENTS), rng.choice(GO_IDENTS[:8] * 3 + GO_IDENTS)
    args = [a.format(f=fence, u=unit, s=stage) for a in rng.choice(GO_ENGINE_ARGS)]
    add("guard_operation", op="engine_invocation", args=args)
    spelled = rng.choice([{"kind": "lower-fence", "fence": fence}, {"kind": "abort-bolt", "unit": unit, "slug": stage}, {"kind": "restart-stage", "stage": stage},
                          {"kind": "record-unit-completion", "stage": stage, "unit": unit}, {"kind": "reopen-unit", "stage": stage, "unit": unit}, {"kind": "review-advisory"}])
    add("guard_operation", op="matches_engine_args", operation=spelled if rng.random() < 0.6 else go_operation(valid=rng.random() < 0.85), args=args)
for _ in range(scale * 30):
    add("guard_operation", op="wording", remedy=rng.choice(GO_REMEDIES),
        code=rng.choice(["UNIT_COMPLETION_MISSING", "REVIEW_STALE", "X", ""]), stage=rng.choice(["Code Generation", "Build", "s"]),
        target=rng.choice(["the plan", "Code Generation for beta", "it's work"]), unit=rng.choice([None, "beta", "a b"]),
        fence=rng.choice([None] + GO_FENCES), source_unbindable=rng.random() < 0.3, every_unit=rng.random() < 0.4)

PD_HASH = ["sha256:" + "a" * 64, "sha256:" + "b" * 64, "sha256:" + "0" * 64]


def pd_evidence(unit, strong=True):
    hash_ = rng.choice(PD_HASH + [None]) if not strong or rng.random() < 0.2 else PD_HASH[0]
    flags = {k: (True if strong and rng.random() < 0.85 else rng.random() < 0.5) for k in
             ["planExists", "instructionsExist", "approved", "contractValid", "fingerprintValid", "receiptValid"]}
    ev = {"unit": unit, **flags, "contractHash": hash_}
    if rng.random() < 0.3:
        ev["reason"] = rng.choice(["the plan moved after approval.", "stale receipt", "", "ended attempt..."])
    if rng.random() < 0.3:
        ev["reviewAppendix"] = rng.choice(["## Review\n\nLooks fine\n", "## Review\n", "\n## Review\n\n- one\n- two\n", "## Review: x\nbody\n", "Review\nno heading\n", ""])
    return ev


def pd_prompt(units, evidence=()):
    lines = []
    if rng.random() < 0.6:
        # One target, as a brief names it, with that target's contract and perhaps its appendix quoted.
        target = rng.choice(list(evidence)) if evidence else None
        if target is None or target["unit"] is None:
            lines.append("AIDLC-STAGE: " + rng.choice(["code-generation", "Code Generation"]))
        else:
            lines.append("AIDLC-UNIT: " + target["unit"])
        if target is not None and rng.random() < 0.8:
            lines.append("AIDLC-TESTING-CONTRACT: " + (target.get("contractHash") or rng.choice(PD_HASH)))
        if target is not None and target.get("reviewAppendix") and rng.random() < 0.6:
            appendix = target["reviewAppendix"]
            lines.append(rng.choice([appendix, re.sub(r"\s+", " ", appendix).strip(), "The review: " + appendix.replace("## Review", "").strip(), appendix.upper()]))
    for _ in range(rng.randrange(0, 4)):
        pick = rng.random()
        if pick < 0.35:
            lines.append(rng.choice(["", " ", "\t"]) + "AIDLC-UNIT" + rng.choice(["", " "]) + ":" + rng.choice(["", " ", "  "]) + rng.choice([u for u in units if u] + ["ghost", "", " "]) + rng.choice(["", " "]))
        elif pick < 0.6:
            lines.append(rng.choice(["", "  "]) + "AIDLC-STAGE: " + rng.choice(["code-generation", "Code Generation", "code generation", "build", "", " "]))
        elif pick < 0.85:
            lines.append("AIDLC-TESTING-CONTRACT: " + rng.choice(PD_HASH + ["sha256:xyz", ""]))
        else:
            lines.append(rng.choice(["Looks fine", "The review said: Looks fine", "- one\n- two", "body", "aidlc-unit: a", "AIDLC-UNIT: a; AIDLC-UNIT: b"]))
    rng.shuffle(lines)
    return rng.choice(["\n", "\r\n"]).join(lines) + rng.choice(["", "\n"])


for _ in range(scale * 80):
    names = rng.sample(["auth", "api", "web", "a b", "x.y"], rng.randrange(0, 4))
    units = [pd_evidence(None, strong=rng.random() < 0.7)] + [pd_evidence(name, strong=rng.random() < 0.7) for name in names]
    if rng.random() < 0.1:
        units = units[1:]
    add("plan_dispatch", tool=rng.choice(["Task"] * 5 + ["Agent"] * 2 + ["Bash", "Write", ""]),
        subagent=rng.choice(["aidlc-developer-agent"] * 6 + ["aidlc-reviewer-agent", ""]),
        prompt=pd_prompt(names + [None], units), current_stage=rng.choice(["code-generation"] * 4 + ["Code Generation", " code-generation ", "build", ""]),
        units=units)

PR_MENTIONED = [[], ["stage:code-generation"], ["auth"], ["a b"], ["auth", "api"], ["auth", "stage:code-generation"], ["it's"], [""]]
PR_TARGETS = [None, [], [None], ["auth"], ["a b", None], ["x", "y"], [""]]
PR_DETAIL = [None, "", "the plan moved after approval.", "stale receipt...", "ended attempt"]
for _ in range(scale * 25):
    mentioned = rng.choice(PR_MENTIONED)
    targets = rng.choice(PR_TARGETS)
    add("plan_reason", op="appendix", mentioned=mentioned)
    add("plan_reason", op="block", mentioned=mentioned, detail=rng.choice(PR_DETAIL), targets=targets)
    add("plan_reason", op="handoff", mentioned=mentioned, cause=rng.choice(["targets", "unknown-target", "contract"]), targets=targets)
    add("plan_reason", op="mutation", unit=rng.choice([None, "auth", "a b"]), opaque=rng.random() < 0.5, detail=rng.choice(PR_DETAIL))
    evidence = [pd_evidence(u, strong=rng.random() < 0.3) for u in [None, "auth", "api"]]
    for ev in evidence:
        if rng.random() < 0.5:
            ev["reason"] = rng.choice(["the plan moved after approval.", "stale receipt", ""])
    add("plan_reason", op="receipt", evidence=evidence, mentioned=mentioned)

CM_LINES = ["AIDLC-TESTING-CONTRACT: {h}", "  AIDLC-TESTING-CONTRACT:{h}  ", "AIDLC-TESTING-CONTRACT : {h}", "AIDLC-TESTING-CONTRACT: {h} x",
            "aidlc-testing-contract: {h}", "AIDLC-TESTING-CONTRACT: sha256:{short}", "AIDLC-TESTING-CONTRACT: {H}", "text", "", "AIDLC-TESTING-CONTRACT:"]
for _ in range(scale * 30):
    lines = []
    for _ in range(rng.randrange(0, 6)):
        h = rng.choice(PD_HASH)
        lines.append(rng.choice(CM_LINES).format(h=h, H=h.upper(), short="a" * 63))
    add("contract_markers", text=rng.choice(["\n", "\r\n"]).join(lines))

QF_HEADINGS = ["## Plan Approval", "## Q1: Plan Approval", "## 2. Plan Approval", "## Question 3 - Plan Approval", "## Plan Approval?",
               "## **Plan Approval**", "## _Plan Approval_:", "### Plan Approval", "# Plan Approval", "## plan approval", "## Plan  Approval",
               "## Approve the plan", "## Q1", "## 2.", "## Question 3:", "## Notes", "### Notes", "## Q2: Scope", "#### Deep", "##Plan Approval",
               "## Plan Approval ##", "## *Plan Approval*"]
QF_ANSWERS = ["[Answer]: Approve Plan", "[Answer]: \"Approve Plan\"", "[Answer]: a) Approve Plan", "[Answer]: B. 'Approve Plan'",
              "[Answer]: approve plan", "[Answer]: Plan approval off", "[Answer]: Request Changes", "[Answer]: ___", "[Answer]:", "[Answer]: _",
              "[Answer]:   Approve Plan  ", "[Answer]: Approve Plan!", "[answer]: Approve Plan", " [Answer]: Approve Plan", "[Answer] Approve Plan"]
QF_TAGS = ["[Approval Fingerprint]: sha256:v3:" + "a" * 64, "[Approval Fingerprint]: sha256:v2:" + "b" * 64, "[Approval Fingerprint]: sha256:" + "c" * 64,
           "[Approval Fingerprint]:", "[Approval Fingerprint]: sha256:v4:" + "a" * 64, "[Planned Source]: " + "d" * 40, "[Planned Source]: " + "e" * 64,
           "[Planned Source]: unbindable", "[Planned Source]:", "[Planned Source]: x"]
QF_OTHER = ["body text", "- \"Approve Plan\": proceed", "", "```\n[Answer]: Approve Plan\n```", "<!-- [Answer]: Approve Plan -->", "<div>\n[Answer]: Approve Plan\n</div>",
            "    [Answer]: Approve Plan", "**Plan Approval**", "Plan Approval", "2.", "> [Answer]: Approve Plan"]
for _ in range(scale * 60):
    lines = []
    for _ in range(rng.randrange(1, 12)):
        pick = rng.random()
        if pick < 0.3:
            lines.append(rng.choice(QF_HEADINGS))
        elif pick < 0.55:
            lines.append(rng.choice(QF_ANSWERS))
        elif pick < 0.75:
            lines.append(rng.choice(QF_TAGS))
        else:
            lines.append(rng.choice(QF_OTHER))
    body = rng.choice(["\n", "\n\n", "\r\n"]).join(lines) + rng.choice(["", "\n"])
    if rng.random() < 0.1:
        body = "﻿" + body
    add("questions_file", body=body)
# Written by hand: the section's edges the generated files reach too rarely.
QF_FP = "[Approval Fingerprint]: sha256:v3:"
for body in [
    "## Plan Approval\n\n### Q2: Next\n\n[Answer]: Approve Plan\n",
    "## Plan Approval\n\n### 2.\n\n[Answer]: Approve Plan\n",
    "## Plan Approval\n\n### Notes\n\n[Answer]: Approve Plan\n",
    "## Plan Approval\n\n[Answer]: ___\n\n### Notes\n\n[Answer]: Approve Plan\n",
    "## Plan Approval\n\n[Answer]: Approve Plan\n\n### Notes\n\n[Answer]: ___\n",
    "## Plan Approval\n\n" + QF_FP + "a" * 64 + "\n\n### Notes\n\n" + QF_FP + "b" * 64 + "\n[Answer]: Approve Plan\n",
    "## Plan Approval\n\n[Planned Source]: unbindable\n\n### Notes\n\n[Planned Source]: " + "d" * 40 + "\n[Answer]: Approve Plan\n",
    "## Plan Approval\n\n### Notes\n\n" + QF_FP + "b" * 64 + "\n[Planned Source]: unbindable\n[Answer]: Approve Plan\n",
    "#### Plan Approval\n\n## 2.\nPlan Approval\n\n### Notes\n\n[Answer]: Approve Plan\n",
    "## 2.\nPlan Approval\n\n[Answer]: ___\n\n### Notes\n\n[Answer]: Approve Plan\n",
    "## 2.\n\n**Plan Approval**\n[Answer]: Approve Plan\n",
    "## 2.\nNot it\nPlan Approval\n[Answer]: Approve Plan\n",
    "## **\n[Answer]: Approve Plan\n",
    "## ****\n[Answer]: Approve Plan\n",
    "## __Plan Approval__\n[Answer]: Approve Plan\n",
    "## *Plan Approval*:\n[Answer]: Approve Plan\n",
    "## Plan Approval\n[Answer]: Approve Plan\n## Plan Approval\n[Answer]: ___\n",
    "## Q1: Plan Approval\n[Answer]: ___\n## Q2: Plan Approval\n[Answer]: Approve Plan\n## Q3: Scope\n[Answer]: x\n",
]:
    add("questions_file", body=body)

RA_PARTS = ["# Plan", "## Steps", "### Detail", "body text", "", "## Review", "## Review  ", "## Review ##", "##  Review", "## Reviewed", "Review\n---",
            "Review\n===", "<h2>Rendered</h2>", "<h1>Top</h1>", "<div>\n## Review\n</div>", "<!-- ## Review -->", "<!-- <h1>x</h1> -->",
            "<script><h2>s</h2></script>", "```\n## Review\n```", "> ## Review", "- ## Review", "text\n## Review", "verdict", "- one\n- two",
            "<H2 class=\"x\">y</H2>", "<h2/>", "<h20>no</h20>"]
for _ in range(scale * 60):
    parts = [rng.choice(RA_PARTS) for _ in range(rng.randrange(1, 9))]
    if rng.random() < 0.5:
        parts.append("## Review")
        parts.extend(rng.choice(RA_PARTS[3:5] + ["verdict", "### Finding", "- one\n- two"]) for _ in range(rng.randrange(0, 3)))
    sep = rng.choice(["\n\n", "\n", "\r\n\r\n", "\n\n\n", "\n  \n"])
    plan = sep.join(parts) + rng.choice(["", "\n", "\n\n", "\r\n"])
    add("review_appendix", plan=plan)
for plan in [
    "# Plan\n\n## Review\n\n<!-- <h1>x</h1> -->\n",
    "# Plan\n\n## Review\n\n<!-- <h1>x</h1>\n",
    "# Plan\n\n## Review\n\n<h20>no</h20>\n",
    "# Plan\n\n## Review\n\n<div><h20>no</h20></div>\n",
    "# Plan\n\n## Review\n\n<div><h2x>no</h2x></div>\n",
    "# Plan\n\n## Review\n\n<div>\n<h1 >yes</h1>\n</div>\n",
    "# Plan\n\n## Review\n\n<div><h2/></div>\n",
    "# Plan\n\n## Review\n\n<h2>yes</h2>\n",
    "# Plan\n\n## Review\n\n<h1/>\n",
    "# Plan\n\n## Review\n\n<H1 id=\"x\">y</H1>\n",
    "# Plan\n\n## Review\n\n<script>\n<h1>x</h1>\n</script>\n",
    "# Plan\n\n## Review\n\n<textarea><h2>x</h2></textarea>\n",
    "# Plan\n\n## Review\n\n<div><h2>x</h2></div>\n",
    "# Plan\n\n## Review\n\nB\n--\n",
    "# Plan\n\n## Review\n\nB\n==\n",
    "# Plan\n\n## Review\n\n# B\n",
    "# Plan\n\n## Review\n\n### B\n",
    "## Review\n\n## Review\n",
    "## Review\n\ntext\n\n## Review\n",
]:
    add("review_appendix", plan=plan)

# --- What runs while a plan waits (M4, slice 4e-ii-a) ----------------------------
#
# engine_route: the dispatcher's engine namespace, a command to the tool and
# arguments it runs as. every_helper: what every helper may run, judged by that
# tool and verb. plan_prerequisite, engine_directed, read_only_diagnostic: the
# engine commands the Plan Approval guard admits before the person approved.
# plain_powershell, dynamic_evaluation: how the guard reads a command line.
# All but engine_route are private to the hook: the driver asks them through
# an export shim over the hook's own source.

ER_ROUTES = {
    "top": ["status", "recompose", "now", "statusline", "adapter"],
    "state": ["get", "set", "set-skeleton-stance", "set-construction-iteration", "set-construction-checkpoints", "set-construction-execution", "set-construction-verification-command", "checkbox", "count", "advance", "finalize", "complete-workflow", "gate-start", "approve", "reject", "revise", "skip", "resume", "acknowledge-compaction", "reuse-artifact", "lookup", "practices-event", "practices-promote", "set-unit-ownership", "set-unit-gate-rhythm", "refresh-unit-progress", "sync-unit-scope-stage", "fold-unit-merge", "fork", "merge", "park", "unpark", "unit", "set-status", "init"],
    "audit": ["append", "append-batch", "append-raw", "history", "fork", "merge"],
    "graph": ["artifacts", "producers", "consumers", "topo", "cycles", "scope", "validate-scope", "validate-grid", "ars", "compile", "resolve", "export"],
    "runtime": ["compile", "read", "summary", "fragment-fork", "fragment-merge"],
    "sensor": ["list", "describe", "fire"],
    "swarm": ["prepare", "check", "finalize"],
    "bolt": ["start", "complete", "fail", "abort", "set-autonomy", "checkpoint", "swarm-checkpoint", "dispatch-event", "hold-merge", "release-merge"],
    "worktree": ["create", "merge", "discard", "restore", "purge", "list", "verify", "info"],
    "jump": ["resolve", "execute", "reopen"],
    "log": ["decision", "answer", "answers", "review", "link"],
    "learnings": ["surface", "persist"],
    "testing-posture": ["resolve", "render", "fingerprint", "verify", "begin", "brief", "reply", "restore"],
    "validate": ["outputs"],
    "intent": ["list", "switch", "<name>", "create", "archive", "unarchive", "add-repo", "remove-repo"],
    "space": ["list", "switch", "<name>", "create"],
    "scope": ["change", "save", "detect", "resolve-env"],
    "config": ["depth", "test-strategy", "review", "guard-policy", "change-control", "sensors", "learnings", "collaborators", "summary-confirmation", "plan-approval", "guard.plan-approval", "guard.review-freeze", "guard.state-transition", "guard.reviewer-scope", "get", "list"],
    "plugin": ["select", "sync", "list", "validate", "build"],
    "knowledge": ["onboard", "sync", "list", "show", "associate", "dissociate", "rebind", "summarize", "help"],
    "gen": ["runners", "runners --check", "runner-list", "runner-scopes", "stage-table", "scope-table"],
    "workspace": ["detect", "reclassify", "codekb", "codekb-scope-diff", "codekb-snapshot", "codekb-publish", "project-description", "document-input"],
    "review-brief": ["review", "context", "summary"],
    "orchestrate": ["next", "continue", "report", "park", "wait", "team-board", "help"],
    "sensor-claim-sources": ["<args>"],
    "sensor-linter": ["<args>"],
    "sensor-required-sections": ["<args>"],
    "sensor-traceability": ["<args>"],
    "sensor-type-check": ["<args>"],
    "sensor-upstream-coverage": ["<args>"],
    "hook": ["<name>"],
}
ER_VERBS = sorted({verb for verbs in ER_ROUTES.values() for verb in verbs})
ER_HELPERS = {
    "utility": ["help", "version", "now", "status", "detect", "project-description", "codekb-path", "codekb-scope-diff", "codekb-snapshot",
                "codekb-publish", "plugin-list", "plugin-validate", "config-get", "config-list", "resolve-env-scope", "scope-table",
                "stage-table", "select-plugins", "document-input", "config-change", "intent-create", "state-init", "claim", "intent"],
    "state": ["get", "count", "resume", "lookup"], "orchestrate": ["wait", "team-board"], "jump": ["resolve"],
    "unit": ["merge-status", "status", "claim"], "log": ["answers"], "learnings": ["surface"], "runtime": ["read", "summary"],
    "testing-posture": ["resolve", "render", "verify", "reply", "brief"], "worktree": ["list", "verify", "info"], "audit": ["history"],
    "plugin": ["list"], "graph": ["artifacts", "producers", "consumers", "topo", "cycles", "scope", "validate-scope", "validate-grid", "ars", "export"],
    "sensor": ["list", "describe", "fire"], "validate": ["outputs"], "review-brief": ["review", "context", "summary"],
    "attest": ["resolve", "record"], "knowledge": ["list", "show"], "doctor": ["doctor"], "machine-config": ["global"],
}
ER_TAILS = [[], ["x"], ["--json"], ["--stage", "code-generation"], ["--project-dir", "/oracle/other"], ["--project-dir", "/oracle/other", "x"],
            ["--", "literal", "--project-dir", "y"], ["--yes", "--", "--json"], ["--project-dir=/oracle/x"], ["--project-dir"],
            ["--project-dir", "--json"], ["--project-dir", "/a", "--project-dir", "/b"], ["--verbose", "a b"], ["--unit", "auth", "--quiet"],
            ["--help"], ["", "x"], ["--intent", "a"], ["--space=b"], ["a", "--", "b"], ["--no-color", "--offline"]]
ER_FIRST = ["status", "recompose", "now", "statusline", "adapter", "help", "--help", "-h", "--status", "--claim", "--release", "--doctor",
            "--version", "--resume", "--scope", "__sensor-script", "__sensor-script-file", "nope", "top", "hook", "unit", "doctor", "version",
            "config", "park", "next", ""]
ER_SENSORS = ["claim-sources", "linter", "required-sections", "traceability", "type-check", "upstream-coverage", "nope", ""]
ER_WORKSPACE = ["list", "switch", "create", "archive", "unarchive", "add-repo", "remove-repo", "help", "-h", "--help", "--json", "--all", "rename", "show",
                "birth", "my-intent", "--x", "space-create"]
ER_CONFIG_KEYS = ["depth", "test-strategy", "review", "guard-policy", "change-control", "sensors", "learnings", "collaborators",
                  "summary-confirmation", "plan-approval", "guard.plan-approval", "guard.review-freeze", "guard.state-transition",
                  "guard.reviewer-scope", "guard.human-presence", "nope", ""]


def er_both(args):
    # A relative --project-dir is resolved against the dispatcher's working
    # directory, which is the host's: those lines are left to the unit tests.
    before_literal = args[:args.index("--")] if "--" in args else args
    if any(arg == "--project-dir" and index + 1 < len(before_literal) and not before_literal[index + 1].startswith(("/", "--"))
           and before_literal[index + 1] != "" for index, arg in enumerate(before_literal)):
        return
    add("engine_route", args=args)
    add("every_helper", args=args)


for noun, verbs in ER_ROUTES.items():
    for verb in verbs:
        er_both(["engine", noun, verb])
        er_both(["engine", noun, verb] + rng.choice(ER_TAILS[1:]))
    er_both(["engine", noun])
    er_both(["engine", noun, rng.choice(["nope", "", "--help", "help", "-h"])])
for stem, verbs in ER_HELPERS.items():
    for verb in verbs:
        er_both(["engine", stem, verb])
        er_both(["engine", stem, verb] + rng.choice(ER_TAILS[1:]))
for first in ER_FIRST:
    er_both(["engine", first])
    er_both(["engine", first] + rng.choice(ER_TAILS[1:]))
    # A line that does not open with `engine` is not the engine namespace:
    # only the helper rule is asked, which admits none.
    add("every_helper", args=[first, "graph", "topo"])
for sensor in ER_SENSORS:
    er_both(["engine", "__sensor-script", sensor, "a"])
    er_both(["engine", "__sensor-script-file", sensor, "a"])
for noun in ["intent", "space"]:
    for verb in ER_WORKSPACE:
        er_both(["engine", noun, verb])
        er_both(["engine", noun, verb, rng.choice(["a", "--x", "b c", ""])])
        er_both(["engine", noun, verb, "a", rng.choice(["--reason", "--json", "--all", "x"]), "y"])
for key in ER_CONFIG_KEYS:
    er_both(["engine", "config", "set", key])
    er_both(["engine", "config", "set", key, rng.choice(["off", "on", "standard", ""])])
    er_both(["engine", "config", "set", key, "off", "--depth", "standard"])
for verb in ["get", "list", "help", "--help", "-h", "nope"]:
    er_both(["engine", "config", verb, "depth"])
for verb in ["select", "sync", "list", "validate", "build", "help", "-h", "--help", "nope"]:
    er_both(["engine", "plugin", verb])
    er_both(["engine", "plugin", verb, "a"])
for tail in [["runners"], ["runners", "--check"], ["runners", "--check", "x"], ["runners", "x"], ["runner-list"], ["runner-scopes", "a"],
             ["stage-table"], ["scope-table", "--json"], ["nope"], []]:
    er_both(["engine", "gen"] + tail)
# Written by hand: the edges the sweeps above reach too rarely.
for args in [
    ["engine", "space", "list", "--all"], ["engine", "intent", "list", "--all"], ["engine", "space", "list", "--all", "x"],
    ["engine", "intent", "list", "x", "--all"], ["engine", "graph", "topo", "--project-dir", ""], ["engine", "graph", "topo", "--project-dir", "", "x"],
    ["engine", "log", "answer", "--", "a", "--project-dir", "/oracle/y"], ["engine", "log", "answer", "--", "--project-dir"],
    ["engine", "log", "answer", "--", "--project-dir=/oracle/y", "--json"], ["engine", "utility", "--project-dir", "/oracle/other", "status"],
    ["engine", "utility", "--project-dir", "/oracle/other", "config-change"], ["engine", "utility", "status", "--project-dir", "/oracle/other"],
    ["engine", "doctor", "--project-dir", "/oracle/other", "doctor"], ["engine", "attest", "--project-dir", "/oracle/other", "resolve", "x"],
]:
    er_both(args)
for _ in range(scale * 60):
    noun = rng.choice(list(ER_ROUTES) + list(ER_HELPERS) + ER_FIRST)
    args = ["engine", noun]
    if rng.random() < 0.9:
        args.append(rng.choice(ER_ROUTES.get(noun, []) + ER_HELPERS.get(noun, []) + [rng.choice(ER_VERBS)]))
    for _ in range(rng.randrange(0, 3)):
        args += rng.choice(ER_TAILS)
    if rng.random() < 0.15:
        args.insert(rng.randrange(0, len(args) + 1), rng.choice(["--json", "--yes", "--quiet", "--project-dir", "--"]))
    er_both(args)

PP_TEMPLATES = [
    "engine orchestrate {v}", "engine orchestrate report --stage {s} --result {r}", "engine orchestrate report --result {r} --stage {s} --stage {s}",
    "engine orchestrate report --skeleton-stance {k}", "engine orchestrate report --skeleton-stance {k} --result {r}",
    "engine orchestrate report --skeleton-stance {k} --stage {s}", "engine orchestrate report --skeleton-stance", "engine state unpark",
    "engine state unit {u}", "engine state unit {u} --unit auth", "engine state {c}", "engine bolt set-autonomy --mode x",
    "engine bolt {b}", "engine bolt {b} --action {a}", "engine bolt {b} --action", "engine bolt {b} --action {a} --action {a}",
    "engine bolt {b} --action --x", "engine testing-posture {t}", "engine runtime {m}", "engine log answers", "engine audit {h}",
    "engine config set {p} {o}", "engine config set {p} {o} x", "engine config set {p}", "engine log {l} --stage {s} --checkpoint {k2}",
    "engine log {l} --checkpoint {k2}", "engine log {l} --stage {s}", "engine log {l} --checkpoint {k2} --stage {s} --checkpoint {k2}",
    "engine config set guard.{f} {o}", "engine utility config-change --guard.{f} {o}",
    "engine bolt abort --name u --slug s --reason stale-review --discard", "{v} orchestrate next", "engine", "engine {v}",
]
PP_VALUES = {
    "v": ["next", "continue", "park", "report", "wait", "help", "team-board"],
    "s": ["code-generation", "build", "Code Generation", "--x", ""],
    "r": ["approved", "rejected", "revised", "completed", "awaiting-approval", "resumed", ""],
    "k": ["on", "off", "scope-dependent", "maybe", ""],
    "u": ["start", "pause", "resume", "complete", "abandon", ""],
    "c": ["set-construction-checkpoints", "set-construction-execution", "set-construction-iteration", "set-construction-verification-command",
          "set-skeleton-stance", "approve", "get"],
    "b": ["checkpoint", "swarm-checkpoint", "start", "complete"],
    "a": ["status", "ask", "approve", "reject", "verify", "run", ""],
    "t": ["resolve", "render", "fingerprint", "verify", "reply", "restore", "begin", "brief"],
    "m": ["summary", "read", "compile"],
    "h": ["history", "append"],
    "p": ["plan-approval", "guard.plan-approval", "guard.review-freeze", "depth", "guard-policy"],
    "o": ["on", "off", "relaxed", ""],
    "l": ["decision", "answer", "answers", "review"],
    "k2": ["plan-approval", "guard-recovery", "other", ""],
    "f": ["plan-approval", "review-freeze", "state-transition", "reviewer-scope", "human-presence"],
}


def pp_fill(template):
    # One word per placeholder: an empty value stays a word of its own.
    return [PP_VALUES[word[1:-1]][rng.randrange(len(PP_VALUES[word[1:-1]]))] if word.startswith("{") and word.endswith("}") and word[1:-1] in PP_VALUES
            else "guard." + rng.choice(PP_VALUES["f"]) if word == "guard.{f}"
            else "--guard." + rng.choice(PP_VALUES["f"]) if word == "--guard.{f}"
            else word for word in template.split(" ")]


for template in PP_TEMPLATES:
    for _ in range(scale * 2):
        add("plan_prerequisite", args=pp_fill(template), gate_held=rng.random() < 0.5, spoke=rng.random() < 0.5)
add("plan_prerequisite", args=["engine", "bolt", "abort", "--name", "u", "--slug", "s", "--reason", "stale review recovery exhausted", "--discard"],
    gate_held=False, spoke=False)
# A flag's last value decides, and one occurrence without a value voids them all.
PP_REPORT = ["engine", "orchestrate", "report"]
for tail in [
    ["--stage", "code-generation", "--result", "approved"], ["--stage", "code-generation", "--result", "rejected"],
    ["--stage", "code-generation", "--result", "revised"], ["--stage", "", "--stage", "code-generation", "--result", "approved"],
    ["--stage", "--x", "--stage", "code-generation", "--result", "rejected"], ["--stage", "build", "--stage", "code-generation", "--result", "approved"],
    ["--stage", "code-generation", "--stage", "build", "--result", "approved"], ["--stage", "code-generation", "--result", "approved", "--result"],
    ["--result", "approved", "--stage", "code-generation", "--stage"], ["--stage", "code-generation", "--result", "", "--result", "rejected"],
]:
    for gate_held in [True, False]:
        add("plan_prerequisite", args=PP_REPORT + tail, gate_held=gate_held, spoke=False)
for tail in [["--action", "", "--action", "ask"], ["--action", "--x", "--action", "ask"], ["--action", "verify", "--action", "ask"],
             ["--action", "ask", "--action", "verify"], ["--action", "ask", "--action"]]:
    add("plan_prerequisite", args=["engine", "bolt", "checkpoint"] + tail, gate_held=False, spoke=False)
for noun, verbs in ER_ROUTES.items():
    for verb in verbs:
        add("plan_prerequisite", args=["engine", noun, verb], gate_held=rng.random() < 0.5, spoke=rng.random() < 0.5)
        add("engine_directed", args=["engine", noun, verb], asked=True)
    add("engine_directed", args=["engine", noun], asked=rng.random() < 0.5)

ED_TEMPLATES = [
    "engine review-brief {rb}", "engine log {l} --stage {s}", "engine log {l} --stage {s} --checkpoint {k2}", "engine log {l} --stage {s} --checkpoint={k2}",
    "engine log {l} --stage {s} --stage {s}", "engine log {l}", "engine status", "engine status --json", "engine orchestrate help",
    "engine orchestrate report --result {r}", "engine orchestrate report --result {r} --stage {s}", "engine orchestrate report --result resumed --result {r}",
    "engine jump {j} --target a", "engine recompose", "engine recompose --skip a", "engine recompose --skip a --add b --reason c",
    "engine recompose --skip=a --reason", "engine recompose --force", "engine recompose --skip a extra", "engine recompose --skip",
    "engine scope change --scope feature", "engine scope change --scope feature --depth standard --review advisory",
    "engine scope change --scope=feature --sensors=off", "engine scope change --scope feature --guard-policy off", "engine scope save --scope x",
    "engine scope change", "engine config set {g} standard", "engine config set {g} standard --depth x --review=y",
    "engine config set {g} standard --guard-policy off", "engine config set {g}", "engine config get {g}", "engine intent create --scope x",
    "engine intent {i} api", "engine utility intent {i} api", "engine intent {i} api x", "engine intent {i}", "engine utility intent {i} api x",
    "utility intent {i} api", "engine workspace {w}", "engine log review --stage {s}", "status", "engine",
]
ED_VALUES = {**PP_VALUES, "rb": ["review", "context", "summary", "nope"], "j": ["execute", "reopen", "resolve"],
             "g": ["depth", "test-strategy", "review", "sensors", "learnings", "collaborators", "guard-policy", "plan-approval", "change-control"],
             "i": ["add-repo", "remove-repo", "archive", "create", "switch"], "w": ["reclassify", "codekb-scope-diff", "detect", "codekb"]}
for template in ED_TEMPLATES:
    for _ in range(scale):
        args = [ED_VALUES[word[1:-1]][rng.randrange(len(ED_VALUES[word[1:-1]]))] if word.startswith("{") and word.endswith("}") else word
                for word in template.split(" ")]
        add("engine_directed", args=args, asked=rng.random() < 0.6)

for prefix in [["engine"], ["engine", "utility"], ["utility"], []]:
    for verb in ["add-repo", "remove-repo"]:
        for asked in [True, False]:
            add("engine_directed", args=prefix + ["intent", verb, "api"], asked=asked)
        add("engine_directed", args=prefix + ["intent", verb], asked=True)
        add("engine_directed", args=prefix + ["intent", verb, "api", "--x"], asked=True)

RD_HEADS = ["status", "--status", "version", "--version", "help", "--help", "doctor", "--doctor", "engine", "now", "next", "Status", ""]
for head in RD_HEADS:
    for tail in [[], ["now"], ["now", "x"], ["doctor"], ["--export"], ["status"], ["version", "x"], ["x"]]:
        add("read_only_diagnostic", args=[head] + tail, stem=rng.choice(["doctor", "utility", "state", ""]))
        add("read_only_diagnostic", args=tail, stem=rng.choice(["doctor", "utility"]))

PS_WORDS = ["git", "status", "Get-Content", "get-content", "a.txt", "-First", "1", "Select-Object", "'a b'", "\"a b\"", "''", "\"\"", "'it''s'",
            "\"$x\"", "\"a`n\"", "$env:X", "@args", "(1)", "{x}", "# c", "-x.y", "--x.y", "-x", "a\\b", "a\\", "C:\\Tools\\x.exe", ".\\tool.ps1",
            "a/b", "a,b", "a%b", "a^b", "a!b", "a[0]", "a=b", "A=b", "é", "a\u2014b", "\u2018q\u2019", "a\tb", "a\u00a0b", "env", "sudo", "command",
            "time", "if", "exec", "cd", "Set-Location", "aidlc", "aidlc.cmd", "engine", "orchestrate", "next", "Write-Output", "x\"y", "a'b", "a&b",
            "bun", "rm", "-rf", "out.txt", "~", "*.md", "a:b", "a+b", "a@b", "1", "-", "--"]
PS_JOINS = [" ; ", "; ", " | ", "|", " || ", " && ", " & ", ";;", " ;", "  "]
PS_REDIRECTS = [" 2>$null", " >$null", " 2>&1", " *>$null", " 3>> $null", " > out.txt", " 2>$NULL", " 1>$null", " >>$null", " 2>&1 | ", " 7>$null",
                " >$nullx", " 2>&2", ">$null"]
PS_PLAIN_FIRST = ["git", "Get-Content", "get-childitem", "Select-Object", "Write-Output", "aidlc", "aidlc.cmd", "bun", "rm", "env", "sudo",
                  "command", "time", "exec", "cd", "Set-Location", "C:\\Tools\\x.exe", ".\\tool.ps1", "a/b", "if", "'my tool'", "\"my tool\""]
PS_PLAIN_WORDS = ["status", "a.txt", "-First", "1", "'a b'", "\"a b\"", "''", "a\\b", "a\\", "--x.y", "-x", "a/b", "a:b", "a+b", "a@b", "é", "engine",
                  "orchestrate", "next", "-rf", "out.txt", "~", "-", "--", "x\"y", "'it works'", "a_b", "a.b.c", "-x.y", "A=b"]
for _ in range(scale * 120):
    plain = rng.random() < 0.6
    parts = []
    for index in range(rng.randrange(1, 4)):
        if plain:
            words = [rng.choice(PS_PLAIN_FIRST)] + [rng.choice(PS_PLAIN_WORDS) for _ in range(rng.randrange(0, 4))]
        else:
            words = [rng.choice(PS_WORDS) for _ in range(rng.randrange(1, 5))]
        text = " ".join(words)
        if rng.random() < (0.5 if plain and words[0][0] in "'\"" else 0.2):
            text = "& " + text
        if rng.random() < 0.25:
            text += rng.choice(PS_REDIRECTS[:5] if plain else PS_REDIRECTS)
        parts.append(text)
    command = parts[0]
    for part in parts[1:]:
        command += rng.choice(PS_JOINS[:4] if plain else PS_JOINS) + part
    if rng.random() < 0.05:
        command = rng.choice([" ", "&", "& ", ";", "|"]) + command
    add("plain_powershell", command=command)
for command in ["", " ", "git status", "& git status", "& 'C:\\Program Files\\Git\\git.exe' status", "& \"C:\\x y\\git.exe\" status 2>$null",
                "Get-Content a.txt | Select-Object -First 1", "aidlc.cmd engine orchestrate next", "Get-ChildItem 'a\\' ; git log",
                "git commit -m 'it works'", "git commit -m \"it works\"", "tool 'a\\'", "Get-Content 'a\\'", "tool ''", "Get-Content ''",
                "a ;", "a |", "| a", "a ; ; b", "& & a", "a & b", "&a", "a 2>$null b", "2>$null a", "a>$null", "a 2>&1;b", "a  b   c", "'a' b",
                "& 'a' b", "& '' b", "a=b c", "& a=b c", "1a b", "-a b", "a.b c", "a:b c", "a/b c", "a\\b c", "a\\ c"]:
    add("plain_powershell", command=command)

DE_PARTS = ["echo", "a", "$HOME", "\\$HOME", "'$HOME'", "\"$HOME\"", "\"a\"", "'a'", "`ls`", "'`ls`'", "\\`", "$(ls)", "<(ls)", ">(cat)", "< (x)", "> (x)",
            "a>b", "a<b", "\\", "'", "\"", "it's", "\"it's $x\"", "'a'\"$b\"", "\\'$x", "'\\'$x'", "é", "a\\", "<", ">", "(", "\"<(\"", "'>('"]
for _ in range(scale * 40):
    add("dynamic_evaluation", command=" ".join(rng.choice(DE_PARTS) for _ in range(rng.randrange(0, 6))))

# What a tool call would change while a plan waits (aidlc-plan-approval-guard.ts,
# `mutationIntent` and the predicates of a write target), asked in a project
# laid out under a scratch root `{P}`. The project is `{P}/p`; the hook's
# process is in `{P}`. A tree may carry a piece of work with a person's turn on
# record: what upstream reads of the person there is part of its answer, the
# case says the same, and focus's replay is handed what the case says.
mi = random.Random(seed * 7919 + 23)
MI_INTENTS = "p/aidlc/spaces/default/intents"
MI_RECORD = MI_INTENTS + "/260105-add-login"
MI_TOOLS = ["aidlc.ts", "aidlc-utility.ts", "aidlc-graph.ts", "aidlc-doctor.ts", "aidlc-state.ts", "aidlc-orchestrate.ts", "aidlc-sensor.ts"]


def mi_state(stage, box, parked, other="x"):
    lines = ["# AI-DLC State Tracking", "", "## Current Status", "- **Current Stage**: " + stage]
    if parked is not None:
        lines.append("- **Parked**: " + parked)
    lines += ["", "## Stage Progress", "- [" + box + "] code-generation — EXECUTE", "- [" + other + "] build — EXECUTE", ""]
    return "\n".join(lines)


def mi_turn(reply):
    return ("# AI-DLC Audit Log\n\n## Human_Turn\n**Timestamp**: 2026-01-05T10:01:00Z\n**Event**: HUMAN_TURN\n**Session**: s1\n"
            + ("**Reply**: " + reply + "\n" if reply else "") + "\n---\n\n")


def mi_tree(record, state, layout="plain"):
    files = {"p/src/a.txt": "a\n", "p/a.txt": "a\n", "out": None}
    links = []
    tools = "p/.claude/tools/"
    if layout == "harness-link":
        # The harness folder is a link: the tools are real files reached through it.
        tools = "real/tools/"
        links.append(["p/.claude", "../real"])
    for name in MI_TOOLS:
        # A tool that is not there, and a folder where a tool is expected.
        files[tools + name] = None if layout == "tool-folders" else "export {}\n"
    if layout == "no-tools":
        files = {"p/src/a.txt": "a\n", "p/a.txt": "a\n", "out": None, "p/.claude/tools": None}
    files[tools + "sub/aidlc-graph.ts"] = "export {}\n"
    if layout == "odd":
        # Names beside a tool's, and folders whose names a shell would expand.
        files.update({tools + "aidlc-graph.tsx": "export {}\n", tools + "xaidlc-graph.ts": "export {}\n", "q[x]": None, "q{x}": None,
                      "q*": None, "q?": None})
    if layout in ("plain", "project-link"):
        links += [["p/lt", ".claude/tools"], ["p/.claude/tools/aidlc-linked.ts", "aidlc-graph.ts"], ["plink", "p"]]
    if record != "none":
        files[MI_RECORD + "/aidlc-state.md"] = state
    if record == "turn":
        files[MI_RECORD + "/audit/laptop-abc123.md"] = mi_turn(None)
    if record == "question":
        files[MI_RECORD + "/audit/laptop-abc123.md"] = mi_turn("question")
    return files, links


def mi_case(tool, input_, hint=False, cwd="{P}/p", record=None, layout="plain", gate=None, unattended=None, other="x"):
    record = record if record is not None else mi.choice(["none", "none", "state", "turn", "turn", "turn", "question"])
    stage, box = gate if gate is not None else (mi.choice(["code-generation", "code-generation", "Code Generation", "build", ""]),
                                                 mi.choice(["?", "?", "-", "x", " "]))
    parked = mi.choice([None, None, "", "2026-01-05T10:02:00Z", "no"])
    unattended = unattended if unattended is not None else mi.random() < 0.2
    state = mi_state(stage, box, parked, other)
    files, links = mi_tree(record, state, layout)
    spoke = record == "turn"
    # The project named through a link to it: its real path is another.
    add("mutation_intent", tool=tool, input=input_, hint=hint, cwd=cwd, state=state, files=files, links=links, names=[],
        project="plink" if layout == "project-link" else "p", harness=".claude", unattended=unattended,
        person={"spoke": spoke, "asked": spoke and not unattended, "lastStep": record != "none" and bool((parked or "").strip())})


MI_READS = [
    "cat a.txt", "ls -la", "grep -r x src", "echo hi", "echo hi > out.txt", "echo hi > {P}/out/x", "echo hi >> src/a.txt", "sort a.txt",
    "sort -o b.txt a.txt", "sort --output=b.txt a.txt", "sort --output b.txt a.txt", "sort -u a.txt", "uniq a.txt", "uniq a.txt b.txt",
    "uniq -c a.txt", "uniq -c a.txt b.txt", "uniq", "wc -l a.txt", "/bin/cat a.txt", "CAT a.txt", "cat.exe a.txt", "which bun", "[ -f a.txt ]",
    "stat a.txt | head -1", "cat a.txt | tee b.txt", "cat a.txt | sort -o b.txt", "env cat a.txt", "xargs cat", "time ls",
]
MI_GIT = [
    "git status", "git -C src log", "git -C", "git branch", "git branch --show-current", "git branch -D x --show-current", "git diff --output=x",
    "git diff --output x", "git log --oneline", "git commit -m x", "git --git-dir=.git status", "git --git-dir .git show", "git",
    "git --namespace n push", "git -c a=b status", "git '' status", "git ls-files", "git rev-parse HEAD", "git grep x", "git push", "git stash",
    "git --work-tree=. diff", "git --work-tree . --namespace n grep x", "GIT status", "git.exe status", "git --no-pager log",
]
MI_CD = [
    "cd {P}/p", "cd -- {P}/p", "cd {P}/p/src", "cd src", "cd {P}/p/.", "cd {P}/p/..", "cd {P}/p/*", "cd {P}/p x", "env cd {P}/p", "cd",
    "cd {P}/p/", "cd {P}//p", "cd {P}/p && ls", "cd {P}/p/src && cat a.txt", "cd {P}/missing", "cd {P}/p\u000b", "cd '{P}/p'",
    "command cd {P}/p", "cd {P}/out", "cd {P}/plink", "cd --", "cd -- {P}/p x", "cd {P}/p[", "cd {P}/{p}", "cd {P}/p?", "CD {P}/p",
    "cd {P}/p \\\n", "cd {P}/p/src/..", "cd -P {P}/p", "cd {P}/p; cat a.txt", "ls && cd {P}/p",
]
MI_NATIVE = [
    "aidlc status", "aidlc --status", "aidlc version", "aidlc help", "aidlc doctor", "aidlc doctor --export", "aidlc engine now",
    "aidlc engine now x", "aidlc engine orchestrate next", "aidlc engine orchestrate report --stage code-generation --result approved",
    "aidlc engine orchestrate report --stage build --result approved", "aidlc park", "aidlc park --reason x", "aidlc engine orchestrate park",
    "aidlc engine log decision --stage code-generation --checkpoint plan-approval", "aidlc engine log decision --stage code-generation",
    "aidlc engine log answer --stage code-generation", "aidlc engine log review --stage code-generation", "aidlc engine testing-posture begin",
    "aidlc engine config set guard.review-freeze on", "aidlc engine config set guard.review-freeze off", "aidlc engine config set guard-policy strict",
    "aidlc engine config set guard-policy relaxed", "aidlc engine config set guard-policy off", "aidlc engine config set plan-approval on",
    "aidlc engine config set plan-approval off", "aidlc engine config set guard.plan-approval off", "aidlc engine config set guard.plan-approval on",
    "aidlc engine config set summary-confirmation off", "aidlc engine config set summary-confirmation on",
    "aidlc engine config set guard.state-transition on", "aidlc engine config set guard.state-transition off",
    "aidlc engine config set guard.reviewer-scope off", "aidlc engine config set guard.reviewer-scope on",
    "aidlc engine config set guard.human-presence off", "aidlc engine config set depth standard", "aidlc engine config set depth",
    "aidlc engine config set guard.review-freeze on x", "aidlc engine config set guard.review-freeze ON", "aidlc engine config set guard-policy on",
    "aidlc engine config get guard.review-freeze on", "aidlc config flags --clear-bypass AIDLC_DISABLE_SENSORS",
    "aidlc config flags --bypass AIDLC_DISABLE_SENSORS", "aidlc config flags --bypass AIDLC_DISABLE_SENSORS --clear-bypass AIDLC_DISABLE_LEARNINGS",
    "aidlc config flags --clear-bypass AIDLC_DISABLE_LEARNINGS --bypass AIDLC_DISABLE_SENSORS", "aidlc config flags --clear-bypass NOPE",
    "aidlc config flags --bypass NOPE", "aidlc config flags --clear-bypass", "aidlc config flags --bypass", "aidlc config flags",
    "aidlc config flags --json", "aidlc config flags --clear-bypass AIDLC_DISABLE_SENSORS --project-dir {P}/p --json --local",
    "aidlc config flags --clear-bypass AIDLC_DISABLE_SENSORS --project-dir {P}/out", "aidlc config flags --clear-bypass AIDLC_DISABLE_SENSORS --project-dir",
    "aidlc config flags --clear-bypass AIDLC_DISABLE_SENSORS --project-dir {P}/p/src/..", "aidlc config flags --clear-bypass AIDLC_DISABLE_SENSORS --project-dir p",
    "aidlc config flags --clear-bypass AIDLC_DISABLE_SENSORS --project-dir {P}/plink", "aidlc config flags --clear-bypass AIDLC_DISABLE_SENSORS --project-dir {P}/missing",
    "aidlc config flags --clear-bypass AIDLC_DISABLE_SENSORS --force", "aidlc config flags --clear-bypass AIDLC_DISABLE_SENSORS --yes --quiet --no-color --verbose --global --project",
    "aidlc config flags --project-dir {P}/p", "aidlc config flags --bypass AIDLC_SKIP_ARTIFACT_GUARD --yes", "aidlc config flags --bypass AIDLC_UNATTENDED",
    "aidlc config set x", "aidlc config", "aidlc engine config flags --clear-bypass AIDLC_DISABLE_SENSORS", "aidlc engine graph topo",
    "aidlc engine sensor fire x", "aidlc engine sensor list", "aidlc engine state approve", "aidlc engine state get x", "aidlc engine review-brief review",
    "aidlc engine jump execute --target a", "aidlc engine recompose --skip a", "aidlc engine scope change --scope feature",
    "aidlc engine config set depth standard --review advisory", "aidlc engine intent create --scope x", "aidlc engine intent add-repo api",
    "aidlc engine utility intent remove-repo api", "aidlc engine workspace reclassify", "aidlc engine status", "aidlc engine orchestrate help",
    "aidlc engine orchestrate report --result resumed", "aidlc engine swarm prepare --units a,b", "aidlc engine bolt start", "aidlc", "aidlc engine",
    "AIDLC.EXE status", "aidlc.exe engine now", "Aidlc status", "aidlc.cmd status", "aidlc.ps1 status", "./aidlc status", "/usr/local/bin/aidlc status",
    "env aidlc status", "sudo aidlc status", "X=1 aidlc status", "PATH=x aidlc status", "xargs aidlc status", "command aidlc status",
    "time aidlc status", "nohup aidlc status", "env -C src aidlc status", "aidlc status > out.txt", "aidlc status | cat", "aidlc status && aidlc engine now",
    "aidlc status; rm a.txt", "aidlc engine graph topo --project-dir {P}/out",
]
MI_BUN = [
    "bun .claude/tools/aidlc.ts status", "bun run .claude/tools/aidlc.ts engine now", "bun .claude/tools/aidlc.ts engine orchestrate next",
    "bun .claude/tools/aidlc.ts engine state approve", "bun .claude/tools/aidlc.ts park", "bun .claude/tools/aidlc.ts doctor",
    "bun .claude/tools/aidlc.ts engine graph topo", "bun .claude/tools/aidlc.ts engine config set guard.review-freeze off",
    "bun .claude/tools/aidlc.ts", "bun .claude/tools/aidlc-graph.ts topo", "bun .claude/tools/aidlc-graph.ts build",
    "bun .claude/tools/aidlc-utility.ts status", "bun .claude/tools/aidlc-utility.ts version", "bun .claude/tools/aidlc-utility.ts now",
    "bun .claude/tools/aidlc-utility.ts now x", "bun .claude/tools/aidlc-utility.ts intent add-repo api", "bun .claude/tools/aidlc-utility.ts detect",
    "bun .claude/tools/aidlc-utility.ts state-init", "bun .claude/tools/aidlc-doctor.ts doctor", "bun .claude/tools/aidlc-doctor.ts doctor --export",
    "bun .claude/tools/aidlc-doctor.ts", "bun .claude/tools/aidlc-doctor.ts status", "bun .claude/tools/aidlc-state.ts approve",
    "bun .claude/tools/aidlc-state.ts get x", "bun .claude/tools/aidlc-orchestrate.ts next", "bun .claude/tools/aidlc-orchestrate.ts report --result resumed",
    "bun .claude/tools/aidlc-sensor.ts list", "bun -r x .claude/tools/aidlc-graph.ts topo", "bun --preload=x .claude/tools/aidlc-graph.ts topo",
    "bun --require=x .claude/tools/aidlc-graph.ts topo", "bun .claude/tools/aidlc-graph.ts topo --require x", "bun .claude/tools/aidlc-graph.ts topo --preload",
    "bun {P}/p/.claude/tools/aidlc-graph.ts topo", "bun lt/aidlc-graph.ts topo", "bun .claude/tools/aidlc-linked.ts topo",
    "bun .claude/tools/aidlc-missing.ts topo", "bun .claude/tools/other.ts", "bun .claude/tools/sub/aidlc-graph.ts topo",
    "bun ../p/.claude/tools/aidlc-graph.ts topo", "bun ./.claude/tools/aidlc-graph.ts topo", "bun .claude/tools/../tools/aidlc-graph.ts topo",
    "bun {P}/plink/.claude/tools/aidlc-graph.ts topo", "BUN.EXE .claude/tools/aidlc-graph.ts topo", "bun.exe .claude/tools/aidlc-graph.ts topo",
    "Bun .claude/tools/aidlc.ts status", "/usr/bin/bun .claude/tools/aidlc-graph.ts topo", "/usr/bin/bun .claude/tools/aidlc.ts status",
    "env bun .claude/tools/aidlc-graph.ts topo", "X=1 bun .claude/tools/aidlc-graph.ts topo", "PATH=x bun .claude/tools/aidlc-graph.ts topo",
    "xargs bun .claude/tools/aidlc-graph.ts topo", "bun", "bun -e 'x'", "bun test", "bun run build", "bun run", "bun run run .claude/tools/aidlc.ts status",
    "bun --watch .claude/tools/aidlc-graph.ts topo", "bun .claude/tools/Aidlc-graph.ts topo", "bun .claude/tools/aidlc-.ts topo",
    "bun .claude/tools/aidlc-graph.ts", "bun .claude/tools/aidlc-graph.ts topo > out.txt", "bun .claude/tools/aidlc-graph.tsx topo",
    "bun '' .claude/tools/aidlc-graph.ts topo", "bun .claude/tools/aidlc-graph.ts topo && bun .claude/tools/aidlc-state.ts approve",
    "node .claude/tools/aidlc-graph.ts topo", "bunx .claude/tools/aidlc-graph.ts topo",
]
MI_WRITES = [
    "rm a.txt", "rm *.txt", "rm $X", "cp a.txt b.txt", "touch {P}/out/x", "touch x", "sed -i s/a/b/ a.txt", "tee b.txt", "mv a.txt b.txt", "mkdir d",
    "npm test", "node -e 'x'", "node x.js", "python3 -c x", "python3 x.py", "sh run.sh", "./run.sh", "bash -c 'ls'", "eval ls", "source x", ". x",
    "x.sh", "run.bat", "tool.PS1", "dd if=a of=b", "truncate -s 0 a.txt", "install a b", "perl -pi -e s/a/b/ a.txt", "perl x.pl", "unlink a.txt",
    "exec ls", "busybox ls", "deno eval x", "ruby -e x", "php -r x", "ls > $X", "rm -- a.txt", "rm a?.txt", "rm `x`", "RM a.txt", "/bin/rm a.txt",
    "env rm a.txt", "rm", "touch", "cp a.txt {P}/out/b.txt", "mv {P}/out/a {P}/out/b", "tee {P}/out/x < a.txt", "python3 -m http.server",
    "node --print x", "bun -p x", "ruby x.rb -c", "zsh", "pwsh -File x.ps1", "cmd /c dir", "fish -c ls", "ksh x", "dash x",
]
MI_DYNAMIC = [
    "echo $HOME", "echo `ls`", "cat <(ls)", "echo $(aidlc status)", "echo '$HOME'", "echo \"$AIDLC\"", "echo $x aidlcx", "echo $x my-aidlc run",
    "PATH=x ls", "export PATH=x; ls", "ls\r", "ls \\\n -la", "echo a \\> b", "ls\u00a0-la", "cat a.txt\u000c", "echo a \\; b", "echo a \\& b",
    "echo \\| b", "echo a \\< b", "echo a\\ b", "ls\t-la", "ls\n-la", "echo \u2028", "echo '\\>'", "echo \"a\\\nb\"", "",  " ", "#", "ls # aidlc",
]
MI_POWERSHELL = [
    "Get-Content a.txt", "get-content a.txt", "Get-ChildItem", "gc a.txt", "Get-Content a.txt | Select-Object -First 1", "Set-Location {P}/p",
    "sl {P}/p", "sl -Path {P}/p", "Set-Location -LiteralPath {P}/p", "Set-Location -literalpath {P}/p x", "chdir {P}/p", "cd {P}/p",
    "cd -Path {P}/p", "Set-Location {P}/p/src", "C:\\Tools\\cat.exe a", ".\\aidlc.cmd status", "aidlc.cmd status", "aidlc.cmd engine now",
    "aidlc.cmd engine log decision --stage code-generation --checkpoint plan-approval", "aidlc.cmd engine config set guard.review-freeze on",
    "aidlc.cmd status 'a\"b'", "aidlc.cmd status ''", "aidlc.cmd status 'a&b'", "aidlc.cmd status 'a|b'", "aidlc.cmd status 'a b'",
    "aidlc.cmd engine state approve", "AIDLC.CMD status", "& aidlc status", "aidlc status", "aidlc engine orchestrate next", "aidlc.exe status",
    "AIDLC status", "aidlc park", "aidlc.cmd park", "Get-Content $x", "git status 2>$null", "git status", "bun .claude/tools/aidlc-graph.ts topo",
    "bun .claude\\tools\\aidlc-graph.ts topo", "Get-Content a.txt > b.txt", "Write-Output hi", "Test-Path a.txt", "Select-String x a.txt",
    "C:\\Tools\\Get-Content.exe a", "./Get-Content a.txt", "Get-Content.exe a.txt", "env Get-Content a.txt", "Remove-Item a.txt",
    "Set-Content a.txt x", "rm a.txt", "D:\\aidlc.cmd status", ".claude\\tools\\aidlc.cmd status", "./aidlc status", "./aidlc.cmd status", "sl",
    "Set-Location", "Set-Location -Path", "sl {P}/p x", "sort a.txt", "cat a.txt", "ls ; Get-Content a.txt", "Get-Content a.txt ; rm a.txt",
    "aidlc.cmd status ; Get-ChildItem", "Format-List", "Measure-Object", "Resolve-Path a.txt", "Out-String", "out-file b.txt", "SL {P}/p",
    "set-location '{P}/p'", "& 'Get-Content' a.txt", "Get-Content 'a b.txt'", "aidlc.cmd config flags --clear-bypass AIDLC_DISABLE_SENSORS",
]
MI_CWDS = ["{P}/p", "{P}/p", "{P}/p", "{P}/p/src", "{P}"]

for pool in [MI_READS, MI_GIT, MI_CD, MI_NATIVE, MI_BUN, MI_WRITES, MI_DYNAMIC]:
    for command in pool:
        mi_case("Bash", {"command": command}, record="none", gate=("code-generation", "?"), unattended=False)
        for _ in range(max(1, scale // 3)):
            mi_case("Bash", {"command": command}, hint=mi.random() < 0.08, cwd=mi.choice(MI_CWDS))
for command in MI_NATIVE + MI_BUN:
    # The person has spoken: with the work parked, and with an unattended driver.
    mi_case("Bash", {"command": command}, record="turn", gate=(mi.choice(["code-generation", "build"]), mi.choice(["?", "-"])), unattended=False)
    mi_case("Bash", {"command": command}, record="turn", unattended=True)
for command in MI_BUN:
    mi_case("Bash", {"command": command}, layout=mi.choice(["harness-link", "bare"]))
    mi_case("Bash", {"command": command}, cwd="{P}/p/.claude/tools")
for command in MI_POWERSHELL:
    mi_case("Bash", {"command": command}, hint=True, record="none", gate=("code-generation", "?"), unattended=False)
    for _ in range(max(1, scale // 3)):
        mi_case("Bash", {"command": command}, hint=mi.random() < 0.8, cwd=mi.choice(MI_CWDS))
for _ in range(scale * 60):
    pools = [MI_READS, MI_GIT, MI_CD, MI_NATIVE, MI_BUN, MI_WRITES, MI_DYNAMIC]
    command = mi.choice(mi.choice(pools))
    for _ in range(mi.randrange(1, 3)):
        command += mi.choice([" && ", "; ", " | ", " || ", "\n", " & "]) + mi.choice(mi.choice(pools))
    mi_case("Bash", {"command": command}, hint=mi.random() < 0.05, cwd=mi.choice(MI_CWDS))
for tool, input_ in [
    ("Write", {"file_path": "{P}/p/src/x.ts", "content": "x"}), ("Write", {"file_path": "src/x.ts"}), ("Edit", {"file_path": "/abs/x"}),
    ("MultiEdit", {"file_path": "a", "path": "b", "paths": ["c", "", 1, None, "{P}/out/d"]}), ("NotebookEdit", {"notebook_path": "n.ipynb"}),
    ("NotebookEdit", {"notebook_path": "n.ipynb", "file_path": "f", "path": "p"}), ("Write", {}), ("Write", None), ("Write", {"file_path": ""}),
    ("Write", {"file_path": 3}), ("Write", {"paths": "x"}), ("Write", {"paths": []}), ("Read", {"file_path": "a"}), ("Task", {"prompt": "x"}),
    ("Bash", {}), ("Bash", None), ("Bash", {"command": 3}), ("Bash", {"command": None}), ("bash", {"command": "ls"}), ("write", {"file_path": "a"}),
    ("Write", {"file_path": "../x"}), ("Write", {"file_path": "~/x"}), ("Write", {"file_path": "a/../b/./c"}), ("Edit", {"file_path": "{P}/p/a.txt"}),
    ("Bash", {"command": "ls", "file_path": "a"}), ("", {"file_path": "a"}), ("WebFetch", {"url": "x"}),
]:
    for cwd in ["{P}/p", "{P}/p/src", "{P}"]:
        mi_case(tool, input_, cwd=cwd, hint=mi.random() < 0.2)

# The edges a mutation sweep asked for.
for stage, box, other in [("code-generation", "-", "?"), ("code-generation", "x", "?"), ("build", "?", "?"), ("code-generation", "?", "?")]:
    mi_case("Bash", {"command": "aidlc engine orchestrate report --stage code-generation --result approved"}, gate=(stage, box), other=other)
for command in [
    "git diff -o x", "git status -o", "git -o status", "sort -o", "aidlc x flags --clear-bypass AIDLC_DISABLE_SENSORS",
    "aidlc engine flags --clear-bypass AIDLC_DISABLE_SENSORS", "aidlc x config set guard.review-freeze on",
    "aidlc engine config unset guard.review-freeze on", "aidlc engine x set guard.review-freeze on",
    "aidlc engine config set guard.review-freeze on --json", "aidlc engine config set plan-approval strict",
    "aidlc engine config set guard-policy strict extra",
]:
    for record in ["none", "turn"]:
        mi_case("Bash", {"command": command}, record=record, gate=("code-generation", "?"), unattended=False)
for command in MI_BUN:
    for layout in ["no-tools", "tool-folders", "project-link"]:
        mi_case("Bash", {"command": command}, layout=layout, record="none", gate=("code-generation", "?"), unattended=False)
    mi_case("Bash", {"command": command.replace(".claude/tools/", "{P}/plink/.claude/tools/")}, layout="project-link", cwd="{P}/plink")
for command, layout, cwd, hint in [
    ("bun .claude/tools/aidlc-graph.ts topo -r x", "plain", "{P}/p", False), ("bun .claude/tools/aidlc-graph.ts topo --require=x", "plain", "{P}/p", False),
    ("bun .claude/tools/aidlc-graph.ts topo --preload=x", "plain", "{P}/p", False), ("bun -/../.claude/tools/aidlc-graph.ts topo", "plain", "{P}/p", False),
    ("bun .claude/tools/xaidlc-graph.ts topo", "odd", "{P}/p", False), ("bun .claude/tools/aidlc-graph.tsx topo", "odd", "{P}/p", False),
    ("bun aidlc-graph.ts topo", "plain", "{P}/p/.claude/tools", False), ("bun ./aidlc.ts status", "plain", "{P}/p/.claude/tools", False),
    ("bun ../.claude/tools/aidlc-graph.ts topo", "plain", "{P}/p/src", False), ("bun '' topo", "plain", "{P}/p/.claude/tools/aidlc-graph.ts", False),
    ("bun '' status", "plain", "{P}/p/.claude/tools/aidlc.ts", False), ("bun '' topo", "tool-folders", "{P}/p/.claude/tools/aidlc-graph.ts", False),
    ("./cat a.txt", "plain", "{P}/p", True), ("bin/git status", "plain", "{P}/p", True), ("cd.exe {P}/p", "plain", "{P}/p", True),
    ("SL.EXE {P}/p", "plain", "{P}/p", True), ("Set-Location.exe {P}/p", "plain", "{P}/p", True), ("rm src\\a.txt", "plain", "{P}/p", True),
    ("tee src\\b.txt", "plain", "{P}/p", True), ("sl {P}/p\r", "plain", "{P}/p", True), ("sl\u000b{P}/p", "plain", "{P}/p", True),
    ("sl {P}/p \u000c", "plain", "{P}/p", True), ("sl\t{P}/p", "plain", "{P}/p", True), ("cd -Path {P}/p", "plain", "{P}/p", False),
    ("cd -literalpath {P}/p", "plain", "{P}/p", False), ("cd p", "plain", "{P}/p", False), ("cd \\\n{P}/p", "plain", "{P}/p", False),
    ("cd {P}/p\\\n", "plain", "{P}/p", False), ("cd ''", "plain", "{P}/p", False), ("cd ''", "plain", "{P}", False),
    ("sl {P}/p", "plain", "{P}/p", False), ("chdir {P}/p", "plain", "{P}/p", False), ("set-location {P}/p", "plain", "{P}/p", False),
    ("cd '{P}/q[x]'", "odd", "{P}/q[x]", False), ("cd '{P}/q{x}'", "odd", "{P}/q{x}", False), ("cd '{P}/q*'", "odd", "{P}/q*", False),
    ("cd '{P}/q?'", "odd", "{P}/q?", False), ("cd {P}/out", "odd", "{P}/out", False), ("cd {P}/missing", "plain", "{P}/missing", False),
    ("cd {P}/p/a.txt", "plain", "{P}/p/a.txt", False), ("cd {P}/p/src", "project-link", "{P}/plink/src", False),
    ("cd {P}/plink/src", "project-link", "{P}/plink/src", False), ("rm a.txt *.md", "plain", "{P}/p", False), ("rm a.txt b?.txt", "plain", "{P}/p", False),
    ("rm a.txt '$x'", "plain", "{P}/p", False), ("rm a.txt '`x`'", "plain", "{P}/p", False), ("rm a.txt 'b c.txt'", "plain", "{P}/p", False),
]:
    mi_case("Bash", {"command": command}, hint=hint, layout=layout, cwd=cwd, record="none", gate=("code-generation", "?"), unattended=False)
for command in ["cd {P}/plink", "cd {P}/p", "aidlc config flags --clear-bypass AIDLC_DISABLE_SENSORS --project-dir {P}/plink",
                "aidlc config flags --clear-bypass AIDLC_DISABLE_SENSORS --project-dir {P}/p", "touch {P}/plink/x", "touch {P}/p/x"]:
    for cwd in ["{P}/plink", "{P}/p"]:
        mi_case("Bash", {"command": command}, layout="project-link", cwd=cwd)

# Where a write target lies: outside the project, a stage's own record output,
# a person's answer text, the stage's diary, the composer's proposal.
MT_ENGINE = MI_INTENTS + "/.aidlc-engine"
MT_TARGETS = [
    "{P}/p/src/a.txt", "{P}/p/src/new.txt", "{P}/p/src/new/deep.txt", "{P}/out/x", "{P}/out", "{P}", "{P}/p", "{P}/p/", "/nonexistent-aidlc-oracle/x",
    "{P}/inlink/x", "{P}/inlink", "{P}/dangling/x", "{P}/dangling", "{P}/p/outlink/x", "{P}/p/outlink", "{P}/p/lnk/a.txt", "{P}/p/../out/x",
    "{P}/p/src/../../out/x", "src/a.txt", "p/src/a.txt", "out/x", "{P}/pp/x", "{P}/p/src/a.txt/x", "{P}/out/a.txt/x", "",
] + ["{P}/" + rel for rel in [
    MI_RECORD + "/construction/code-generation/memory.md", MI_RECORD + "/construction/code-generation/other.md",
    MI_RECORD + "/construction/code-generation/sub/memory.md", MI_RECORD + "/construction/code-generation",
    MI_RECORD + "/construction/build/memory.md", MI_RECORD + "/aidlc-state.md", MI_RECORD + "/sub/aidlc-state.md", MI_RECORD + "/audit/x.md",
    MI_RECORD + "/x/audit", MI_RECORD + "/auditx/y.md", MI_RECORD + "/.aidlc-engine/answer-text/a.txt",
    MI_RECORD + "/.aidlc-engine/answer-text/sub/b.txt", MI_RECORD + "/.aidlc-engine/answer-text", MI_RECORD + "/.aidlc-engine/answer-text/new.txt",
    MI_RECORD + "/.aidlc-engine/other/a.txt", MI_RECORD + "/.aidlc-engine/answer-textx/a.txt", MI_RECORD + "/inception/req.md",
    MI_RECORD + "/inception/new.md", MI_RECORD, MI_INTENTS + "/intents.json", MI_INTENTS + "/x/intents.json", MT_ENGINE + "/composer-proposal.json",
    MT_ENGINE + "/composer-proposal.json/x", MT_ENGINE + "/other.json", MT_ENGINE + "/answer-text/a.txt", MI_INTENTS + "/construction/code-generation/memory.md",
    MI_INTENTS + "/other/construction/code-generation/memory.md", "p/aidlc/spaces", "p/aidlc/spaces/x", "p/aidlc/spaces/..x", "p/aidlc/spaces/..",
    "p/aidlc/x.md", "p/aidlc/spaces/other/intents/r/construction/code-generation/memory.md", "p/aidlc/spaces/default/memory/team.md",
    MI_RECORD + "/construction/code-generation/../code-generation/memory.md", MI_RECORD + "/construction//code-generation/memory.md",
    MI_RECORD + "/CONSTRUCTION/code-generation/Memory.MD",
]]
MT_THROUGH_LINK = [target.replace("{P}/p/", "{P}/plink/") for target in MT_TARGETS if target.startswith("{P}/p/")] + ["{P}/plink", "{P}/toplink/x"]
MT_DIARY = MI_RECORD + "/construction/code-generation/memory.md"
MT_ANSWER = MI_RECORD + "/.aidlc-engine/answer-text/a.txt"
MT_PROPOSAL = MT_ENGINE + "/composer-proposal.json"


def mt_case(record=True, present=(), links=(), names=(), extra=None, intents=True, project="p"):
    files = {"p/src/a.txt": "a\n", "out/a.txt": "a\n"}
    if intents:
        files[MI_INTENTS] = None
    if record:
        files[MI_RECORD + "/aidlc-state.md"] = mi_state("code-generation", "?", None)
        files[MI_RECORD + "/inception/req.md"] = "r\n"
    for rel in present:
        files[rel] = "x\n"
    files.update(extra or {})
    add("mutation_target", targets=MT_TARGETS + (MT_THROUGH_LINK if project != "p" else []), files=files,
        links=[list(link) for link in links], names=[list(name) for name in names], project=project, harness=".claude")


MT_OUTSIDE_LINKS = [("inlink", "p/src"), ("dangling", "nowhere"), ("p/outlink", "../out"), ("p/lnk", "src")]
for record in [True, False]:
    mt_case(record)
    mt_case(record, links=MT_OUTSIDE_LINKS)
    mt_case(record, present=[MT_DIARY, MT_ANSWER, MT_PROPOSAL, MI_INTENTS + "/construction/code-generation/memory.md", MT_ENGINE + "/answer-text/a.txt"],
            links=MT_OUTSIDE_LINKS)
# A second name for the file, and a link in its place or on the way to it.
mt_case(names=[(MT_DIARY, "p/src/a.txt"), (MT_ANSWER, "p/src/a.txt"), (MT_PROPOSAL, "p/src/a.txt"), (MI_RECORD + "/inception/new.md", "out/a.txt")])
mt_case(links=[(MT_DIARY, "../../inception/req.md"), (MT_ANSWER, "../../inception/req.md"), (MT_PROPOSAL, "nowhere")])
mt_case(links=[(MI_RECORD + "/construction", "inception"), (MI_RECORD + "/.aidlc-engine", "inception"), (MT_ENGINE, "../../../../../out")])
mt_case(links=[(MI_RECORD + "/construction/code-generation", "../inception"), (MI_RECORD + "/.aidlc-engine/answer-text", "../inception")])
mt_case(record=False, links=[("p/aidlc", "../out")], intents=False)
mt_case(record=False, links=[("p/aidlc/spaces", "../../out"), ("inlink", "p")], intents=False)
mt_case(extra={MT_DIARY: None, MT_ANSWER: None, MT_PROPOSAL: None})
# A file where a folder is expected: the path goes through it.
mt_case(extra={MI_RECORD + "/construction": "x\n", MI_RECORD + "/.aidlc-engine": "x\n", MT_ENGINE: "x\n"})
# The project named through a link to it; no records folder at all; a link under the answer-text folder.
for record in [True, False]:
    mt_case(record, links=[("plink", "p"), ("toplink", "p/src")] + MT_OUTSIDE_LINKS, project="plink",
            present=[MT_DIARY, MT_ANSWER, MT_PROPOSAL] if record else [])
mt_case(record=False, intents=False)
mt_case(links=[(MI_RECORD + "/.aidlc-engine/answer-text/sub", "../../inception"), (MI_RECORD + "/construction/code-generation/sub", "../../inception")])
# Two records and no cursor: no piece of work is selected.
mt_case(extra={MI_INTENTS + "/260106-other/aidlc-state.md": mi_state("build", "-", None)}, present=[MT_DIARY, MT_ANSWER])
mt_case(extra={MI_INTENTS + "/260106-other/aidlc-state.md": mi_state("build", "-", None), MI_INTENTS + "/active-intent": "260105-add-login\n"},
        present=[MT_DIARY, MT_ANSWER])

for case in cases:
    sys.stdout.write(json.dumps(case, ensure_ascii=False) + "\n")
