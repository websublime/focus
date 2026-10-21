#!/usr/bin/env python3
"""The one map between upstream's names and Focus's.

    names.py tree <dir>...    rename every path and rewrite every text file under each <dir>, in place
    names.py text             the same map over standard input, to standard output

Focus is a fork of upstream frozen at the pinned commit, and everything it
writes, reads or says carries its own name: the workspace directory is
`focus/`, core files are `focus-<name>.md`, the core plugin is `focus`,
variables are `FOCUS_*`. Upstream says `aidlc` in each of those places and in
nothing else that matters here, so the two vocabularies differ by one textual
substitution, and that substitution is this file.

It is what lets the reference keep its job. Upstream runs on a tree in its own
names; Focus runs on the same tree with the map applied; upstream's answer,
with the map applied, is what Focus must say. `refresh.sh` applies it to
everything it vendors under `fixtures/`, and `differential.py` to each pair of
trees it compares, so nothing stored in this repository uses upstream's names
except the tools that drive upstream.

Two properties the comparison relies on, both checked where they could fail:

- the two names have the same length, so no byte count, column or size limit
  moves (the phrases below are the exception, and none is in a compiled
  file);
- they sort differently against other names (`aidlc` < `bravo` < `focus`), so
  a tree holding a name between the two can order its files differently in
  each engine. No vendored tree does, and the differential would show it.

The methodology keeps its name: "AI-DLC" is not mapped, except in the few
phrases where upstream's engine signs something it writes (PHRASES).
"""
import os
import sys

PAIRS = (("aidlc", "focus"), ("AIDLC", "FOCUS"), ("Aidlc", "Focus"))

# Where upstream's engine puts its own name on something it writes. These are
# whole phrases, not the word: in prose "AI-DLC" is the method and stays.
PHRASES = (
    ("# AI-DLC Audit Log", "# Focus Audit Log"),
    ("# AI-DLC State Tracking", "# Focus State Tracking"),
    ("AI-DLC keeps this copy in step with", "Focus keeps this copy in step with"),
    # The engine speaking of itself at a person's terminal.
    ("AI-DLC cannot see a chat in this terminal", "Focus cannot see a chat in this terminal"),
    ("To drive AI-DLC yourself", "To drive Focus yourself"),
    ("your AI-DLC commands", "your Focus commands"),
    ("in the AI-DLC settings", "in the Focus settings"),
    # The engine speaking of itself to the agent at a gate.
    ("Do not run AI-DLC's hook scripts yourself", "Do not run Focus's hook scripts yourself"),
    ("switch any AI-DLC check off", "switch any Focus check off"),
    ("because AI-DLC's hooks are not running here", "because Focus's hooks are not running here"),
    ("Your answer didn't reach AI-DLC.", "Your answer didn't reach Focus."),
    ("a command to AIDLC, not a reply", "a command to Focus, not a reply"),
    # The status as the engine heads it.
    ("AI-DLC Workflow Status", "Focus Workflow Status"),
    ("No active AI-DLC workflow found.", "No active Focus workflow found."),
    # The one variable upstream prefixes otherwise. Every variable Focus reads is FOCUS_*.
    ("AWS_AIDLC_DEFAULT_SCOPE", "FOCUS_DEFAULT_SCOPE"),
)


def to_focus(text):
    """Upstream's vocabulary to Focus's."""
    # Phrases first: they are written in upstream's words, which the pairs would change.
    for theirs, ours in PHRASES + PAIRS:
        text = text.replace(theirs, ours)
    return text


def to_upstream(text):
    """The reverse, for reading a Focus tree with an edit written in upstream's words.

    Not an inverse in general (prose that already said "focus" comes back as
    "aidlc"), but `to_focus(to_upstream(x)) == x` for any text that holds no
    upstream name, which is every file of a mapped tree.
    """
    # Phrases first: once the word is mapped back, a phrase no longer reads as written.
    for theirs, ours in PHRASES + PAIRS:
        text = text.replace(ours, theirs)
    return text


def rename_tree(root):
    """Apply the map to every path and every UTF-8 file under `root`, in place."""
    for folder, dirs, files in os.walk(root, topdown=False):
        for name in files:
            path = os.path.join(folder, name)
            if not os.path.islink(path):
                with open(path, "rb") as handle:
                    data = handle.read()
                try:
                    mapped = to_focus(data.decode("utf-8")).encode("utf-8")
                except UnicodeDecodeError:
                    mapped = data
                if mapped != data:
                    with open(path, "wb") as handle:
                        handle.write(mapped)
            _rename(folder, name)
        for name in dirs:
            _rename(folder, name)
    return root


def _rename(folder, name):
    mapped = to_focus(name)
    if mapped == name:
        return
    target = os.path.join(folder, mapped)
    if os.path.lexists(target):
        raise SystemExit(f"names.py: {os.path.join(folder, name)} maps onto {target}, which already exists")
    os.rename(os.path.join(folder, name), target)


if __name__ == "__main__":
    if len(sys.argv) >= 3 and sys.argv[1] == "tree":
        for directory in sys.argv[2:]:
            rename_tree(directory)
    elif sys.argv[1:] == ["text"]:
        sys.stdout.write(to_focus(sys.stdin.read()))
    else:
        raise SystemExit(__doc__)
