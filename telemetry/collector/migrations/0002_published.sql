-- What `GET /v1/stats` publishes for each finished ISO week, as JSON. Each week's figures are
-- worked out once, after the week ends, and never changed, so no two responses differ in a figure
-- that could be subtracted from another.
CREATE TABLE published (
  week TEXT PRIMARY KEY,
  figures TEXT NOT NULL
);
