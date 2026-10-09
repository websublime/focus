#!/usr/bin/env python3
"""Compare two oracle answer files line by line.

    compare.py <expected.ndjson> <actual.ndjson>

Exits 0 when focus gave upstream's answer to every case, 1 otherwise, printing
the first differences.

One kind of case may be declined instead of answered: `guardwild`, a memory
file with Markdown that focus has no parser for. There focus answers
`{"refused": reason}`; that is counted and shown, never passed off as
agreement, and it is the only kind where it is allowed.
"""
import collections
import json
import sys

expected = open(sys.argv[1], encoding="utf-8").read().split("\n")
actual = open(sys.argv[2], encoding="utf-8").read().split("\n")
differ, refused = [], collections.Counter()
for i in range(max(len(expected), len(actual))):
    want = expected[i] if i < len(expected) else None
    got = actual[i] if i < len(actual) else None
    if want == got:
        continue
    if got and want and want.startswith('{"id":"guardwild-') and '"out":{"refused":' in got:
        refused[json.loads(got)["out"]["refused"].split(" ", 2)[2]] += 1
        continue
    differ.append(i)
kinds = collections.Counter(json.loads(line)["id"].split("-")[0] for line in expected if line)
print(f"cases: {sum(kinds.values())} ({', '.join(f'{n} {k}' for k, n in sorted(kinds.items()))})")
if refused:
    print(f"declined by focus (guardwild only): {sum(refused.values())} of {kinds['guardwild']}")
    for reason, count in refused.most_common():
        print(f"  {count:5d}  {reason}")
print(f"differences: {len(differ)}")
for i in differ[:8]:
    print(f"--- line {i + 1}")
    print("  upstream:", (expected[i] if i < len(expected) else "<missing>")[:700])
    print("  focus:   ", (actual[i] if i < len(actual) else "<missing>")[:700])
sys.exit(1 if differ else 0)
