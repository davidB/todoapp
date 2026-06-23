//! M1 use-case tests against the in-memory store (spec §10, §11).

use tda_app::{Anchor, Services};
use tda_core::{DueFilter, Filter, Id, Query, SortField, SortKey, Status};
use tda_store_mem::{FixedClock, MemStore, SeqIds};

/// Owns the ports so a borrowed `Services` can be built per call.
struct Fx {
    store: MemStore,
    clock: FixedClock,
    ids: SeqIds,
}

impl Fx {
    fn new() -> Self {
        Fx {
            store: MemStore::new(),
            clock: FixedClock::default(), // today = 2026-06-22
            ids: SeqIds::default(),
        }
    }
    fn svc(&self) -> Services<'_, MemStore> {
        Services {
            store: &self.store,
            links: &self.store,
            collections: &self.store,
            clock: &self.clock,
            ids: &self.ids,
        }
    }
}

#[tokio::test]
async fn create_and_edit() {
    let fx = Fx::new();
    let s = fx.svc();
    let t = s
        .create("Write spec", None, Status::Draft, [])
        .await
        .unwrap();
    let t = s.set_status(&t.id, Status::Todo).await.unwrap();
    let t = s.set_notes(&t.id, Some("markdown".into())).await.unwrap();
    let t = s.add_tag(&t.id, "doc").await.unwrap();
    assert_eq!(t.status, Status::Todo);
    assert_eq!(t.notes.as_deref(), Some("markdown"));
    assert!(t.tags.contains("doc"));
}

#[tokio::test]
async fn batch_create_uses_indentation_for_depth() {
    let fx = Fx::new();
    let s = fx.svc();
    let made = s
        .batch_create("Parent\n  Child A\n  Child B\n    Grandchild")
        .await
        .unwrap();
    assert_eq!(made.len(), 4);
    let parent = &made[0];
    let kids = s.children_of(&parent.id).await;
    assert_eq!(kids.len(), 2);
    // grandchild sits under the most recent depth-1 task: Child B (index 2)
    assert_eq!(s.children_of(&made[2].id).await.len(), 1);
}

#[tokio::test]
async fn move_subtree_and_reject_cycle() {
    let fx = Fx::new();
    let s = fx.svc();
    let a = s.create("A", None, Status::Todo, []).await.unwrap();
    let b = s.create("B", Some(&a.id), Status::Todo, []).await.unwrap();
    let c = s.create("C", Some(&b.id), Status::Todo, []).await.unwrap();

    // move C under A: fine
    s.move_task(&c.id, &a.id, None).await.unwrap();
    assert_eq!(s.parent_of(&c.id).await, Some(a.id.clone()));

    // move A under its own descendant B: cycle, rejected
    assert!(s.move_task(&a.id, &b.id, None).await.is_err());
}

#[tokio::test]
async fn reorder_with_anchor() {
    let fx = Fx::new();
    let s = fx.svc();
    let p = s.create("P", None, Status::Todo, []).await.unwrap();
    let x = s.create("X", Some(&p.id), Status::Todo, []).await.unwrap();
    let y = s.create("Y", Some(&p.id), Status::Todo, []).await.unwrap();
    let z = s.create("Z", Some(&p.id), Status::Todo, []).await.unwrap();
    // order is X, Y, Z; move Z before X
    s.reorder(&z.id, Anchor::Before(x.id.clone()))
        .await
        .unwrap();
    let order: Vec<Id> = s
        .children_of(&p.id)
        .await
        .into_iter()
        .map(|l| l.to)
        .collect();
    assert_eq!(order, vec![z.id, x.id, y.id]);
}

