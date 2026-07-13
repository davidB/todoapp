//! Turso persistence adapter (spec §2 M2, §7): the repository ports backed by an
//! embedded SQLite-compatible database. Components map to typed `c_*` tables;
//! `ComponentStore::get/set` bridge each value through `serde_json` so per-table
//! code only maps JSON ⟷ columns.
//!
//! ponytail: query `select` pushes only the *filter* into SQL (the O(n) scan the
//! ports otherwise pay); the tree-priority sort + breadcrumbs stay link-walks in
//! `todoapp-app`. Push the sort into a recursive CTE only if it shows up hot.

use async_trait::async_trait;
use todoapp_core::{
    BlobStore, Collection, CollectionKind, CollectionRepository, Component, ComponentStore, Date,
    DueFilter, Filter, Id, Link, LinkKind, LinkRepository, Position, Query, QueryEngine, Status,
    TaskEntityStore, Timestamp,
};
use turso::Value;

/// Schema migrations, applied in order; `PRAGMA user_version` tracks how many ran
/// (spec §6: versioned from M2). Add the next migration as a new array element.
const MIGRATIONS: &[&str] = &[
    include_str!("schema.sql"),
    include_str!("schema_002_paused_status.sql"),
    include_str!("schema_003_recurrence.sql"),
    include_str!("schema_004_issueref.sql"),
    include_str!("schema_005_timelog.sql"),
    include_str!("schema_006_archived.sql"),
    include_str!("schema_007_attachments.sql"),
    include_str!("schema_008_workspace.sql"),
];

pub struct TursoStore {
    conn: turso::Connection,
}

impl TursoStore {
    pub async fn open(path: &str) -> turso::Result<Self> {
        let db = turso::Builder::new_local(path).build().await?;
        let conn = db.connect()?;
        migrate(&conn).await?;
        Ok(Self { conn })
    }

    /// Fresh in-memory database — for tests.
    pub async fn open_memory() -> Self {
        Self::open(":memory:").await.expect("open in-memory turso")
    }
}

async fn migrate(conn: &turso::Connection) -> turso::Result<()> {
    // Read into a value and drop the `Rows` before executing: turso won't apply
    // statements on a connection that still has a live result set.
    let version = {
        let mut rows = conn.query("PRAGMA user_version", ()).await?;
        match rows.next().await? {
            Some(r) => as_int(r.get_value(0)?),
            None => 0,
        }
    };
    for (i, migration) in MIGRATIONS.iter().enumerate() {
        if (i as i64) < version {
            continue;
        }
        // Strip `--` comments (which may contain `;`) before splitting on `;`,
        // then run each statement (turso `execute` takes one at a time).
        // ponytail: assumes no `--` inside a string literal — true for this DDL.
        let sql: String = migration
            .lines()
            .map(|l| match l.find("--") {
                Some(i) => &l[..i],
                None => l,
            })
            .collect::<Vec<_>>()
            .join("\n");
        for stmt in sql.split(';') {
            let stmt = stmt.trim();
            if !stmt.is_empty() {
                conn.execute(stmt, ()).await?;
            }
        }
    }
    conn.execute(&format!("PRAGMA user_version = {}", MIGRATIONS.len()), ())
        .await?;
    Ok(())
}

// ---- value helpers --------------------------------------------------------

fn text(s: String) -> Value {
    Value::Text(s)
}

fn as_text(v: Value) -> String {
    match v {
        Value::Text(s) => s,
        _ => String::new(),
    }
}

fn as_int(v: Value) -> i64 {
    match v {
        Value::Integer(i) => i,
        Value::Real(r) => r as i64,
        _ => 0,
    }
}

fn as_real(v: Value) -> f64 {
    match v {
        Value::Real(r) => r,
        Value::Integer(i) => i as f64,
        _ => 0.0,
    }
}

/// A `serde_json` scalar (string/number/bool) → a SQL-bindable value.
fn to_value(v: serde_json::Value) -> Value {
    match v {
        serde_json::Value::String(s) => Value::Text(s),
        serde_json::Value::Number(n) => Value::Integer(n.as_i64().unwrap_or(0)),
        serde_json::Value::Bool(b) => Value::Integer(b as i64),
        _ => Value::Null,
    }
}

