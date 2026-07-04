-- `Attachments` capability (spec §3) + blob storage (`BlobStore` port). A
-- `LINK` attachment never has a `blob_id`; `FILE`/`IMAGE` may or may not.
CREATE TABLE blob ( id TEXT PRIMARY KEY, content BLOB NOT NULL, created_at INTEGER NOT NULL );

CREATE TABLE c_attachment (
  task_id TEXT NOT NULL REFERENCES task(id),
  attachment_id TEXT NOT NULL,
  kind TEXT NOT NULL CHECK (kind IN ('link','file','image')),
  title TEXT NOT NULL,
  url TEXT,
  blob_id TEXT REFERENCES blob(id),
  mime TEXT,
  PRIMARY KEY (task_id, attachment_id)
);
