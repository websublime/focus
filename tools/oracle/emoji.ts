// Is `rgi_emoji.txt` the runtime's own list of emoji?
//
//   bun emoji.ts
//
// Upstream reads a heading past a leading emoji with `\p{RGI_Emoji}`
// (`headingKey`, aidlc-artifact-vocabulary.ts), a property the runtime answers
// from its own tables, and a pattern cannot be asked to list what it matches.
// So the list is brought from the Unicode data, and checked here from both
// sides: the runtime takes every listed sequence for an emoji, and takes
// nothing else among the sequences an emoji could be mistaken for (every
// code point alone, with each variation selector, with a keycap, with each
// skin tone; every pair of regional indicators; every listed sequence short
// of one code point, or with one more). Exits 1, naming the first few, when
// either fails: the corpus would then be asking about the wrong list.
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";

const listed = new Set(
  readFileSync(join(dirname(import.meta.path), "rgi_emoji.txt"), "utf-8")
    .split("\n")
    .filter((line) => line !== "" && !line.startsWith("#"))
    .map((line) => String.fromCodePoint(...line.split(" ").map((point) => parseInt(point, 16)))),
);
const emoji = /^\p{RGI_Emoji}$/v;
const shown = (text: string) => [...text].map((c) => c.codePointAt(0)!.toString(16).toUpperCase()).join(" ");
const refused = [...listed].filter((sequence) => !emoji.test(sequence));
const unlisted: string[] = [];
let asked = 0;
const ask = (text: string) => {
  asked++;
  if (!listed.has(text) && emoji.test(text)) unlisted.push(text);
};
const tones = [0x1f3fb, 0x1f3fc, 0x1f3fd, 0x1f3fe, 0x1f3ff].map((tone) => String.fromCodePoint(tone));
for (let point = 0; point <= 0x10ffff; point++) {
  if (point >= 0xd800 && point <= 0xdfff) continue;
  const c = String.fromCodePoint(point);
  for (const more of ["", "️", "︎", "️⃣", "⃣", ...tones]) ask(c + more);
}
for (let a = 0x1f1e6; a <= 0x1f1ff; a++) for (let b = 0x1f1e6; b <= 0x1f1ff; b++) ask(String.fromCodePoint(a, b));
for (const sequence of listed) {
  const points = [...sequence];
  for (let cut = 1; cut < points.length; cut++) ask(points.slice(0, cut).join(""));
  for (let drop = 0; drop < points.length; drop++) ask(points.filter((_, at) => at !== drop).join(""));
  for (const more of ["️", "︎", "‍", ...tones]) ask(sequence + more);
}
console.log(`emoji: ${listed.size} listed, ${refused.length} refused; ${asked} asked, ${unlisted.length} taken and not listed`);
for (const text of [...refused.slice(0, 5), ...unlisted.slice(0, 5)]) console.log(`  ${shown(text)}`);
process.exit(refused.length || unlisted.length ? 1 : 0);
