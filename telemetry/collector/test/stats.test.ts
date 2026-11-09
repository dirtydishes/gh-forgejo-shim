import { describe, expect, it } from "vitest";
import { FIELDS, UNDER, rounded, weekFigures, type Rounded, type Sum } from "../src/stats";
import { isoWeek } from "../src/week";

/** One `reports` counter: a day, a combination of values, and how many reports had them. */
interface Row {
  day: string;
  mode: string;
  client: string;
  forge: string;
  forge_major: number;
  os: string;
  arch: string;
  version: string;
  gh: string;
  count: number;
}

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

// 2026-11-02 is a Monday, in 2026-W45.
const WEEK = "2026-W45";
const DAYS = ["2026-11-02", "2026-11-03", "2026-11-04", "2026-11-05", "2026-11-06", "2026-11-07", "2026-11-08"];
/** What the cron's query sums from these counters: each day's total, and each field's values. */
const sums = (rows: Row[]): Sum[] =>
  rows.flatMap((r) => [
    { field: "", value: "", day: r.day, n: r.count },
    ...FIELDS.map((field) => ({ field, value: String(r[field]), day: r.day, n: r.count })),
  ]);
const week = (rows: Row[]) => weekFigures(WEEK, sums(rows));

describe("rounded", () => {
  it("rounds to the nearest 10 and never shows a count below 10", () => {
    const cases: Array<[number, Rounded]> = [
      [0, UNDER], [1, UNDER], [9, UNDER], [10, 10], [14, 10], [15, 20], [24, 20], [124, 120], [125, 130],
    ];
    for (const [count, shown] of cases) expect(rounded(count), String(count)).toBe(shown);
  });

  it("gives every figure a range of at least five counts, wherever a sender shifts it", () => {
    // Someone who sends k reports of their own knows k, so they read a figure as the real count
    // plus k. For any k, each real count shares its figure with another: one figure never pins it.
    for (let k = 0; k <= 30; k++) {
      for (let real = 1; real <= 30; real++) {
        const same = [...Array(60).keys()].filter((other) => rounded(other + k) === rounded(real + k));
        expect(same.length, `k ${k}, real ${real}`).toBeGreaterThanOrEqual(2);
      }
    }
  });
});

