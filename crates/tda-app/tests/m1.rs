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
    fn svc(&self) -> Services<'_> {
        Services {
            tasks: &self.store,
            links: &self.store,
            collections: &self.store,
            clock: &self.clock,
            ids: &self.ids,
        }
    }
}

#[test]
fn create_and_edit() {
    let fx = Fx::new();
    let s = fx.svc();
    let t = s.create("Write spec", None, Status::Draft, []).unwrap();
    let t = s.set_status(&t.id, Status::Todo).unwrap();
    let t = s.set_notes(&t.id, Some("markdown".into())).unwrap();
    let t = s.add_tag(&t.id, "doc").unwrap();
    assert_eq!(t.status, Status::Todo);
    assert_eq!(t.notes.as_deref(), Some("markdown"));
    assert!(t.tags.contains("doc"));
}

#[test]
fn batch_create_uses_indentation_for_depth() {
    let fx = Fx::new();
    let s = fx.svc();
    let made = s
        .batch_create("Parent\n  Child A\n  Child B\n    Grandchild")
        .unwrap();
    assert_eq!(made.len(), 4);
    let parent = &made[0];
    let kids = s.children_of(&parent.id);
    assert_eq!(kids.len(), 2);
    // grandchild sits under the most recent depth-1 task: Child B (index 2)
    assert_eq!(s.children_of(&made[2].id).len(), 1);
}

#[test]
fn move_subtree_and_reject_cycle() {
    let fx = Fx::new();
    let s = fx.svc();
    let a = s.create("A", None, Status::Todo, []).unwrap();
    let b = s.create("B", Some(&a.id), Status::Todo, []).unwrap();
    let c = s.create("C", Some(&b.id), Status::Todo, []).unwrap();

    // move C under A: fine
    s.move_task(&c.id, &a.id, None).unwrap();
    assert_eq!(s.parent_of(&c.id), Some(a.id.clone()));

    // move A under its own descendant B: cycle, rejected
    assert!(s.move_task(&a.id, &b.id, None).is_err());
}

#[test]
fn reorder_with_anchor() {
    let fx = Fx::new();
    let s = fx.svc();
    let p = s.create("P", None, Status::Todo, []).unwrap();
    let x = s.create("X", Some(&p.id), Status::Todo, []).unwrap();
    let y = s.create("Y", Some(&p.id), Status::Todo, []).unwrap();
    let z = s.create("Z", Some(&p.id), Status::Todo, []).unwrap();
    // order is X, Y, Z; move Z before X
    s.reorder(&z.id, Anchor::Before(x.id.clone())).unwrap();
    let order: Vec<Id> = s.children_of(&p.id).into_iter().map(|l| l.to).collect();
    assert_eq!(order, vec![z.id, x.id, y.id]);
}

#[test]
fn blocks_link_rejects_cycle_and_blocks_start() {
    let fx = Fx::new();
    let s = fx.svc();
    let a = s.create("A", None, Status::Todo, []).unwrap();
    let b = s.create("B", None, Status::Todo, []).unwrap();
    s.block(&a.id, &b.id).unwrap(); // A blocks B
    assert!(s.block(&b.id, &a.id).is_err()); // would cycle

    // B is blocked because blocker A is not done → cannot start
    assert!(s.is_blocked(&b.id));
    assert!(s.claim(&b.id, Id::new("u")).is_err());

    // complete A (todo→wip→done) → B unblocks
    s.set_status(&a.id, Status::Wip).unwrap();
    s.set_status(&a.id, Status::Done).unwrap();
    assert!(!s.is_blocked(&b.id));
    s.claim(&b.id, Id::new("u")).unwrap();
}

#[test]
fn aggregate_rolls_up_subtree() {
    let fx = Fx::new();
    let s = fx.svc();
    let root = s.create("root", None, Status::Todo, []).unwrap();
    let a = s.create("a", Some(&root.id), Status::Done, []).unwrap();
    let _b = s.create("b", Some(&root.id), Status::Todo, []).unwrap();
    s.set_estimate(&a.id, Some(30)).unwrap();
    s.add_time_spent(&a.id, 25).unwrap();
    s.set_due(&a.id, Some("2026-07-01".into())).unwrap();
    s.set_due(&root.id, Some("2026-06-30".into())).unwrap();

    let agg = s.aggregate(&root.id).unwrap();
    assert_eq!(agg.total, 3);
    assert_eq!(agg.done, 1);
    assert_eq!(agg.eta_minutes, 30);
    assert_eq!(agg.time_spent_minutes, 25);
    assert_eq!(agg.earliest_due.as_deref(), Some("2026-06-30"));
}

#[test]
fn query_filters_sorts_and_breadcrumbs() {
    let fx = Fx::new();
    let s = fx.svc();
    let proj = s.create("Project", None, Status::Todo, []).unwrap();
    let t1 = s.create("first", Some(&proj.id), Status::Todo, []).unwrap();
    let _t2 = s
        .create("second", Some(&proj.id), Status::Done, [])
        .unwrap();
    s.add_tag(&t1.id, "urgent").unwrap();

    // status:todo, tag:urgent → only t1, with breadcrumb [Project]
    let hits = s.evaluate(&Query {
        filter: Filter {
            status: vec![Status::Todo],
            tags: vec!["urgent".into()],
            ..Default::default()
        },
        ..Default::default()
    });
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].task.id, t1.id);
    assert_eq!(hits[0].path, vec!["Project".to_string()]);
}

