// Oracle driver: answer cases with upstream's own code.
//
//   bun tools/oracle/driver.ts <upstream checkout> <cases.ndjson>  > expected.ndjson
//
// Each input line is one case; each output line is {"id", "out"} in the shape
// `focus dev oracle` prints for the same case (the `oracle` module of the crate
// that owns the kind). The two outputs are compared as text, so key order is
// part of the answer.
import { chmodSync, linkSync, mkdirSync, mkdtempSync, readFileSync, rmSync, symlinkSync, utimesSync, writeFileSync } from "node:fs";
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
const freeze = await import(resolve(upstream, "core", "hooks", "review-freeze-command.ts"));
const integrity = await import(resolve(upstream, "core", "hooks", "runtime-integrity.ts"));
const reviewerScope = await import(resolve(upstream, "core", "hooks", "aidlc-reviewer-scope.ts"));
const stateGuard = await import(resolve(upstream, "core", "hooks", "aidlc-state-transition-guard.ts"));
const planGuard = await import(resolve(upstream, "core", "hooks", "aidlc-plan-approval-guard.ts"));
// The Plan Approval guard's readers of a command are private to the hook.
// They are asked through a copy of the hook's own source with an export list
// appended: the functions are upstream's, unmodified. The copy sits beside
// the original, where its relative imports resolve, for as long as this
// process runs.
const PLAN_GUARD_PRIVATE = [
  "everyHelperMayRun", "isPlanApprovalPrerequisite", "engineDirectedWhilePlanWaits", "intentRepoChangeAdmitted",
  "isReadOnlyDiagnostic", "isReadOnlyToolDiagnostic", "plainPowerShell", "powerShellReading", "shellUsesDynamicEvaluation",
  "mutationIntent", "codeGenerationGateHeld", "lastStepAdmitsPersonsMoves", "isOutsideProject", "isStageRecordOutput",
  "isAnswerTextTarget", "isStageDiaryTarget", "isComposerProposalTarget",
];
const planGuardShim = resolve(upstream, "core", "hooks", "aidlc-plan-approval-guard.focus-oracle.ts");
writeFileSync(
  planGuardShim,
  `${readFileSync(resolve(upstream, "core", "hooks", "aidlc-plan-approval-guard.ts"), "utf-8")}\nexport { ${PLAN_GUARD_PRIVATE.join(", ")} };\n`,
);
process.on("exit", () => rmSync(planGuardShim, { force: true }));
const planGuardPrivate = await import(planGuardShim);
// The source walk is private to the library. It is asked the same way: a
// copy of the library's own source with an export list appended.
const libShim = tool("aidlc-lib.focus-oracle.ts");
// (`rawFingerprintForm` and `lastWorkspaceSourceFailure` are asked of the copy too: what a walk remembers is the
// module's own.)
writeFileSync(libShim, `${readFileSync(tool("aidlc-lib.ts"), "utf-8")}\nexport { walkWorkspaceSource };\n`);
process.on("exit", () => rmSync(libShim, { force: true }));
const libPrivate = await import(libShim);
const dispatcher = await import(tool("aidlc.ts"));
const guardOperation = await import(tool("aidlc-guard-operation.ts"));
const posture = await import(tool("aidlc-testing-posture.ts"));
const vocabulary = await import(tool("aidlc-artifact-vocabulary.ts"));
const nodePath = await import("node:path");

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

function writtenListing(listing: Map<string, string>): unknown {
  try {
    return { text: lib.serializeSourceListing(listing) };
  } catch (err) {
    return { error: (err as Error).message };
  }
}