/// `Component::NAME` → its `c_*` table.
fn ctable(name: &str) -> &'static str {
    match name {
        "title" => "c_title",
        "status" => "c_status",
        "notes" => "c_notes",
        "schedule" => "c_schedule",
        "estimate" => "c_estimate",
        "timespent" => "c_timespent",
        "tags" => "c_tag",
        "assignments" => "c_assignment",
        "recurrence" => "c_recurrence",
        "issueref" => "c_issueref",
        "workspace" => "c_workspace",
        "timelog" => "c_timelog",
        "archived" => "c_archived",
        "attachments" => "c_attachment",
        _ => unreachable!("unknown component {name}"),
    }
}

/// Components whose value is a nested/structured shape (not a flat scalar or
/// list) are stored as one JSON-serialized TEXT column in their own table,
/// rather than exploded into per-field columns.
fn json_blob_table(name: &str) -> Option<&'static str> {
    match name {
        "recurrence" => Some("c_recurrence"),
        "issueref" => Some("c_issueref"),
        "workspace" => Some("c_workspace"),
        _ => None,
    }
}

/// Components that are pure presence-markers (no data column at all — a row
/// existing *is* the value, e.g. `Archived`).
fn marker_table(name: &str) -> Option<&'static str> {
    match name {
        "archived" => Some("c_archived"),
        _ => None,
    }
}

fn kind_str(kind: LinkKind) -> &'static str {
    match kind {
        LinkKind::Child => "child",
        LinkKind::Blocks => "blocks",
    }
}

fn parse_kind(s: &str) -> LinkKind {
    match s {
        "blocks" => LinkKind::Blocks,
        _ => LinkKind::Child,
    }
}

fn status_str(s: Status) -> &'static str {
    match s {
        Status::Draft => "draft",
        Status::Todo => "todo",
        Status::Wip => "wip",
        Status::Paused => "paused",
        Status::Done => "done",
    }
}

impl TursoStore {
    /// Single scalar column as a JSON value, or `None` if the row is absent.
    async fn scalar(
        &self,
        sql: &str,
        tid: &str,
        as_json: fn(Value) -> serde_json::Value,
    ) -> Option<serde_json::Value> {
        let mut rows = self.conn.query(sql, (tid.to_string(),)).await.unwrap();
        let row = rows.next().await.unwrap()?;
        Some(as_json(row.get_value(0).unwrap()))
    }

    /// Ids of every `child` descendant of `root` (excludes `root`), walked
    /// level by level. ponytail: turso (beta) has no recursive CTE, so the walk
    /// is iterative in Rust — same shape as the core reference scan.
    async fn descendants(&self, root: &Id) -> Vec<String> {
        let mut seen: Vec<String> = Vec::new();
        let mut frontier: Vec<String> = vec![root.0.clone()];
        while !frontier.is_empty() {
            let holes = vec!["?"; frontier.len()].join(",");
            let sql = format!("SELECT to_id FROM link WHERE kind='child' AND from_id IN ({holes})");
            let params: Vec<Value> = frontier.iter().cloned().map(text).collect();
            let mut next = Vec::new();
            {
                let mut rows = self.conn.query(&sql, params).await.unwrap();
                while let Some(r) = rows.next().await.unwrap() {
                    let id = as_text(r.get_value(0).unwrap());
                    if !seen.contains(&id) {
                        seen.push(id.clone());
                        next.push(id);
                    }
                }
            }
            frontier = next;
        }
        seen
    }

    async fn put_scalar(&self, table: &str, col: &str, tid: &str, v: serde_json::Value) {
        let sql = format!("INSERT OR REPLACE INTO {table}(task_id,{col}) VALUES (?,?)");
        self.conn
            .execute(&sql, (tid.to_string(), to_value(v)))
            .await
            .unwrap();
    }
}

