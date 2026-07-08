//! The shared port-conformance suite (spec §11) against the Turso store.

use todoapp_store_turso::TursoStore;

todoapp_conformance::conformance_suite!(TursoStore::open_memory().await);
