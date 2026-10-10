#!/usr/bin/env python3
"""Compare two oracle answer files line by line.

    compare.py <expected.ndjson> <actual.ndjson> [cases.ndjson]

Exits 0 when focus gave upstream's answer to every case, 1 otherwise, printing
the first differences.

One kind of case is not held to equality: `markdown`, a document's structure line by line. Upstream approximates it and
Focus reads it from a parser, so each line is judged (see `markdown_rule.py`):
it must be upstream's, or be a line upstream did not place, or be read by the
referee as Focus reads it. The verdicts are counted and shown. Judging needs
the documents, so the cases file is required when there are such cases.
"""
import collections
import json
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import markdown_rule  # noqa: E402

expected = open(sys.argv[1], encoding="utf-8").read().split("\n")
actual = open(sys.argv[2], encoding="utf-8").read().split("\n")
cases = open(sys.argv[3], encoding="utf-8").read().split("\n") if len(sys.argv) > 3 else None
differ, markdown = [], markdown_rule.Tally()
for i in range(max(len(expected), len(actual))):
    want = expected[i] if i < len(expected) else None
    got = actual[i] if i < len(actual) else None
    if want == got:
        continue
    if got and want and ',"referee":{"lines":' in want:
        if cases is None:
            sys.exit("compare.py: markdown cases need the cases file as a third argument")
        markdown.add(json.loads(cases[i]), json.loads(want)["out"], json.loads(got)["out"])
        continue
    differ.append(i)
kinds = collections.Counter(json.loads(line)["id"].split("-")[0] for line in expected if line)
print(f"cases: {sum(kinds.values())} ({', '.join(f'{n} {k}' for k, n in sorted(kinds.items()))})")
unexplained = 0
if markdown.documents["documents"]:
    print(f"markdown: {markdown.summary()}")
    for verdict in sorted(set(markdown.lines) - set(markdown_rule.ACCEPTED)):
        unexplained += markdown.lines[verdict]
        for example in markdown.examples[verdict][:3]:
            print(f"  {verdict}: {example[:400]}")
    unexplained += len(markdown.failures)
    for failure in markdown.failures[:5]:
        print(f"  {failure[:400]}")
print(f"differences: {len(differ) + unexplained}")
for i in differ[:8]:
    print(f"--- line {i + 1}")
    print("  upstream:", (expected[i] if i < len(expected) else "<missing>")[:700])
    print("  focus:   ", (actual[i] if i < len(actual) else "<missing>")[:700])
sys.exit(1 if differ or unexplained else 0)