describe("weekFigures", () => {
  it("publishes installs as the busiest day, reports as the week's sum, and one breakdown per field", () => {
    const rows = [
      row(DAYS[0], 30),
      row(DAYS[1], 50),
      row(DAYS[1], 12, { client: "cursor", os: "linux", version: "0.3.1" }),
      row(DAYS[2], 40),
    ];
    expect(week(rows)).toEqual({
      week: WEEK,
      installs: 60,
      reports: 130,
      client: {
        codex: 50, chatgpt: UNDER, "claude-code": UNDER, "claude-desktop": UNDER, cursor: 10, other: UNDER,
        none: UNDER,
      },
      forge: { forgejo: 60, gitea: UNDER, other: UNDER, none: UNDER },
      mode: { client: 60, server: UNDER },
      os: { macos: 50, linux: 10, other: UNDER },
      version: { "0.3.0": 50, "0.3.1": 10, other: UNDER },
    });
  });

  it("lists every fixed value, so under 10 may be none at all", () => {
    expect(week([])).toEqual(week([row(DAYS[0], 1, { client: "cursor", forge: "gitea", os: "other" })]));
  });

  it("counts versions below 10 together as other", () => {
    const rows = [
      row(DAYS[0], 20, { version: "0.3.1" }),
      row(DAYS[0], 6, { version: "0.2.0" }),
      row(DAYS[0], 6, { version: "0.4.0-rc.1" }),
    ];
    expect(week(rows).version).toEqual({ "0.3.1": 20, other: 10 });
    expect(JSON.stringify(week(rows))).not.toContain("0.4.0-rc.1");
  });

  it("names an absent value none", () => {
    const server = week([row(DAYS[0], 12, { mode: "server", client: "", gh: "", forge: "", forge_major: -1 })]);
    expect(server.client.none).toBe(10);
    expect(server.forge.none).toBe(10);
  });

  it("publishes nothing per day, per combination, per first week or per unsupported name", () => {
    expect(Object.keys(week([row(DAYS[0], 30)]))).toEqual(["week", "installs", "reports", ...FIELDS]);
  });

  it("no longer lets three fields on one day solve for a hidden count (#287)", () => {
    // #287's first shape. Four small combinations on one day, codex/macos/0.3.0 = A, beside two
    // large ones. With combinations published, codex's, 0.3.0's and linux's hidden parts gave
    // A = (3 + 3 - 4) / 2 = 1. Now no combination is published, and A of 0, 1 or 2 reads the same.
    const shape = (a: number) => [
      row(DAYS[0], a),
      row(DAYS[0], 2, { client: "cursor", os: "linux" }),
      row(DAYS[0], 2, { os: "linux", version: "0.3.1" }),
      row(DAYS[0], 2, { client: "cursor", version: "0.3.1" }),
      row(DAYS[0], 50, { version: "0.3.1" }),
      row(DAYS[0], 50, { client: "cursor", os: "linux", version: "0.3.1" }),
    ];
    expect(week(shape(1))).toEqual(week(shape(0)));
    expect(week(shape(1))).toEqual(week(shape(2)));
  });

  it("no longer lets one field over a week's days solve for a hidden count (#287)", () => {
    // #287's second shape: codex and chatgpt hidden on Monday and Tuesday, cursor and other on
    // Wednesday and Thursday, joined by codex on Wednesday = A. With daily figures published, the
    // row and column totals fixed A at 1. Now nothing is published per day, and A of 0, 1 or 2
    // reads the same.
    const shape = (a: number) => [
      ...DAYS.slice(0, 4).map((day) => row(day, 41, { client: "claude-code" })),
      ...DAYS.slice(0, 2).flatMap((day) => [row(day, 2), row(day, 2, { client: "chatgpt" })]),
      ...DAYS.slice(2, 4).flatMap((day) => [row(day, 2, { client: "cursor" }), row(day, 2, { client: "other" })]),
      row(DAYS[2], a),
    ];
    expect(week(shape(1))).toEqual(week(shape(0)));
    expect(week(shape(1))).toEqual(week(shape(2)));
  });

  it("no longer shows a sender who tops up a rare set how many real installs share it (#285)", () => {
    // #285: send two reports with a rare set of values and see whether the published figure says 3.
    // Now no set of values is published, each value's figure rounds, and 0, 1 or 2 real installs
    // with that set read the same.
    const rare = { client: "cursor", os: "linux", forge: "gitea", forge_major: 1, gh: "2.99" };
    const shape = (real: number) => [
      row(DAYS[1], 40),
      row(DAYS[1], 2, rare), // the sender's own
      row(DAYS[1], real, rare),
    ];
    expect(week(shape(1))).toEqual(week(shape(0)));
    expect(week(shape(1))).toEqual(week(shape(2)));
  });

  it("doesn't show a sender one real install where others report too (parallax r7#2)", () => {
    // A sender floods one day: 94 common reports and 9 with a rare set, so the day's total sits at
    // the top of its range and each rare value one short of 10. With 39 other installs reporting
    // that day, one or two real installs with the rare set read the same.
    const rare = { client: "cursor", os: "linux", forge: "gitea", forge_major: 1, version: "0.4.0" };
    const shape = (real: number) => [
      row(DAYS[1], 94), // the sender's own
      row(DAYS[1], 9, rare), // the sender's own
      row(DAYS[1], 39),
      row(DAYS[1], real, rare),
    ];
    expect(week(shape(1))).toEqual(week(shape(2)));
  });

  /**
   * For many made-up weeks: a reader who takes each figure as its range, and assumes each field's
   * figures add up to `installs` (the most they could assume), can narrow an "under 10" figure, but
   * never to one count from 1 to 8.
   */
  it("never lets a reader work out a count from 1 to 8 on made-up data", () => {
    let seed = 7;
    const random = (n: number) => {
      seed = (seed * 1103515245 + 12345) % 2147483648;
      return seed % n;
    };
    const values = {
      client: ["codex", "chatgpt", "claude-code", "cursor", "other"],
      forge: ["forgejo", "gitea", ""],
      os: ["macos", "linux"],
      version: ["0.3.0", "0.3.1", "0.2.0", "0.4.0", "0.4.1", "0.5.0"],
    };
    const range = (figure: Rounded): [number, number] =>
      figure === UNDER ? [0, 9] : figure === 10 ? [10, 14] : [figure - 5, figure + 4];
    let narrowed = 0;
    for (let trial = 0; trial < 300; trial++) {
      const rows: Row[] = [];
      // One day in three trials, so each field's figures really do add up to `installs`.
      const days = trial % 3 === 0 ? 1 : 1 + random(7);
      for (let i = 0; i < 4 + random(30); i++) {
        rows.push(
          row(DAYS[random(days)], 1 + random(trial % 2 ? 12 : 5), {
            client: values.client[random(5)],
            forge: values.forge[random(3)],
            os: values.os[random(2)],
            version: values.version[random(trial % 2 ? 6 : 2)],
          }),
        );
      }
      const result = week(rows);
      const tables = FIELDS.map((field) => Object.values(result[field] as Record<string, Rounded>));
      for (const figure of [result.installs, result.reports, ...tables.flat()]) {
        expect(figure === UNDER || (figure >= 10 && figure % 10 === 0), String(figure)).toBe(true);
      }

      let [low, high] = range(result.installs);
      for (const table of tables) {
        low = Math.max(low, table.reduce<number>((sum, figure) => sum + range(figure)[0], 0));
        high = Math.min(high, table.reduce<number>((sum, figure) => sum + range(figure)[1], 0));
      }
      for (const table of tables) {
        table.forEach((figure, i) => {
          if (figure !== UNDER) return;
          const others = table.filter((_, j) => j !== i).map(range);
          const least = Math.max(0, low - others.reduce((sum, [, most]) => sum + most, 0));
          const most = Math.min(9, high - others.reduce((sum, [fewest]) => sum + fewest, 0));
          if (most < 9 || least > 0) narrowed++;
          expect(least === most && least >= 1 && least <= 8, `${JSON.stringify(rows)}`).toBe(false);
        });
      }
    }
    // The check has teeth: some figures do narrow.
    expect(narrowed).toBeGreaterThan(0);
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