#[async_trait(?Send)]
impl ComponentStore for TursoStore {
    async fn get<C: Component>(&self, id: &Id) -> Option<C> {
        let tid = id.0.as_str();
        // Nested/structured components (not a flat scalar or list) are stored
        // as one JSON-serialized TEXT column, decoded directly rather than
        // through the scalar-value bridge the rest of this match uses.
        if let Some(table) = json_blob_table(C::NAME) {
            let text = self
                .scalar(
                    &format!("SELECT data FROM {table} WHERE task_id=?"),
                    tid,
                    |v| serde_json::Value::String(as_text(v)),
                )
                .await?;
            return serde_json::from_str::<C>(text.as_str()?).ok();
        }
        if let Some(table) = marker_table(C::NAME) {
            let mut rows = self
                .conn
                .query(
                    &format!("SELECT 1 FROM {table} WHERE task_id=?"),
                    (tid.to_string(),),
                )
                .await
                .unwrap();
            return if rows.next().await.unwrap().is_some() {
                serde_json::from_value::<C>(serde_json::Value::Null).ok()
            } else {
                None
            };
        }
        let jv: serde_json::Value = match C::NAME {
            "title" => {
                self.scalar("SELECT title FROM c_title WHERE task_id=?", tid, |v| {
                    serde_json::Value::String(as_text(v))
                })
                .await?
            }
            "status" => {
                self.scalar("SELECT status FROM c_status WHERE task_id=?", tid, |v| {
                    serde_json::Value::String(as_text(v))
                })
                .await?
            }
            "notes" => {
                self.scalar("SELECT notes FROM c_notes WHERE task_id=?", tid, |v| {
                    serde_json::Value::String(as_text(v))
                })
                .await?
            }
            "schedule" => {
                self.scalar(
                    "SELECT due_date FROM c_schedule WHERE task_id=?",
                    tid,
                    |v| serde_json::Value::String(as_text(v)),
                )
                .await?
            }
            "estimate" => {
                self.scalar(
                    "SELECT eta_minutes FROM c_estimate WHERE task_id=?",
                    tid,
                    |v| serde_json::Value::Number(as_int(v).into()),
                )
                .await?
            }
            "timespent" => {
                self.scalar(
                    "SELECT minutes FROM c_timespent WHERE task_id=?",
                    tid,
                    |v| serde_json::Value::Number(as_int(v).into()),
                )
                .await?
            }
            "tags" => {
                let mut rows = self
                    .conn
                    .query(
                        "SELECT tag FROM c_tag WHERE task_id=? ORDER BY tag",
                        (tid.to_string(),),
                    )
                    .await
                    .unwrap();
                let mut tags = Vec::new();
                while let Some(r) = rows.next().await.unwrap() {
                    tags.push(serde_json::Value::String(as_text(r.get_value(0).unwrap())));
                }
                if tags.is_empty() {
                    return None;
                }
                serde_json::Value::Array(tags)
            }
            "timelog" => {
                let mut rows = self
                    .conn
                    .query(
                        "SELECT day, minutes FROM c_timelog WHERE task_id=? ORDER BY day",
                        (tid.to_string(),),
                    )
                    .await
                    .unwrap();
                let mut map = serde_json::Map::new();
                while let Some(r) = rows.next().await.unwrap() {
                    let day = as_text(r.get_value(0).unwrap());
                    let minutes = as_int(r.get_value(1).unwrap());
                    map.insert(day, serde_json::Value::Number(minutes.into()));
                }
                if map.is_empty() {
                    return None;
                }
                serde_json::Value::Object(map)
            }
            "assignments" => {
                let mut rows = self
                    .conn
                    .query(
                        "SELECT actor_id, claimed FROM c_assignment WHERE task_id=? ORDER BY actor_id",
                        (tid.to_string(),),
                    )
                    .await
                    .unwrap();
                let mut asg = Vec::new();
                while let Some(r) = rows.next().await.unwrap() {
                    let actor = as_text(r.get_value(0).unwrap());
                    let claimed = as_int(r.get_value(1).unwrap()) != 0;
                    asg.push(serde_json::json!({ "actor": actor, "claimed": claimed }));
                }
                if asg.is_empty() {
                    return None;
                }
                serde_json::Value::Array(asg)
            }
            "attachments" => {
                let mut rows = self
                    .conn
                    .query(
                        "SELECT attachment_id, kind, title, url, blob_id, mime FROM c_attachment \
                         WHERE task_id=? ORDER BY attachment_id",
                        (tid.to_string(),),
                    )
                    .await
                    .unwrap();
                let mut atts = Vec::new();
                while let Some(r) = rows.next().await.unwrap() {
                    let opt_text = |v: Value| match v {
                        Value::Text(s) => serde_json::Value::String(s),
                        _ => serde_json::Value::Null,
                    };
                    atts.push(serde_json::json!({
                        "id": as_text(r.get_value(0).unwrap()),
                        "kind": as_text(r.get_value(1).unwrap()),
                        "title": as_text(r.get_value(2).unwrap()),
                        "url": opt_text(r.get_value(3).unwrap()),
                        "blob": opt_text(r.get_value(4).unwrap()),
                        "mime": opt_text(r.get_value(5).unwrap()),
                    }));
                }
                if atts.is_empty() {
                    return None;
                }
                serde_json::Value::Array(atts)
            }
            _ => return None,
        };
        serde_json::from_value::<C>(jv).ok()
    }

