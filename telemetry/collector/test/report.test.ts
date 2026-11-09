import { env, exports } from "cloudflare:workers";
import { beforeEach, describe, expect, it } from "vitest";
import { expire, freeze } from "../src/index";
import { daysBefore, isoWeek, monday, utcDay } from "../src/week";

const today = utcDay(new Date());

/** The report doc/telemetry.md shows, dated this week. */
const example = () => ({
  schema: 1,
  version: "0.3.0",
  os: "macos",
  arch: "aarch64",
  mode: "client",
  client: "codex",
  forge: "forgejo",
  forge_major: 15,
  gh: "2.103",
  first_week: isoWeek(today),
  unsupported: { "graphql:Repository.projectsV2": 2 },
});

const post = (body: unknown, headers: Record<string, string> = {}) =>
  exports.default.fetch(
    new Request("https://telemetry.example/v1/report", {
      method: "POST",
      headers: { "content-type": "application/json", ...headers },
      body: typeof body === "string" ? body : JSON.stringify(body),
    }),
  );

const rows = async (table: string) =>
  (await env.DB.prepare(`SELECT * FROM ${table}`).all()).results;

// Each test starts from empty counters and nothing published.
beforeEach(async () => {
  await env.DB.batch(
    ["reports", "cohorts", "unsupported", "published"].map((table) => env.DB.prepare(`DELETE FROM ${table}`)),
  );
});

describe("POST /v1/report", () => {
  it("counts a valid report and keeps nothing about its sender", async () => {
    const response = await post(example(), {
      "cf-connecting-ip": "203.0.113.9",
      "user-agent": "reqwest/0.12",
      "x-forwarded-for": "203.0.113.9",
    });
    expect(response.status).toBe(204);
    expect(await post(example())).toHaveProperty("status", 204);
    expect(await rows("reports")).toEqual([
      {
        day: today,
        mode: "client",
        client: "codex",
        forge: "forgejo",
        forge_major: 15,
        os: "macos",
        arch: "aarch64",
        version: "0.3.0",
        gh: "2.103",
        count: 2,
      },
    ]);
    expect(await rows("cohorts")).toEqual([{ day: today, first_week: isoWeek(today), count: 2 }]);
    expect(await rows("unsupported")).toEqual([
      { day: today, name: "graphql:Repository.projectsV2", reports: 2, total: 4 },
    ]);
    const stored = JSON.stringify([await rows("reports"), await rows("unsupported")]);
    expect(stored).not.toContain("203.0.113.9");
    expect(stored).not.toContain("reqwest");
  });

  it("counts a server report, which has no client or gh", async () => {
    const report: Record<string, unknown> = example();
    delete report.client;
    delete report.gh;
    report.mode = "server";
    report.unsupported = {};
    expect((await post(report)).status).toBe(204);
    expect(await rows("reports")).toMatchObject([{ mode: "server", client: "", gh: "" }]);
  });

  it("refuses anything outside the schema, whole", async () => {
    const cases: Array<[string, (report: Record<string, unknown>) => void]> = [
      ["an unknown field", (r) => (r.install_id = "abc")],
      ["a host", (r) => (r.host = "forge.example.com")],
      ["another schema", (r) => (r.schema = 2)],
      ["an unknown client", (r) => (r.client = "vim")],
      ["a raw client name", (r) => (r.client = "/Applications/Codex.app")],
      ["an unknown forge", (r) => (r.forge = "gitlab")],
      ["a forge major without a forge", (r) => delete r.forge],
      ["a fractional forge major", (r) => (r.forge_major = 1.5)],
      ["a bad version", (r) => (r.version = "0.3.0; DROP TABLE reports")],
      ["free text in the version", (r) => (r.version = "0.3.0-alice.private.forge")],
      ["a bad gh", (r) => (r.gh = "2.103.3")],
      ["a future first week", (r) => (r.first_week = "2999-W01")],
      ["a week before reports began", (r) => (r.first_week = "2025-W52")],
      ["an unsupported name outside the vocabulary", (r) => (r.unsupported = { "rest:repos.acme": 1 })],
      ["an unsupported path", (r) => (r.unsupported = { "/repos/acme/secret": 1 })],
      ["a zero count", (r) => (r.unsupported = { "rest:repos.releases": 0 })],
      ["a huge count", (r) => (r.unsupported = { "rest:repos.releases": 1_000_001 })],
      ["a client in server mode", (r) => (r.mode = "server")],
      ["no client in client mode", (r) => delete r.client],
      ["no first week", (r) => delete r.first_week],
    ];
    for (const [name, change] of cases) {
      const report: Record<string, unknown> = example();
      change(report);
      const response = await post(report);
      expect(response.status, name).toBe(400);
    }
    expect((await post("not json")).status).toBe(400);
    expect((await post([example()])).status).toBe(400);
    expect((await post("x".repeat(17 * 1024))).status).toBe(413);
    expect(await rows("reports")).toEqual([]);
    expect(await rows("unsupported")).toEqual([]);
  });

  it("answers only POST", async () => {
    const response = await exports.default.fetch("https://telemetry.example/v1/report");
    expect(response.status).toBe(405);
    expect((await exports.default.fetch("https://telemetry.example/")).status).toBe(404);
  });
});

