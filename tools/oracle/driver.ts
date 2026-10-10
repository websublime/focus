// Oracle driver: answer cases with upstream's own code.
//
//   bun tools/oracle/driver.ts <upstream checkout> <cases.ndjson>  > expected.ndjson
//
// Each input line is one case; each output line is {"id", "out"} in the shape
// `focus dev oracle` prints for the same case (the `oracle` module of the crate
// that owns the kind). The two outputs are compared as text, so key order is
// part of the answer.
import { mkdirSync, mkdtempSync, readFileSync, rmSync, utimesSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, relative, resolve, sep } from "node:path";
import { referee, second } from "./referee/referee.ts";

const [upstream, casesPath] = process.argv.slice(2);
if (!upstream || !casesPath) {
  console.error("usage: bun driver.ts <upstream checkout> <cases.ndjson>");
  process.exit(2);
}
const tool = (name: string) => resolve(upstream, "core", "tools", name);
const lib = await import(tool("aidlc-lib.ts"));
const stageSchema = await import(tool("aidlc-stage-schema.ts"));
const sensorSchema = await import(tool("aidlc-sensor-schema.ts"));
const ruleSchema = await import(tool("aidlc-rule-schema.ts"));
const graph = await import(tool("aidlc-graph.ts"));
const audit = await import(tool("aidlc-audit.ts"));
const settings = await import(tool("aidlc-settings.ts"));

// A scratch project for the cases that can only be asked by writing a file.
// A case names it `{P}`; answers name it the same way, so they do not depend
// on where it was.
const scratch = mkdtempSync(join(tmpdir(), "focus-oracle-"));
process.on("exit", () => rmSync(scratch, { recursive: true, force: true }));
let shards = 0;
let trees = 0;

// Text the JSON parser wrote is the runtime's, not the engine's: both sides
// keep the engine's sentence and replace what follows it.
const message = (err: unknown) =>
  (err instanceof Error ? err.message : String(err)).replace(
    /(has an unparseable grid fence: ).*/s,
    "$1<runtime>",
  );

