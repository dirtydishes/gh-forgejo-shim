import { daysBefore, isoWeek } from "./week";

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
  first_week: string;
  count: number;
}

/** One `unsupported` row. */
export interface UnsupportedRow {
  day: string;
  name: string;
  reports: number;
  total: number;
}

/**
 * Any published count below this is left out, with its labels, so no cell can describe one or two
 * installs. It applies to every cell: totals, one-field totals, combinations, retention and
 * unsupported names.
 */
export const MINIMUM = 3;
/** Days of daily figures `/v1/stats` publishes. */
export const DAYS = 90;
/** Weeks of weekly figures `/v1/stats` publishes. */
export const WEEKS = 26;

/** The one-field breakdowns. */
export const FIELDS = ["client", "forge", "mode", "os", "version"] as const;
/** The combination's fields. */
const COMBINATION = ["mode", "client", "forge", "forge_major", "os", "arch", "version", "gh"] as const;

type Labels = Record<string, string | number>;

/**
 * Every install sends at most one report a day, so a day's reports count the installs that ran
 * that day. A week's `reports` sums its days; its `installs` is its busiest day's, the most installs
 * the counts can prove ran that week. An absent value reads `none`.
 */
export function stats(rows: Row[], unsupported: UnsupportedRow[], today: string) {
  const firstDay = daysBefore(today, DAYS - 1);
  const firstWeek = isoWeek(daysBefore(today, WEEKS * 7 - 1));
  const label = (row: Row, field: string): string | number => {
    const value = row[field as keyof Row];
    return value === "" || value === -1 ? "none" : value;
  };
  const pick = (row: Row, fields: readonly string[]): Labels =>
    Object.fromEntries(fields.map((field) => [field, label(row, field)]));

  /** Daily and weekly figures for each distinct value of `fields`. */
  const breakdown = (fields: readonly string[]) => {
    const daily = new Map<string, { day: string; labels: Labels; reports: number }>();
    for (const row of rows) {
      const labels = pick(row, fields);
      const key = JSON.stringify([row.day, labels]);
      const cell = daily.get(key) ?? { day: row.day, labels, reports: 0 };
      cell.reports += row.count;
      daily.set(key, cell);
    }
    const weekly = new Map<string, { week: string; labels: Labels; reports: number; installs: number }>();
    for (const cell of daily.values()) {
      const week = isoWeek(cell.day);
      const key = JSON.stringify([week, cell.labels]);
      const total = weekly.get(key) ?? { week, labels: cell.labels, reports: 0, installs: 0 };
      total.reports += cell.reports;
      total.installs = Math.max(total.installs, cell.reports);
      weekly.set(key, total);
    }
    return {
      days: [...daily.values()]
        .filter((cell) => cell.day >= firstDay && cell.reports >= MINIMUM)
        .map((cell) => ({ day: cell.day, ...cell.labels, installs: cell.reports, reports: cell.reports }))
        .sort(order),
      weeks: [...weekly.values()]
        .filter((cell) => cell.week >= firstWeek && cell.installs >= MINIMUM)
        .map((cell) => ({ week: cell.week, ...cell.labels, installs: cell.installs, reports: cell.reports }))
        .sort(order),
    };
  };

  // Retention: for each first week, the installs the counts prove ran in each later week.
  const cohortDays = new Map<string, number>();
  for (const row of rows) {
    const key = JSON.stringify([row.first_week, row.day]);
    cohortDays.set(key, (cohortDays.get(key) ?? 0) + row.count);
  }
  const cohorts = new Map<string, { first_week: string; week: string; installs: number }>();
  for (const [key, reports] of cohortDays) {
    const [first_week, day] = JSON.parse(key) as [string, string];
    const week = isoWeek(day);
    const cell = cohorts.get(`${first_week} ${week}`) ?? { first_week, week, installs: 0 };
    cell.installs = Math.max(cell.installs, reports);
    cohorts.set(`${first_week} ${week}`, cell);
  }

  const names = new Map<string, { week: string; name: string; reports: number; total: number }>();
  for (const row of unsupported) {
    const week = isoWeek(row.day);
    const cell = names.get(`${week} ${row.name}`) ?? { week, name: row.name, reports: 0, total: 0 };
    cell.reports += row.reports;
    cell.total += row.total;
    names.set(`${week} ${row.name}`, cell);
  }

  return {
    generated: today,
    minimum_count: MINIMUM,
    totals: breakdown([]),
    by_field: Object.fromEntries(FIELDS.map((field) => [field, breakdown([field])])),
    combinations: breakdown(COMBINATION),
    retention: [...cohorts.values()]
      .filter((cell) => cell.week >= firstWeek && cell.installs >= MINIMUM)
      .sort(order),
    // A week's count of reports that named each one, and the times they counted it.
    unsupported: [...names.values()]
      .filter((cell) => cell.week >= firstWeek && cell.reports >= MINIMUM)
      .sort(order),
  };
}

/** Oldest first, then by the cell's labels, so the output is stable. */
function order(a: object, b: object): number {
  const key = (cell: object) =>
    JSON.stringify(Object.entries(cell).filter(([name]) => name !== "installs" && name !== "reports" && name !== "total"));
  return key(a) < key(b) ? -1 : key(a) > key(b) ? 1 : 0;
}
