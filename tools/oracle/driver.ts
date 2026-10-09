// Oracle driver: answer cases with upstream's own code.
//
//   bun tools/oracle/driver.ts <upstream checkout> <cases.ndjson>  > expected.ndjson
//
// Each input line is one case; each output line is {"id", "out"} in the shape
// `focus dev oracle` prints for the same case (the `oracle` module of the crate
// that owns the kind). The two outputs are compared as text, so key order is
// part of the answer.
import { mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";

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

// A scratch project for the cases that can only be asked by writing a file.
// A case names it `{P}`; answers name it the same way, so they do not depend
// on where it was.
const scratch = mkdtempSync(join(tmpdir(), "focus-oracle-"));
process.on("exit", () => rmSync(scratch, { recursive: true, force: true }));
let shards = 0;

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
