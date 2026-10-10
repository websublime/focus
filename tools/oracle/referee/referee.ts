// A third reading of Markdown, and a fourth.
//
// Focus reads a document's structure from a parser (pulldown-cmark);
// upstream works it out by rendering the document with probe words in it.
// Both mean to read CommonMark with GFM tables, and on documents people
// write they agree line for line. Where they differ, neither can be the
// judge. Two parsers that report source positions are:
//
// - micromark (through mdast), with GFM tables and strikethrough: the
//   referee. It also says where inline code and raw HTML are.
// - commonmark.js, the reference implementation of CommonMark: the second
//   opinion. It knows no tables, so it is asked only about lines none of
//   the others calls a table, and only where the referee and Focus differ.
//   In nested lists the referee has readings of its own (an ordered list
//   that does not start at 1, right after indented code, is no list to it),
//   and there the reference implementation settles it.
//
// `referee(content)` answers:
//   lines:  for each source line, [kind, containers, contentStart, group]
//           in the terms of the oracle's `markdown` answer
//   tokens: every inline code span and raw HTML tag, as
//           [kind, firstLine, lastLine, startColumn, endColumn]: the column
//           it starts at on its first line and ends before on its last
// `second(content)` answers the same `lines`, from commonmark.js.
//
// The oracle's driver puts both next to upstream's answer. On its own:
//   bun referee.ts <cases.ndjson>
// prints the two for each `markdown` case.
//
// Needs its packages: `bun install` in this directory.
import { readFileSync } from "node:fs";
import { Parser } from "commonmark";
import { fromMarkdown } from "mdast-util-from-markdown";
import { gfmStrikethroughFromMarkdown } from "mdast-util-gfm-strikethrough";
import { gfmTableFromMarkdown } from "mdast-util-gfm-table";
import { gfmStrikethrough } from "micromark-extension-gfm-strikethrough";
import { gfmTable } from "micromark-extension-gfm-table";

type Held = { quote: true } | { quote: false; ordered: boolean; first: number };
export type Structure = [string, string | null, number | null, number | null];
export type Token = [string, number, number, number, number];

// The column after a line's container prefixes and indentation, as upstream
// and Focus both count it (`markdownContentStart`).
function contentStart(line: string, index: number, path: Held[]): number {
  let position = 0;
  for (const held of path) {
    const prefix = held.quote
      ? /^[ \t]{0,3}>[ \t]?/
      : index <= held.first
        ? /^[ \t]{0,3}(?:[-*+]|\d{1,9}[.)])(?:[ \t]+|$)/
        : /^[ \t]*/;
    position += prefix.exec(line.slice(position))?.[0].length ?? 0;
  }
  return position + /^[ \t]*/.exec(line.slice(position))![0].length;
}

// What both parsers are given: the document as upstream hands it to its
// renderer.
function prepared(content: string): string {
  return content
    .replace(/^\uFEFF/, "")
    .replace(/\r\n?/g, "\n")
    .replaceAll("\u0000", "\uFFFD")
    .replace(/[\u0001-\u0004]/g, "x");
}

// One record per line, filled as blocks claim their lines. A line no block
// claims is blank, or holds container markers only.
function claims(raw: string[]) {
  const lines: Structure[] = raw.map((line) => ["blank", null, /^[ \t]*/.exec(line)![0].length, null]);
  const claimed = raw.map(() => false);
  // `first` and `last` are line indexes, both inside the block.
  const claim = (first: number, last: number, kind: string, path: Held[]) => {
    const raws = kind === "codeFenced" || kind === "codeIndented" || kind === "htmlFlow";
    const structured = kind === "heading" || kind === "paragraph" || kind === "table" || kind === "htmlFlow";
    for (let index = first; index <= last && index < raw.length; index++) {
      // Blank as CommonMark has it: spaces and tabs. A form feed is text.
      if (claimed[index] || (!raws && /^[ \t]*$/.test(raw[index]))) continue;
      claimed[index] = true;
      lines[index] = [
        kind,
        structured ? path.map((held) => (held.quote ? ">" : held.ordered ? "o" : "u")).join(" ") : null,
        // A rule and a definition are reported with their plain indentation.
        raws && kind !== "htmlFlow" ? null : contentStart(raw[index], index, structured ? path : []),
        kind === "htmlFlow" ? first : null,
      ];
    }
  };
  return { lines, claim };
}

