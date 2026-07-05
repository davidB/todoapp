-- `TimeLog` capability (spec §3): a per-day breakdown of time spent, one row
-- per (task, day) — same shape as `c_tag`.
CREATE TABLE c_timelog ( task_id TEXT NOT NULL REFERENCES task(id), day TEXT NOT NULL,
                         minutes INTEGER NOT NULL, PRIMARY KEY (task_id, day) );
