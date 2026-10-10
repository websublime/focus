"""How a `markdown` answer from Focus is held to upstream's.

Upstream finds a document's structure by rendering it with probe words in it
and looking for where they come out. That is an approximation of the
CommonMark structure it means to read: it leaves some lines unplaced, and in
nested containers it places some wrongly. Focus reads the structure from a
parser. So the two answers are not required to be equal; each line is judged:

  same             Focus's record is upstream's.
  unplaced         upstream did not place the line, and the referee reads it
                   as Focus does.
  no text          the line is white space and container markers, and upstream
                   and Focus differ only in whether it is blank or inside a
                   code or HTML block; or upstream did not place it and Focus
                   reads it as one of those. Nothing reads such a line.
  upstream alone   the two differ, and the referee (a third parser, see
                   `referee/referee.ts`) reads the line as Focus does.

"The referee reads the line as Focus does" means micromark does; or, on a
line that none of the three calls a table, that commonmark.js does. The
second is the reference implementation of CommonMark and knows no tables; it
is there because micromark has readings of its own in nested lists.

  follows          the two agree on the line's structure and differ on a code
                   span or tag that runs over a line judged one of the above.
  repair column    a code span or tag carries on from the line before onto a
                   line that opens with an indented HTML tag. Upstream renders
                   such a line with a no-break space in its last column of
                   indentation, to keep its renderer from opening an HTML
                   block there, and reports the span as starting at that
                   column, one short. Focus reports where the line's text
                   starts.

and, as failures of Focus unless a caller says otherwise:

  focus alone      upstream and the referee agree, and Focus reads otherwise;
                   or upstream did not place the line and the referee reads
                   it otherwise than Focus.
  three ways       all three read the line differently.
  port             same structure, nothing nearby to explain it: a defect in
                   what Focus builds on its parser.

Upstream stops probing a document after a fixed number of renders and leaves
what it had not reached: lines unplaced, and code spans and tags unrecorded
on lines it did place. Both are judged as above, like any other line.

A document Focus refuses to read (its answer is an error) fails as a whole.
"""
import collections
import re

# Quote markers, and list markers with nothing or white space after them.
MARKERS_ONLY = re.compile(r"^[ \t]*(?:(?:>|(?:[-*+]|[0-9]{1,9}[.)])(?=[ \t]|$))[ \t]*)*$")
RAW_OR_BLANK = {"blank", "codeFenced", "codeIndented", "htmlFlow"}
# What a document in the vendored corpus may hold; a careless one may also
# hold `follows` and, up to a ceiling, the two of READING.
ACCEPTED = ("same", "unplaced", "no text", "upstream alone", "repair column")
READING = ("focus alone", "three ways")
# The fields of a line's record.
KIND, HTML_KIND, CONTENT_START, CONTAINERS, GROUP, SPANS = range(6)


def source_lines(content):
    return content.removeprefix("﻿").replace("\r\n", "\n").replace("\r", "\n").split("\n")


def structure(record):
    """What the referee also reports: kind, containers, content start, HTML block."""
    return [record[KIND], record[CONTAINERS], record[CONTENT_START], record[GROUP]]


def tokens(record):
    """The constructs a line's spans belong to, as the referee names them: by
    kind and by the lines they run over. Several on a line may share a name."""
    return collections.Counter((span[2], span[3], span[4]) for span in record[SPANS])


def columns_settled(index, theirs, mine, starts, ends):
    """Whether the two records name the same constructs, differ only in
    where one starts on its first line or ends on its last, and the referee
    has it where Focus does."""
    if theirs[:SPANS] != mine[:SPANS] or len(theirs[SPANS]) != len(mine[SPANS]):
        return False
    settled = False
    for a, b in zip(theirs[SPANS], mine[SPANS]):
        if a == b:
            continue
        name = (b[2], b[3], b[4])
        if a[2:] != b[2:]:
            return False
        if a[0] != b[0] and not (b[3] == index and b[0] in starts[name] and a[0] not in starts[name]):
            return False
        if a[1] != b[1] and not (b[4] == index and b[1] in ends[name] and a[1] not in ends[name]):
            return False
        settled = True
    return settled


def repair_column(text, index, theirs, mine):
    """Whether the two records differ only as described under `repair column`.
    The projections may differ with the span: they keep the text before it."""
    if theirs[:SPANS] != mine[:SPANS] or len(theirs[SPANS]) != len(mine[SPANS]):
        return False
    differing = [(a, b) for a, b in zip(theirs[SPANS], mine[SPANS]) if a != b]
    return len(differing) == 1 and all(
        a[1:] == b[1:] and a[0] == b[0] - 1 and b[3] < index and b[0] == mine[CONTENT_START]
        and text.encode("utf-16-le")[2 * b[0]:2 * b[0] + 2] == b"<\0"
        for a, b in differing)


