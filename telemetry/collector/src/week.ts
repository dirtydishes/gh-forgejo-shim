/** The UTC date of `time`, as `YYYY-MM-DD`. */
export function utcDay(time: Date): string {
  return time.toISOString().slice(0, 10);
}

/** The ISO 8601 week of a `YYYY-MM-DD` day, as `YYYY-Www`. */
export function isoWeek(day: string): string {
  const date = new Date(`${day}T00:00:00Z`);
  // Monday is 0.
  const weekday = (date.getUTCDay() + 6) % 7;
  const thursday = new Date(date.getTime() + (3 - weekday) * 86_400_000);
  const year = thursday.getUTCFullYear();
  const january1 = Date.UTC(year, 0, 1);
  const week = Math.floor((thursday.getTime() - january1) / 86_400_000 / 7) + 1;
  return `${year}-W${String(week).padStart(2, "0")}`;
}

/** The day `days` days before `day`. */
export function daysBefore(day: string, days: number): string {
  return utcDay(new Date(new Date(`${day}T00:00:00Z`).getTime() - days * 86_400_000));
}
