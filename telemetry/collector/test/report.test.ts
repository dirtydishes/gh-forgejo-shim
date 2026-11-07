import { env, exports } from "cloudflare:workers";
import { beforeEach, describe, expect, it } from "vitest";
import { expire } from "../src/index";
import { isoWeek, utcDay } from "../src/week";

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

// Each test starts from empty counters.
beforeEach(async () => {
  await env.DB.batch([env.DB.prepare("DELETE FROM reports"), env.DB.prepare("DELETE FROM unsupported")]);
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
        first_week: isoWeek(today),
        count: 2,
      },
    ]);
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
        "INSERT INTO reports VALUES ('2025-01-01', 'client', 'codex', '', -1, 'linux', 'x86_64', '0.3.0', '', '2026-W45', 5)",
      ),
      env.DB.prepare(
        "INSERT INTO reports VALUES ('2026-11-01', 'client', 'codex', '', -1, 'linux', 'x86_64', '0.3.0', '', '2026-W45', 5)",
      ),
      env.DB.prepare("INSERT INTO unsupported VALUES ('2025-01-01', 'rest:other', 3, 3)"),
    ]);
    await expire(env, "2026-11-07");
    expect((await rows("reports")).map((row) => row.day)).toEqual(["2026-11-01"]);
    expect(await rows("unsupported")).toEqual([]);
  });
});

describe("GET /v1/stats", () => {
  it("publishes totals with small counts left out", async () => {
    for (let i = 0; i < 3; i++) await post(example());
    const other = example();
    other.client = "cursor";
    await post(other);
    const response = await exports.default.fetch("https://telemetry.example/v1/stats");
    expect(response.status).toBe(200);
    expect(response.headers.get("access-control-allow-origin")).toBe("*");
    const body = (await response.json()) as Record<string, any>;
    expect(body.totals.days).toEqual([{ day: today, installs: 4, reports: 4 }]);
    expect(body.by_field.client.days).toEqual([
      { day: today, client: "codex", installs: 3, reports: 3 },
    ]);
    expect(JSON.stringify(body)).not.toContain("cursor");
    // All four reports named it, twice each.
    expect(body.unsupported).toEqual([
      { week: isoWeek(today), name: "graphql:Repository.projectsV2", reports: 4, total: 8 },
    ]);
  });
});