    async fn set<C: Component>(&self, id: &Id, value: C) {
        let tid = id.0.clone();
        if let Some(table) = json_blob_table(C::NAME) {
            let json = serde_json::to_string(&value).unwrap();
            self.put_scalar(table, "data", &tid, serde_json::Value::String(json))
                .await;
            return;
        }
        if let Some(table) = marker_table(C::NAME) {
            self.conn
                .execute(
                    &format!("INSERT OR REPLACE INTO {table}(task_id) VALUES (?)"),
                    (tid,),
                )
                .await
                .unwrap();
            return;
        }
        let v = serde_json::to_value(&value).unwrap();
        match C::NAME {
            "title" => self.put_scalar("c_title", "title", &tid, v).await,
            "status" => self.put_scalar("c_status", "status", &tid, v).await,
            "notes" => self.put_scalar("c_notes", "notes", &tid, v).await,
            "schedule" => self.put_scalar("c_schedule", "due_date", &tid, v).await,
            "estimate" => self.put_scalar("c_estimate", "eta_minutes", &tid, v).await,
            "timespent" => self.put_scalar("c_timespent", "minutes", &tid, v).await,
            "tags" => {
                self.conn
                    .execute("DELETE FROM c_tag WHERE task_id=?", (tid.clone(),))
                    .await
                    .unwrap();
                if let serde_json::Value::Array(arr) = v {
                    for t in arr {
                        if let Some(s) = t.as_str() {
                            self.conn
                                .execute(
                                    "INSERT OR REPLACE INTO c_tag(task_id,tag) VALUES (?,?)",
                                    (tid.clone(), s.to_string()),
                                )
                                .await
                                .unwrap();
                        }
                    }
                }
            }
            "timelog" => {
                self.conn
                    .execute("DELETE FROM c_timelog WHERE task_id=?", (tid.clone(),))
                    .await
                    .unwrap();
                if let serde_json::Value::Object(map) = v {
                    for (day, minutes) in map {
                        let m = minutes.as_i64().unwrap_or(0);
                        self.conn
                            .execute(
                                "INSERT OR REPLACE INTO c_timelog(task_id,day,minutes) VALUES (?,?,?)",
                                (tid.clone(), day, m),
                            )
                            .await
                            .unwrap();
                    }
                }
            }
            "assignments" => {
                self.conn
                    .execute("DELETE FROM c_assignment WHERE task_id=?", (tid.clone(),))
                    .await
                    .unwrap();
                if let serde_json::Value::Array(arr) = v {
                    for a in arr {
                        let actor = a
                            .get("actor")
                            .and_then(|x| x.as_str())
                            .unwrap_or("")
                            .to_string();
                        let claimed = a.get("claimed").and_then(|x| x.as_bool()).unwrap_or(false);
                        self.conn
                            .execute(
                                "INSERT OR REPLACE INTO c_assignment(task_id,actor_id,claimed) VALUES (?,?,?)",
                                (tid.clone(), actor, claimed as i64),
                            )
                            .await
                            .unwrap();
                    }
                }
            }
            "attachments" => {
                self.conn
                    .execute("DELETE FROM c_attachment WHERE task_id=?", (tid.clone(),))
                    .await
                    .unwrap();
                if let serde_json::Value::Array(arr) = v {
                    for a in arr {
                        let str_field =
                            |k: &str| a.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
                        let opt_field = |k: &str| -> Value {
                            a.get(k)
                                .and_then(|x| x.as_str())
                                .map_or(Value::Null, |s| Value::Text(s.to_string()))
                        };
                        self.conn
                            .execute(
                                "INSERT OR REPLACE INTO \
                                 c_attachment(task_id,attachment_id,kind,title,url,blob_id,mime) \
                                 VALUES (?,?,?,?,?,?,?)",
                                (
                                    tid.clone(),
                                    str_field("id"),
                                    str_field("kind"),
                                    str_field("title"),
                                    opt_field("url"),
                                    opt_field("blob"),
                                    opt_field("mime"),
                                ),
                            )
                            .await
                            .unwrap();
                    }
                }
            }
            _ => {}
        }
    }

