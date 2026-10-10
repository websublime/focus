"""Markdown documents for the oracle.

    writer(rng, careful=True)   documents as a careful author writes them
    writer(rng, careful=False)  anything the syntax allows, and what it does not

Upstream works a document's structure out by rendering it with probe words
in it; Focus reads it from a parser. On what a careful author writes the two
must agree line for line, and those documents go into the vendored corpus
(`gen_cases.py`). On the rest they are compared live, with a third parser as
referee (`differential.py --markdown`).

`writer` returns a function that makes one document per call.
"""

WORDS = ["alpha", "beta", "gamma", "Q1.", "the", "plan", "is", "ready", "x", "2", "a.b", "é", "naïve", "**bold**",
         "_em_", "~~gone~~", "[Answer]:", "Looks", "correct", "&amp;", "&#35;", "\\*", "\\`", "(note)", "!", "...",
         "\U0001F600", "#", "##", "-", "+", "1.", ">", "|", "=", "[link](http://x.y/z)", "[ref]", "[ref][other]",
         "![img](a.png)", "<http://auto.link>", "<b>", "</b>", "<br/>", "<span class=\"x\">", "<h2>", "</h2>",
         "<!-- c -->", "<!---->", "<!-->", "`code`", "`a b`", "`` a`b ``", "` x `", "```", "``", "`", "<x", "a<b",
         "5 < 6", "<3", "\ttab", "  ", "\\"]
CAREFUL_WORDS = ["alpha", "beta", "gamma", "Q1.", "the", "plan", "is", "ready", "x", "2", "a.b", "é", "naïve",
                 "**bold**", "_em_", "~~gone~~", "[Answer]:", "Looks", "correct", "&amp;", "&#35;", "\\*", "\\`",
                 "(note)", "!", "...", "\U0001F600", "[link](http://x.y/z)", "[ref]", "[ref][other]",
                 "![img](a.png)", "<http://auto.link>", "<b>bold</b>", "<br/>", "<span class=\"x\">s</span>",
                 "<!-- c -->", "`code`", "`a b`", "`` a`b ``", "` x `", "5 < 6", "a < b", "v1.2", "C#", "50%"]
CODE_LINES = ["let x = 1;", "## not a heading", "<!-- not a comment", "-->", "[Answer]: no", "", "", "---", "***",
              "    indented", "| a | b |", "```", "~~~", "````", "> quoted", "- item", "\ttab", "$ run --flag", "!!!"]
CAREFUL_CODE_LINES = ["let x = 1;", "## not a heading", "<!-- not a comment", "-->", "[Answer]: no", "", "", "---",
                      "***", "    indented", "| a | b |", "> quoted", "- item", "$ run --flag", "!!!", "<div>",
                      "**Mode**: strict", "`tick`"]
FIELD_LINES = ["**Field**: value", "[Answer]: Looks correct", "- **Mode**: strict", "Mode: relaxed", "A. option",
               "text with trailing spaces  ", "ends with backslash\\", "@mention #tag", "https://plain.link/x"]


