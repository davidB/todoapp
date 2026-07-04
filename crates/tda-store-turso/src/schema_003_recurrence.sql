-- `Recurrence` capability (spec §3): a nested rule (cycle variant + optional
-- time), stored as one JSON-serialized TEXT column rather than exploded into
-- per-variant columns.
CREATE TABLE c_recurrence ( task_id TEXT PRIMARY KEY REFERENCES task(id), data TEXT NOT NULL );