#[test]
fn within_predicate_scopes_to_subtree() {
    let fx = Fx::new();
    let s = fx.svc();
    let under = s.create("Under", None, Status::Todo, []).unwrap();
    let inside = s
        .create("inside", Some(&under.id), Status::Todo, [])
        .unwrap();
    let _outside = s.create("outside", None, Status::Todo, []).unwrap();

    let hits = s.evaluate(&Query {
        filter: Filter {
            within: Some(under.id.clone()),
            ..Default::default()
        },
        ..Default::default()
    });
    let ids: Vec<Id> = hits.into_iter().map(|h| h.task.id).collect();
    assert_eq!(ids, vec![inside.id]);
}

#[test]
fn due_filters() {
    let fx = Fx::new();
    let s = fx.svc();
    let today = s.create("today", None, Status::Todo, []).unwrap();
    let past = s.create("past", None, Status::Todo, []).unwrap();
    s.set_due(&today.id, Some("2026-06-22".into())).unwrap();
    s.set_due(&past.id, Some("2026-01-01".into())).unwrap();

    let due_today = s.due_today();
    assert_eq!(due_today.len(), 1);
    assert_eq!(due_today[0].task.id, today.id);

    let overdue = s.evaluate(&Query {
        filter: Filter {
            due: Some(DueFilter::Overdue),
            ..Default::default()
        },
        ..Default::default()
    });
    assert_eq!(overdue.len(), 1);
    assert_eq!(overdue[0].task.id, past.id);
}

#[test]
fn what_next_is_todo_by_priority() {
    let fx = Fx::new();
    let s = fx.svc();
    let p = s.create("P", None, Status::Todo, []).unwrap();
    let a = s.create("a", Some(&p.id), Status::Todo, []).unwrap();
    let b = s.create("b", Some(&p.id), Status::Todo, []).unwrap();
    s.set_status(&b.id, Status::Draft).unwrap(); // not ready → excluded
    // reorder a after b's slot to prove priority ordering, then restore
    let hits = s.what_next();
    // P and a are todo; priority order = tree order: P (root) then a (child)
    let ids: Vec<Id> = hits.into_iter().map(|h| h.task.id).collect();
    assert_eq!(ids, vec![p.id, a.id]);
}

#[test]
fn priority_sort_follows_tree_order() {
    let fx = Fx::new();
    let s = fx.svc();
    let p = s.create("P", None, Status::Todo, []).unwrap();
    let x = s.create("x", Some(&p.id), Status::Todo, []).unwrap();
    let y = s.create("y", Some(&p.id), Status::Todo, []).unwrap();
    s.reorder(&y.id, Anchor::Before(x.id.clone())).unwrap();

    let hits = s.evaluate(&Query {
        filter: Filter {
            within: Some(p.id.clone()),
            ..Default::default()
        },
        sort: vec![SortKey {
            key: SortField::Priority,
            dir: tda_core::Dir::Asc,
        }],
    });
    let ids: Vec<Id> = hits.into_iter().map(|h| h.task.id).collect();
    assert_eq!(ids, vec![y.id, x.id]); // y reordered before x
}

#[test]
fn json_round_trip_is_identity() {
    let fx = Fx::new();
    let s = fx.svc();
    let root = s.create("Root", None, Status::Todo, []).unwrap();
    let a = s.create("A", Some(&root.id), Status::Todo, []).unwrap();
    let b = s.create("B", Some(&root.id), Status::Done, []).unwrap();
    s.block(&a.id, &b.id).unwrap();
    s.add_tag(&a.id, "x").unwrap();
    let json = s.export_json(&root.id).unwrap();

    // import into a fresh store and re-export → byte-identical
    let fx2 = Fx::new();
    let s2 = fx2.svc();
    s2.import_json(&json).unwrap();
    assert_eq!(s2.export_json(&root.id).unwrap(), json);
}

#[test]
fn markdown_export_snapshot() {
    let fx = Fx::new();
    let s = fx.svc();
    let root = s.create("Roadmap", None, Status::Todo, []).unwrap();
    let m0 = s
        .create("M0 skeleton", Some(&root.id), Status::Done, [])
        .unwrap();
    let _ = s.create("CI", Some(&m0.id), Status::Done, []).unwrap();
    let _ = s
        .create("M1 core", Some(&root.id), Status::Wip, [])
        .unwrap();

    insta::assert_snapshot!(s.export_md(&root.id).unwrap());
}

#[test]
fn markdown_import_round_trips_structure() {
    let fx = Fx::new();
    let s = fx.svc();
    let md = "- [ ] Roadmap\n  - [x] M0 skeleton\n  - [ ] M1 core\n";
    let roots = s.import_md(md).unwrap();
    assert_eq!(roots.len(), 1);
    let out = s.export_md(&roots[0].id).unwrap();
    assert_eq!(out, md);
}