#[tokio::test]
async fn blocks_link_rejects_cycle_and_blocks_start() {
    let fx = Fx::new();
    let s = fx.svc();
    let a = s.create("A", None, Status::Todo, []).await.unwrap();
    let b = s.create("B", None, Status::Todo, []).await.unwrap();
    s.block(&a.id, &b.id).await.unwrap(); // A blocks B
    assert!(s.block(&b.id, &a.id).await.is_err()); // would cycle

    // B is blocked because blocker A is not done → cannot start
    assert!(s.is_blocked(&b.id).await);
    assert!(s.claim(&b.id, Id::new("u")).await.is_err());

    // complete A (todo→wip→done) → B unblocks
    s.set_status(&a.id, Status::Wip).await.unwrap();
    s.set_status(&a.id, Status::Done).await.unwrap();
    assert!(!s.is_blocked(&b.id).await);
    s.claim(&b.id, Id::new("u")).await.unwrap();
}

#[tokio::test]
async fn aggregate_rolls_up_subtree() {
    let fx = Fx::new();
    let s = fx.svc();
    let root = s.create("root", None, Status::Todo, []).await.unwrap();
    let a = s
        .create("a", Some(&root.id), Status::Done, [])
        .await
        .unwrap();
    let _b = s
        .create("b", Some(&root.id), Status::Todo, [])
        .await
        .unwrap();
    s.set_estimate(&a.id, Some(30)).await.unwrap();
    s.add_time_spent(&a.id, 25).await.unwrap();
    s.set_due(&a.id, Some("2026-07-01".into())).await.unwrap();
    s.set_due(&root.id, Some("2026-06-30".into()))
        .await
        .unwrap();

    let agg = s.aggregate(&root.id).await.unwrap();
    assert_eq!(agg.total, 3);
    assert_eq!(agg.done, 1);
    assert_eq!(agg.eta_minutes, 30);
    assert_eq!(agg.time_spent_minutes, 25);
    assert_eq!(agg.earliest_due.as_deref(), Some("2026-06-30"));
}

#[tokio::test]
async fn query_filters_sorts_and_breadcrumbs() {
    let fx = Fx::new();
    let s = fx.svc();
    let proj = s.create("Project", None, Status::Todo, []).await.unwrap();
    let t1 = s
        .create("first", Some(&proj.id), Status::Todo, [])
        .await
        .unwrap();
    let _t2 = s
        .create("second", Some(&proj.id), Status::Done, [])
        .await
        .unwrap();
    s.add_tag(&t1.id, "urgent").await.unwrap();

    // status:todo, tag:urgent → only t1, with breadcrumb [Project]
    let hits = s
        .evaluate(&Query {
            filter: Filter {
                status: vec![Status::Todo],
                tags: vec!["urgent".into()],
                ..Default::default()
            },
            ..Default::default()
        })
        .await;
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].task.id, t1.id);
    assert_eq!(hits[0].path, vec!["Project".to_string()]);
}

#[tokio::test]
async fn within_predicate_scopes_to_subtree() {
    let fx = Fx::new();
    let s = fx.svc();
    let under = s.create("Under", None, Status::Todo, []).await.unwrap();
    let inside = s
        .create("inside", Some(&under.id), Status::Todo, [])
        .await
        .unwrap();
    let _outside = s.create("outside", None, Status::Todo, []).await.unwrap();

    let hits = s
        .evaluate(&Query {
            filter: Filter {
                within: Some(under.id.clone()),
                ..Default::default()
            },
            ..Default::default()
        })
        .await;
    let ids: Vec<Id> = hits.into_iter().map(|h| h.task.id).collect();
    assert_eq!(ids, vec![inside.id]);
}

#[tokio::test]
async fn due_filters() {
    let fx = Fx::new();
    let s = fx.svc();
    let today = s.create("today", None, Status::Todo, []).await.unwrap();
    let past = s.create("past", None, Status::Todo, []).await.unwrap();
    s.set_due(&today.id, Some("2026-06-22".into()))
        .await
        .unwrap();
    s.set_due(&past.id, Some("2026-01-01".into()))
        .await
        .unwrap();

    let due_today = s.due_today().await;
    assert_eq!(due_today.len(), 1);
    assert_eq!(due_today[0].task.id, today.id);

    let overdue = s
        .evaluate(&Query {
            filter: Filter {
                due: Some(DueFilter::Overdue),
                ..Default::default()
            },
            ..Default::default()
        })
        .await;
    assert_eq!(overdue.len(), 1);
    assert_eq!(overdue[0].task.id, past.id);
}

