-- spec §7. Minimal `task` entity + one component table per capability (presence
-- of a row IS the capability). The `actor` table is omitted until a port needs
-- it (M5); `c_assignment.actor_id` is plain TEXT. FK references document intent;
-- delete cascade is done explicitly in the adapter (no reliance on PRAGMA fk).

CREATE TABLE task (
  id          TEXT PRIMARY KEY,
  created_at  INTEGER NOT NULL,
  updated_at  INTEGER NOT NULL
);

CREATE TABLE c_title    ( task_id TEXT PRIMARY KEY REFERENCES task(id), title TEXT NOT NULL );
CREATE TABLE c_status   ( task_id TEXT PRIMARY KEY REFERENCES task(id),
                          status TEXT NOT NULL CHECK (status IN ('draft','todo','wip','done')) );
CREATE TABLE c_notes    ( task_id TEXT PRIMARY KEY REFERENCES task(id), notes TEXT NOT NULL );
CREATE TABLE c_schedule ( task_id TEXT PRIMARY KEY REFERENCES task(id), due_date TEXT NOT NULL );
CREATE TABLE c_estimate ( task_id TEXT PRIMARY KEY REFERENCES task(id), eta_minutes INTEGER NOT NULL );
CREATE TABLE c_timespent( task_id TEXT PRIMARY KEY REFERENCES task(id), minutes INTEGER NOT NULL );

CREATE TABLE c_tag        ( task_id TEXT NOT NULL REFERENCES task(id), tag TEXT NOT NULL,
                            PRIMARY KEY (task_id, tag) );
CREATE INDEX tag_by_name ON c_tag(tag);
CREATE TABLE c_assignment ( task_id TEXT NOT NULL REFERENCES task(id), actor_id TEXT NOT NULL,
                            claimed INTEGER NOT NULL, PRIMARY KEY (task_id, actor_id) );

CREATE TABLE link (
  from_id   TEXT NOT NULL,
  to_id     TEXT NOT NULL,
  kind      TEXT NOT NULL CHECK (kind IN ('child','blocks')),
  position  REAL NOT NULL,
  PRIMARY KEY (from_id, to_id, kind)
);
CREATE INDEX link_from ON link(from_id, kind, position);
CREATE INDEX link_to   ON link(to_id, kind);
CREATE UNIQUE INDEX one_parent ON link(to_id) WHERE kind = 'child';

CREATE TABLE collection (
  id    TEXT PRIMARY KEY,
  name  TEXT NOT NULL,
  kind  TEXT NOT NULL CHECK (kind IN ('tree','query')),
  spec  TEXT
);
