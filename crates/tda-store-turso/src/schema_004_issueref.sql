-- `IssueRef` capability (spec §3): a static external issue-tracker reference,
-- stored as one JSON-serialized TEXT column (provider/id/url).
CREATE TABLE c_issueref ( task_id TEXT PRIMARY KEY REFERENCES task(id), data TEXT NOT NULL );
