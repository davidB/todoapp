//! Guard: user values are bound parameters, never concatenated into SQL. A
//! classic injection payload must be stored/matched as a literal string and must
//! not alter the schema.

use tda_app::Services;
use tda_conformance::{FixedClock, SeqIds};
use tda_core::{Filter, Query, Status};
use tda_store_turso::TursoStore;

#[tokio::test]
async fn injection_payloads_are_treated_as_data() {
    let store = TursoStore::open_memory().await;
    let clock = FixedClock::default();
    let ids = SeqIds::default();
    let s = Services {
        store: &store,
        links: &store,
        collections: &store,
        query: &store,
        clock: &clock,
        ids: &ids,
    };

    let evil = "Robert'); DROP TABLE task;--";
    let t = s.create(evil, None, Status::Todo, []).await.unwrap();
    s.add_tag(&t.id, evil).await.unwrap();
    s.set_notes(&t.id, Some(evil.into())).await.unwrap();
    // a benign neighbour proves the `task` table still exists after the payload
    let other = s.create("survivor", None, Status::Todo, []).await.unwrap();

    // stored verbatim; nothing dropped
    let snap = s.snapshot(&t.id).await.unwrap();
    assert_eq!(snap.title, evil);
    assert!(snap.tags.contains(evil));
    assert_eq!(s.roots().await.len(), 2);
    assert!(s.snapshot(&other.id).await.is_ok());

    // payload as search text and as a tag filter is matched as a literal
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
