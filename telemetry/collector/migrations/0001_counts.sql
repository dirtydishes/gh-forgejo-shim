-- Daily counters. A row is one UTC day and one combination of report values; `count` is how
-- many reports that day had them. No row holds an address, header, user agent or report body.
-- An absent value is '' (text) or -1 (forge_major). The first week is counted apart, in
-- `cohorts`, so no row ties a combination to the week an install began.
CREATE TABLE reports (
  day TEXT NOT NULL,
  mode TEXT NOT NULL,
  client TEXT NOT NULL,
  forge TEXT NOT NULL,
  forge_major INTEGER NOT NULL,
  os TEXT NOT NULL,
  arch TEXT NOT NULL,
  version TEXT NOT NULL,
  gh TEXT NOT NULL,
  count INTEGER NOT NULL,
  PRIMARY KEY (day, mode, client, forge, forge_major, os, arch, version, gh)
);

-- For each day and first week: how many reports named that week.
CREATE TABLE cohorts (
  day TEXT NOT NULL,
  first_week TEXT NOT NULL,
  count INTEGER NOT NULL,
  PRIMARY KEY (day, first_week)
);

-- For each day and unsupported name: how many reports named it, and their counts' sum.
CREATE TABLE unsupported (
  day TEXT NOT NULL,
  name TEXT NOT NULL,
  reports INTEGER NOT NULL,
  total INTEGER NOT NULL,
  PRIMARY KEY (day, name)
);
