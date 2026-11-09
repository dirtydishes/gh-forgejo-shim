import contract from "../../../contract/telemetry.json";

/**
 * One day's count for one value of one field, as the cron's query sums the `reports` counters.
 * `field` is "" for the day's total, and an absent value is "".
 */
export interface Sum {
  field: string;
  value: string;
  day: string;
  n: number;
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
 * One finished week's published figures, from that week's [`Sum`]s.
 *
 * Every install sends at most one report a day, so a day's reports count the installs that ran
 * that day. `installs` is the week's busiest day's count, the most installs the counts prove ran
 * that week, and each breakdown gives a value its own busiest day. Nothing published says which
 * day that was or how many days had reports, so a figure can't be multiplied out into a sum of
 * days, and a field's figures needn't add up to `installs`. Versions with fewer than 10 installs
 * are counted together as `other`, so no version's presence shows that it ran. Nothing is
 * published per day, per combination of values, per first week or per unsupported name.
 */
export function weekFigures(week: string, sums: Sum[]) {
  // For each field and value, its count on each day.
  const counts = new Map<string, Map<string, Map<string, number>>>();
  for (const { field, value, day, n } of sums) {
    const values = counts.get(field) ?? new Map<string, Map<string, number>>();
    const days = values.get(value || "none") ?? new Map<string, number>();
    days.set(day, (days.get(day) ?? 0) + n);
    values.set(value || "none", days);
    counts.set(field, values);
  }
  const busiest = (days?: Map<string, number>) => Math.max(0, ...(days?.values() ?? []));
  const of = (field: string) => counts.get(field) ?? new Map<string, Map<string, number>>();

  const fixed = Object.fromEntries(
    Object.entries(VALUES).map(([field, values]) => [
      field,
      Object.fromEntries(values.map((found) => [found, rounded(busiest(of(field).get(found)))])),
    ]),
  ) as Record<keyof typeof VALUES, Record<string, Rounded>>;

  const versions = of("version");
  const named = [...versions.keys()].filter((found) => busiest(versions.get(found)) >= ROUNDING).sort();
  const other = new Map<string, number>();
  for (const [found, days] of versions) {
    if (busiest(days) >= ROUNDING) continue;
    for (const [day, n] of days) other.set(day, (other.get(day) ?? 0) + n);
  }

  return {
    week,
    // The day's total has field "" and value "", read as none.
    installs: rounded(busiest(of("").get("none"))),
    reports: rounded([...(of("").get("none")?.values() ?? [])].reduce((sum, n) => sum + n, 0)),
    ...fixed,
    version: {
      ...Object.fromEntries(named.map((found) => [found, rounded(busiest(versions.get(found)))])),
      [OTHER_VERSION]: rounded(busiest(other)),
    } as Record<string, Rounded>,
  };
}
