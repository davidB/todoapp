-- `Archived` capability (spec §3): an orthogonal flag, independent of
-- `Status` — presence of a row *is* the flag, no data column needed.
CREATE TABLE c_archived ( task_id TEXT PRIMARY KEY REFERENCES task(id) );
