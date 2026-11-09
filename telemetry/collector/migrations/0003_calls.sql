-- Schema 2's counts, by the agent whose gh call they came from. For each day, client, kind and
-- name: how many reports counted it, and their counts' sum. `kind` is 'unsupported' (the gateway
-- lacked the feature) or 'failed' (it has the feature and failed on its own side). `client` is ''
-- for fga-server, which can't tell agents apart. Schema 1's counts, which name no agent, stay in
-- `unsupported`.
CREATE TABLE calls (
  day TEXT NOT NULL,
  client TEXT NOT NULL,
  kind TEXT NOT NULL,
  name TEXT NOT NULL,
  reports INTEGER NOT NULL,
  total INTEGER NOT NULL,
  PRIMARY KEY (day, client, kind, name)
);
