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

/**
 * The noise's scale. Each figure's noise is a whole number k, drawn with probability in proportion
 * to e^(-|k| / NOISE_SCALE): 0 is likeliest, and about three draws in four fall within ±8. One
 * report moves at most six figures (installs and one value of each field) by one each, so it moves
 * the odds of what they show together by at most e^(6 / NOISE_SCALE), under 3 times, except when it
tips its version over 10 and so moves that version's count out of `other`.
 */
export const NOISE_SCALE = 6;

/** A uniform number in (0, 1]. */
export type Uniform = () => number;

const cryptoUniform: Uniform = () => (crypto.getRandomValues(new Uint32Array(1))[0] + 1) / 2 ** 32;

/**
 * One figure's noise, from the two-sided geometric distribution: the difference of two geometric
 * draws, each at least k with probability e^(-k / NOISE_SCALE).
 */
export function noise(uniform: Uniform = cryptoUniform): number {
  const geometric = () => Math.floor(Math.log(uniform()) * -NOISE_SCALE);
  return geometric() - geometric();
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
 *
 * Each figure gets its own [`noise`] before it's rounded, and whether a version is named depends on
 * its count with noise. A sender who knows how many of a figure's reports are theirs can't place it
 * at a rounding line, since the noise moves that line by more than one report. `draw` is for tests.
 */
export function weekFigures(week: string, sums: Sum[], draw: () => number = noise) {
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
  const shown = (count: number) => rounded(count + draw());

  const fixed = Object.fromEntries(
    Object.entries(VALUES).map(([field, values]) => [
      field,
      Object.fromEntries(values.map((found) => [found, shown(busiest(of(field).get(found)))])),
    ]),
  ) as Record<keyof typeof VALUES, Record<string, Rounded>>;

  const versions = of("version");
  const noisy = new Map([...versions].map(([found, days]) => [found, busiest(days) + draw()]));
  const named = [...noisy.keys()].filter((found) => noisy.get(found)! >= ROUNDING).sort();
  const other = new Map<string, number>();
  for (const [found, days] of versions) {
    if (noisy.get(found)! >= ROUNDING) continue;
    for (const [day, n] of days) other.set(day, (other.get(day) ?? 0) + n);
  }

  return {
    week,
    // The day's total has field "" and value "", read as none.
    installs: shown(busiest(of("").get("none"))),
    ...fixed,
    version: {
      ...Object.fromEntries(named.map((found) => [found, rounded(noisy.get(found)!)])),
      [OTHER_VERSION]: shown(busiest(other)),
    } as Record<string, Rounded>,
  };
}
