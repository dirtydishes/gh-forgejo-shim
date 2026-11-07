import { describe, expect, it } from "vitest";
import { stats, type Row } from "../src/stats";
import { isoWeek } from "../src/week";

const row = (day: string, count: number, values: Partial<Row> = {}): Row => ({
  day,
  mode: "client",
  client: "codex",
  forge: "forgejo",
  forge_major: 15,
  os: "macos",
  arch: "aarch64",
  version: "0.3.0",
  gh: "2.103",
  first_week: "2026-W45",
  count,
  ...values,
});

describe("stats", () => {
  // 2026-11-02 is a Monday, in 2026-W45.
  const today = "2026-11-08";

  it("counts installs per day and proves installs per week from the busiest day", () => {
    const rows = [
      row("2026-11-02", 3),
      row("2026-11-03", 5),
      row("2026-11-04", 4, { client: "claude-code" }),
    ];
    const result = stats(rows, [], today);
    expect(result.minimum_count).toBe(3);
    expect(result.totals.days).toEqual([
      { day: "2026-11-02", installs: 3, reports: 3 },
      { day: "2026-11-03", installs: 5, reports: 5 },
      { day: "2026-11-04", installs: 4, reports: 4 },
    ]);
    expect(result.totals.weeks).toEqual([{ week: "2026-W45", installs: 5, reports: 12 }]);
    expect(result.by_field.client.weeks).toEqual([
      { week: "2026-W45", client: "claude-code", installs: 4, reports: 4 },
      { week: "2026-W45", client: "codex", installs: 5, reports: 8 },
    ]);
    expect(result.by_field.mode.weeks).toEqual([
      { week: "2026-W45", mode: "client", installs: 5, reports: 12 },
    ]);
  });

  it("leaves out every cell below 3, with its labels", () => {
    const rows = [
      row("2026-11-02", 3),
      row("2026-11-02", 2, { os: "linux", version: "0.3.1" }),
      row("2026-11-02", 1, { mode: "server", client: "", gh: "", forge: "gitea", forge_major: 1 }),
    ];
    const result = stats(rows, [{ day: "2026-11-02", name: "rest:repos.releases", reports: 2, total: 40 }], today);
    const text = JSON.stringify(result);
    for (const label of ["linux", "0.3.1", "server", "gitea", "rest:repos.releases"]) {
      expect(text).not.toContain(label);
    }
    expect(result.totals.days).toEqual([{ day: "2026-11-02", installs: 6, reports: 6 }]);
    expect(result.combinations.days).toEqual([
      {
        day: "2026-11-02", mode: "client", client: "codex", forge: "forgejo", forge_major: 15,
        os: "macos", arch: "aarch64", version: "0.3.0", gh: "2.103", installs: 3, reports: 3,
      },
    ]);
    for (const cells of [
      result.totals.days, result.totals.weeks, result.combinations.days, result.combinations.weeks,
      result.retention, result.unsupported,
      ...Object.values(result.by_field).flatMap((field) => [field.days, field.weeks]),
    ]) {
      for (const cell of cells as unknown as Array<Record<string, number>>) {
        expect(cell.installs ?? cell.reports).toBeGreaterThanOrEqual(3);
      }
    }
  });

  it("names an absent value none", () => {
    const rows = [row("2026-11-02", 3, { mode: "server", client: "", gh: "", forge_major: -1 })];
    expect(stats(rows, [], today).by_field.client.days).toEqual([
      { day: "2026-11-02", client: "none", installs: 3, reports: 3 },
    ]);
  });

  it("follows each first week's installs into later weeks", () => {
    const rows = [
      row("2026-11-02", 4),
      row("2026-11-03", 6),
      row("2026-11-10", 3),
      row("2026-11-11", 2, { first_week: "2026-W46" }),
    ];
    expect(stats(rows, [], "2026-11-12").retention).toEqual([
      { first_week: "2026-W45", week: "2026-W45", installs: 6 },
      { first_week: "2026-W45", week: "2026-W46", installs: 3 },
    ]);
  });

  it("publishes 90 days and 26 weeks", () => {
    const old = row("2026-05-01", 9);
    const result = stats([old, row("2026-11-02", 3)], [], today);
    expect(result.totals.days.map((cell) => cell.day)).toEqual(["2026-11-02"]);
    expect(result.totals.weeks.map((cell) => cell.week)).toEqual(["2026-W45"]);
    expect(isoWeek("2026-05-01")).toBe("2026-W18");
  });
});

describe("isoWeek", () => {
  it("follows ISO 8601 at year ends", () => {
    expect(isoWeek("2026-11-07")).toBe("2026-W45");
    expect(isoWeek("2027-01-01")).toBe("2026-W53");
    expect(isoWeek("2024-12-30")).toBe("2025-W01");
    expect(isoWeek("2021-01-03")).toBe("2020-W53");
  });
});
