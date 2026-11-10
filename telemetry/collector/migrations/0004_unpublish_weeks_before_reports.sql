-- The first collector worked out figures for the 26 weeks before the first report, 2026-W20 to
-- 2026-W45, from counters that were all empty, so they carried nothing but noise. The collector
-- now starts at the first report's week, so these are never worked out again.
DELETE FROM published WHERE week < '2026-W46';
