-- `Workspace` capability (spec §3): binds a task subtree to a project
-- folder/repo, stored as one JSON-serialized TEXT column (name/path).
CREATE TABLE c_workspace ( task_id TEXT PRIMARY KEY REFERENCES task(id), data TEXT NOT NULL );
