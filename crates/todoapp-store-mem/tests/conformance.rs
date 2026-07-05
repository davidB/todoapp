//! The shared port-conformance suite (spec §11) against the in-memory store.

use todoapp_store_mem::MemStore;

todoapp_conformance::conformance_suite!(MemStore::new());
