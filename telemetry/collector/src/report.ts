import contract from "../../../contract/telemetry.json";
import { isoWeek } from "./week";

/** A report as gh-forgejo-shim and fga-server send it (docs/telemetry.md), once checked. */
export interface Report {
  schema: 1 | 2;
  version: string;
  os: string;
  arch: string;
  mode: "client" | "server";
  client?: string;
  forge?: string;
  forge_major?: number;
  gh?: string;
  first_week: string;
  counts: Count[];
}

/**
 * One name's count: how often the gateway lacked it (`unsupported`) or failed it (`failed`).
 * `client` is the agent whose gh call it came from, or "" when the report doesn't say: schema 1
 * puts every agent's counts together, and fga-server can't tell agents apart.
 */
export interface Count {
  kind: "unsupported" | "failed";
  client: string;
  name: string;
  count: number;
}

const FIELDS = [
  "schema", "version", "os", "arch", "mode", "client", "forge", "forge_major", "gh", "first_week",
  "unsupported",
];
/** Each schema's fields: 2 adds `failed`, and splits both counts by client in a client report. */
const SCHEMA_FIELDS = { 1: new Set(FIELDS), 2: new Set([...FIELDS, "failed"]) };
const VOCABULARY = new Set<string>(contract.unsupported);
/** The largest count one name may carry, as the client caps it. */
export const MAX_COUNT = 1_000_000;
/** The first week a report can name: reports began in 2026. */
const FIRST_WEEK = "2026-W01";

const isIn = (list: readonly string[], value: unknown): value is string =>
  typeof value === "string" && list.includes(value);

const isObject = (value: unknown): value is Record<string, unknown> =>
  typeof value === "object" && value !== null && !Array.isArray(value);

/** `names`' counts as `kind` from `client`, or why they're refused. */
function named(kind: Count["kind"], client: string, names: unknown): Count[] | string {
  if (!isObject(names)) return `bad ${kind}`;
  const counts: Count[] = [];
  for (const [name, count] of Object.entries(names)) {
    if (!VOCABULARY.has(name)) return `${kind} names a name outside the vocabulary`;
    if (!Number.isInteger(count) || (count as number) < 1 || (count as number) > MAX_COUNT) {
      return `bad ${kind} count`;
    }
    counts.push({ kind, client, name, count: count as number });
  }
  return counts;
}

/**
 * A report's counts of `kind`. Schema 1 and fga-server's reports hold names and counts; a schema 2
 * client report holds them under each client.
 */
function counted(kind: Count["kind"], report: Record<string, unknown>): Count[] | string {
  if (report.schema === 1 || report.mode === "server") return named(kind, "", report[kind]);
  const byClient = report[kind];
  if (!isObject(byClient)) return `bad ${kind}`;
  const counts: Count[] = [];
  for (const [client, names] of Object.entries(byClient)) {
    if (!isIn(contract.client, client)) return `${kind} names an unknown client`;
    const found = named(kind, client, names);
    if (typeof found === "string") return found;
    counts.push(...found);
  }
  return counts;
}

/**
 * The report in `body`, or why it's refused. Anything outside its schema is refused whole: an
 * unknown field, a value outside the contract's lists, or a name outside its vocabulary, so nothing
 * a sender makes up is ever counted.
 */
export function validate(report: unknown, today: string): Report | string {
  if (!isObject(report)) return "not a JSON object";
  if (report.schema !== 1 && report.schema !== 2) return "schema must be 1 or 2";
  const fields = SCHEMA_FIELDS[report.schema];
  for (const key of Object.keys(report)) {
    if (!fields.has(key)) return `unknown field ${JSON.stringify(key)}`;
  }
  // Numbers, and only a plain pre-release suffix, as the client sends it: a custom build's text
  // never reaches a counter.
  if (typeof report.version !== "string" || !/^\d{1,3}\.\d{1,3}\.\d{1,3}(-(alpha|beta|rc)\.\d{1,3})?$/.test(report.version)) {
    return "bad version";
  }
  if (!isIn(contract.os, report.os)) return "bad os";
  if (!isIn(contract.arch, report.arch)) return "bad arch";
  if (!isIn(contract.mode, report.mode)) return "bad mode";
  if (report.mode === "server") {
    if ("client" in report || "gh" in report) return "a server report has no client or gh";
  } else if (!isIn(contract.client, report.client)) {
    return "bad client";
  }
  if ("forge" in report && !isIn(contract.forge, report.forge)) return "bad forge";
  if ("forge_major" in report) {
    const major = report.forge_major;
    if (!("forge" in report) || !Number.isInteger(major) || (major as number) < 0 || (major as number) > 999) {
      return "bad forge_major";
    }
  }
  if ("gh" in report && (typeof report.gh !== "string" || !/^\d{1,3}\.\d{1,3}$/.test(report.gh))) {
    return "bad gh";
  }
  const week = report.first_week;
  if (typeof week !== "string" || !/^\d{4}-W(0[1-9]|[1-4]\d|5[0-3])$/.test(week) || week < FIRST_WEEK || week > isoWeek(today)) {
    return "bad first_week";
  }
  const counts: Count[] = [];
  for (const kind of report.schema === 1 ? (["unsupported"] as const) : (["unsupported", "failed"] as const)) {
    const found = counted(kind, report);
    if (typeof found === "string") return found;
    counts.push(...found);
  }
  return {
    schema: report.schema,
    version: report.version,
    os: report.os,
    arch: report.arch,
    mode: report.mode as Report["mode"],
    client: report.client as string | undefined,
    forge: report.forge as string | undefined,
    forge_major: report.forge_major as number | undefined,
    gh: report.gh as string | undefined,
    first_week: week,
    counts,
  };
}