    async fn remove<C: Component>(&self, id: &Id) {
        let sql = format!("DELETE FROM {} WHERE task_id=?", ctable(C::NAME));
        self.conn.execute(&sql, (id.0.clone(),)).await.unwrap();
    }
}

#[async_trait(?Send)]
impl TaskEntityStore for TursoStore {
    async fn create(&self, id: &Id, created: Timestamp, updated: Timestamp) {
        self.conn
            .execute(
                "INSERT OR REPLACE INTO task(id,created_at,updated_at) VALUES (?,?,?)",
                (
                    id.0.clone(),
                    created.as_millisecond(),
                    updated.as_millisecond(),
                ),
            )
            .await
            .unwrap();
    }

    async fn touch(&self, id: &Id, updated: Timestamp) {
        self.conn
            .execute(
                "UPDATE task SET updated_at=? WHERE id=?",
                (updated.as_millisecond(), id.0.clone()),
            )
            .await
            .unwrap();
    }

    async fn meta(&self, id: &Id) -> Option<(Timestamp, Timestamp)> {
        let mut rows = self
            .conn
            .query(
                "SELECT created_at, updated_at FROM task WHERE id=?",
                (id.0.clone(),),
            )
            .await
            .unwrap();
        let row = rows.next().await.unwrap()?;
        Some((
            Timestamp::from_millisecond(as_int(row.get_value(0).unwrap())),
            Timestamp::from_millisecond(as_int(row.get_value(1).unwrap())),
        ))
    }

    async fn delete(&self, id: &Id) {
        let tid = id.0.clone();
        for t in [
            "c_title",
            "c_status",
            "c_notes",
            "c_schedule",
            "c_estimate",
            "c_timespent",
            "c_tag",
            "c_assignment",
            "c_recurrence",
            "c_issueref",
            "c_workspace",
            "c_timelog",
            "c_archived",
            "c_attachment",
        ] {
            self.conn
                .execute(&format!("DELETE FROM {t} WHERE task_id=?"), (tid.clone(),))
                .await
                .unwrap();
        }
        self.conn
            .execute("DELETE FROM task WHERE id=?", (tid,))
            .await
            .unwrap();
    }

    async fn all(&self) -> Vec<Id> {
        let rows = self.conn.query("SELECT id FROM task", ()).await.unwrap();
        collect_rows(rows, |r| Id::new(as_text(r.get_value(0).unwrap()))).await
    }
}

async fn collect_rows<T>(mut rows: turso::Rows, f: impl Fn(&turso::Row) -> T) -> Vec<T> {
    let mut out = Vec::new();
    while let Some(r) = rows.next().await.unwrap() {
        out.push(f(&r));
    }
    out
}

fn row_to_link(r: &turso::Row) -> Link {
    Link {
        from: Id::new(as_text(r.get_value(0).unwrap())),
        to: Id::new(as_text(r.get_value(1).unwrap())),
        kind: parse_kind(&as_text(r.get_value(2).unwrap())),
        position: Position(as_real(r.get_value(3).unwrap())),
    }
}

#[async_trait(?Send)]
impl LinkRepository for TursoStore {
    async fn put(&self, link: Link) {
        self.conn
            .execute(
                "INSERT OR REPLACE INTO link(from_id,to_id,kind,position) VALUES (?,?,?,?)",
                (
                    link.from.0.clone(),
                    link.to.0.clone(),
                    kind_str(link.kind).to_string(),
                    link.position.0,
                ),
            )
            .await
            .unwrap();
    }