describe("retention", () => {
  it("deletes counters older than 400 days", async () => {
    await env.DB.batch([
      env.DB.prepare(
        "INSERT INTO reports VALUES ('2025-01-01', 'client', 'codex', '', -1, 'linux', 'x86_64', '0.3.0', '', 5)",
      ),
      env.DB.prepare(
        "INSERT INTO reports VALUES ('2026-11-01', 'client', 'codex', '', -1, 'linux', 'x86_64', '0.3.0', '', 5)",
      ),
      env.DB.prepare("INSERT INTO cohorts VALUES ('2025-01-01', '2026-W45', 5)"),
      env.DB.prepare("INSERT INTO unsupported VALUES ('2025-01-01', 'rest:other', 3, 3)"),
      env.DB.prepare("INSERT INTO published VALUES ('2025-W01', '{}')"),
      env.DB.prepare("INSERT INTO published VALUES ('2026-W44', '{}')"),
    ]);
    await expire(env, "2026-11-07");
    expect((await rows("reports")).map((row) => row.day)).toEqual(["2026-11-01"]);
    expect(await rows("unsupported")).toEqual([]);
    expect(await rows("cohorts")).toEqual([]);
    expect((await rows("published")).map((row) => row.week)).toEqual(["2026-W44"]);
  });
});

/** Monday of last week, the most recent finished week. */
const lastWeek = daysBefore(monday(today), 7);

/** A counter for `day`, as if `count` installs had reported those values. */
const counted = (day: string, count: number, client = "codex") =>
  env.DB.prepare(
    "INSERT INTO reports VALUES (?, 'client', ?, 'forgejo', 15, 'macos', 'aarch64', '0.3.0', '2.103', ?)",
  ).bind(day, client, count);

/** No noise, for tests that check exact figures. */
const exact = () => 0;

const stats = async () =>
  (await (await exports.default.fetch("https://telemetry.example/v1/stats")).json()) as Record<string, any>;

