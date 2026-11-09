# The usage-report collector

A Cloudflare Worker with a D1 database. [docs/telemetry.md](../../docs/telemetry.md) says what a
report holds, what the collector keeps and what it publishes.

- `POST /v1/report` checks a report against [`contract/telemetry.json`](../../contract/telemetry.json)
  and adds one to that day's counters.
- `GET /v1/stats` publishes each finished week's one-field totals, with noise and rounded.
- A daily cron works out each finished week's totals once, stores them, and deletes counters older
  than 400 days.

`npm test` runs the tests in the Workers runtime against a local D1. `npm run check` typechecks.

## Reading the detailed counts

The counters in D1 are private: only the maintainer reads them, straight from the database. Run a
query with:

```sh
npx wrangler d1 execute gfj-telemetry --remote --command "<query>"
```

`--remote` reads the deployed database; without it, wrangler reads a local copy. Each query
groups days into ISO weeks, labelled by their Monday (`week_of`). A day's reports count the installs
that ran that day, so a week's `installs` is its busiest day's.

Installs and reports per week:

```sql
SELECT week_of, MAX(n) AS installs, SUM(n) AS reports FROM (
  SELECT date(day, 'weekday 0', '-6 days') AS week_of, day, SUM(count) AS n
  FROM reports GROUP BY day
) GROUP BY week_of ORDER BY week_of;
```

Installs per week by client. An empty `client` is fga-server. For other fields, put `forge,
forge_major`, `version`, `os`, `arch`, `gh` or `mode` in place of `client`, or several at once:

```sql
SELECT week_of, client, MAX(n) AS installs FROM (
  SELECT date(day, 'weekday 0', '-6 days') AS week_of, day, client, SUM(count) AS n
  FROM reports GROUP BY day, client
) GROUP BY week_of, client ORDER BY week_of, installs DESC;
```

Retention: for each first week, the installs still running in each later week:

```sql
SELECT first_week, week_of, MAX(n) AS installs FROM (
  SELECT date(day, 'weekday 0', '-6 days') AS week_of, day, first_week, SUM(count) AS n
  FROM cohorts GROUP BY day, first_week
) GROUP BY first_week, week_of ORDER BY first_week, week_of;
```

What broke, and in which agent: per week, for each agent, kind and name, the installs that counted
it (its busiest day's reports) and how many times. `kind` is `unsupported` (the gateway lacks the
feature) or `failed` (it has the feature and failed on its side). An empty `client` is fga-server.
These come from schema 2 reports:

```sql
SELECT date(day, 'weekday 0', '-6 days') AS week_of, client, kind, name,
  MAX(reports) AS installs, SUM(total) AS times
FROM calls GROUP BY week_of, client, kind, name ORDER BY week_of, kind, installs DESC;
```

Schema 1 reports count unsupported names without saying which agent asked, in `unsupported`:

```sql
SELECT date(day, 'weekday 0', '-6 days') AS week_of, name,
  MAX(reports) AS installs, SUM(total) AS times
FROM unsupported GROUP BY week_of, name ORDER BY week_of, installs DESC;
```

Don't publish what these queries show. The detailed counts include counts of 1 and 2, which
`/v1/stats` hides with noise and rounding.
