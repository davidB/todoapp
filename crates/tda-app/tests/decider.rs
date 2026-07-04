//! Decider guard / denial-path tests (spec §5a, §11). These exercise the pure
//! decision logic directly via `decide`/`apply` over a `MemStore` — the
//! capability-keyed core moved here from `tda-core` so the core stays free of a
//! `tokio` dev-dependency.

use std::collections::BTreeSet;

use tda_core::{
    Archived, Assignment, Assignments, Command, ComponentStore, Date, DecideCtx, Denied, Due,
    Duration, Id, IssueRef, Recurrence, RepeatCycle, Schedule, Status, TimeLog, TimeSpent, Weekday,
    apply, decide,
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

#[tokio::test]
async fn completing_a_recurring_task_resets_it_instead_of_staying_done() {
    let (store, id) = task(Status::Todo).await;
    store
        .set(&id, Schedule(Due::parse("2026-07-01").unwrap()))
        .await;
    store
        .set(
            &id,
            Recurrence {
                cycle: RepeatCycle::Weekly {
                    weekdays: BTreeSet::from([Weekday::Wed]),
                },
                time: None,
            },
        )
        .await;

    let ctx = DecideCtx::default();
    let ev = decide(&store, &id, &Command::SetStatus(Status::Done), &ctx)
        .await
        .unwrap();
    for e in &ev {
        apply(&store, &id, e).await;
    }

    // 2026-07-01 is a Wednesday, so the next Wednesday is 2026-07-08.
    assert_eq!(store.get::<Status>(&id).await, Some(Status::Todo));
    assert_eq!(
        store.get::<Schedule>(&id).await.map(|s| s.0),
        Some(Due::parse("2026-07-08").unwrap())
    );
}

#[tokio::test]
async fn completing_a_non_recurring_task_stays_done() {
    let (store, id) = task(Status::Todo).await;
    let ctx = DecideCtx::default();
    let ev = decide(&store, &id, &Command::SetStatus(Status::Done), &ctx)
        .await
        .unwrap();
    for e in &ev {
        apply(&store, &id, e).await;
    }
    assert_eq!(store.get::<Status>(&id).await, Some(Status::Done));
}

#[tokio::test]
async fn issue_ref_set_and_cleared() {
    let (store, id) = task(Status::Todo).await;
    let ctx = DecideCtx::default();
    let issue_ref = IssueRef {
        provider: "GITHUB".into(),
        id: "25".into(),
        url: None,
    };
    let ev = decide(
        &store,
        &id,
        &Command::SetIssueRef(Some(issue_ref.clone())),
        &ctx,
    )
    .await
    .unwrap();
    for e in &ev {
        apply(&store, &id, e).await;
    }
    assert_eq!(store.get::<IssueRef>(&id).await, Some(issue_ref));

    let ev = decide(&store, &id, &Command::SetIssueRef(None), &ctx)
        .await
        .unwrap();
    for e in &ev {
        apply(&store, &id, e).await;
    }
    assert_eq!(store.get::<IssueRef>(&id).await, None);
}

#[tokio::test]
async fn time_log_recomputes_cumulative_time_spent() {
    let (store, id) = task(Status::Todo).await;
    let ctx = DecideCtx::default();
    let map = std::collections::BTreeMap::from([
        (
            Date::parse("2026-07-01").unwrap(),
            Duration::from_minutes(30),
        ),
        (
            Date::parse("2026-07-02").unwrap(),
            Duration::from_minutes(45),
        ),
    ]);
    let ev = decide(&store, &id, &Command::SetTimeLog(map.clone()), &ctx)
        .await
        .unwrap();
    for e in &ev {
        apply(&store, &id, e).await;
    }
    assert_eq!(store.get::<TimeLog>(&id).await.map(|t| t.0), Some(map));
    assert_eq!(
        store.get::<TimeSpent>(&id).await.map(|t| t.0),
        Some(Duration::from_minutes(75))
    );

    // clearing the log also clears the derived total
    let ev = decide(
        &store,
        &id,
        &Command::SetTimeLog(std::collections::BTreeMap::new()),
        &ctx,
    )
    .await
    .unwrap();
    for e in &ev {
        apply(&store, &id, e).await;
    }
    assert_eq!(store.get::<TimeLog>(&id).await, None);
    assert_eq!(store.get::<TimeSpent>(&id).await, None);
}

#[tokio::test]
async fn archived_is_orthogonal_to_status() {
    let (store, id) = task(Status::Done).await;
    let ctx = DecideCtx::default();
    let ev = decide(&store, &id, &Command::SetArchived(true), &ctx)
        .await
        .unwrap();
    for e in &ev {
        apply(&store, &id, e).await;
    }
    assert!(store.get::<Archived>(&id).await.is_some());
    // archiving doesn't touch status
    assert_eq!(store.get::<Status>(&id).await, Some(Status::Done));

    let ev = decide(&store, &id, &Command::SetArchived(false), &ctx)
        .await
        .unwrap();
    for e in &ev {
        apply(&store, &id, e).await;
    }
    assert!(store.get::<Archived>(&id).await.is_none());
}