def judge(content, upstream, focus):
    """Verdicts for one document: a list with (verdict, detail) for each line,
    then a list of document-level failures."""
    if "lines" not in focus:
        return [], [f"focus refused the document: {focus.get('error')}"]
    source = source_lines(content)
    referee = upstream["referee"]
    real = collections.Counter(tuple(token[:3]) for token in referee["tokens"])
    starts, ends = collections.defaultdict(set), collections.defaultdict(set)
    for token in referee["tokens"]:
        starts[tuple(token[:3])].add(token[3])
        ends[tuple(token[:3])].add(token[4])
    verdicts = []
    for index, (theirs, mine) in enumerate(zip(upstream["lines"], focus["lines"])):
        # The referee's reading is listed only where it is not upstream's,
        # and the second opinion only where it is not the referee's.
        said = referee["lines"].get(str(index)) or (theirs and structure(theirs))
        other = referee["second"].get(str(index), said)
        tabular = "table" in {mine[KIND], said[KIND], theirs[KIND] if theirs else None}
        backed = structure(mine) == said or (not tabular and structure(mine) == other)
        if theirs is None:
            if backed:
                verdicts.append(("unplaced", ""))
            elif MARKERS_ONLY.match(source[index]) and mine[KIND] in RAW_OR_BLANK:
                verdicts.append(("no text", ""))
            else:
                verdicts.append(("focus alone", f"upstream nothing, focus {structure(mine)} referee {said}"))
        elif theirs == mine:
            verdicts.append(("same", ""))
        elif MARKERS_ONLY.match(source[index]) and {theirs[KIND], mine[KIND]} <= RAW_OR_BLANK:
            verdicts.append(("no text", ""))
        elif structure(theirs) != structure(mine):
            detail = f"upstream {structure(theirs)} focus {structure(mine)} referee {said}"
            if backed:
                verdicts.append(("upstream alone", detail))
            elif structure(theirs) == said:
                verdicts.append(("focus alone", detail))
            else:
                verdicts.append(("three ways", detail))
        else:
            verdicts.append(("spans", f"upstream {theirs[SPANS:]} focus {mine[SPANS:]}"))
    # A difference in spans is the referee's to settle, or follows from a
    # line the span's construct runs over.
    for index, (verdict, detail) in enumerate(verdicts):
        if verdict != "spans":
            continue
        theirs, mine = upstream["lines"][index], focus["lines"][index]
        # The referee settles it for Focus when it sees every construct
        # Focus records, and fewer than upstream records where upstream
        # records more.
        of_focus, of_upstream = tokens(mine), tokens(theirs)
        if of_focus != of_upstream:
            def sees(one, other):
                return (all(count <= real[name] for name, count in one.items())
                        and all(other[name] > real[name] for name in other - one))

            if sees(of_focus, of_upstream):
                verdicts[index] = ("upstream alone", detail)
                continue
            if sees(of_upstream, of_focus):
                # How a table row splits into cells decides what is a code
                # span in it, and the parsers differ on an escaped pipe.
                verdicts[index] = ("focus alone", detail)
                continue
        if of_focus == of_upstream and columns_settled(index, theirs, mine, starts, ends):
            verdicts[index] = ("upstream alone", detail)
            continue
        if repair_column(source[index], index, theirs, mine):
            verdicts[index] = ("repair column", detail)
            continue
        differing = (of_focus - of_upstream) + (of_upstream - of_focus)
        covered = {line for _, first, last in differing for line in range(first, last + 1)}
        near = [verdicts[line][0] for line in covered if line != index and line < len(verdicts)]
        if any(verdict in READING for verdict in near):
            verdicts[index] = ("follows a reading", detail)
        elif any(verdict in ("unplaced", "no text", "upstream alone") for verdict in near):
            verdicts[index] = ("follows", detail)
        else:
            verdicts[index] = ("port", detail)
    # Definitions are compared where their lines were; labels when every
    # line was.
    failures = []
    loose = {index for index, (verdict, _) in enumerate(verdicts) if verdict != "same"}
    theirs = {tuple(entry) for entry in upstream["definitions"]}
    mine = {tuple(entry) for entry in focus["definitions"]}
    # A definition is judged with the definitions it touches: where one side
    # ends a definition a line early, the other's next one starts there.
    defined = {line for entry in theirs | mine for line in range(entry[1], entry[2] + 1)}

    def run(entry):
        first, last = entry[1], entry[2]
        while first - 1 in defined:
            first -= 1
        while last + 1 in defined:
            last += 1
        return set(range(first, last + 1))

    stray = sorted(entry for entry in theirs ^ mine if not loose & run(entry))
    if stray:
        failures.append(f"definitions differ on lines both placed alike: {stray}")
    if not loose and upstream["labels"] != focus["labels"]:
        failures.append(f"labels: upstream {upstream['labels']} focus {focus['labels']}")
    if len(upstream["lines"]) != len(focus["lines"]):
        failures.append(f"upstream has {len(upstream['lines'])} lines, focus {len(focus['lines'])}")
    return verdicts, failures


class Tally:
    """Verdicts over many documents."""

    def __init__(self):
        self.lines = collections.Counter()
        self.documents = collections.Counter()
        self.examples = collections.defaultdict(list)
        self.failures = []
        # For each document: the verdicts its lines got, and whether it failed as a whole.
        self.held = []

    def add(self, case, upstream, focus):
        self.documents["documents"] += 1
        verdicts, failures = judge(case["content"], upstream, focus)
        for index, (verdict, detail) in enumerate(verdicts):
            self.lines[verdict] += 1
            if verdict not in ("same", "unplaced") and len(self.examples[verdict]) < 6:
                self.examples[verdict].append(f"{case['id']} line {index + 1}: {detail}")
        for verdict in {verdict for verdict, _ in verdicts} - {"same", "unplaced"}:
            self.documents[verdict] += 1
        self.failures.extend(f"{case['id']}: {failure}" for failure in failures)
        self.held.append(({verdict for verdict, _ in verdicts}, bool(failures)))

    def of(self, *verdicts):
        return sum(self.lines[verdict] for verdict in verdicts)

    def summary(self):
        judged = self.documents["documents"]
        parts = [f"{judged} documents", f"{sum(self.lines.values())} lines"]
        for verdict in ("same", "unplaced", "no text", "upstream alone", "repair column", "follows", "focus alone",
                        "three ways", "follows a reading", "port"):
            if self.lines[verdict]:
                where = f" in {self.documents[verdict]} of {judged}" if verdict not in ("same", "unplaced") else ""
                parts.append(f"{self.lines[verdict]} {verdict}{where}")
        return ", ".join(parts)