const fenced = (opening: string) => /^(?:`{3,}|~{3,})/.test(opening);

export function referee(content: string): { lines: Structure[]; tokens: Token[] } {
  const source = prepared(content);
  const raw = source.split("\n");
  const { lines, claim } = claims(raw);
  const tokens: Token[] = [];
  const tree = fromMarkdown(source, {
    extensions: [gfmTable(), gfmStrikethrough()],
    mdastExtensions: [gfmTableFromMarkdown(), gfmStrikethroughFromMarkdown()],
  });
  const block = (node: any, kind: string, path: Held[]) => {
    // A block cut short by the end of its container is reported as ending
    // at the start of the line after it.
    const first = node.position.start.line - 1;
    const cut = node.position.end.column === 1 && node.position.end.line - 1 > first;
    claim(first, node.position.end.line - (cut ? 2 : 1), kind, path);
  };
  const inline = (node: any) => {
    const { start, end } = node.position ?? {};
    if (node.type === "inlineCode") tokens.push(["codeText", start.line - 1, end.line - 1, start.column - 1, end.column - 1]);
    if (node.type === "html") {
      const kind = String(node.value).startsWith("<!--") ? "htmlComment" : "htmlText";
      tokens.push([kind, start.line - 1, end.line - 1, start.column - 1, end.column - 1]);
    }
    for (const kid of node.children ?? []) inline(kid);
  };
  const visit = (node: any, path: Held[]) => {
    switch (node.type) {
      case "paragraph":
      case "heading":
      case "table":
        block(node, node.type, path);
        for (const kid of node.children) inline(kid);
        return;
      case "thematicBreak":
      case "definition":
        block(node, node.type, path);
        return;
      case "html":
        block(node, "htmlFlow", path);
        return;
      case "code": {
        const opening = raw[node.position.start.line - 1].slice(node.position.start.column - 1);
        block(node, fenced(opening) ? "codeFenced" : "codeIndented", path);
        return;
      }
      case "blockquote":
        for (const kid of node.children) visit(kid, [...path, { quote: true }]);
        return;
      case "list":
        for (const item of node.children) {
          const held: Held = { quote: false, ordered: Boolean(node.ordered), first: item.position.start.line - 1 };
          for (const kid of item.children) visit(kid, [...path, held]);
        }
        return;
      default:
        for (const kid of node.children ?? []) visit(kid, path);
    }
  };
  visit(tree, []);
  return { lines, tokens };
}

export function second(content: string): Structure[] {
  const source = prepared(content);
  const raw = source.split("\n");
  const { lines, claim } = claims(raw);
  const kinds: Record<string, string> = { paragraph: "paragraph", heading: "heading", thematic_break: "thematicBreak", html_block: "htmlFlow" };
  const visit = (node: any, path: Held[]) => {
    const [[first], [last]] = node.sourcepos;
    if (node.type === "code_block") {
      const opening = raw[first - 1].slice(node.sourcepos[0][1] - 1);
      claim(first - 1, last - 1, fenced(opening) ? "codeFenced" : "codeIndented", path);
    } else if (kinds[node.type]) {
      claim(first - 1, last - 1, kinds[node.type], path);
    } else {
      const inside: Held[] =
        node.type === "block_quote"
          ? [...path, { quote: true }]
          : node.type === "item"
            ? [...path, { quote: false, ordered: node.listType === "ordered", first: first - 1 }]
            : path;
      for (let kid = node.firstChild; kid; kid = kid.next) visit(kid, inside);
    }
  };
  visit(new Parser().parse(source), []);
  return lines;
}

if (import.meta.main) {
  const out: string[] = [];
  for (const text of readFileSync(process.argv[2], "utf-8").split("\n")) {
    if (!text) continue;
    const c = JSON.parse(text);
    if (c.kind === "markdown") out.push(JSON.stringify({ id: c.id, ...referee(c.content), second: second(c.content) }));
  }
  process.stdout.write(out.join("\n") + (out.length > 0 ? "\n" : ""));
}
