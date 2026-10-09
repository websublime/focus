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

# Guard Policy declarations in a memory file. "guard" cases stay inside what
# focus's line reader claims to read exactly as upstream's Markdown parser
# does, so focus must answer every one of them. "guardwild" cases add the
# constructs outside that claim; there focus may refuse, but must never answer
# differently.
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
               "<?php\nMode: strict\n?>", "a \\` b `\nMode: strict\n` c", "  <!-- indented comment\nMode: off\n-->",
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

for case in cases:
    sys.stdout.write(json.dumps(case, ensure_ascii=False) + "\n")
