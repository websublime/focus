#!/usr/bin/env python3
"""Generate oracle cases.

    gen_cases.py <core dir> <seed> <scale>  > cases.ndjson

Cases are inputs only. The answers come from upstream (driver.ts) and from
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

for case in cases:
    sys.stdout.write(json.dumps(case, ensure_ascii=False) + "\n")