describe("GET /v1/stats", () => {
  it("gives two responses nothing to subtract", async () => {
    // Saving the stats before and after one more report would show that report's values.
    for (let i = 0; i < 3; i++) await post(example());
    await freeze(env, today);
    const before = await stats();
    const other = example();
    other.client = "cursor";
    await post(other);
    await freeze(env, today);
    expect(await stats()).toEqual(before);
  });

  it("publishes a finished week's rounded one-field totals and nothing else", async () => {
    await env.DB.batch([
      counted(lastWeek, 13),
      counted(lastWeek, 1, "cursor"),
      env.DB.prepare("INSERT INTO cohorts VALUES (?, ?, 14)").bind(lastWeek, isoWeek(lastWeek)),
      env.DB.prepare("INSERT INTO unsupported VALUES (?, 'graphql:Repository.projectsV2', 14, 28)").bind(lastWeek),
    ]);
    await freeze(env, today, exact);
    const response = await exports.default.fetch("https://telemetry.example/v1/stats");
    expect(response.status).toBe(200);
    expect(response.headers.get("access-control-allow-origin")).toBe("*");
    const body = (await response.json()) as Record<string, any>;
    expect(Object.keys(body)).toEqual(["generated", "rounded_to", "weeks"]);
    expect(body.generated).toBe(today);
    expect(body.rounded_to).toBe(10);
    expect(body.weeks.at(-1)).toMatchObject({
      week: isoWeek(lastWeek),
      installs: 10,
      client: { codex: 10, cursor: "under 10", chatgpt: "under 10" },
      version: { "0.3.0": 10, other: "under 10" },
    });
    // First weeks and unsupported names stay in the database.
    expect(JSON.stringify(body)).not.toContain("projectsV2");
    expect(JSON.stringify(body)).not.toContain("first_week");
  });

  it("never changes a week's figures once they're published", async () => {
    await env.DB.batch([counted(lastWeek, 13), counted(daysBefore(lastWeek, -1), 25)]);
    await freeze(env, today, exact);
    const before = await stats();
    expect(before.weeks.at(-1)).toMatchObject({ week: isoWeek(lastWeek), installs: 30 });
    await env.DB.batch([counted(lastWeek, 40, "cursor"), counted(daysBefore(lastWeek, -2), 90)]);
    await freeze(env, today);
    expect(await stats()).toEqual(before);
  });

  it("publishes the last 26 weeks", async () => {
    const fifteenWeeks = daysBefore(monday(today), 15 * 7);
    await env.DB.batch([
      counted(daysBefore(monday(today), 27 * 7), 90),
      counted(fifteenWeeks, 40),
      counted(lastWeek, 30),
    ]);
    await freeze(env, today, exact);
    const weeks = (await stats()).weeks as Array<Record<string, any>>;
    expect(weeks.length).toBe(26);
    expect(weeks[0].week).toBe(isoWeek(daysBefore(monday(today), 26 * 7)));
    expect(weeks.filter((found) => found.installs !== "under 10").map((found) => [found.week, found.installs])).toEqual([
      [isoWeek(fifteenWeeks), 40],
      [isoWeek(lastWeek), 30],
    ]);
    expect((await rows("published")).length).toBe(26);
  });

  it("works out 26 weeks within the Free plan's 50 database calls (parallax r7#4)", async () => {
    await env.DB.batch(
      [...Array(26).keys()].map((back) => counted(daysBefore(monday(today), (back + 1) * 7), 20)),
    );
    const db = metered(env.DB);
    await freeze(db.env, today, exact);
    await expire(db.env, today);
    expect(db.calls()).toBeLessThanOrEqual(4);
    const weeks = (await stats()).weeks as Array<Record<string, any>>;
    expect(weeks.map((found) => found.installs)).toEqual(Array(26).fill(20));
  });
});

/** `db`, counting each call that reaches the database; a batch counts once, as D1 counts it. */
function metered(db: D1Database) {
  let calls = 0;
  const real = new WeakMap<object, D1PreparedStatement>();
  const wrap = (statement: D1PreparedStatement): D1PreparedStatement => {
    const counting = {
      bind: (...values: unknown[]) => wrap(statement.bind(...values)),
      all: () => (calls++, statement.all()),
      run: () => (calls++, statement.run()),
      first: () => (calls++, statement.first()),
    } as unknown as D1PreparedStatement;
    real.set(counting, statement);
    return counting;
  };
  const DB = {
    prepare: (query: string) => wrap(db.prepare(query)),
    batch: (statements: D1PreparedStatement[]) => (calls++, db.batch(statements.map((found) => real.get(found)!))),
  } as unknown as D1Database;
  return { env: { DB }, calls: () => calls };
}