function answer(c: Record<string, any>): unknown {
  switch (c.kind) {
    case "stage": {
      let parsed: Record<string, unknown>;
      try {
        parsed = lib.parseStageFrontmatter(c.raw);
      } catch (err) {
        return { error: message(err) };
      }
      const result = stageSchema.validateStageFrontmatter(parsed, c.agents ? { agents: c.agents } : undefined);
      return { parsed, valid: result.valid, errors: result.valid ? [] : result.errors };
    }
    case "sensor": {
      let manifest: Record<string, unknown>;
      try {
        manifest = sensorSchema.parseSensorManifest(c.raw);
      } catch (err) {
        return { error: message(err) };
      }
      // The invocation contract's key set is read by the sensor dispatcher,
      // which focus has not ported; it takes no part in the compile.
      delete manifest.input_schema;
      let invalid: string | null = null;
      try {
        sensorSchema.validateSensorManifest(manifest, "FILE", c.filename_id);
      } catch (err) {
        invalid = message(err);
      }
      return { manifest, invalid };
    }
    case "rule": {
      const frontmatter = ruleSchema.parseRuleFrontmatter(c.raw);
      let invalid: string | null = null;
      try {
        ruleSchema.validateRuleFrontmatter(frontmatter, "FILE");
      } catch (err) {
        invalid = message(err);
      }
      return { frontmatter, invalid };
    }
    case "scalar":
      return { value: lib.scalarField(c.fm, c.key) };
    case "list":
      return { value: lib.listField(c.fm, c.key) };
    case "collate":
      return { sign: Math.sign(c.a.localeCompare(c.b)) };
    case "icase":
      // What each pattern matches in the text, case ignored, outside the
      // Unicode modes: the mode nearly all of upstream's patterns are in.
      return {
        found: (c.patterns as string[]).map((pattern) =>
          [...(c.hay as string).matchAll(new RegExp(pattern, "gi"))].map((match) => match[0]),
        ),
      };
    case "num": {
      const x = Number(c.text);
      return {
        string: String(x),
        fixed: Number.isFinite(x) ? x.toFixed(c.digits) : null,
        round: String(Math.round(x)),
        json: JSON.stringify({ x }),
      };
    }
    case "cycles": {
      let topo: unknown;
      try {
        topo = graph.topoSort(c.stages);
      } catch (err) {
        topo = { error: message(err) };
      }
      return { cycles: graph.findCycles(c.stages), topo };
    }
    case "record": {
      let parsed: { name: string; identity: string; stages: Record<string, string> };
      try {
        parsed = graph.parseComposedScopeRecord(c.body, "FILE");
      } catch (err) {
        return { error: message(err) };
      }
      let rendered: unknown;
      try {
        rendered = graph.renderComposedScopeRecord(parsed.identity, parsed.stages);
      } catch (err) {
        rendered = { error: message(err) };
      }
      return { name: parsed.name, identity: parsed.identity, stages: parsed.stages, rendered };
    }
    case "foldback": {
      const records: Record<string, unknown> = {};
      for (const [name, stages] of Object.entries(c.records)) records[name] = { name, identity: "", stages };
      const fold = graph.composedFoldBack(records, c.on_disk, new Set(c.stock), new Set(c.installed));
      return {
        json: fold.json,
        names: [...fold.names],
        recordNames: [...fold.recordNames],
        gridOnlyNames: [...fold.gridOnlyNames],
        merged: JSON.stringify(graph.mergeComposedScopes(c.fresh, fold.json, fold.names)),
      };
    }
    case "settings":
      return graph.validateScopeSettings(c.raw);
    case "guard":
    case "guardwild": {
      // The loop of memoryGuardPolicyDeclarations, on one file's text: the
      // current heading first, then the retired one.
      for (const heading of ["## Guard Policy", "## Change Control"]) {
        const body = lib.memorySectionBody(c.content, heading);
        if (!body.trim()) continue;
        const mode = lib.structuredField(body, "Mode");
        if (mode === null) continue;
        return { heading, mode };
      }
      return { heading: null, mode: null };
    }
    case "audit_entry": {
      // Rendering is not exported; appending to a shard of our own is the way
      // in. The first block's timestamp is the only thing that varies.
      const fields: Record<string, string> = {};
      for (const [key, value] of c.fields) fields[key] = value.replaceAll("{P}", scratch);
      const shard = join(scratch, `shard-${shards++}.md`);
      try {
        audit.appendAuditEntryAtPathUnlocked(c.eventType, fields, scratch, shard);
      } catch (err) {
        return { error: message(err).replaceAll(scratch, "{P}") };
      }
      const written = readFileSync(shard, "utf-8").replace(/^\*\*Timestamp\*\*: \S+$/m, "**Timestamp**: <ts>");
      return { shard: written.replaceAll(scratch, "{P}") };
    }
    case "selection": {
      // A project built from the case's files; answers name paths relative to it.
      const root = join(scratch, `tree-${trees++}`);
      mkdirSync(root, { recursive: true });
      for (const [rel, text] of Object.entries(c.files as Record<string, string>)) {
        mkdirSync(dirname(join(root, rel)), { recursive: true });
        writeFileSync(join(root, rel), text);
      }
      const selector = c.selector ?? {};
      const before = [process.env.AIDLC_SESSION_OVERRIDE, process.env.AIDLC_SESSION_OVERRIDE_SOURCE];
      if (c.override) {
        process.env.AIDLC_SESSION_OVERRIDE = c.override.id;
        if (c.override.payload) process.env.AIDLC_SESSION_OVERRIDE_SOURCE = "payload";
      }
      const inTree = (path: string | null) => (path === null ? null : relative(root, path).split(sep).join("/"));
      try {
        const chosen = lib.resolveWorkflowSelection(root, selector);
        const out: Record<string, unknown> = {
          space: chosen.space,
          intent: chosen.intent,
          sessionId: chosen.sessionId,
          binding: chosen.binding,
        };
        // The path helpers take no session, so they are asked only when the case names none.
        if (selector.sessionId === undefined) {
          out.recordDir = inTree(lib.recordDir(root, selector.intent, selector.space));
          out.relativeRecordDir = lib.relativeRecordDir(root, selector.intent, selector.space);
        }
        out.intents = lib.listIntentDirs(root, chosen.space);
        return out;
      } catch (err) {
        // A registry row the untyped read trips over fails with the runtime's words.
        return { error: err instanceof TypeError ? "<runtime>" : message(err).replaceAll(root, "{P}") };
      } finally {
        for (const [index, name] of ["AIDLC_SESSION_OVERRIDE", "AIDLC_SESSION_OVERRIDE_SOURCE"].entries()) {
          if (before[index] === undefined) delete process.env[name];
          else process.env[name] = before[index];
        }
        rmSync(root, { recursive: true, force: true });
      }
    }
    case "shard_read": {
      const skip = lib.copiedAuditBlocks(c.shards, () => c.own);
      const events: unknown[] = [];
      const notes: unknown[] = [];
      c.shards.forEach((shard: { shard: string; content: string }, index: number) => {
        for (const row of lib.parseAuditShardEvents(shard.content, shard.shard, index, skip[index])) {
          events.push({ event: row.event, timestamp: row.timestamp, pos: row.pos, shardIndex: row.shardIndex, block: row.block });
        }
        for (const note of lib.parseAuditShardNotes(shard.content, shard.shard, index, skip[index])) {
          notes.push({ heading: note.heading, text: note.text, timestamp: note.timestamp, pos: note.pos, shardIndex: note.shardIndex });
        }
      });
      return { skip: skip.map((set: Set<number>) => [...set].sort((a, b) => a - b)), events, notes };
    }
    case "ledger": {
      // A project read as a clone reads it. The machine's name and a minted
      // token are not the engine's to decide: `{H}` and `{T}` in the answer,
      // and `{H}` in a case's path or clone file is this machine's name.
      const machine: string = lib.auditShardHostSegment();
      const here = (text: string) => text.replaceAll("{H}", machine);
      for (const rel of Object.keys(c.files)) {
        if (rel.split("/").pop()!.startsWith(`${machine}-`)) {
          throw new Error(`${c.id}: a file is named after this machine (${machine}); the answer could not tell the two apart`);
        }
      }
      const root = join(scratch, `tree-${trees++}`);
      mkdirSync(root, { recursive: true });
      for (const [rel, text] of Object.entries(c.files as Record<string, string>)) {
        mkdirSync(dirname(join(root, here(rel))), { recursive: true });
        writeFileSync(join(root, here(rel)), here(text));
      }
      for (const [rel, seconds] of Object.entries((c.mtimes ?? {}) as Record<string, number>)) {
        utimesSync(join(root, here(rel)), seconds, seconds);
      }
      const selector = c.selector ?? {};
      const asked = JSON.stringify(c.files);
      const cloneId = () => {
        try {
          return readFileSync(join(root, "aidlc", ".aidlc-clone-id"), "utf-8");
        } catch {
          return null;
        }
      };
      try {
        let auditFile: string | undefined;
        if (c.mintFirst) auditFile = lib.auditFilePath(root, selector.intent, selector.space);
        const shards: string[] = lib.auditShards(root, selector.intent, selector.space);
        const unreadable: string[] = [];
        const events = lib.readAuditShardEvents(root, selector.intent, selector.space, unreadable);
        const all: string = lib.readAllAuditShards(root, selector.intent, selector.space);
        const readerLeft = cloneId();
        auditFile ??= lib.auditFilePath(root, selector.intent, selector.space) as string;
        // The shard this clone writes is `<host>-<token>.md`: a token no file
        // of the case holds was minted here.
        const own = auditFile.split(sep).pop()!;
        const token = own.slice(own.lastIndexOf("-") + 1, -".md".length);
        const minted = !asked.includes(token);
        const fileName = (name: string) => {
          let shown = name;
          if (minted && shown.endsWith(`-${token}.md`)) shown = `${shown.slice(0, -`${token}.md`.length)}{T}.md`;
          if (shown.startsWith(`${machine}-`)) shown = `{H}${shown.slice(machine.length)}`;
          return shown;
        };
        const inTree = (path: string) => {
          const parts = relative(root, path).split(sep);
          parts.push(fileName(parts.pop()!));
          return parts.join("/");
        };
        const idFile = (text: string | null) => {
          if (text === null) return null;
          const lines = text.split("\n");
          if (minted && lines[0] === token) lines[0] = "{T}";
          if (lines[1] === machine) lines[1] = "{H}";
          return lines.join("\n");
        };
        return {
          auditFile: inTree(auditFile),
          cloneId: idFile(cloneId()),
          readerLeft: idFile(readerLeft),
          shards: shards.map(inTree),
          unreadable: unreadable.map(inTree),
          events: events.map((row: any) => [inTree(row.shard), row.shardIndex, row.pos, row.event, row.timestamp]),
          all,
        };
      } catch (err) {
        return { error: message(err).replaceAll(root, "{P}") };
      } finally {
        rmSync(root, { recursive: true, force: true });
      }
    }
    case "settings_file":
    case "settings_layers": {
      // The machine's file lives in the install root, which each case gets
      // for itself. `{M}` and `{P}` in an answer are that root and the project.
      const root = join(scratch, `tree-${trees++}`);
      const project = join(root, "project");
      const machine = join(root, "machine");
      mkdirSync(project, { recursive: true });
      mkdirSync(machine, { recursive: true });
      const before = process.env.AIDLC_INSTALL_ROOT;
      process.env.AIDLC_INSTALL_ROOT = machine;
      settings._resetSettingsCacheForTests();
      const named = (text: string) =>
        text
          .replace(/(: invalid JSON: ).*/s, "$1<runtime>")
          .replaceAll(machine, "{M}")
          .replaceAll(project, "{P}");
      try {
        if (c.kind === "settings_file") {
          let parsed: unknown;
          try {
            parsed = JSON.parse(c.raw);
          } catch {
            return { error: "<json>" };
          }
          try {
            return { value: settings.normalizeAidlcSettings(parsed, c.layer, "{F}") };
          } catch (err) {
            return { error: named(message(err)) };
          }
        }
        for (const [rel, text] of Object.entries(c.files as Record<string, string>)) {
          mkdirSync(dirname(join(root, rel)), { recursive: true });
          writeFileSync(join(root, rel), text);
        }
        const out: Record<string, unknown> = {};
        try {
          const resolved = settings.resolveAidlcSettings(project);
          out.value = resolved.value;
          out.sources = resolved.sources;
          out.present = [resolved.files.machine.present, resolved.files.project.present, resolved.files.local.present];
        } catch (err) {
          out.error = named(message(err));
        }
        const flags: Record<string, unknown> = {};
        const switches: Record<string, string> = {};
        const recorded: Record<string, string | null> = {};
        for (const name of c.ask as string[]) {
          try {
            flags[name] = lib.resolveProjectFlag(name, c.env, project) ?? null;
          } catch {
            flags[name] = { error: true };
          }
          switches[name] = named(lib.killSwitchSource(name, c.env, project));
          recorded[name] = settings.bypassRecordedIn(project, name)?.target ?? null;
        }
        return { ...out, flags, switches, recorded, isSwitch: (c.probe as string[]).map((s) => lib.isKillSwitchSource(s)) };
      } finally {
        if (before === undefined) delete process.env.AIDLC_INSTALL_ROOT;
        else process.env.AIDLC_INSTALL_ROOT = before;
        rmSync(root, { recursive: true, force: true });
      }
    }
    case "markdown": {
      // One record for each line: what a consumer reads off the analysis and
      // off the three projections. A line the analysis did not place answers
      // null and is not compared (see below). Columns are UTF-16 code
      // units. An HTML block is known by the line it starts on, and a
      // container by its shape: `>`, `u` or `o`. The number an ordered list
      // starts at is left out: nothing upstream reads it, and its analysis
      // takes it from a render with probes in it, where a list may have
      // split and start elsewhere .
      const blocks = lib.markdownBlocks(c.content);
      const visible = lib.visibleMarkdownLines(c.content);
      const boundaries = lib.visibleMarkdownLines(c.content, { preserveCommentBoundaries: true });
      const indented = lib.visibleMarkdownLines(c.content, { preserveIndentedCode: true });
      const fenceLine = (text: string) =>
        /^(`{3,}|~{3,})/.test(text.replace(/^(?:[ \t]{0,3}(?:>[ \t]?|(?:[-*+]|\d{1,9}[.)])(?:[ \t]+|$)))*[ \t]*/, ""));
      const source = c.content.replace(/^\uFEFF/, "").replace(/\r\n?/g, "\n").split("\n");
      const lines = blocks.lines.map((line: any, index: number) => {
        // Not placed by the analysis: left `unknown`; called a fence only
        // because the line reads like one (no block behind it); or a fence
        // line whose probe rendered nowhere, which the analysis takes for a
        // link reference definition.
        if (line.kind === "unknown") return null;
        if (line.kind === "codeFenced" && line.block === -1) return null;
        if (line.kind === "definition" && fenceLine(source[index])) return null;
        const code = line.kind === "codeFenced" || line.kind === "codeIndented";
        const structured = ["heading", "paragraph", "table", "htmlFlow"].includes(line.kind);
        return [
          line.kind,
          line.htmlKind,
          code ? null : line.contentStart,
          structured
            ? line.containers
                .map((held: any) => (held.kind === "blockQuote" ? ">" : held.ordered ? "o" : "u"))
                .join(" ")
            : null,
          line.kind === "htmlFlow"
            ? blocks.lines.findIndex((other: any) => other.kind === "htmlFlow" && other.block === line.block)
            : null,
          line.invisible.map((span: any) => [span.start, span.end, span.kind, span.tokenStartLine, span.tokenEndLine]),
          // A projection that leaves the line as it is answers `true`, and
          // so does one that answers what the plain projection does.
          visible[index] === source[index] ? true : visible[index],
          boundaries[index] === visible[index] ? true : boundaries[index],
          indented[index] === visible[index] ? true : indented[index],
        ];
      });
      const third = referee(c.content);
      return {
        lines,
        definitions: blocks.definitions.map((d: any) => [d.label, d.startLine, d.endLine]),
        labels: blocks.labels,
        // A third reading, by a parser that reports source positions. It
        // is what a line is held to where upstream's analysis and Focus's
        // differ (see `markdown_rule.py`). Only the lines it reads otherwise
        // than upstream are listed, by index. `second` is the reference
        // implementation's reading, listed where it is not the referee's.
        referee: {
          lines: Object.fromEntries(
            third.lines.flatMap((said, index) => {
              const line = lines[index];
              const agreed = line !== null && JSON.stringify([line[0], line[3], line[2], line[4]]) === JSON.stringify(said);
              return agreed ? [] : [[String(index), said]];
            }),
          ),
          tokens: third.tokens,
          second: Object.fromEntries(
            second(c.content).flatMap((said, index) =>
              JSON.stringify(said) === JSON.stringify(third.lines[index]) ? [] : [[String(index), said]],
            ),
          ),
        },
      };
    }
    case "audit_find":
      return { found: lib.findAllEvents(c.audit, c.event, c.slug ?? undefined) };
    case "audit_field":
      return { value: lib.auditBlockField(c.block, c.name) };
    case "bullet_field":
      return { value: lib.getField(c.content, c.field) };
    case "unsafe_line":
      return { unsafe: lib.hasUnsafeSingleLineCharacter(c.value) };
    case "field_args":
      return { fields: lib.parseFieldArgs(c.args) };
    default:
      return { unsupported: c.kind };
  }
}

const out: string[] = [];
for (const line of readFileSync(casesPath, "utf-8").split("\n")) {
  if (line === "") continue;
  const c = JSON.parse(line);
  out.push(JSON.stringify({ id: c.id, out: answer(c) }));
}
process.stdout.write(out.join("\n") + "\n");