    async fn remove(&self, from: &Id, to: &Id, kind: LinkKind) {
        self.conn
            .execute(
                "DELETE FROM link WHERE from_id=? AND to_id=? AND kind=?",
                (from.0.clone(), to.0.clone(), kind_str(kind).to_string()),
            )
            .await
            .unwrap();
    }

    async fn outgoing(&self, from: &Id, kind: LinkKind) -> Vec<Link> {
        let rows = self
            .conn
            .query(
                "SELECT from_id,to_id,kind,position FROM link WHERE from_id=? AND kind=? ORDER BY position ASC",
                (from.0.clone(), kind_str(kind).to_string()),
            )
            .await
            .unwrap();
        collect_rows(rows, row_to_link).await
    }

    async fn incoming(&self, to: &Id, kind: LinkKind) -> Vec<Link> {
        let rows = self
            .conn
            .query(
                "SELECT from_id,to_id,kind,position FROM link WHERE to_id=? AND kind=?",
                (to.0.clone(), kind_str(kind).to_string()),
            )
            .await
            .unwrap();
        collect_rows(rows, row_to_link).await
    }
}

fn row_to_collection(r: &turso::Row) -> Collection {
    let kind = match as_text(r.get_value(2).unwrap()).as_str() {
        "query" => CollectionKind::Query,
        _ => CollectionKind::Tree,
    };
    let spec = match r.get_value(3).unwrap() {
        Value::Text(s) => serde_json::from_str::<Query>(&s).ok(),
        _ => None,
    };
    Collection {
        id: Id::new(as_text(r.get_value(0).unwrap())),
        name: as_text(r.get_value(1).unwrap()),
        kind,
        spec,
    }
}

#[async_trait(?Send)]
impl CollectionRepository for TursoStore {
    async fn save(&self, collection: Collection) {
        let kind = match collection.kind {
            CollectionKind::Tree => "tree",
            CollectionKind::Query => "query",
        };
        let spec = match collection.spec.as_ref() {
            Some(q) => Value::Text(serde_json::to_string(q).unwrap()),
            None => Value::Null,
        };
        self.conn
            .execute(
                "INSERT OR REPLACE INTO collection(id,name,kind,spec) VALUES (?,?,?,?)",
                (
                    collection.id.0.clone(),
                    collection.name.clone(),
                    kind.to_string(),
                    spec,
                ),
            )
            .await
            .unwrap();
    }

    async fn get(&self, id: &Id) -> Option<Collection> {
        let mut rows = self
            .conn
            .query(
                "SELECT id,name,kind,spec FROM collection WHERE id=?",
                (id.0.clone(),),
            )
            .await
            .unwrap();
        let row = rows.next().await.unwrap()?;
        Some(row_to_collection(&row))
    }

    async fn by_name(&self, name: &str) -> Option<Collection> {
        let mut rows = self
            .conn
            .query(
                "SELECT id,name,kind,spec FROM collection WHERE name=? LIMIT 1",
                (name.to_string(),),
            )
            .await
            .unwrap();
        let row = rows.next().await.unwrap()?;
        Some(row_to_collection(&row))
    }

    async fn all(&self) -> Vec<Collection> {
        let rows = self
            .conn
            .query("SELECT id,name,kind,spec FROM collection", ())
            .await
            .unwrap();
        collect_rows(rows, row_to_collection).await
    }
}