def writer(rng, careful, hostility=0.15):
    """A function that writes one document per call.

    `hostility` is the share of documents that get a few characters inserted,
    dropped or replaced after they are written. A careful writer never does.
    """
    words_of = CAREFUL_WORDS if careful else WORDS
    code_lines = CAREFUL_CODE_LINES if careful else CODE_LINES

    def wild(chance=1.0):
        """Whether to take the unusual branch."""
        return not careful and rng.random() < chance

    def pick(usual, unusual):
        return rng.choice(unusual) if unusual and wild(0.25) else rng.choice(usual)

    def words(low=1, high=6):
        return " ".join(rng.choice(words_of) for _ in range(rng.randint(low, high)))

    def inline_line():
        roll = rng.random()
        if roll < 0.55:
            return words()
        if roll < 0.7:
            code = rng.choice(["code", "a b", " padded ", "x|y", "a\\\\|b", "<b>", "<!--"])
            return f"{words(0, 2)} `{code}` {words(0, 3)}".strip()
        if roll < 0.8:
            return f"{words(0, 2)} <!-- {words(1, 3)} --> {words(0, 2)}".strip()
        if roll < 0.9:
            if careful:
                tag = rng.choice(["b", "span", "a", "h2", "h3", "x-y"])
                attributes = rng.choice(["", "", ' class="x"', ' href="u"', " id=x"])
                return f"{words(0, 2)} <{tag}{attributes}>{words(1, 2)}</{tag}> {words(0, 2)}".strip()
            opening = rng.choice(["b", "span", 'a href="u"', "h2", "h3 id=x", "x-y z=1", "br /"])
            closing = rng.choice(["b", "span", "a", "h2"])
            return f"{words(0, 2)} <{opening}>{words(0, 2)}</{closing}> {words(0, 2)}".strip()
        return rng.choice(FIELD_LINES)

    def paragraph():
        lines = [inline_line() for _ in range(rng.choice([1, 1, 1, 2, 2, 3]))]
        roll = rng.random()
        if roll < 0.1:
            # A construct that runs over a line break.
            closed = [("`", "`"), ("``", "``"), ("<!--", "-->"), ("<span\n  class=\"x\"", ">s</span>"),
                      ("<a href=\"u\"\ntitle=\"t\"", ">link</a>"), ("[link](", ")"), ("**", "**")]
            opener, closer = rng.choice(closed if careful else closed + [("`", ""), ("<!--", "")])
            lines = [f"{words(0, 2)} {opener}{words(1, 2)}", f"{words(1, 2)}{closer} {words(0, 2)}"] + lines[1:]
            lines = "\n".join(lines).split("\n")
        if roll > 0.93 and wild():
            lines[rng.randrange(len(lines))] = rng.choice(
                ["    " + words(), "\t" + words(), "   " + words(), "  - not a list?"])
        return [line.strip() if careful else line for line in lines if line.strip() or wild(0.3)] or [words()]

    def heading():
        titles = [words(1, 4), "Q1. Title", "Guard Policy", "Requested Changes Feedback", "<!-- c --> Title",
                  "A `code` title"]
        roll = rng.random()
        if roll < 0.7:
            level = pick([1, 2, 2, 2, 3, 4, 6], [7])
            closing = pick(["", "", "", " #", " ##"], [" ####   ", "#"])
            space = pick([" "], ["  ", "\t", ""])
            indent = pick([""], [" ", "   ", "    "])
            return [f"{indent}{'#' * level}{space}{rng.choice(titles if careful else titles + [''])}{closing}"]
        text = [inline_line() for _ in range(rng.choice([1, 1, 2]))]
        return text + [pick(["===", "---", "=", "-"], ["== ", " ---", "    ---", "=-=", "--- x", "- - -"])]

    def fence():
        char = rng.choice(["`", "`", "`", "~"])
        length = rng.choice([3, 3, 3, 4, 5])
        info = rng.choice(["", "", "js", "text", " python title", "{.x}"] + ([] if careful else ["+++ x", "a`b", "~~~", " "]))
        indent = rng.choice(["", "", "", " ", "   "])
        body = [rng.choice(code_lines) for _ in range(rng.choice([0, 1, 1, 2, 3, 4]))]
        closer = pick([char * length] * 6 + [char * (length + 1)] + ([] if careful else [None]),
                      [char * (length - 1), ("~" if char == "`" else "`") * length, char * length + " x",
                       "   " + char * length, "    " + char * length])
        lines = [f"{indent}{char * length}{info}"] + [indent + line if line else line for line in body]
        if closer is not None:
            lines.append(indent + closer)
        return lines

    def indented():
        lines = [pick(["    "], ["\t", "     ", " \t"]) + rng.choice(code_lines + ["code"])
                 if careful or rng.random() < 0.9 else "" for _ in range(rng.choice([1, 2, 3]))]
        return lines

    def html_block():
        roll = rng.random()
        if roll < 0.3:
            body = [rng.choice(["## hidden heading", "[Answer]: hidden", words(), "", "", "```", "- x"]
                               + ([] if careful else ["<!-- nested", "--> not yet?"]))
                    for _ in range(rng.choice([0, 1, 2, 3]))]
            if careful:
                start, end = rng.choice(["<!--", "<!--", "<!-- note"]), rng.choice(["-->", "-->", "--> trailing text"])
            else:
                end = rng.choice(["-->", "-->", "-->", "--> trailing text", "--> <!-- again --> more", "", "--!>", "->"])
                start = rng.choice(["<!--", "<!--", "<!-- note", "<!--\tx", " <!--", "   <!--", "<!---", "<!-->",
                                    "<!--->", "<!-- a --> b", "<!-- one --> <!-- two"])
            if rng.random() < 0.3:
                return [f"{start} {words(1, 2)} {end}".rstrip()]
            return [start] + body + ([end] if end else [])
        if roll < 0.45:
            tag = rng.choice(["script", "pre", "style", "textarea"] + ([] if careful else ["SCRIPT"]))
            endings = [[f"</{tag}>"], [f"</{tag}> after"]] + ([] if careful else [[], [f"x </{tag}> y"]])
            return [f"<{tag}>"] + [rng.choice(code_lines) for _ in range(rng.choice([0, 1, 3]))] + rng.choice(endings)
        if roll < 0.55:
            if careful:
                return [rng.choice(["<?php echo 1; ?>", "<!DOCTYPE html>"])]
            opening = rng.choice(["<?php echo 1; ?>", "<?xml", "<!DOCTYPE html>", "<!ELEMENT x", "<![CDATA[", "<!x"])
            return [opening] + ([rng.choice(code_lines), rng.choice(["?>", "]]>", ">", "x"])] if rng.random() < 0.5 else [])
        if roll < 0.85:
            tag = rng.choice(["div", "div", "table", "p", "h2", "details", "summary", "ul", "li", "section", "hr",
                              "search", "blockquote", "td"] + ([] if careful else ["DIV"]))
            body = [rng.choice(["## inside html", "[Answer]: inside", words(), "<!-- c -->", "`code`", "  indented"]
                               + ([] if careful else [f"</{tag}>"]))
                    for _ in range(rng.choice([0, 1, 2]))]
            if careful:
                return [rng.choice([f"<{tag}>", f"<{tag} class=\"x\">", f"<{tag}>text"])] + body + [f"</{tag}>"]
            opening = rng.choice([f"<{tag}>", f"<{tag} class=\"x\">", f"<{tag}", f"</{tag}>", f"<{tag}/>", f"  <{tag}>",
                                  f"<{tag}>text", f"<{tag} a='<!--'>", f"<{tag}> <!-- c -->"])
            return [opening] + body
        if careful:
            tag = rng.choice(["span", "x-custom", "b"])
            return rng.choice([[f"<{tag}>", words(), f"</{tag}>"], [f"<{tag}>{words()}</{tag}>"]])
        tag = rng.choice(["span", "a href=\"x\"", "x-custom", "b", "custom attr=\"v\"", "img src=x /", "a\nhref=\"multi\""])
        return rng.choice([[f"<{tag}>"], [f"<{tag}>", words()], [f"</{tag.split()[0]}>"], [f"<{tag}> trailing"],
                           [f"<{tag}>", ""]])

    def table():
        columns = rng.choice([1, 2, 2, 3])
        cell = lambda: rng.choice(["a", "b c", "`code`", "`a\\|b`", "a\\|b", "", "**x**", "<b>t</b>", "<!-- c -->", "1",
                                   "!", "`x|y`", "-", words(1, 2)])
        if not wild(0.5):
            # As a careful author writes one: every row has its pipes and its cells.
            row = lambda n: "| " + " | ".join(cell() or "x" for _ in range(n)) + " |"
            delimiter = "| " + " | ".join(rng.choice(["---", ":--", "--:", ":-:"]) for _ in range(columns)) + " |"
            return [row(columns), delimiter] + [row(columns) for _ in range(rng.choice([0, 1, 2, 3]))]
        row = lambda n: (rng.choice(["| ", "| ", "|", ""]) + " | ".join(cell() for _ in range(n))
                         + rng.choice([" |", " |", "|", ""]))
        delimiter = (rng.choice(["| ", "|", ""])
                     + " | ".join(rng.choice(["---", "---", ":--", "--:", ":-:", "-", "----"])
                                  for _ in range(columns if rng.random() < 0.85 else columns + 1))
                     + rng.choice([" |", "|", ""]))
        return [row(columns), delimiter] + [row(rng.choice([columns, columns, columns, columns - 1 or 1, columns + 1]))
                                            for _ in range(rng.choice([0, 1, 2, 3]))]

    def rule():
        return [pick(["---", "***", "___", "- - -", "* * *", "-----"],
                     [" ---", "   ***", "    ---", "--", "**", "_ _ _ _", "---x", "+++", "==="])]

    def definition():
        if careful:
            label = rng.choice(["ref", "other", "Ref", "a b", "x-1", "é", "long " * 3 + "label"])
            destination = rng.choice(["/url", "<http://x.y>", "http://x.y/z", "#frag"])
            title = rng.choice(["", "", ' "title"', " 'title'", " (title)", '\n  "title on next line"'])
            return f"[{label}]: {destination}{title}".split("\n")
        label = rng.choice(["ref", "other", "Ref", "a b", "a  b", "x-1", "é", "label\\]x", "", "long " * 3,
                            "with `code`", "foo]bar"])
        destination = rng.choice(["/url", "<http://x.y>", "http://x.y/z", "<>", "", "a b", "#frag", "</url"])
        title = rng.choice(["", "", ' "title"', " 'title'", " (title)", '\n  "title on next line"', ' "broken',
                            ' "t" trailing'])
        return f"{pick([''], [' ', '   ', '    '])}[{label}]:{pick([' '], ['', chr(10) + '  '])}{destination}{title}".split("\n")

    leaves = [(paragraph, 30), (heading, 14), (fence, 12), (indented, 4), (html_block, 12), (table, 6), (rule, 5),
              (definition, 5)]

    def leaf():
        roll = rng.uniform(0, sum(weight for _, weight in leaves))
        for make, weight in leaves:
            roll -= weight
            if roll <= 0:
                return make()
        return paragraph()

    def blocks(depth):
        deepest = 2 if careful else 3
        out = []
        for _ in range(rng.choice([1, 1, 2, 2, 3] if depth else [2, 3, 4, 5, 6, 8])):
            roll = rng.random()
            if depth < deepest and roll < 0.14:
                lines = bullet_list(depth + 1)
            elif depth < deepest and roll < 0.24:
                lines = quote(depth + 1)
            else:
                lines = leaf()
            if out and (rng.random() < 0.8 or not wild()):
                out.append("")
                if rng.random() < 0.08:
                    out.append("")
            out.extend(lines)
        return out

    def bullet_list(depth):
        ordered = rng.random() < 0.35
        number = rng.choice([1, 1, 1, 2, 7, 10] + ([] if careful else [0, 123456789]))
        char = rng.choice(["-", "-", "*", "+"])
        delimiter = rng.choice([".", ".", ")"])
        out = []
        for item in range(rng.choice([1, 2, 2, 3])):
            marker = f"{number + item}{delimiter}" if ordered else char
            gap = pick([" "], ["  ", "   ", "\t", "     "])
            content = blocks(depth) if careful or rng.random() < 0.85 else [""]
            width = len(marker) + (len(gap) if len(gap) <= 4 and gap != "\t" else 1)
            if gap == "\t":
                width = 4 if len(marker) < 4 else len(marker) + 1
            indent = " " * width
            lines = []
            for index, line in enumerate(content):
                if index == 0:
                    lines.append(f"{marker}{gap}{line}" if line else marker)
                elif not line:
                    lines.append("")
                else:
                    # Mostly indented as the item's content, sometimes not
                    # enough (lazy, or out of the item).
                    roll = rng.random() if wild() else 0.0
                    lines.append((indent if roll < 0.88 else indent[:-1] if roll < 0.93 else "" if roll < 0.97
                                  else indent + "  ") + line)
            if out and rng.random() < 0.35:
                out.append("")
            out.extend(lines)
        lead = pick([""], [" ", "  ", "   "])
        return [lead + line if line else line for line in out]

    def quote(depth):
        out = []
        for index, line in enumerate(blocks(depth)):
            roll = rng.random() if wild() else 0.0
            if index == 0 or roll < 0.86:
                out.append(pick(["> "], [">", " > ", "   > ", ">  ", ">\t"]) + line if line else pick([">"], ["> ", ""]))
            elif roll < 0.95:
                out.append(line)  # lazy, or the quote ends
            else:
                out.append(">" + rng.choice(["> ", " > "]) + line)
        return out

    def fuzz(text):
        chars = list(text)
        for _ in range(rng.randint(1, 4)):
            at = rng.randrange(len(chars) + 1)
            roll = rng.random()
            piece = rng.choice(["`", "<", ">", "\n", "\n\n", " ", "    ", "\t", "#", "-", "*", "|", "\\", "[", "]", "!",
                                "&", "~", "=", "<!--", "-->", "```", "\u00a0", "\u2003", "\x00", "\x01", "\x0b", "\x0c",
                                "é", "\U0001F600"])
            if roll < 0.6:
                chars.insert(at, piece)
            elif roll < 0.85 and chars:
                del chars[min(at, len(chars) - 1)]
            elif chars:
                chars[min(at, len(chars) - 1)] = piece
        return "".join(chars)

    def document():
        text = "\n".join(blocks(0))
        if not careful and rng.random() < hostility:
            text = fuzz(text)
        ending = rng.random()
        if ending < 0.6:
            text += "\n"
        elif ending < 0.7:
            text += "\n\n"
        shape = rng.random()
        if shape < 0.06:
            text = text.replace("\n", "\r\n")
        elif shape < 0.08:
            text = text.replace("\n", "\r")
        elif shape < 0.1:
            text = "\ufeff" + text
        return text

    return document


if __name__ == "__main__":
    # markdown_docs.py <seed> <count> careful|free [hostility]: cases, one per line.
    import json
    import random
    import sys

    write = writer(random.Random(int(sys.argv[1])), sys.argv[3] == "careful",
                   float(sys.argv[4]) if len(sys.argv) > 4 else 0.15)
    for index in range(int(sys.argv[2])):
        print(json.dumps({"id": f"markdown-{index:05}", "kind": "markdown", "content": write()}, ensure_ascii=False))
