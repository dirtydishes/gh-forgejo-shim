import { isoWeek } from "./week";

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

/** One `cohorts` row: a day, a first week, and how many reports that day named it. */
export interface CohortRow {
  day: string;
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

/** No published figure counts fewer installs than this. */
export const MINIMUM = 3;

/** The one-field breakdowns. */
export const FIELDS = ["client", "forge", "mode", "os", "version"] as const;
/** The combination's fields. */
const COMBINATION = ["mode", "client", "forge", "forge_major", "os", "arch", "version", "gh"] as const;

type Labels = Record<string, string | number>;

/** One figure: a table, a day or week, and its labels. */
interface Cell {
  table: string;
  period: string;
  labels: Labels;
  /** Reports: additive, so a parent's is its parts' sum. */
  reports: number;
  /** The most installs the counts prove: a day's reports, or a week's busiest day's. */
  installs: number;
  hidden: boolean;
}

/** A figure and the figures that sum to it. */
interface Group {
  parent: Cell;
  parts: Cell[];
}

/**
 * Hides every cell with fewer than [`MINIMUM`] installs, then enough others that no hidden cell can
 * be worked out by adding or subtracting published ones: wherever a published figure's parts
 * include hidden ones, at least two are hidden and they sum to [`MINIMUM`] or more, and a hidden
 * figure always has a hidden part. Each pass hides the smallest published part of a group that
 * breaks the rule, until none does.
 */
export function suppress(cells: Cell[], groups: Group[]): void {
  for (const cell of cells) cell.hidden ||= cell.installs < MINIMUM;
  for (let changed = true; changed; ) {
    changed = false;
    for (const { parent, parts } of groups) {
      const shown = parts.filter((part) => !part.hidden);
      if (shown.length === 0) continue;
      const hidden = parts.filter((part) => part.hidden);
      const sum = hidden.reduce((total, part) => total + part.reports, 0);
      const exposed = parent.hidden
        ? hidden.length === 0
        : hidden.length > 0 && (hidden.length < 2 || sum < MINIMUM);
      if (exposed) {
        shown.reduce((smallest, part) => (part.reports < smallest.reports ? part : smallest)).hidden = true;
        changed = true;
      }
    }
  }
}

/** What `/v1/stats` publishes for a set of weeks. */
export type Figures = ReturnType<typeof figures>;

/**
 * Every install sends at most one report a day, so a day's reports count the installs that ran
 * that day. A week's `reports` sums its days; its `installs` is its busiest day's, the most installs
 * the counts can prove ran that week. An absent value reads `none`.
 *
 * No sum it guards crosses from one ISO week to another, so the figures for several weeks are each
 * week's figures side by side.
 */
export function figures(rows: Row[], cohortRows: CohortRow[], unsupported: UnsupportedRow[]) {
  const label = (row: Row, field: string): string | number => {
    const value = row[field as keyof Row];
    return value === "" || value === -1 ? "none" : value;
  };
  const tables: Array<[string, readonly string[]]> = [
    ["totals", []],
    ...FIELDS.map((field): [string, readonly string[]] => [field, [field]]),
    ["combinations", COMBINATION],
  ];

  // Daily cells, then weekly ones built from them.
  const cells = new Map<string, Cell>();
  const cell = (table: string, period: string, labels: Labels, hidden: boolean): Cell => {
    const key = JSON.stringify([table, period, labels]);
    let found = cells.get(key);
    if (!found) {
      found = { table, period, labels, reports: 0, installs: 0, hidden };
      cells.set(key, found);
    }
    return found;
  };
  for (const row of rows) {
    for (const [table, fields] of tables) {
      const labels = Object.fromEntries(fields.map((field) => [field, label(row, field)]));
      const day = cell(table, row.day, labels, false);
      day.reports += row.count;
      day.installs += row.count;
    }
  }
  // Retention: each first week's reports are a part of the day's total too. Its daily cells are
  // never published, but they're counted, so a week's retention can't be subtracted from its total.
  for (const row of cohortRows) {
    const day = cell("retention", row.day, { first_week: row.first_week }, true);
    day.reports += row.count;
    day.installs += row.count;
  }
  const days = [...cells.values()];
  const weekOf = new Map<Cell, Cell>();
  for (const day of days) {
    const week = isoWeek(day.period);
    const total = cell(day.table, week, day.labels, false);
    total.reports += day.reports;
    total.installs = Math.max(total.installs, day.installs);
    weekOf.set(day, total);
  }

  // The sums a reader could subtract: each period's total over each table's cells, each one-field
  // cell over the combinations with its value, and each week's cell over its days.
  const byParent = new Map<Cell, Cell[]>();
  const add = (parent: Cell, part: Cell) => {
    const parts = byParent.get(parent) ?? [];
    parts.push(part);
    byParent.set(parent, parts);
  };
  for (const part of cells.values()) {
    if (part.table === "totals") continue;
    add(cell("totals", part.period, {}, false), part);
    if (part.table === "combinations") {
      for (const field of FIELDS) {
        add(cell(field, part.period, { [field]: part.labels[field] }, false), part);
      }
    }
  }
  for (const [day, week] of weekOf) add(week, day);
  const groups = [...byParent].map(([parent, parts]) => ({ parent, parts }));
  // Grouped by table, so a table's cells only ever sum to its own total.
  const split = groups.flatMap(({ parent, parts }) => {
    const tablesOf = new Map<string, Cell[]>();
    for (const part of parts) tablesOf.set(part.table, [...(tablesOf.get(part.table) ?? []), part]);
    return [...tablesOf.values()].map((same) => ({ parent, parts: same }));
  });
  suppress([...cells.values()], split);

  const publish = (table: string) => {
    const shown = [...cells.values()].filter((found) => found.table === table && !found.hidden);
    return {
      days: shown
        .filter((found) => found.period.length === 10)
        .map((found) => ({ day: found.period, ...found.labels, installs: found.installs, reports: found.reports }))
        .sort(order),
      weeks: shown
        .filter((found) => found.period.length === 8)
        .map((found) => ({ week: found.period, ...found.labels, installs: found.installs, reports: found.reports }))
        .sort(order),
    };
  };

  // Unsupported names: a week's installs is its busiest day's reports naming it, since one install
  // names it at most once a day; `total` sums the times they counted it.
  const names = new Map<string, { week: string; name: string; installs: number; total: number }>();
  for (const row of unsupported) {
    const week = isoWeek(row.day);
    const found = names.get(`${week} ${row.name}`) ?? { week, name: row.name, installs: 0, total: 0 };
    found.installs = Math.max(found.installs, row.reports);
    found.total += row.total;
    names.set(`${week} ${row.name}`, found);
  }

  return {
    totals: publish("totals"),
    by_field: Object.fromEntries(FIELDS.map((field) => [field, publish(field)])),
    combinations: publish("combinations"),
    // For each first week, the installs the counts prove ran in each later week.
    retention: publish("retention").weeks.map((found) => ({
      first_week: (found as Labels).first_week,
      week: found.week,
      installs: found.installs,
    })),
    unsupported: [...names.values()]
      .filter((found) => found.installs >= MINIMUM)
      .sort(order),
  };
}

/** Oldest first, then by the cell's labels, so the output is stable. */
export function order(a: object, b: object): number {
  const key = (found: object) =>
    JSON.stringify(
      Object.entries(found).filter(([name]) => !["installs", "reports", "total"].includes(name)),
    );
  return key(a) < key(b) ? -1 : key(a) > key(b) ? 1 : 0;
}
