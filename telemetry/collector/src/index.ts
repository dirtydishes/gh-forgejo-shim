/**
 * gh-forgejo-shim's usage-report collector (docs/telemetry.md).
 *
 * `POST /v1/report` checks a report against the contract and adds one to that day's counter for
 * its values. The counters are private: only the maintainer reads them, from the database. Once a
 * week ends, the daily cron works out its one-field totals, with noise and rounded, and stores them; `GET /v1/stats`
 * publishes what's stored. The Worker reads nothing from a request but its method, path and body,
 * keeps no address, header, user agent or body, and logs nothing.
 */
import { validate } from "./report";
import { FIELDS, ROUNDING, noise, weekFigures, type Sum, type Week } from "./stats";
import { daysBefore, isoWeek, monday, utcDay } from "./week";

export interface Env {
  DB: D1Database;
}

/** The largest report body accepted. A full report is well under 1 KiB. */
const MAX_BODY = 16 * 1024;
/** How long counters, and the figures worked out from them, are kept. */
export const RETENTION_DAYS = 400;
/** Weeks `/v1/stats` publishes, and so the finished weeks the cron works out. */
export const WEEKS = 26;

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
    const today = utcDay(new Date());
    await freeze(env, today);
    await expire(env, today);
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
      `INSERT INTO reports (day, mode, client, forge, forge_major, os, arch, version, gh, count)
       VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, 1)
       ON CONFLICT (day, mode, client, forge, forge_major, os, arch, version, gh)
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
    ),
    env.DB.prepare(
      `INSERT INTO cohorts (day, first_week, count) VALUES (?, ?, 1)
       ON CONFLICT (day, first_week) DO UPDATE SET count = count + 1`,
    ).bind(day, checked.first_week),
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

/**
 * Between ?1 and ?2, each day's total (`field` ""), then each day's count for each value of each
 * published field. The database does the summing, so the Worker handles a few rows a day however
 * many counters there are.
 */
const SUMS = [
  "SELECT '' AS field, '' AS value, day, SUM(count) AS n FROM reports WHERE day BETWEEN ?1 AND ?2 GROUP BY day",
  ...FIELDS.map(
    (field) =>
      `SELECT '${field}' AS field, ${field} AS value, day, SUM(count) AS n FROM reports
       WHERE day BETWEEN ?1 AND ?2 GROUP BY day, ${field}`,
  ),
];

/**
 * Works out and stores the figures for each of the last [`WEEKS`] finished weeks that has none
 * stored. A report counts on the day it arrives, so a finished week's counters never change; its
 * figures are worked out from them once and never again, so two responses never differ in a figure
 * and can't be subtracted from each other, nor its noise averaged away. `draw` is for tests. It makes at most three database calls, however many
 * weeks are missing, to stay within the Workers Free plan's limits.
 */
export async function freeze(env: Env, today: string, draw: () => number = noise): Promise<void> {
  const thisWeek = monday(today);
  const stored = await env.DB.prepare("SELECT week FROM published WHERE week >= ?")
    .bind(isoWeek(daysBefore(thisWeek, WEEKS * 7)))
    .all<{ week: string }>();
  const done = new Set(stored.results.map((row) => row.week));
  const missing: string[] = [];
  for (let back = WEEKS; back >= 1; back--) {
    if (!done.has(isoWeek(daysBefore(thisWeek, back * 7)))) missing.push(daysBefore(thisWeek, back * 7));
  }
  if (missing.length === 0) return;
  const range = [missing[0], daysBefore(missing[missing.length - 1], -6)];
  const sums = await env.DB.batch<Sum>(SUMS.map((query) => env.DB.prepare(query).bind(...range)));
  const byWeek = new Map<string, Sum[]>();
  for (const sum of sums.flatMap((found) => found.results)) {
    const week = byWeek.get(isoWeek(sum.day)) ?? [];
    week.push(sum);
    byWeek.set(isoWeek(sum.day), week);
  }
  await env.DB.batch(
    missing.map((first) =>
      env.DB.prepare("INSERT INTO published (week, figures) VALUES (?, ?) ON CONFLICT (week) DO NOTHING").bind(
        isoWeek(first),
        JSON.stringify(weekFigures(isoWeek(first), byWeek.get(isoWeek(first)) ?? [], draw)),
      ),
    ),
  );
}

/** The stored figures for the last [`WEEKS`] finished weeks, oldest first; never the current week. */
async function publish(env: Env): Promise<Response> {
  const today = utcDay(new Date());
  const stored = await env.DB.prepare("SELECT figures FROM published WHERE week >= ? ORDER BY week")
    .bind(isoWeek(daysBefore(monday(today), WEEKS * 7)))
    .all<{ figures: string }>();
  const body = {
    generated: today,
    rounded_to: ROUNDING,
    weeks: stored.results.map((row) => JSON.parse(row.figures) as Week),
  };
  return json(body, 200, {
    "access-control-allow-origin": "*",
    "cache-control": "public, max-age=3600",
  });
}

/**
 * Deletes every counter older than [`RETENTION_DAYS`], and the figures of every week before the
 * cutoff's. That's long after [`freeze`] stops working a week out, so no week's figures are ever
 * worked out again from what's left of its counters.
 */
export async function expire(env: Env, today: string): Promise<void> {
  const cutoff = daysBefore(today, RETENTION_DAYS);
  await env.DB.batch([
    env.DB.prepare("DELETE FROM reports WHERE day < ?").bind(cutoff),
    env.DB.prepare("DELETE FROM cohorts WHERE day < ?").bind(cutoff),
    env.DB.prepare("DELETE FROM unsupported WHERE day < ?").bind(cutoff),
    env.DB.prepare("DELETE FROM published WHERE week < ?").bind(isoWeek(cutoff)),
  ]);
}
