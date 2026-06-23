//! Property tests for the two invariants the spec flags as load-bearing (§7,
//! §11): `blocks` DAG acyclicity and `child` ordering under inserts/moves.

use proptest::prelude::*;
use tda_app::{Anchor, Services};
use tda_core::{Id, LinkKind, Status};
use tda_store_mem::{FixedClock, MemStore, SeqIds};

fn fresh() -> (MemStore, FixedClock, SeqIds) {
    (MemStore::new(), FixedClock::default(), SeqIds::default())
}

proptest! {
    /// No sequence of accepted `block` edges can ever close a `blocks` cycle:
    /// after each accepted edge, the graph stays acyclic.
    #[test]
    fn blocks_graph_stays_acyclic(pairs in proptest::collection::vec((0usize..6, 0usize..6), 0..40)) {
        let (store, clock, ids) = fresh();
        let s = Services { tasks: &store, links: &store, collections: &store, clock: &clock, ids: &ids };
        let nodes: Vec<Id> = (0..6).map(|_| s.create("n", None, Status::Todo, []).unwrap().id).collect();

        for (a, b) in pairs {
            // ignore rejected edges (self/cycle); accepted ones must keep it acyclic
            let _ = s.block(&nodes[a], &nodes[b]);
            prop_assert!(acyclic(&store, &nodes));
        }
    }

    /// Children always come back strictly ordered by position, and reordering
    /// one child to the front actually puts it first.
    #[test]
    fn child_order_is_consistent(n in 1usize..8, front in 0usize..8) {
        let (store, clock, ids) = fresh();
        let s = Services { tasks: &store, links: &store, collections: &store, clock: &clock, ids: &ids };
        let p = s.create("p", None, Status::Todo, []).unwrap();
        let kids: Vec<Id> = (0..n).map(|_| s.create("k", Some(&p.id), Status::Todo, []).unwrap().id).collect();

        // positions strictly increasing
        let pos: Vec<f64> = s.children_of(&p.id).iter().map(|l| l.position.0).collect();
        prop_assert!(pos.windows(2).all(|w| w[0] < w[1]));

        // move one child to the very front
        let idx = front % n;
        let first_now = s.children_of(&p.id)[0].to.clone();
        if s.children_of(&p.id)[0].to != kids[idx] {
            s.reorder(&kids[idx], Anchor::Before(first_now)).unwrap();
        }
        prop_assert_eq!(s.children_of(&p.id)[0].to.clone(), kids[idx].clone());
        // still strictly ordered and same membership count
        let pos: Vec<f64> = s.children_of(&p.id).iter().map(|l| l.position.0).collect();
        prop_assert_eq!(pos.len(), n);
        prop_assert!(pos.windows(2).all(|w| w[0] < w[1]));
    }
}

/// Brute-force acyclicity check over the `blocks` edges.
fn acyclic(store: &MemStore, nodes: &[Id]) -> bool {
    use tda_core::LinkRepository;
    fn reaches(store: &MemStore, from: &Id, target: &Id, seen: &mut Vec<Id>) -> bool {
        for l in store.outgoing(from, LinkKind::Blocks) {
            if &l.to == target {
                return true;
            }
            if !seen.contains(&l.to) {
                seen.push(l.to.clone());
                if reaches(store, &l.to, target, seen) {
                    return true;
                }
            }
        }
        false
    }
    // a cycle exists iff some node reaches itself
    !nodes.iter().any(|n| reaches(store, n, n, &mut Vec::new()))
}
