//! Super Productivity importer test: a small literal fixture covering the
//! field mappings documented in the import plan (project→parent-task,
//! subtask hierarchy, tags, due date/time, estimate/time-spent/time-log,
//! issue reference, link attachment, archived task).

use tda_app::Services;
use tda_conformance::{FixedClock, SeqIds};
use tda_core::{Archived, ComponentStore, Due, Status};
use tda_store_mem::MemStore;

fn fixture() -> serde_json::Value {
    serde_json::json!({
        "data": {
            "task": {
                "ids": ["root1", "child1", "tagged1"],
                "entities": {
                    "root1": {
                        "id": "root1",
                        "title": "root task",
                        "isDone": false,
                        "projectId": "proj1",
                        "dueDay": "2026-07-01",
                        "timeEstimate": 1_800_000,
                        "timeSpent": 1_800_000,
                        "timeSpentOnDay": {"2026-07-01": 900_000, "2026-07-02": 900_000},
                        "issueType": "GITHUB",
                        "issueId": "25",
                        "attachments": [
                            {"id": "att1", "type": "LINK", "path": "https://example.com", "title": "docs"}
                        ]
                    },
                    "child1": {
                        "id": "child1",
                        "parentId": "root1",
                        "title": "child task",
                        "isDone": false,
                        "projectId": "proj1"
                    },
                    "tagged1": {
                        "id": "tagged1",
                        "title": "tagged task",
                        "isDone": false,
                        "projectId": "proj1",
                        "tagIds": ["tag1"],
                        "dueWithTime": 1_782_140_400_000i64
                    }
                }
            },
            "project": {
                "ids": ["proj1"],
                "entities": {"proj1": {"title": "Inbox"}}
            },
            "tag": {
                "ids": ["tag1"],
                "entities": {"tag1": {"title": "urgent"}}
            },
            "archiveYoung": {
                "task": {
                    "ids": ["arch1"],
                    "entities": {
                        "arch1": {
                            "id": "arch1",
                            "title": "archived task",
                            "isDone": true,
                            "projectId": "proj1"
                        }
                    }
                }
            }
        }
    })
}

#[tokio::test]
async fn imports_projects_hierarchy_tags_due_time_issue_ref_attachment_and_archive() {
    let store = MemStore::new();
    let clock = FixedClock::default();
    let ids = SeqIds::default();
    let svc = Services {
        store: &store,
        links: &store,
        collections: &store,
        query: &store,
        clock: &clock,
        ids: &ids,
        blobs: &store,
    };

    let roots = svc
        .import_superproductivity(&fixture().to_string())
        .await
        .unwrap();

    // One project root task ("Inbox"), all four SP tasks under it.
    assert_eq!(roots.len(), 1);
    let project_root = &roots[0];
    assert_eq!(project_root.title, "Inbox");
    let children = svc.children_of(&project_root.id).await;
    // root1, tagged1, arch1 attach directly; child1 nests under root1 instead.
    assert_eq!(children.len(), 3);

    // Find each imported task by title via a snapshot walk.
    let mut by_title = std::collections::HashMap::new();
    for l in &children {
        let snap = svc.snapshot(&l.to).await.unwrap();
        by_title.insert(snap.title.clone(), snap);
    }

    let root_task = &by_title["root task"];
    assert_eq!(root_task.due_date, Some(Due::parse("2026-07-01").unwrap()));
    assert_eq!(root_task.eta_minutes.unwrap().as_minutes(), 30);
    assert_eq!(root_task.time_spent_minutes.as_minutes(), 30); // 15+15 from time_log
    assert_eq!(root_task.time_log.len(), 2);
    assert_eq!(root_task.issue_ref.as_ref().unwrap().provider, "GITHUB");
    assert_eq!(root_task.issue_ref.as_ref().unwrap().id, "25");
    assert_eq!(root_task.attachments.len(), 1);
    assert_eq!(
        root_task.attachments[0].url.as_deref(),
        Some("https://example.com")
    );

    // child1 is nested under root1, not directly under the project root.
    let root_children = svc.children_of(&root_task.id).await;
    assert_eq!(root_children.len(), 1);
    let child_snap = svc.snapshot(&root_children[0].to).await.unwrap();
    assert_eq!(child_snap.title, "child task");

    let tagged = &by_title["tagged task"];
    assert!(tagged.tags.contains("urgent"));
    assert!(tagged.due_date.unwrap().time.is_some()); // dueWithTime carries a time-of-day

    let archived = &by_title["archived task"];
    assert_eq!(archived.status, Status::Done);
    assert!(archived.archived);
    assert!(store.get::<Archived>(&archived.id).await.is_some());
}
