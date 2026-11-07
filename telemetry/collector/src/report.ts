import contract from "../../../contract/telemetry.json";
import { isoWeek } from "./week";

/** A report as gh-forgejo-shim and fga-server send it (docs/telemetry.md). */
export interface Report {
  schema: 1;
  version: string;
  os: string;
  arch: string;
  mode: "client" | "server";
  client?: string;
  forge?: string;
  forge_major?: number;
  gh?: string;
  first_week: string;
  unsupported: Record<string, number>;
}

const FIELDS = new Set([
  "schema", "version", "os", "arch", "mode", "client", "forge", "forge_major", "gh", "first_week",
  "unsupported",
]);
const VOCABULARY = new Set<string>(contract.unsupported);
/** The largest count one name may carry, as the client caps it. */
export const MAX_COUNT = 1_000_000;
/** The first week a report can name: reports began in 2026. */
const FIRST_WEEK = "2026-W01";

const isIn = (list: readonly string[], value: unknown): value is string =>
  typeof value === "string" && list.includes(value);

/**
 * The report in `body`, or why it's refused. Anything outside the schema is refused whole: an
 * unknown field, a value outside the contract's lists, or an unsupported name outside its
 * vocabulary, so nothing a sender makes up is ever counted.
 */
export function validate(body: unknown, today: string): Report | string {
  if (typeof body !== "object" || body === null || Array.isArray(body)) {
    return "not a JSON object";
  }
  const report = body as Record<string, unknown>;
  for (const key of Object.keys(report)) {
    if (!FIELDS.has(key)) return `unknown field ${JSON.stringify(key)}`;
  }
  if (report.schema !== contract.schema) return "schema must be 1";
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
  const unsupported = report.unsupported;
  if (typeof unsupported !== "object" || unsupported === null || Array.isArray(unsupported)) {
    return "bad unsupported";
  }
  for (const [name, count] of Object.entries(unsupported)) {
    if (!VOCABULARY.has(name)) return "unsupported names a name outside the vocabulary";
    if (!Number.isInteger(count) || (count as number) < 1 || (count as number) > MAX_COUNT) {
      return "bad unsupported count";
    }
  }
  return report as unknown as Report;
}
