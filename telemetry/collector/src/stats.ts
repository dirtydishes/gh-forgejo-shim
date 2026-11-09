import contract from "../../../contract/telemetry.json";

/** One `reports` row: a day, a combination of values, and how many reports had them. */
export interface Row {
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

/** Every published figure is rounded to the nearest multiple of this. */
export const ROUNDING = 10;
/** What a figure below [`ROUNDING`] reads instead of a number. */
export const UNDER = "under 10";

/** A published figure: a multiple of [`ROUNDING`], or [`UNDER`]. */
export type Rounded = number | typeof UNDER;

/** `count` rounded to the nearest 10, halves up; a count from 0 to 9 reads "under 10". */
export function rounded(count: number): Rounded {
  return count < ROUNDING ? UNDER : Math.round(count / ROUNDING) * ROUNDING;
}

/** The one-field breakdowns. */
export const FIELDS = ["client", "forge", "mode", "os", "version"] as const;

/**
 * Each fixed field's values, every one listed every week, so that a value's presence says nothing:
 * "under 10" may be 0. An absent value reads `none`.
 */
const VALUES: Record<Exclude<(typeof FIELDS)[number], "version">, readonly string[]> = {
  client: [...contract.client, "none"],
  forge: [...contract.forge, "none"],
  mode: contract.mode,
  os: contract.os,
};

/** The label versions below [`ROUNDING`] are counted under together. */
export const OTHER_VERSION = "other";

/** What `/v1/stats` publishes for one finished week. */
export type Week = ReturnType<typeof weekFigures>;

/**
 * One finished week's published figures, from that week's `reports` rows.
 *
 * Every install sends at most one report a day, so a day's reports count the installs that ran
 * that day. `installs` is the week's busiest day's count, the most installs the counts prove ran
 * that week, and `reports` sums its days. Each breakdown gives a value its own busiest day, so a
 * field's figures needn't add up to `installs`. Versions with fewer than 10 installs are counted
 * together as `other`, so no version's presence shows that it ran. Nothing is published per day,
 * per combination of values, per first week or per unsupported name.
 */
export function weekFigures(week: string, rows: Row[]) {
  // For each label, its count on each day.
  const byDay = (label: (row: Row) => string) => {
    const counts = new Map<string, Map<string, number>>();
    for (const row of rows) {
      const days = counts.get(label(row)) ?? new Map<string, number>();
      days.set(row.day, (days.get(row.day) ?? 0) + row.count);
      counts.set(label(row), days);
    }
    return counts;
  };
  const busiest = (days?: Map<string, number>) => Math.max(0, ...(days?.values() ?? []));
  const value = (row: Row, field: string) => String(row[field as keyof Row] || "none");

  const fixed = Object.fromEntries(
    Object.entries(VALUES).map(([field, values]) => {
      const counts = byDay((row) => value(row, field));
      return [field, Object.fromEntries(values.map((found) => [found, rounded(busiest(counts.get(found)))]))];
    }),
  ) as Record<keyof typeof VALUES, Record<string, Rounded>>;

  const versions = byDay((row) => row.version);
  const named = [...versions.keys()].filter((found) => busiest(versions.get(found)) >= ROUNDING).sort();
  const other = byDay((row) => (named.includes(row.version) ? "" : OTHER_VERSION)).get(OTHER_VERSION);

  return {
    week,
    installs: rounded(busiest(byDay(() => "").get(""))),
    reports: rounded(rows.reduce((sum, row) => sum + row.count, 0)),
    ...fixed,
    version: {
      ...Object.fromEntries(named.map((found) => [found, rounded(busiest(versions.get(found)))])),
      [OTHER_VERSION]: rounded(busiest(other)),
    } as Record<string, Rounded>,
  };
}
