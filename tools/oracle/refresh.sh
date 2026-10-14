#!/usr/bin/env bash
# Refresh everything under fixtures/ from an upstream checkout, then check
# focus against it.
#
#   tools/oracle/refresh.sh <upstream checkout> [corpus seed] [corpus scale]
#
# Needs bun (to run upstream) and cargo. Steps:
#   1. build upstream's distributions (bun scripts/package.ts), and install
#      the packages of the Markdown referee;
#   2. copy the authored Markdown the compile reads, the stage-graph.json and
#      scope-grid.json upstream compiled from it, and the installed Claude Code
#      tree the graph verbs read, into fixtures/upstream/;
#   3. regenerate the oracle corpus and have upstream answer it;
#   4. run both engines side by side on edited project trees
#      (differential.py), and record what upstream did in each check.
# After a refresh, `cargo test` replays the new fixtures with no bun involved.
#
# Upstream says `aidlc` where Focus says `focus`. Everything this script
# stores under fixtures/ has been through that map (names.py), so the
# repository holds Focus's names only: upstream is asked in its own words, and
# its files and answers are translated on the way in.
set -euo pipefail

UPSTREAM=$(cd "$1" && pwd)
SEED=${2:-1}
SCALE=${3:-6}
ROOT=$(cd "$(dirname "$0")/../.." && pwd)
cd "$ROOT"

(cd "$UPSTREAM" && bun install --frozen-lockfile >/dev/null && bun scripts/package.ts >/dev/null)
# The third parser the Markdown cases are judged with (referee/referee.ts).
(cd tools/oracle/referee && bun install --frozen-lockfile >/dev/null)

FIX=fixtures/upstream
rm -rf "$FIX/core" "$FIX/golden"
mkdir -p "$FIX/core/aidlc-common" "$FIX/golden"
cp -r "$UPSTREAM/core/aidlc-common/stages" "$FIX/core/aidlc-common/"
cp -r "$UPSTREAM/core/agents" "$UPSTREAM/core/sensors" "$UPSTREAM/core/scopes" "$UPSTREAM/core/memory" "$FIX/core/"
cp "$UPSTREAM/LICENSE" "$FIX/LICENSE"
for pair in claude:.claude cursor:.cursor copilot:.aidlc; do
  harness=${pair%%:*} dir=${pair##*:}
  mkdir -p "$FIX/golden/$harness"
  cp "$UPSTREAM/dist/$harness/$dir/tools/data/stage-graph.json" \
    "$UPSTREAM/dist/$harness/$dir/tools/data/scope-grid.json" "$FIX/golden/$harness/"
done
# The part of an installed Claude Code project the graph verbs read or write,
# exactly as upstream's packager laid it out. Recorded checks are stored as
# differences from this tree.
INSTALLED="$FIX/installed/claude"
rm -rf "$FIX/installed"
mkdir -p "$INSTALLED/.claude/aidlc-common" "$INSTALLED/.claude/tools/data"
cp -r "$UPSTREAM/dist/claude/.claude/aidlc-common/stages" "$INSTALLED/.claude/aidlc-common/"
cp -r "$UPSTREAM/dist/claude/.claude/agents" "$UPSTREAM/dist/claude/.claude/scopes" \
  "$UPSTREAM/dist/claude/.claude/sensors" "$INSTALLED/.claude/"
for file in harness.json stage-graph.json scope-grid.json ars-priors.json; do
  cp "$UPSTREAM/dist/claude/.claude/tools/data/$file" "$INSTALLED/.claude/tools/data/"
done
# The memory seed: what intent creation restores to the default space's memory
# tree when a copy left it out.
cp -r "$UPSTREAM/dist/claude/.claude/tools/data/memory-seed" "$INSTALLED/.claude/tools/data/"
cp -r "$UPSTREAM/dist/claude/aidlc" "$INSTALLED/"
python3 -I tools/oracle/names.py tree "$FIX/core" "$FIX/golden" "$FIX/installed"
sed -i.bak \
  -e "s|^commit: .*|commit: $(git -C "$UPSTREAM" rev-parse HEAD)|" \
  -e "s|^committed: .*|committed: $(git -C "$UPSTREAM" log -1 --format=%cI)|" \
  -e "s|^version: .*|version: $(grep -oE '[0-9]+\.[0-9]+\.[0-9]+' "$UPSTREAM/core/tools/aidlc-version.ts" | head -1)|" \
  "$FIX/PIN"
rm -f "$FIX/PIN.bak"

# Cases are generated from upstream's own files and answered by upstream, then
# both the questions and the answers are translated.
ASKED=$(mktemp -d)
bun tools/oracle/patterns.ts "$UPSTREAM" >"$ASKED/patterns.json"
python3 -I tools/oracle/gen_cases.py "$UPSTREAM/core" "$SEED" "$SCALE" "$ASKED/patterns.json" >"$ASKED/cases.ndjson"
bun tools/oracle/driver.ts "$UPSTREAM" "$ASKED/cases.ndjson" >"$ASKED/expected.ndjson"
python3 -I tools/oracle/names.py text <"$ASKED/cases.ndjson" >fixtures/oracle/cases.ndjson
python3 -I tools/oracle/names.py text <"$ASKED/expected.ndjson" >fixtures/oracle/expected.ndjson
rm -rf "$ASKED"
printf 'seed: %s\nscale: %s\ncases: %s\n' "$SEED" "$SCALE" "$(wc -l <fixtures/oracle/cases.ndjson)" >fixtures/oracle/CORPUS

cargo build --release --quiet
python3 -I tools/oracle/differential.py "$UPSTREAM" target/release/focus --record fixtures/recorded
cargo test --quiet
