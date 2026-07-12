//! Port-conformance suite (spec §11): the M1 use-case tests, parametrized over
//! the store. This crate's own `tests/` targets invoke [`conformance_suite!`]
//! with a fresh-store constructor for each adapter; the *same* bodies run
//! against `todoapp-store-mem` and `todoapp-store-turso`, proving they are
//! interchangeable behind the ports. This crate depends on the adapters
//! (rather than being depended on by them), keeping the workspace dependency
//! graph acyclic.

/// Generate the conformance tests for a store. `$make` is an expression that
/// yields a fresh store implementing every port (evaluated in `async` context,
/// so it may `.await`). Invoke once per store crate from a `tests/` file.
#[macro_export]
macro_rules! conformance_suite {
    ($make:expr) => {
        mod conformance {
            #![allow(unused_imports)]
            use super::*;
            use ::todoapp_app::{Anchor, Services};
            use ::todoapp_core::testing::{FixedClock, SeqIds};
            use ::todoapp_core::{
                Date, Dir, DueFilter, Duration, Filter, Id, Query, SortField, SortKey, Status,
            };

            /// Fresh store + fixtures, kept alive by the caller's locals.
            macro_rules! svc {
                ($store:ident, $clock:ident, $ids:ident) => {
                    let $store = $make;
                    let $clock = FixedClock::default(); // today = 2026-06-22
                    let $ids = SeqIds::default();
                };
            }
            macro_rules! services {
                ($store:ident, $clock:ident, $ids:ident) => {
                    Services {
                        store: &$store,
                        links: &$store,
                        collections: &$store,
                        query: &$store,
                        clock: &$clock,
                        ids: &$ids,
                        blobs: &$store,
                    }
                };
            }

            #[tokio::test]
            async fn create_and_edit() {
                svc!(store, clock, ids);
                let s = services!(store, clock, ids);
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
            async fn at_mention_in_title_assigns_and_strips_on_create_and_edit() {
                svc!(store, clock, ids);
                let s = services!(store, clock, ids);
                let t = s
                    .create("fix @alice bug", None, Status::Todo, [])
                    .await
                    .unwrap();
                assert_eq!(t.title, "fix bug");
                assert_eq!(t.assignments.len(), 1);
                assert_eq!(t.assignments[0].actor, Id::new("alice"));

                let t = s.set_title(&t.id, "fix @bob bug too").await.unwrap();
                assert_eq!(t.title, "fix bug too");
                let actors: Vec<Id> = t.assignments.iter().map(|a| a.actor.clone()).collect();
                assert!(actors.contains(&Id::new("alice")));
                assert!(actors.contains(&Id::new("bob")));
            }

            #[tokio::test]
            async fn batch_create_uses_indentation_for_depth() {
                svc!(store, clock, ids);
                let s = services!(store, clock, ids);
                let made = s
                    .batch_create("Parent\n  Child A\n  Child B\n    Grandchild")
                    .await
                    .unwrap();
                assert_eq!(made.len(), 4);
                let parent = &made[0];
                let kids = s.children_of(&parent.id).await;
                assert_eq!(kids.len(), 2);
                assert_eq!(s.children_of(&made[2].id).await.len(), 1);
            }

            #[tokio::test]
            async fn move_subtree_and_reject_cycle() {
                svc!(store, clock, ids);
                let s = services!(store, clock, ids);
                let a = s.create("A", None, Status::Todo, []).await.unwrap();
                let b = s.create("B", Some(&a.id), Status::Todo, []).await.unwrap();
                let c = s.create("C", Some(&b.id), Status::Todo, []).await.unwrap();
                s.move_task(&c.id, &a.id, None).await.unwrap();
                assert_eq!(s.parent_of(&c.id).await, Some(a.id.clone()));
                assert!(s.move_task(&a.id, &b.id, None).await.is_err());
            }

            #[tokio::test]
            async fn roots_lists_top_level_tasks() {
                svc!(store, clock, ids);
                let s = services!(store, clock, ids);
                let a = s.create("A", None, Status::Todo, []).await.unwrap();
                let b = s.create("B", None, Status::Todo, []).await.unwrap();
                let c = s.create("C", Some(&a.id), Status::Todo, []).await.unwrap();
                assert_eq!(s.roots().await, vec![a.id.clone(), b.id.clone()]);
                assert!(!s.roots().await.contains(&c.id));
                assert_eq!(s.parent_of(&a.id).await, None);
                s.move_task(&b.id, &a.id, None).await.unwrap();
                assert_eq!(s.roots().await, vec![a.id.clone()]);

                let json = s.export_json(&a.id).await.unwrap();
                svc!(store2, clock2, ids2);
                let s2 = services!(store2, clock2, ids2);
                s2.import_json(&json, None).await.unwrap();
                assert_eq!(s2.roots().await, vec![a.id]);
            }

            #[tokio::test]
            async fn reorder_with_anchor() {
                svc!(store, clock, ids);
                let s = services!(store, clock, ids);
                let p = s.create("P", None, Status::Todo, []).await.unwrap();
                let x = s.create("X", Some(&p.id), Status::Todo, []).await.unwrap();
                let y = s.create("Y", Some(&p.id), Status::Todo, []).await.unwrap();
                let z = s.create("Z", Some(&p.id), Status::Todo, []).await.unwrap();
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
                svc!(store, clock, ids);
                let s = services!(store, clock, ids);
                let a = s.create("A", None, Status::Todo, []).await.unwrap();
                let b = s.create("B", None, Status::Todo, []).await.unwrap();
                s.block(&a.id, &b.id).await.unwrap();
                assert!(s.block(&b.id, &a.id).await.is_err());
                assert!(s.is_blocked(&b.id).await);
                assert!(s.claim(&b.id, Id::new("u")).await.is_err());
                s.set_status(&a.id, Status::Wip).await.unwrap();
                s.set_status(&a.id, Status::Done).await.unwrap();
                assert!(!s.is_blocked(&b.id).await);
                s.claim(&b.id, Id::new("u")).await.unwrap();
            }

            #[tokio::test]
            async fn aggregate_rolls_up_subtree() {
                svc!(store, clock, ids);
                let s = services!(store, clock, ids);
                let root = s.create("root", None, Status::Todo, []).await.unwrap();
                let a = s
                    .create("a", Some(&root.id), Status::Done, [])
                    .await
                    .unwrap();
                let b = s
                    .create("b", Some(&root.id), Status::Todo, [])
                    .await
                    .unwrap();
                s.set_estimate(&a.id, Some(Duration::from_minutes(30)))
                    .await
                    .unwrap();
                s.add_time_spent(&a.id, Duration::from_minutes(25))
                    .await
                    .unwrap();
                s.set_due(&a.id, Some(Date::parse("2026-07-01").unwrap().into()))
                    .await
                    .unwrap();
                s.set_due(&root.id, Some(Date::parse("2026-06-30").unwrap().into()))
                    .await
                    .unwrap();
                s.set_estimate(&b.id, Some(Duration::from_minutes(20)))
                    .await
                    .unwrap();
                s.assign(&b.id, Id::new("alice")).await.unwrap();
                let agg = s.aggregate(&root.id).await.unwrap();
                assert_eq!(agg.total, 3);
                assert_eq!(agg.done, 1);
                assert_eq!(agg.estimate, Duration::from_minutes(50));
                // `a` is Done, so its 30m estimate is excluded from `remaining`.
                assert_eq!(agg.remaining, Duration::from_minutes(20));
                assert_eq!(agg.time_spent, Duration::from_minutes(25));
                assert_eq!(
                    agg.earliest_due,
                    Some(Date::parse("2026-06-30").unwrap().into())
                );
                assert_eq!(
                    agg.assignees,
                    std::collections::BTreeSet::from([Id::new("alice")])
                );
                // Worst-case rollup: root is Todo, a is Done, b is Todo -> Todo.
                assert_eq!(agg.status, Status::Todo);

                s.set_status(&b.id, Status::Done).await.unwrap();
                s.set_status(&root.id, Status::Done).await.unwrap();
                let agg = s.aggregate(&root.id).await.unwrap();
                assert_eq!(agg.status, Status::Done);
            }

            #[tokio::test]
            async fn query_filters_sorts_and_breadcrumbs() {
                svc!(store, clock, ids);
                let s = services!(store, clock, ids);
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
                svc!(store, clock, ids);
                let s = services!(store, clock, ids);
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
                svc!(store, clock, ids);
                let s = services!(store, clock, ids);
                let today = s.create("today", None, Status::Todo, []).await.unwrap();
                let past = s.create("past", None, Status::Todo, []).await.unwrap();
                s.set_due(&today.id, Some(Date::parse("2026-06-22").unwrap().into()))
                    .await
                    .unwrap();
                s.set_due(&past.id, Some(Date::parse("2026-01-01").unwrap().into()))
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
                svc!(store, clock, ids);
                let s = services!(store, clock, ids);
                let p = s.create("P", None, Status::Todo, []).await.unwrap();
                let a = s.create("a", Some(&p.id), Status::Todo, []).await.unwrap();
                let b = s.create("b", Some(&p.id), Status::Todo, []).await.unwrap();
                s.set_status(&b.id, Status::Draft).await.unwrap();
                let hits = s.what_next().await;
                let ids: Vec<Id> = hits.into_iter().map(|h| h.task.id).collect();
                assert_eq!(ids, vec![p.id, a.id]);
            }

            #[tokio::test]
            async fn priority_sort_follows_tree_order() {
                svc!(store, clock, ids);
                let s = services!(store, clock, ids);
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
                            dir: Dir::Asc,
                        }],
                    })
                    .await;
                let ids: Vec<Id> = hits.into_iter().map(|h| h.task.id).collect();
                assert_eq!(ids, vec![y.id, x.id]);
            }

            #[tokio::test]
            async fn json_round_trip_is_identity() {
                svc!(store, clock, ids);
                let s = services!(store, clock, ids);
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
                svc!(store2, clock2, ids2);
                let s2 = services!(store2, clock2, ids2);
                s2.import_json(&json, None).await.unwrap();
                assert_eq!(s2.export_json(&root.id).await.unwrap(), json);
            }

            #[tokio::test]
            async fn markdown_export_matches() {
                svc!(store, clock, ids);
                let s = services!(store, clock, ids);
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
                let expected =
                    "- [ ] Roadmap\n  - [x] M0 skeleton\n    - [x] CI\n  - [ ] M1 core\n";
                assert_eq!(s.export_md(&root.id).await.unwrap(), expected);
            }

            #[tokio::test]
            async fn markdown_import_round_trips_structure() {
                svc!(store, clock, ids);
                let s = services!(store, clock, ids);
                let md = "- [ ] Roadmap\n  - [x] M0 skeleton\n  - [ ] M1 core\n";
                let roots = s.import_md(md, None).await.unwrap();
                assert_eq!(roots.len(), 1);
                let out = s.export_md(&roots[0].id).await.unwrap();
                assert_eq!(out, md);
            }

            /// User values are opaque data: special characters (notably a SQL
            /// injection payload) round-trip verbatim, alter no schema, and are
            /// matched as literals — proving any SQL store binds, never
            /// concatenates (no-op for the in-memory store, real teeth for Turso).
            #[tokio::test]
            async fn special_characters_are_stored_as_data() {
                svc!(store, clock, ids);
                let s = services!(store, clock, ids);
                let evil = "Robert'); DROP TABLE task;--";
                let t = s.create(evil, None, Status::Todo, []).await.unwrap();
                s.add_tag(&t.id, evil).await.unwrap();
                s.set_notes(&t.id, Some(evil.into())).await.unwrap();
                // a benign neighbour proves the store is intact after the payload
                let other = s.create("survivor", None, Status::Todo, []).await.unwrap();

                let snap = s.snapshot(&t.id).await.unwrap();
                assert_eq!(snap.title, evil);
                assert_eq!(snap.notes.as_deref(), Some(evil));
                assert!(snap.tags.contains(evil));
                assert_eq!(s.roots().await.len(), 2);
                assert!(s.snapshot(&other.id).await.is_ok());

                let by_text = s
                    .evaluate(&Query {
                        filter: Filter {
                            text: Some(evil.into()),
                            ..Default::default()
                        },
                        ..Default::default()
                    })
                    .await;
                assert_eq!(by_text.len(), 1);
                assert_eq!(by_text[0].task.id, t.id);

                let by_tag = s
                    .evaluate(&Query {
                        filter: Filter {
                            tags: vec![evil.into()],
                            ..Default::default()
                        },
                        ..Default::default()
                    })
                    .await;
                assert_eq!(by_tag.len(), 1);
            }

            #[tokio::test]
            async fn delete_leaf_task_removes_components_and_link() {
                svc!(store, clock, ids);
                let s = services!(store, clock, ids);
                let p = s.create("P", None, Status::Todo, []).await.unwrap();
                let c = s.create("C", Some(&p.id), Status::Todo, []).await.unwrap();
                s.delete_task(&c.id, false).await.unwrap();
                assert!(s.snapshot(&c.id).await.is_err());
                assert!(s.children_of(&p.id).await.is_empty());
            }

            #[tokio::test]
            async fn delete_task_with_children_rejected_without_recursive() {
                svc!(store, clock, ids);
                let s = services!(store, clock, ids);
                let p = s.create("P", None, Status::Todo, []).await.unwrap();
                let c = s.create("C", Some(&p.id), Status::Todo, []).await.unwrap();
                assert!(s.delete_task(&p.id, false).await.is_err());
                assert!(s.snapshot(&p.id).await.is_ok());
                assert!(s.snapshot(&c.id).await.is_ok());
            }

            #[tokio::test]
            async fn delete_task_recursive_cascades_subtree() {
                svc!(store, clock, ids);
                let s = services!(store, clock, ids);
                let p = s.create("P", None, Status::Todo, []).await.unwrap();
                let c = s.create("C", Some(&p.id), Status::Todo, []).await.unwrap();
                s.delete_task(&p.id, true).await.unwrap();
                assert!(s.snapshot(&p.id).await.is_err());
                assert!(s.snapshot(&c.id).await.is_err());
            }

            #[tokio::test]
            async fn delete_task_removes_unshared_blob_keeps_shared_blob() {
                svc!(store, clock, ids);
                let s = services!(store, clock, ids);
                let a = s.create("A", None, Status::Todo, []).await.unwrap();
                let b = s.create("B", None, Status::Todo, []).await.unwrap();
                let a = s
                    .add_attachment_from_bytes(&a.id, "f.txt", b"same bytes".to_vec(), None)
                    .await
                    .unwrap();
                let b = s
                    .add_attachment_from_bytes(&b.id, "f.txt", b"same bytes".to_vec(), None)
                    .await
                    .unwrap();
                let blob = a.attachments[0].blob.clone().unwrap();
                assert_eq!(b.attachments[0].blob, Some(blob.clone()));

                s.delete_task(&a.id, false).await.unwrap();
                assert!(s.blobs.get(&blob).await.is_some());

                s.delete_task(&b.id, false).await.unwrap();
                assert!(s.blobs.get(&blob).await.is_none());
            }

            #[tokio::test]
            async fn delete_task_removes_dangling_blocks_links() {
                svc!(store, clock, ids);
                let s = services!(store, clock, ids);
                let a = s.create("A", None, Status::Todo, []).await.unwrap();
                let b = s.create("B", None, Status::Todo, []).await.unwrap();
                s.block(&a.id, &b.id).await.unwrap();
                assert!(s.is_blocked(&b.id).await);
                s.delete_task(&a.id, false).await.unwrap();
                assert!(!s.is_blocked(&b.id).await);
            }
        }
    };
}
