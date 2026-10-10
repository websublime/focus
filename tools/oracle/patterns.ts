// Upstream's case-insensitive regular expressions, as it wrote them.
//
//   bun patterns.ts <upstream checkout>
//
// prints a JSON list of the source of every regex literal under `core/` and
// `harness/` (tests aside) whose flags have `i` and neither `u` nor `v`: the
// mode in which Focus's engine compares characters otherwise than JavaScript
// and `focus-js` corrects it (crates/focus-js/src/re.rs). The oracle corpus
// runs each of them over a text with every cased character (kind `icase`),
// so a pattern Focus would refuse, or answer differently, shows there before
// the module that uses it is ported.
//
// The literals are found with TypeScript's own parser, taken from upstream's
// packages, so a `/` inside a class or a string is not a problem.
import { readdirSync, readFileSync } from "node:fs";
import { join } from "node:path";

const upstream = process.argv[2];
const ts = (await import(join(upstream, "node_modules/typescript"))).default;
const found = new Set<string>();

function scan(dir: string): void {
  for (const entry of readdirSync(dir, { withFileTypes: true }).sort((a, b) => (a.name < b.name ? -1 : 1))) {
    const path = join(dir, entry.name);
    if (entry.isDirectory()) {
      if (entry.name !== "node_modules") scan(path);
      continue;
    }
    if (!/\.(?:ts|mts|js|mjs)$/.test(entry.name) || /\.test\./.test(entry.name)) continue;
    const file = ts.createSourceFile(path, readFileSync(path, "utf-8"), ts.ScriptTarget.Latest, true);
    const visit = (node: any): void => {
      if (ts.isRegularExpressionLiteral(node)) {
        const end = node.text.lastIndexOf("/");
        const flags = node.text.slice(end + 1);
        if (flags.includes("i") && !/[uv]/.test(flags)) found.add(node.text.slice(1, end));
      }
      ts.forEachChild(node, visit);
    };
    visit(file);
  }
}

scan(join(upstream, "core"));
scan(join(upstream, "harness"));
process.stdout.write(`${JSON.stringify([...found].sort())}\n`);
