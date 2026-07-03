-- Add `paused` to the c_status CHECK constraint. SQLite can't ALTER a CHECK
-- constraint in place, so rebuild the table (spec §8: Status gains a 5th value).
CREATE TABLE c_status_new ( task_id TEXT PRIMARY KEY REFERENCES task(id),
                            status TEXT NOT NULL CHECK (status IN ('draft','todo','wip','paused','done')) );
INSERT INTO c_status_new SELECT * FROM c_status;
DROP TABLE c_status;
ALTER TABLE c_status_new RENAME TO c_status;