#[async_trait(?Send)]
impl QueryEngine for TursoStore {
    /// The filter half of evaluation, pushed into a SQL `WHERE` (spec §7). Sort +
    /// breadcrumbs are the caller's (`todoapp-app`) job.
    async fn select(&self, filter: &Filter, today: Date) -> Vec<Id> {
        let mut clauses: Vec<String> = Vec::new();
        let mut params: Vec<Value> = Vec::new();

        if let Some(root) = &filter.within {
            let subtree = self.descendants(root).await;
            if subtree.is_empty() {
                clauses.push("0".into()); // empty subtree → matches nothing
            } else {
                let holes = vec!["?"; subtree.len()].join(",");
                clauses.push(format!("t.id IN ({holes})"));
                for id in subtree {
                    params.push(text(id));
                }
            }
        }
        if let Some(txt) = &filter.text {
            clauses.push(
                "instr(lower(\
                   coalesce((SELECT title FROM c_title WHERE task_id=t.id),'')||' '||\
                   coalesce((SELECT notes FROM c_notes WHERE task_id=t.id),'')\
                 ), ?) > 0"
                    .into(),
            );
            params.push(text(txt.to_lowercase()));
        }
        if !filter.status.is_empty() {
            let holes = vec!["?"; filter.status.len()].join(",");
            clauses.push(format!(
                "EXISTS (SELECT 1 FROM c_status WHERE task_id=t.id AND status IN ({holes}))"
            ));
            for s in &filter.status {
                params.push(text(status_str(*s).to_string()));
            }
        }
        if let Some(a) = &filter.assignee {
            clauses.push(
                "EXISTS (SELECT 1 FROM c_assignment WHERE task_id=t.id AND actor_id=?)".into(),
            );
            params.push(text(a.0.clone()));
        }
        if let Some(claimed) = filter.claimed {
            clauses.push(if claimed {
                "EXISTS (SELECT 1 FROM c_assignment WHERE task_id=t.id AND claimed=1)".into()
            } else {
                "NOT EXISTS (SELECT 1 FROM c_assignment WHERE task_id=t.id AND claimed=1)".into()
            });
        }
        for tag in &filter.tags {
            clauses.push("EXISTS (SELECT 1 FROM c_tag WHERE task_id=t.id AND tag=?)".into());
            params.push(text(tag.clone()));
        }
        if let Some(due) = &filter.due {
            let (op, val) = match due {
                DueFilter::Today => ("=", today.to_string()),
                DueFilter::Overdue => ("<", today.to_string()),
                DueFilter::Before(x) => ("<", x.to_string()),
                DueFilter::On(x) => ("=", x.to_string()),
                DueFilter::After(x) => (">", x.to_string()),
            };
            // `due_date` may now hold "YYYY-MM-DD" or "YYYY-MM-DD HH:MM" (a
            // rendez-vous time is display-only) — compare the date prefix
            // only, so overdue/due-today stay day-granularity regardless.
            clauses.push(format!(
                "EXISTS (SELECT 1 FROM c_schedule WHERE task_id=t.id AND substr(due_date,1,10) {op} ?)"
            ));
            params.push(text(val));
        }
        if let Some(archived) = filter.archived {
            clauses.push(if archived {
                "EXISTS (SELECT 1 FROM c_archived WHERE task_id=t.id)".into()
            } else {
                "NOT EXISTS (SELECT 1 FROM c_archived WHERE task_id=t.id)".into()
            });
        }

        let mut sql = "SELECT t.id FROM task t".to_string();
        if !clauses.is_empty() {
            sql.push_str(" WHERE ");
            sql.push_str(&clauses.join(" AND "));
        }

        let rows = self.conn.query(&sql, params).await.unwrap();
        collect_rows(rows, |r| Id::new(as_text(r.get_value(0).unwrap()))).await
    }
}

#[async_trait(?Send)]
impl BlobStore for TursoStore {
    async fn put(&self, bytes: Vec<u8>) -> Id {
        let id = Id::for_blob(&bytes);
        self.conn
            .execute(
                "INSERT OR REPLACE INTO blob(id,content,created_at) VALUES (?,?,?)",
                (id.0.clone(), Value::Blob(bytes), 0i64),
            )
            .await
            .unwrap();
        id
    }

    async fn get(&self, id: &Id) -> Option<Vec<u8>> {
        let mut rows = self
            .conn
            .query("SELECT content FROM blob WHERE id=?", (id.0.clone(),))
            .await
            .unwrap();
        let row = rows.next().await.unwrap()?;
        match row.get_value(0).unwrap() {
            Value::Blob(b) => Some(b),
            _ => None,
        }
    }

    async fn remove(&self, id: &Id) {
        self.conn
            .execute("DELETE FROM blob WHERE id=?", (id.0.clone(),))
            .await
            .unwrap();
    }
}