#[tokio::test]
async fn what_next_is_todo_by_priority() {
    let fx = Fx::new();
    let s = fx.svc();
    let p = s.create("P", None, Status::Todo, []).await.unwrap();
    let a = s.create("a", Some(&p.id), Status::Todo, []).await.unwrap();
    let b = s.create("b", Some(&p.id), Status::Todo, []).await.unwrap();
    s.set_status(&b.id, Status::Draft).await.unwrap(); // not ready → excluded
    // reorder a after b's slot to prove priority ordering, then restore
    let hits = s.what_next().await;
    // P and a are todo; priority order = tree order: P (root) then a (child)
    let ids: Vec<Id> = hits.into_iter().map(|h| h.task.id).collect();
    assert_eq!(ids, vec![p.id, a.id]);
}

#[tokio::test]
async fn priority_sort_follows_tree_order() {
    let fx = Fx::new();
    let s = fx.svc();
    let p = s.create("P", None, Status::Todo, []).await.unwrap();
    let x = s.create("x", Some(&p.id), Status::Todo, []).await.unwrap();
    let y = s.create("y", Some(&p.id), Status::Todo, []).await.unwrap();
    s.reorder(&y.id, Anchor::Before(x.id.clone()))
        .await
        .unwrap();

    let hits = s
        .evaluate(&Query {
            filter: Filter {
                within: Some(p.id.clone()),
                ..Default::default()
            },
            sort: vec![SortKey {
                key: SortField::Priority,
                dir: tda_core::Dir::Asc,
            }],
        })
        .await;
    let ids: Vec<Id> = hits.into_iter().map(|h| h.task.id).collect();
    assert_eq!(ids, vec![y.id, x.id]); // y reordered before x
}

#[tokio::test]
async fn json_round_trip_is_identity() {
    let fx = Fx::new();
    let s = fx.svc();
    let root = s.create("Root", None, Status::Todo, []).await.unwrap();
    let a = s
        .create("A", Some(&root.id), Status::Todo, [])
        .await
        .unwrap();
    let b = s
        .create("B", Some(&root.id), Status::Done, [])
        .await
        .unwrap();
    s.block(&a.id, &b.id).await.unwrap();
    s.add_tag(&a.id, "x").await.unwrap();
    let json = s.export_json(&root.id).await.unwrap();

    // import into a fresh store and re-export → byte-identical
    let fx2 = Fx::new();
    let s2 = fx2.svc();
    s2.import_json(&json).await.unwrap();
    assert_eq!(s2.export_json(&root.id).await.unwrap(), json);
}

#[tokio::test]
async fn markdown_export_snapshot() {
    let fx = Fx::new();
    let s = fx.svc();
    let root = s.create("Roadmap", None, Status::Todo, []).await.unwrap();
    let m0 = s
        .create("M0 skeleton", Some(&root.id), Status::Done, [])
        .await
        .unwrap();
    let _ = s
        .create("CI", Some(&m0.id), Status::Done, [])
        .await
        .unwrap();
    let _ = s
        .create("M1 core", Some(&root.id), Status::Wip, [])
        .await
        .unwrap();

    insta::assert_snapshot!(s.export_md(&root.id).await.unwrap());
}

#[tokio::test]
async fn markdown_import_round_trips_structure() {
    let fx = Fx::new();
    let s = fx.svc();
    let md = "- [ ] Roadmap\n  - [x] M0 skeleton\n  - [ ] M1 core\n";
    let roots = s.import_md(md).await.unwrap();
    assert_eq!(roots.len(), 1);
    let out = s.export_md(&roots[0].id).await.unwrap();
    assert_eq!(out, md);
}
