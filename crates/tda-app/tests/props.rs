//! Property tests for the two invariants the spec flags as load-bearing (§7,
//! §11): `blocks` DAG acyclicity and `child` ordering under inserts/moves.
//!
//! The use cases are async; proptest bodies are sync, so each wraps its work in
//! a current-thread runtime `block_on`.
//! ponytail: one runtime per case; fine at these sizes.

use std::collections::HashMap;

use proptest::prelude::*;
use proptest::test_runner::TestCaseError;
use tda_app::{Anchor, Services};
use tda_core::{Id, LinkKind, Status};
use tda_store_mem::{FixedClock, MemStore, SeqIds};

fn rt() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap()
}

fn fresh() -> (MemStore, FixedClock, SeqIds) {
    (MemStore::new(), FixedClock::default(), SeqIds::default())
}

proptest! {
    /// No sequence of accepted `block` edges can ever close a `blocks` cycle:
    /// after each accepted edge, the graph stays acyclic.
    #[test]
    fn blocks_graph_stays_acyclic(pairs in proptest::collection::vec((0usize..6, 0usize..6), 0..40)) {
        rt().block_on(async {
            let (store, clock, ids) = fresh();
            let s = Services { tasks: &store, links: &store, collections: &store, clock: &clock, ids: &ids };
            let mut nodes: Vec<Id> = Vec::new();
            for _ in 0..6 {
                nodes.push(s.create("n", None, Status::Todo, []).await.unwrap().id);
            }

            for (a, b) in pairs {
                // ignore rejected edges (self/cycle); accepted ones must keep it acyclic
                let _ = s.block(&nodes[a], &nodes[b]).await;
                prop_assert!(acyclic(&store, &nodes).await);
            }
            Ok::<(), TestCaseError>(())
        })?;
    }

    /// Children always come back strictly ordered by position, and reordering
    /// one child to the front actually puts it first.
    #[test]
    fn child_order_is_consistent(n in 1usize..8, front in 0usize..8) {
        rt().block_on(async {
            let (store, clock, ids) = fresh();
            let s = Services { tasks: &store, links: &store, collections: &store, clock: &clock, ids: &ids };
            let p = s.create("p", None, Status::Todo, []).await.unwrap();
            let mut kids: Vec<Id> = Vec::new();
            for _ in 0..n {
                kids.push(s.create("k", Some(&p.id), Status::Todo, []).await.unwrap().id);
            }

            // positions strictly increasing
            let pos: Vec<f64> = s.children_of(&p.id).await.iter().map(|l| l.position.0).collect();
            prop_assert!(pos.windows(2).all(|w| w[0] < w[1]));

            // move one child to the very front
            let idx = front % n;
            let first_now = s.children_of(&p.id).await[0].to.clone();
            if s.children_of(&p.id).await[0].to != kids[idx] {
                s.reorder(&kids[idx], Anchor::Before(first_now)).await.unwrap();
            }
            prop_assert_eq!(s.children_of(&p.id).await[0].to.clone(), kids[idx].clone());
            // still strictly ordered and same membership count
            let pos: Vec<f64> = s.children_of(&p.id).await.iter().map(|l| l.position.0).collect();
            prop_assert_eq!(pos.len(), n);
            prop_assert!(pos.windows(2).all(|w| w[0] < w[1]));
            Ok::<(), TestCaseError>(())
        })?;
    }
}

/// Brute-force acyclicity check over the `blocks` edges: gather adjacency from
/// the async store, then DFS synchronously.
async fn acyclic(store: &MemStore, nodes: &[Id]) -> bool {
    use tda_core::LinkRepository;
    let mut adj: HashMap<Id, Vec<Id>> = HashMap::new();
    for n in nodes {
        let outs = store.outgoing(n, LinkKind::Blocks).await;
        adj.insert(n.clone(), outs.into_iter().map(|l| l.to).collect());
    }
    fn reaches(adj: &HashMap<Id, Vec<Id>>, from: &Id, target: &Id, seen: &mut Vec<Id>) -> bool {
        for to in adj.get(from).into_iter().flatten() {
            if to == target {
                return true;
            }
            if !seen.contains(to) {
                seen.push(to.clone());
                if reaches(adj, to, target, seen) {
                    return true;
                }
            }
        }
        false
    }
    // a cycle exists iff some node reaches itself
    !nodes.iter().any(|n| reaches(&adj, n, n, &mut Vec::new()))
}
