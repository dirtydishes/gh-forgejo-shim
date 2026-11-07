/**
 * gh-forgejo-shim's usage-report collector (docs/telemetry.md).
 *
 * `POST /v1/report` checks a report against the contract and adds one to that day's counter for
 * its values. `GET /v1/stats` publishes the totals, leaving out any count below 3. The Worker reads
 * nothing from a request but its method, path and body, keeps no address, header, user agent or
 * body, and logs nothing.
 */
import { validate } from "./report";
import { DAYS, WEEKS, stats, type Row, type UnsupportedRow } from "./stats";
import { daysBefore, utcDay } from "./week";

export interface Env {
  DB: D1Database;
}

/** The largest report body accepted. A full report is well under 1 KiB. */
const MAX_BODY = 16 * 1024;
/** How long counters are kept. */
export const RETENTION_DAYS = 400;

const json = (body: unknown, status = 200, headers: Record<string, string> = {}) =>
  new Response(JSON.stringify(body), {
    status,
    headers: { "content-type": "application/json", ...headers },
  });

export default {
  async fetch(request: Request, env: Env): Promise<Response> {
    const { pathname } = new URL(request.url);
    if (pathname === "/v1/report") {
      return request.method === "POST" ? report(request, env) : json({ error: "use POST" }, 405);
    }
    if (pathname === "/v1/stats") {
      return request.method === "GET" ? publish(env) : json({ error: "use GET" }, 405);
    }
    return json({ error: "not found" }, 404);
  },

  async scheduled(_controller: ScheduledController, env: Env): Promise<void> {
    await expire(env, utcDay(new Date()));
  },
} satisfies ExportedHandler<Env>;

async function report(request: Request, env: Env): Promise<Response> {
  const declared = Number(request.headers.get("content-length") ?? "0");
  if (declared > MAX_BODY) return json({ error: "too large" }, 413);
  const text = await request.text();
  if (text.length > MAX_BODY) return json({ error: "too large" }, 413);
  let body: unknown;
  try {
    body = JSON.parse(text);
  } catch {
    return json({ error: "not JSON" }, 400);
  }
  const day = utcDay(new Date());
  const checked = validate(body, day);
  if (typeof checked === "string") return json({ error: checked }, 400);
  const statements = [
    env.DB.prepare(
      `INSERT INTO reports (day, mode, client, forge, forge_major, os, arch, version, gh, first_week, count)
       VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 1)
       ON CONFLICT (day, mode, client, forge, forge_major, os, arch, version, gh, first_week)
       DO UPDATE SET count = count + 1`,
    ).bind(
      day,
      checked.mode,
      checked.client ?? "",
      checked.forge ?? "",
      checked.forge_major ?? -1,
      checked.os,
      checked.arch,
      checked.version,
      checked.gh ?? "",
      checked.first_week,
    ),
    ...Object.entries(checked.unsupported).map(([name, count]) =>
      env.DB.prepare(
        `INSERT INTO unsupported (day, name, reports, total) VALUES (?, ?, 1, ?)
         ON CONFLICT (day, name) DO UPDATE SET reports = reports + 1, total = total + excluded.total`,
      ).bind(day, name, count),
    ),
  ];
  await env.DB.batch(statements);
  return new Response(null, { status: 204 });
}

async function publish(env: Env): Promise<Response> {
  const today = utcDay(new Date());
  // Enough days for the oldest published week.
  const since = daysBefore(today, Math.max(DAYS, WEEKS * 7 + 7));
  const [rows, unsupported] = await Promise.all([
    env.DB.prepare("SELECT * FROM reports WHERE day >= ?").bind(since).all<Row>(),
    env.DB.prepare("SELECT * FROM unsupported WHERE day >= ?").bind(since).all<UnsupportedRow>(),
  ]);
  return json(stats(rows.results, unsupported.results, today), 200, {
    "access-control-allow-origin": "*",
    "cache-control": "public, max-age=3600",
  });
}

/** Deletes every counter older than [`RETENTION_DAYS`]. */
export async function expire(env: Env, today: string): Promise<void> {
  const cutoff = daysBefore(today, RETENTION_DAYS);
  await env.DB.batch([
    env.DB.prepare("DELETE FROM reports WHERE day < ?").bind(cutoff),
    env.DB.prepare("DELETE FROM unsupported WHERE day < ?").bind(cutoff),
  ]);
}