async function answer(c: Record<string, any>): Promise<unknown> {
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
    case "instant":
      // NaN is `null` in JSON, which is also what focus answers for a text it
      // does not read: the cases hold no text the two would read differently.
      return { ms: (c.texts as string[]).map((text) => Date.parse(text)) };
    case "memory_counts": {
      const counted = lib.parseMemoryHeadings(c.raw);
      return {
        interpretations: counted.interpretations,
        deviations: counted.deviations,
        tradeoffs: counted.tradeoffs,
        open_questions: counted.open_questions,
        total: counted.total,
      };
    }
    case "bolt_dag":
      return lib.parseBoltDag(c.body);
    case "source_listing_parse": {
      const sha = lib.sourceListingSha256(c.text);
      let parsed;
      try {
        parsed = lib.parseSourceListing(c.text);
      } catch (err) {
        return { error: (err as Error).message, sha };
      }
      if (parsed === null) return { listing: null, sha };
      return { listing: writtenListing(parsed), size: parsed.size, sha };
    }
    case "source_listing": {
      const before = new Map<string, string>(c.before);
      const after = new Map<string, string>(c.after);
      return {
        before: writtenListing(before),
        after: writtenListing(after),
        changed: lib.sourceListingChangedPaths(before, after),
        keys: lib.renderSourcePathKeys([...before.keys(), ...after.keys()]),
      };
    }
    case "source_walk": {
      // A project under a scratch root: files (null a directory), files
      // given as hex, executables, links and named pipes. The walk is asked
      // for the project as one repository or as a roof over the named ones.
      const root = `/tmp/oracle-${c.id}`;
      rmSync(root, { recursive: true, force: true });
      mkdirSync(root, { recursive: true });
      // A path of the case names the registry file as `{R}`, and so do the
      // answers: the file's own row is then the same row under either name.
      const registry = ".aidlc-source-paths.json";
      const written = (rel: string) => rel.split("{R}").join(registry);
      const shown = (text: string) => text.split(registry).join("{R}");
      const place = (rel: string) => {
        const path = join(root, written(rel));
        mkdirSync(dirname(path), { recursive: true });
        return path;
      };
      for (const [rel, text] of Object.entries(c.files as Record<string, string | null>)) {
        if (text === null) mkdirSync(join(root, written(rel)), { recursive: true });
        else {
          writeFileSync(place(rel), text);
          chmodSync(join(root, written(rel)), 0o644);
        }
      }
      for (const [rel, hex] of Object.entries(c.hex as Record<string, string>)) {
        writeFileSync(place(rel), Buffer.from(hex, "hex"));
        chmodSync(join(root, written(rel)), 0o644);
      }
      for (const rel of c.exec as string[]) chmodSync(join(root, written(rel)), 0o755);
      for (const [rel, target] of c.links as Array<[string, string]>) symlinkSync(target, place(rel));
      for (const rel of c.fifos as string[]) Bun.spawnSync(["mkfifo", "-m", "644", place(rel)]);
      const set = Object.entries(c.env as Record<string, string>);
      for (const [name, value] of set) process.env[name] = value;
      try {
        const walked = libPrivate.walkWorkspaceSource(join(root, c.project), c.repos, c.dotnet);
        if (walked === null) {
          const failure = libPrivate.lastWorkspaceSourceFailure();
          return {
            failure: failure === null ? null : {
              code: failure.code,
              // What the system said is the host's, not the walk's.
              detail: shown(failure.detail.split(": ")[0]),
              path: failure.path === undefined ? null : shown(failure.path),
              repo: failure.repo ?? null,
            },
          };
        }
        // A fingerprint is taken over the paths: one that holds the engine's
        // name differs between the engines by construction. The earlier
        // walk's also takes in the files left out by name since, and the
        // engine's settings files are among them.
        const named = [...walked.state.listing.keys()].some((key: string) => /aidlc/i.test(shown(key)));
        const legacyNamed = named ||
          [...Object.keys(c.files), ...Object.keys(c.hex)].some((rel) => /aidlc/i.test(rel.slice(rel.lastIndexOf("/") + 1)));
        // The rows in an order that does not depend on the engine's name
        // (the order a snapshot is written in is the listing cases').
        const neutral = (row: string) => row.replace(/aidlc/gi, "\u0001");
        const rows = shown(libPrivate.serializeSourceListing(walked.state.listing) as string).split("\n").filter((row) => row !== "")
          .sort((a, b) => (neutral(a) < neutral(b) ? -1 : neutral(a) > neutral(b) ? 1 : 0));
        return {
          fingerprint: named ? "<named>" : walked.state.fingerprint,
          legacy: walked.legacy === null ? null : legacyNamed ? "<named>" : walked.legacy,
          // The fingerprint as it was before line endings were read as LF,
          // when this walk read a file whose raw bytes differ from its text.
          raw: ((raw: string | null) => (raw === null ? null : named ? "<named>" : raw))(
            libPrivate.rawFingerprintForm(walked.state.fingerprint),
          ),
          dotnet: walked.dotnetOutputSeen,
          rows,
        };
      } finally {
        for (const [name] of set) delete process.env[name];
        rmSync(root, { recursive: true, force: true });
      }
    }
    case "progress":
      return {
        checkboxes: lib.parseCheckboxes(c.content),
        suffixes: [...lib.parseStateStageSuffixes(c.content)],
      };
    case "guard_policy": {
      // A project with the case's memory files and scope defaults; the state
      // text is handed in. Every fence is resolved and decided, with the
      // environment the case sets and no markers in the record.
      const root = join(scratch, `tree-${trees++}`);
      const memoryDir = join(root, "aidlc", "spaces", "default", "memory");
      mkdirSync(memoryDir, { recursive: true });
      for (const declared of c.memory as { layer: string; heading: string; value: string }[]) {
        writeFileSync(join(memoryDir, `${declared.layer}.md`), `# ${declared.layer}\n\n${declared.heading}\n\nMode: ${declared.value}\n`);
      }
      const mappingPath = join(root, "scope-mapping.json");
      const mapping: Record<string, { guardPolicy?: string }> = {};
      for (const [name, policy] of Object.entries(c.scopes as Record<string, string | null>)) {
        mapping[name] = policy === null ? {} : { guardPolicy: policy };
      }
      writeFileSync(mappingPath, JSON.stringify(mapping));
      const before: Record<string, string | undefined> = {};
      const names = ["AIDLC_SCOPE_MAPPING", "AIDLC_DISABLE_PLAN_APPROVAL_GUARD", "AIDLC_DISABLE_REVIEW_FREEZE_HOOK",
        "AIDLC_DISABLE_REVIEWER_SCOPE_HOOK", "AIDLC_SKIP_HUMAN_PRESENCE_GUARD", "AIDLC_UNATTENDED"];
      for (const name of names) {
        before[name] = process.env[name];
        delete process.env[name];
      }
      process.env.AIDLC_SCOPE_MAPPING = mappingPath;
      for (const name of c.env as string[]) process.env[name] = "1";
      lib._resetScopeMappingForTests();
      const named = (text: string) => text.replaceAll(root, "{P}");
      try {
        let policy: any;
        try {
          policy = lib.resolveGuardPolicy(root, c.state, { tolerateInvalidState: c.tolerate });
        } catch (err) {
          return { error: named(message(err)) };
        }
        if (policy.memoryStrict) policy.memoryStrict = { ...policy.memoryStrict, path: named(policy.memoryStrict.path) };
        const fences = lib.resolveFences(policy, c.state);
        const decisions = [];
        for (const fence of lib.GUARD_FENCES) {
          const decided = lib.decideFence(root, fence, { stateContent: c.state });
          decisions.push({
            decision: decided.decision,
            authority: { covered: decided.authority.covered, actor: decided.authority.actor, unattended: decided.authority.unattended },
            policy: decided.policy,
            fenceSetting: decided.fenceSetting,
            source: decided.source,
            speaks: lib.guardStandAsideSpeaks(decided),
            line: lib.guardStoodAsideLine(fence, decided.source, "aidlc-state set", true),
          });
        }
        return {
          policy,
          accepts: lib.guardPolicyAcceptsChanges(root, c.state),
          fences: lib.GUARD_FENCES.map((fence: string) => ({
            fence, value: fences[fence].value, source: fences[fence].source,
            label: lib.fenceSourceLabel(fences[fence]), formatted: lib.formatFence(fences[fence]),
          })),
          decisions,
        };
      } finally {
        for (const name of names) {
          if (before[name] === undefined) delete process.env[name];
          else process.env[name] = before[name];
        }
        lib._resetScopeMappingForTests();
        rmSync(root, { recursive: true, force: true });
      }
    }
    case "guards_line": {
      const fences = (c.fences as string[]).map((raw) => lib.parseGuardFence(raw)).filter((fence: string | null) => fence !== null);
      const parsed = lib.parseGuardPolicyStateLine(c.line);
      return {
        parsedGuards: lib.parseGuardsOffLine(c.line),
        parsedPolicy: parsed,
        formattedGuards: lib.formatGuardsOffLine(fences),
        formattedPolicy: parsed ? lib.formatGuardPolicy(parsed.value, parsed.source) : null,
        label: lib.changeControlSourceLabel(c.line),
        guardsOff: lib.setGuardsOffLine(c.content, fences),
        guardsOn: lib.setGuardsOnLine(c.content, fences),
        policyLine: lib.setGuardPolicyLine(c.content, c.line),
      };
    }
    case "heading_key": {
      // A heading given whole answers with its key. One given as the code
      // points of what stands before `after` answers 1 when all of that was
      // taken for decoration, 0 when none was, and with the key otherwise.
      if (c.headings) return { keys: c.headings.map((heading: string) => vocabulary.headingKey(heading)) };
      return {
        stripped: c.emoji.map((points: string) => {
          const whole = String.fromCodePoint(...points.split(" ").map((point) => parseInt(point, 16))) + c.after;
          const key = vocabulary.headingKey(whole);
          return key === c.after.trim() ? 1 : key === whole.trim() ? 0 : key;
        }),
      };
    }
    case "summary": {
      const out: Record<string, unknown> = { answer: lib.summaryConfirmationAnswer(c.content) };
      try {
        out.hash = lib.summaryConfirmationContentHash(c.content);
      } catch (err) {
        out.error = (err as Error).message;
      }
      return out;
    }
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
    case "review_window": {
      // The review attempt of a stage, read off a ledger given as its shards:
      // the receipt rows in order, the attempt's boundary among them, and the
      // units whose bolt is merged or still open.
      const rows = (c.shards as { shard: string; content: string }[]).flatMap((shard, index) =>
        lib.parseAuditShardEvents(shard.content, shard.shard, index, new Set<number>()));
      const stage = c.per_unit ? { slug: c.slug, for_each: "unit-of-work" } : { slug: c.slug };
      // The plan and the order of the stages are read from the compiled data
      // of the distribution, which the library's own folder does not have.
      const data = resolve(upstream, "dist", "claude", ".claude", "tools", "data");
      const before = [process.env.AIDLC_STAGE_GRAPH, process.env.AIDLC_SCOPE_GRID];
      process.env.AIDLC_STAGE_GRAPH = join(data, "stage-graph.json");
      process.env.AIDLC_SCOPE_GRID = join(data, "scope-grid.json");
      let view;
      try {
        view = lib.reviewAttemptWindow(scratch, c.state, stage, rows);
      } finally {
        for (const [index, name] of ["AIDLC_STAGE_GRAPH", "AIDLC_SCOPE_GRID"].entries()) {
          if (before[index] === undefined) delete process.env[name];
          else process.env[name] = before[index];
        }
      }
      return {
        events: view.events.map((row: { shardIndex: number; pos: number }) => `${row.shardIndex}:${row.pos}`),
        floor: view.floorIdx,
        merged: [...view.mergedBoltUnits],
        open: [...view.openBoltUnits],
      };
    }
    case "review_binding": {
      // What a review request's row binds, whether a completion can be held
      // to it, and whether the row replaces the request pending at its pass.
      const binding = lib.reviewRequestBindingFromBlock(c.block);
      const pending = c.pending === null
        ? undefined
        : { binding: c.pending.block === null ? null : lib.reviewRequestBindingFromBlock(c.pending.block) };
      return {
        binding,
        modern: binding === null ? null : lib.reviewRequestBindingIsModern(binding, { workspace_requires: c.workspace_requires }),
        replaces: lib.reviewRequestReplaces(c.block, pending),
      };
    }
    case "audit_field":
      return { value: lib.auditBlockField(c.block, c.name) };
    case "bullet_field":
      return { value: lib.getField(c.content, c.field) };
    case "unsafe_line":
      return { unsafe: lib.hasUnsafeSingleLineCharacter(c.value) };
    case "field_args":
      return { fields: lib.parseFieldArgs(c.args) };
    case "engine_command":
      return {
        classify: lib.classifyRuntimeCompileCommand(c.command),
        literal: lib.parseLiteralShellInvocation(c.command),
        verb: lib.literalOrchestrateVerb(c.command),
      };
    case "shell_invocations":
      return {
        details: freeze.shellCommandInvocationDetails(c.command),
        invocations: freeze.shellCommandInvocations(c.command),
        altersResolution: freeze.shellCommandAltersExecutableResolution(c.command),
      };
    case "shell_targets":
    case "write_targets": {
      // `{P}` is a scratch root spelled the same on both sides: two levels
      // under `/`, so a `..` that climbs out of it lands in the same place,
      // and named after the case, so a `cp` into its parent names the same
      // child.
      const root = `/tmp/focus-oracle-shell-${c.id}`;
      rmSync(root, { recursive: true, force: true });
      const cwd = c.cwd.replace("{P}", root);
      mkdirSync(cwd, { recursive: true });
      for (const dir of c.dirs ?? []) mkdirSync(join(cwd, dir), { recursive: true });
      process.env.HOME = c.home;
      const say = (path: string) => path.split(root).join("{P}");
      try {
        if (c.kind === "write_targets") {
          return { targets: freeze.writeTargets(c.tool, c.input ?? undefined, cwd, c.shell).map(say) };
        }
        const raw: string[] = [];
        const targets = freeze.shellWriteTargets(c.command, cwd, raw, c.shell).map(say);
        return {
          targets,
          raw: raw.map(say),
          roots: freeze.shellDirectoryRoots(c.command, cwd, c.shell).map(say),
          changes: freeze.shellDirectoryChanges(c.command),
        };
      } finally {
        rmSync(root, { recursive: true, force: true });
      }
    }
    case "reviewer_scope": {
      // The matcher is pure over its arguments; the record root and the cwd
      // are absolute, so `path.resolve` never reads this process's cwd.
      const verdict = reviewerScope.evaluateReviewerScope(
        c.tool,
        c.input ?? undefined,
        { unit: c.unit, exempt: c.exempt },
        { recordRoot: c.context.record_root, cwd: c.context.cwd },
      );
      return {
        verdict,
        reason: verdict.block
          ? reviewerScope.blockReason(verdict.target ?? "", { reviewer: "r", stage: "s", unit: c.unit, exempt: c.exempt }, verdict.defaulted)
          : null,
      };
    }
    case "reviewer_dispatch":
      return { record: reviewerScope.parseDispatchRecord(c.raw) };
    case "host_envelope":
      return lib.hostEnvelopeTurnText(c.prompt);
    case "host_template":
      return lib.hostTemplateOrigin(c.prompt, lib.CLAUDE_HOST_TEMPLATES);
    case "supplied_origin":
      return lib.suppliedTurnOrigin(c.origin);
    case "picker_empty":
      return lib.emptyPickerResult(c.response);
    case "transcript_origin": {
      // The transcript the case describes, under this process's scratch root; `{P}` names it.
      const root = mkdtempSync(join(scratch, "transcript-"));
      try {
        const path = join(root, c.file);
        mkdirSync(dirname(path), { recursive: true });
        if (typeof c.rows === "string") writeFileSync(path, c.rows);
        const transcript = typeof c.transcript_path === "string" ? c.transcript_path.replace("{P}", root) : c.transcript_path;
        return lib.claudeTranscriptOrigin(transcript, c.prompt_id, c.prompt);
      } finally {
        rmSync(root, { recursive: true, force: true });
      }
    }
    case "typed_switch":
      return lib.parseTypedGuardSwitchRequest(c.prompt, c.words_answer ? { wordsAnswer: true } : {});
    case "kiro_args":
      return lib.splitKiroCommandArgs(c.raw);
    case "state_transition":
      // The three readers of a shell command the state-transition guard
      // decides from; all pure.
      return {
        direct: stateGuard.directStateTransition(c.command),
        boundary: stateGuard.isLifecycleBoundaryCommand(c.command),
        delegated: stateGuard.delegatedLifecycleCommand(c.command),
      };
    case "runtime_integrity": {
      // A project tree under a scratch root spelled as focus's replay spells
      // its own, laid out from the case: files (`@bytes:N` is a refusable
      // script of N bytes, null a directory), links and executable bits.
      const root = `/tmp/focus-oracle-integrity-${c.id}`;
      rmSync(root, { recursive: true, force: true });
      mkdirSync(root, { recursive: true });
      for (const [rel, text] of Object.entries(c.files as Record<string, string | null>)) {
        const path = join(root, rel);
        if (text === null || rel.endsWith("/")) {
          mkdirSync(path, { recursive: true });
          continue;
        }
        mkdirSync(dirname(path), { recursive: true });
        // A file too large to read: a hook require, then a comment padded
        // to the size, so that reading it would refuse.
        const sized = /^@bytes:(\d+)$/.exec(text);
        const head = 'require("hooks/aidlc-x")\n//';
        writeFileSync(path, sized ? head + "x".repeat(Number(sized[1]) - head.length) : text);
      }
      for (const [rel, target] of c.links as Array<[string, string]>) {
        mkdirSync(dirname(join(root, rel)), { recursive: true });
        symlinkSync(target, join(root, rel));
      }
      for (const rel of c.exec as string[]) {
        try { chmodSync(join(root, rel), 0o755); } catch { /* absent */ }
      }
      const saved = { ...process.env };
      delete process.env.AIDLC_HARNESS_DIR;
      delete process.env.AIDLC_RUNTIME_PROJECT_DIR;
      process.env.AIDLC_PROJECT_DIR = join(root, c.project);
      for (const [name, value] of Object.entries(c.env as Record<string, string>)) process.env[name] = value;
      const input = { ...c.input, cwd: String(c.input.cwd).replace("{P}", root) };
      const say = (text: string) => text.split(root).join("{P}");
      let stderr = "";
      const write = process.stderr.write;
      process.stderr.write = ((chunk: string | Uint8Array) => { stderr += String(chunk); return true; }) as typeof process.stderr.write;
      try {
        const kind = integrity.runtimeIntegrityViolationKind(input);
        const refused = integrity.refuseRuntimeIntegrityViolation(input);
        return { kind, refused, stderr: say(stderr) };
      } finally {
        process.stderr.write = write;
        for (const name of Object.keys(process.env)) if (!(name in saved)) delete process.env[name];
        Object.assign(process.env, saved);
        rmSync(root, { recursive: true, force: true });
      }
    }
    case "node_path": {
      const flavour = c.flavour === "win32" ? nodePath.win32 : nodePath.posix;
      const args: string[] = c.args;
      switch (c.op) {
        case "resolve":
          // The working directory as the first part stands for process.cwd().
          return { value: flavour.resolve(c.cwd, ...args) };
        case "join":
          return { value: flavour.join(...args) };
        case "relative":
          // Both sides resolve against process.cwd(): the case's cwd is
          // prefixed to a relative part so the answer does not depend on it.
          return { value: flavour.relative(...(args.slice(0, 2).map((arg) => flavour.resolve(c.cwd, arg)) as [string, string])) };
        default:
          return { value: flavour[c.op](args[0] ?? "") };
      }
    }
    case "guard_operation": {
      // The recovery operations: each reader by name. Rendering is asked
      // with its mode, harness directory and shell explicit, so the answer
      // does not depend on where upstream runs from.
      const operation = c.operation;
      switch (c.op) {
        case "parse":
          return guardOperation.isGuardRecoveryOperation(operation);
        case "invocation":
          return guardOperation.isGuardRecoveryOperation(operation) ? guardOperation.guardOperationInvocation(operation) : null;
        case "render":
          if (!guardOperation.isGuardRecoveryOperation(operation)) return null;
          try {
            return guardOperation.renderGuardOperation(operation, { mode: c.mode, harnessDir: c.harness, shell: c.shell });
          } catch (err) {
            return { error: message(err) };
          }
        case "matches_command":
          return guardOperation.guardOperationMatchesCommand(operation, c.command);
        case "matches_remedy":
          return guardOperation.guardOperationMatchesRemedy(operation, c.remedy, c.stage, c.unit ?? undefined);
        case "same":
          return guardOperation.sameGuardOperation(c.left, c.right);
        case "continuation":
          return guardOperation.parseGuardRestartContinuationCommand(c.command, c.harness == null ? {} : { harnessDir: c.harness });
        case "engine_invocation":
          return guardOperation.isGuardRecoveryEngineInvocation(c.args);
        case "matches_engine_args":
          return guardOperation.guardOperationMatchesEngineArgs(operation, c.args);
        case "wording": {
          const wording = guardOperation.GUARD_REMEDY_WORDING[c.remedy];
          if (!wording) return null;
          return wording({
            code: c.code, stage: c.stage, target: c.target, unit: c.unit ?? undefined, fence: c.fence ?? undefined,
            sourceUnbindable: c.source_unbindable ?? false, everyUnit: c.every_unit ?? false,
          });
        }
        default:
          return { unsupported: c.op };
      }
    }
    case "plan_dispatch":
      return planGuard.evaluatePlanApprovalDispatch(c.tool, c.subagent, c.prompt, { currentStage: c.current_stage, units: c.units });
    case "plan_reason": {
      // A refusal names the engine's commands in the install's spelling:
      // from the source tree, `bun <harness>/tools/aidlc-<tool>.ts`. Focus
      // has one spelling, `focus engine <tool>`; upstream's is translated to
      // its native form here, as names.py translates its name, so the
      // sentences compare.
      const native = (text: string) => text.replace(/bun \.[A-Za-z0-9_.-]+\/tools\/aidlc-([a-z-]+)\.ts/g, "aidlc engine $1");
      switch (c.op) {
        case "appendix":
          return native(planGuard.appendixBlockReason(c.mentioned));
        case "block":
          return native(planGuard.blockReason(c.mentioned, c.detail ?? null, c.targets ?? null));
        case "handoff":
          return native(planGuard.handoffBlockReason(c.mentioned, c.cause, c.targets ?? null));
        case "mutation":
          return native(planGuard.mutationBlockReason(c.unit ?? null, c.opaque ?? false, c.detail ?? null));
        case "receipt":
          return planGuard.receiptDetail(c.evidence, c.mentioned);
        default:
          return { unsupported: c.op };
      }
    }
    case "contract_markers":
      return posture.promptTestingContractMarkers(c.text);
    case "questions_file":
      return {
        approved: posture.questionsFileApproved(c.body),
        pending: posture.questionsFileHasPendingPlanApproval(c.body),
        fingerprint: posture.questionsFileApprovalFingerprint(c.body),
        planned_source: posture.questionsFilePlannedSource(c.body),
      };
    case "review_appendix":
      return posture.planReviewAppendix(c.plan);
    case "engine_route": {
      // The dispatcher pins its own project ahead of a literal tail; it reads
      // that project from its environment, set here so the answer does not
      // depend on where this runs.
      const before = process.env.AIDLC_PROJECT_DIR;
      process.env.AIDLC_PROJECT_DIR = "/oracle/project";
      try {
        const action = dispatcher.resolveAction([...c.args], false);
        return action.type === "delegate" ? { tool: action.tool, args: action.args } : null;
      } finally {
        if (before === undefined) delete process.env.AIDLC_PROJECT_DIR;
        else process.env.AIDLC_PROJECT_DIR = before;
      }
    }
    case "every_helper": {
      const before = process.env.AIDLC_PROJECT_DIR;
      process.env.AIDLC_PROJECT_DIR = "/oracle/project";
      try {
        return planGuardPrivate.everyHelperMayRun(c.args);
      } finally {
        if (before === undefined) delete process.env.AIDLC_PROJECT_DIR;
        else process.env.AIDLC_PROJECT_DIR = before;
      }
    }
    case "plan_prerequisite":
      return planGuardPrivate.isPlanApprovalPrerequisite(c.args, c.gate_held ?? false, () => c.spoke ?? false);
    case "engine_directed":
      return {
        directed: planGuardPrivate.engineDirectedWhilePlanWaits(c.args, () => c.asked ?? false),
        repo: planGuardPrivate.intentRepoChangeAdmitted(c.args, () => c.asked ?? false),
      };
    case "read_only_diagnostic":
      return {
        diagnostic: planGuardPrivate.isReadOnlyDiagnostic(c.args),
        tool: planGuardPrivate.isReadOnlyToolDiagnostic(c.stem, c.args),
      };
    case "plain_powershell":
      return {
        plain: planGuardPrivate.plainPowerShell(c.command),
        reading: planGuardPrivate.powerShellReading(c.command, freeze.shellCommandInvocationDetails),
      };
    case "dynamic_evaluation":
      return planGuardPrivate.shellUsesDynamicEvaluation(c.command);
    case "mutation_intent":
    case "mutation_target": {
      // A project under a scratch root spelled as focus's replay spells its
      // own: files (null a directory), links, and second names for a file.
      // The hook's process is in the root, its project and harness are the
      // case's, and nothing else of this process's environment reaches it.
      // Neither engine's name is in the root: a command that may run the
      // engine is told by that name anywhere on its line.
      const root = `/tmp/oracle-${c.id}`;
      rmSync(root, { recursive: true, force: true });
      mkdirSync(root, { recursive: true });
      for (const [rel, text] of Object.entries(c.files as Record<string, string | null>)) {
        const path = join(root, rel);
        if (text === null) {
          mkdirSync(path, { recursive: true });
          continue;
        }
        mkdirSync(dirname(path), { recursive: true });
        writeFileSync(path, text);
      }
      for (const [rel, target] of c.links as Array<[string, string]>) {
        mkdirSync(dirname(join(root, rel)), { recursive: true });
        symlinkSync(target, join(root, rel));
      }
      for (const [rel, existing] of c.names as Array<[string, string]>) {
        mkdirSync(dirname(join(root, rel)), { recursive: true });
        linkSync(join(root, existing), join(root, rel));
      }
      const project = join(root, c.project);
      const saved = { ...process.env };
      for (const name of Object.keys(process.env)) {
        if (name.startsWith("AIDLC_") || name === "CLAUDE_PROJECT_DIR") delete process.env[name];
      }
      process.env.AIDLC_PROJECT_DIR = project;
      process.env.AIDLC_HARNESS_DIR = c.harness;
      if (c.unattended) process.env.AIDLC_UNATTENDED = "1";
      process.env.HOME = "/home/me";
      const here = process.cwd();
      process.chdir(root);
      const at = (text: string) => text.split("{P}").join(root);
      const say = (value: unknown) => JSON.parse(JSON.stringify(value).split(root).join("{P}"));
      try {
        if (c.kind === "mutation_target") {
          return (c.targets as string[]).map((written) => {
            const target = at(written);
            return {
              outside: planGuardPrivate.isOutsideProject(project, target),
              output: planGuardPrivate.isStageRecordOutput(project, target),
              answer: planGuardPrivate.isAnswerTextTarget(project, target),
              diary: planGuardPrivate.isStageDiaryTarget(project, target),
              composer: planGuardPrivate.isComposerProposalTarget(project, target),
            };
          });
        }
        const input = c.input === null ? undefined : JSON.parse(at(JSON.stringify(c.input)));
        const intent = await planGuardPrivate.mutationIntent(project, c.tool, input, at(c.cwd), c.state, null, c.hint);
        // A swarm's units are read against the active directive (M5).
        delete intent.swarmUnits;
        return say({
          intent,
          gateHeld: planGuardPrivate.codeGenerationGateHeld(c.state),
          // What upstream reads of the person in this tree: the case says
          // the same, and focus's replay is handed what the case says.
          person: {
            spoke: lib.personSpokeSinceGate(project, { requests: true }),
            asked: lib.personAskedSinceGate(project),
            lastStep: planGuardPrivate.lastStepAdmitsPersonsMoves(project),
          },
        });
      } finally {
        process.chdir(here);
        for (const name of Object.keys(process.env)) if (!(name in saved)) delete process.env[name];
        Object.assign(process.env, saved);
        rmSync(root, { recursive: true, force: true });
      }
    }
    default:
      return { unsupported: c.kind };
  }
}

const out: string[] = [];
for (const line of readFileSync(casesPath, "utf-8").split("\n")) {
  if (line === "") continue;
  const c = JSON.parse(line);
  out.push(JSON.stringify({ id: c.id, out: await answer(c) }));
}
process.stdout.write(out.join("\n") + "\n");
