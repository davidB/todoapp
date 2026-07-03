//! Decider guard / denial-path tests (spec §5a, §11). These exercise the pure
//! decision logic directly via `decide`/`apply` over a `MemStore` — the
//! capability-keyed core moved here from `tda-core` so the core stays free of a
//! `tokio` dev-dependency.

use tda_core::{
    Assignment, Assignments, Command, ComponentStore, DecideCtx, Denied, Id, Status, apply, decide,
};
use tda_store_mem::MemStore;

/// A task carrying just a `Status` component, at id `t1`.
async fn task(status: Status) -> (MemStore, Id) {
    let store = MemStore::new();
    let id = Id::new("t1");
    store.set(&id, status).await;
    (store, id)
}

#[tokio::test]
async fn status_transitions_are_unrestricted() {
    let (store, id) = task(Status::Draft).await;
    let ctx = DecideCtx::default();
    // jumping straight to `done`, and back down to `draft`, are both allowed.
    assert!(
        decide(&store, &id, &Command::SetStatus(Status::Done), &ctx)
            .await
            .is_ok()
    );
    assert!(
        decide(&store, &id, &Command::SetStatus(Status::Draft), &ctx)
            .await
            .is_ok()
    );
}

#[tokio::test]
async fn cannot_claim_a_draft() {
    let (store, id) = task(Status::Draft).await;
    let r = decide(
        &store,
        &id,
        &Command::Claim(Id::new("a")),
        &DecideCtx::default(),
    )
    .await;
    assert_eq!(
        r.unwrap_err(),
        Denied("claim allowed only from todo".into())
    );
}

#[tokio::test]
async fn claim_restricted_to_assignees() {
    let (store, id) = task(Status::Todo).await;
    store
        .set(
            &id,
            Assignments(vec![Assignment {
                actor: Id::new("alice"),
                claimed: false,
            }]),
        )
        .await;
    let ctx = DecideCtx::default();
    // bob is not an assignee
    assert!(
        decide(&store, &id, &Command::Claim(Id::new("bob")), &ctx)
            .await
            .is_err()
    );
    // alice may, and claiming sets wip + claimed
    let ev = decide(&store, &id, &Command::Claim(Id::new("alice")), &ctx)
        .await
        .unwrap();
    for e in &ev {
        apply(&store, &id, e).await;
    }
    assert_eq!(store.get::<Status>(&id).await, Some(Status::Wip));
    assert!(store.get::<Assignments>(&id).await.unwrap().0[0].claimed);
}

#[tokio::test]
async fn cannot_start_while_blocked() {
    let (store, id) = task(Status::Todo).await;
    let ctx = DecideCtx { blocked: true };
    assert!(
        decide(&store, &id, &Command::SetStatus(Status::Wip), &ctx)
            .await
            .is_err()
    );
    assert!(
        decide(&store, &id, &Command::Claim(Id::new("a")), &ctx)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn claim_with_no_assignees_adds_claimer() {
    let (store, id) = task(Status::Todo).await;
    let ev = decide(
        &store,
        &id,
        &Command::Claim(Id::new("solo")),
        &DecideCtx::default(),
    )
    .await
    .unwrap();
    for e in &ev {
        apply(&store, &id, e).await;
    }
    let asg = store.get::<Assignments>(&id).await.unwrap();
    assert_eq!(asg.0.len(), 1);
    assert!(asg.0[0].claimed);
}
