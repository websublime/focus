#!/usr/bin/env python3
"""Lay the oracle corpus out by kind.

    split.py <cases.ndjson> <expected.ndjson> <fixtures/oracle>

The corpus is generated and answered as one stream, in one order: an answer
may depend on the cases of its kind that came before it. It is kept as one
file of cases and one of answers for each kind (`cases/<kind>.ndjson`,
`expected/<kind>.ndjson`), each in the stream's order: a crate replays the
kinds it answers and reads no others, a change to one kind is a change to
its two files, and no file comes near what a forge refuses to take.
"""
import json
import os
import shutil
import sys

cases_path, expected_path, out = sys.argv[1:4]
with open(cases_path, encoding="utf-8") as handle:
    cases = [line for line in handle.read().split("\n") if line]
with open(expected_path, encoding="utf-8") as handle:
    expected = [line for line in handle.read().split("\n") if line]
if len(cases) != len(expected):
    sys.exit(f"{len(cases)} cases and {len(expected)} answers")
by_kind = {}
for case, answer in zip(cases, expected):
    asked = json.loads(case)
    if json.loads(answer)["id"] != asked["id"]:
        sys.exit(f"the answer to {asked['id']} is not in its place")
    by_kind.setdefault(asked["kind"], ([], []))
    by_kind[asked["kind"]][0].append(case)
    by_kind[asked["kind"]][1].append(answer)
for part in ("cases", "expected"):
    shutil.rmtree(os.path.join(out, part), ignore_errors=True)
    os.makedirs(os.path.join(out, part))
for kind, (asked, answered) in sorted(by_kind.items()):
    for part, lines in (("cases", asked), ("expected", answered)):
        with open(os.path.join(out, part, f"{kind}.ndjson"), "w", encoding="utf-8", newline="\n") as handle:
            handle.write("\n".join(lines) + "\n")
print(f"{len(cases)} cases of {len(by_kind)} kinds")
