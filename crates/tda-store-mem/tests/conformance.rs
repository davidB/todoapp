//! The shared port-conformance suite (spec §11) against the in-memory store.

use tda_store_mem::MemStore;

tda_conformance::conformance_suite!(MemStore::new());
