import { describe, expect, it } from "vitest";
import { FIELDS, MINIMUM, figures, type Row } from "../src/stats";
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
  count,
  ...values,
});

type Figure = Record<string, string | number>;

describe("figures", () => {
  // 2026-11-02 is a Monday, in 2026-W45.

  it("counts installs per day and proves installs per week from the busiest day", () => {
    const rows = [row("2026-11-02", 3), row("2026-11-03", 5), row("2026-11-04", 4)];
    const result = figures(rows, [], []);
    expect(result.totals.days).toEqual([
      { day: "2026-11-02", installs: 3, reports: 3 },
      { day: "2026-11-03", installs: 5, reports: 5 },
      { day: "2026-11-04", installs: 4, reports: 4 },
    ]);
    expect(result.totals.weeks).toEqual([{ week: "2026-W45", installs: 5, reports: 12 }]);
    expect(result.by_field.client.weeks).toEqual([
      { week: "2026-W45", client: "codex", installs: 5, reports: 12 },
    ]);
  });

  it("never lets a total minus what's shown give away a small count", () => {
    // Three client reports and one server report: total 4, so mode client's 3 would give away the
    // server's 1. Neither mode shows; the total does.
    const result = figures(
      [row("2026-11-02", 3), row("2026-11-02", 1, { mode: "server", client: "", gh: "" })],
      [],
      [],
    );
    expect(result.totals.days).toEqual([{ day: "2026-11-02", installs: 4, reports: 4 }]);
    expect(result.by_field.mode.days).toEqual([]);
    expect(JSON.stringify(result)).not.toContain("server");
  });

  it("shows a field's values once what's hidden is two or more cells summing to 3 or more", () => {
    const result = figures(
      [
        row("2026-11-02", 5),
        row("2026-11-02", 2, { client: "cursor" }),
        row("2026-11-02", 1, { client: "claude-code" }),
      ],
      [],
      [],
    );
    expect(result.by_field.client.days).toEqual([
      { day: "2026-11-02", client: "codex", installs: 5, reports: 5 },
    ]);
  });

  it("never lets a week minus its shown days give away a small day", () => {
    // codex: 5 on Monday, 2 on Tuesday. The week's 7 less Monday's 5 would show Tuesday's 2, and
    // with Monday hidden, the week's installs of 5 would still show Monday's.
    const result = figures([row("2026-11-02", 5), row("2026-11-03", 2)], [], []);
    const shown = (cells: Figure[]) => cells.map((cell) => cell.day ?? cell.week);
    expect(shown(result.totals.weeks)).toEqual([]);
    expect(shown(result.totals.days)).toEqual([]);
  });

  it("never lets a week's busiest day give away a hidden day", () => {
    // codex: 1 on Monday, 10 on Tuesday, 4 each later day; cursor's only report is Tuesday's 1.
    // A week showing installs 10 beside shown days of 4 would put 10 on a hidden day; the week's
    // reports less the shown days would then leave Monday's 1, and Tuesday's total less 10, cursor's.
    const days = ["2026-11-02", "2026-11-03", "2026-11-04", "2026-11-05", "2026-11-06", "2026-11-07", "2026-11-08"];
    const other = { client: "cursor", os: "linux" };
    const rows = [
      row(days[0], 1),
      row(days[1], 10),
      row(days[1], 1, other),
      ...days.slice(2).map((day) => row(day, 4)),
      ...[0, 2, 3, 4, 5, 6].map((i) => row(days[i], 3, { ...other, arch: "x86_64" })),
    ];
    const result = figures(rows, [], []);
    const codex = (cell: Figure) => cell.client === "codex" && cell.os === "macos";
    for (const week of result.combinations.weeks.filter(codex)) {
      const shown = result.combinations.days.filter(codex).map((cell) => cell.installs);
      expect(shown).toContain(week.installs);
    }
  });

  it("never publishes a figure equal to its only part when that part is hidden", () => {
    // codex reports only on Tuesday, 10 of them; cursor's 1 hides Tuesday's codex. A week of 10
    // from one day would give that day away, and Tuesday's total less it, cursor's 1.
    const other = { client: "cursor", os: "linux" };
    const rows = [
      row("2026-11-03", 10),
      row("2026-11-03", 1, other),
      ...["2026-11-02", "2026-11-04", "2026-11-05"].map((day) => row(day, 3, { ...other, arch: "x86_64" })),
    ];
    const result = figures(rows, [], []);
    const codex = (cell: Figure) => cell.client === "codex" && cell.os === "macos";
    expect(result.combinations.days.filter(codex)).toEqual([]);
    expect(result.combinations.weeks.filter(codex)).toEqual([]);
  });

  it("names an absent value none", () => {
    const rows = [row("2026-11-02", 3, { mode: "server", client: "", gh: "", forge_major: -1 })];
    expect(figures(rows, [], []).by_field.client.days).toEqual([
      { day: "2026-11-02", client: "none", installs: 3, reports: 3 },
    ]);
  });

  it("follows each first week's installs into later weeks", () => {
    const days: Array<[string, string, number]> = [
      ["2026-11-02", "2026-W45", 4],
      ["2026-11-03", "2026-W45", 6],
      ["2026-11-10", "2026-W45", 3],
      ["2026-11-10", "2026-W46", 3],
      ["2026-11-17", "2026-W45", 3],
      ["2026-11-17", "2026-W46", 2],
    ];
    const rows = [...new Set(days.map(([day]) => day))].map((day) =>
      row(day, days.filter(([d]) => d === day).reduce((sum, [, , n]) => sum + n, 0)),
    );
    const cohorts = days.map(([day, first_week, count]) => ({ day, first_week, count }));
    expect(figures(rows, cohorts, []).retention).toEqual([
      { first_week: "2026-W45", week: "2026-W45", installs: 6 },
      { first_week: "2026-W45", week: "2026-W46", installs: 3 },
      { first_week: "2026-W46", week: "2026-W46", installs: 3 },
      // 2026-W47: the week's 5 less W45's 3 would show W46's 2, so neither shows.
    ]);
  });

  it("counts an unsupported name by its busiest day, so one install can't pass for three", () => {
    const days = ["2026-11-02", "2026-11-03", "2026-11-04"];
    const once = days.map((day) => ({ day, name: "rest:repos.releases", reports: 1, total: 5 }));
    expect(figures([], [], once).unsupported).toEqual([]);
    const three = [{ day: "2026-11-02", name: "rest:repos.releases", reports: 3, total: 4 }];
    expect(figures([], [], three).unsupported).toEqual([
      { week: "2026-W45", name: "rest:repos.releases", installs: 3, total: 4 },
    ]);
  });

  /**
   * For many made-up weeks of reports: every published figure counts at least 3, and wherever a
   * published figure's parts aren't all shown, the hidden parts are two or more and sum to 3 or
   * more, so subtraction never isolates one.
   */
  it("holds its rule on made-up data", () => {
    let seed = 7;
    const random = (n: number) => {
      seed = (seed * 1103515245 + 12345) % 2147483648;
      return seed % n;
    };
    const values = {
      client: ["codex", "chatgpt", "claude-code", "cursor", "other"],
      os: ["macos", "linux"],
      version: ["0.3.0", "0.3.1"],
    };
    for (let trial = 0; trial < 40; trial++) {
      const rows: Row[] = [];
      for (let i = 0; i < 25; i++) {
        rows.push(
          row(`2026-11-0${2 + random(5)}`, 1 + random(4), {
            client: values.client[random(5)],
            os: values.os[random(2)],
            version: values.version[random(2)],
          }),
        );
      }
      const result = figures(rows, [], []);
      const all: Figure[] = [
        ...result.totals.days, ...result.totals.weeks,
        ...result.combinations.days, ...result.combinations.weeks,
        ...Object.values(result.by_field).flatMap((table) => [...table.days, ...table.weeks]),
      ];
      for (const figure of all) expect(figure.installs).toBeGreaterThanOrEqual(MINIMUM);

      // Each published week has a published day as busy as it, so its installs shows no hidden day.
      const tables = [result.totals, result.combinations, ...Object.values(result.by_field)];
      for (const table of tables) {
        for (const { week, installs, reports: _, ...labels } of table.weeks as Figure[]) {
          const same = (table.days as Figure[]).filter(
            ({ day, installs: __, reports: ___, ...rest }) =>
              JSON.stringify(rest) === JSON.stringify(labels) && week === isoWeek(String(day)),
          );
          expect(same.map((cell) => cell.installs), `${week} ${JSON.stringify(labels)}`).toContain(installs);
        }
      }

      // Each day: the total over each field's values.
      const truth = (day: string, field: string) => {
        const sums = new Map<string, number>();
        for (const r of rows.filter((r) => r.day === day)) {
          const key = String(r[field as keyof Row]);
          sums.set(key, (sums.get(key) ?? 0) + r.count);
        }
        return sums;
      };
      for (const total of result.totals.days) {
        for (const field of FIELDS) {
          const shown = new Set(
            (result.by_field[field].days as Figure[])
              .filter((cell) => cell.day === total.day)
              .map((cell) => String(cell[field])),
          );
          const hidden = [...truth(String(total.day), field)].filter(([value]) => !shown.has(value));
          const sum = hidden.reduce((s, [, n]) => s + n, 0);
          if (hidden.length > 0 && shown.size > 0) {
            expect(hidden.length, `${total.day} ${field}`).toBeGreaterThanOrEqual(2);
            expect(sum, `${total.day} ${field}`).toBeGreaterThanOrEqual(MINIMUM);
          }
        }
      }
    }
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

describe("retention", () => {
  it("never lets a week's total minus its first weeks give away a small cohort", () => {
    // Four reports on 2026-01-05: three from installs that began that week, one from the next
    // week's only other possible first week. The week's 4 less W01's 3 would show W02's 1.
    const rows = [
      { day: "2026-01-05", mode: "client", client: "codex", forge: "forgejo", forge_major: 15, os: "macos",
        arch: "aarch64", version: "0.3.0", gh: "2.103", count: 4 },
    ];
    const cohorts = [
      { day: "2026-01-05", first_week: "2026-W01", count: 3 },
      { day: "2026-01-05", first_week: "2026-W02", count: 1 },
    ];
    const result = figures(rows, cohorts, []);
    expect(result.totals.weeks).toEqual([{ week: "2026-W02", installs: 4, reports: 4 }]);
    expect(result.retention).toEqual([]);
  });
});
