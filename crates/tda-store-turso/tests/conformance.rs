//! The shared port-conformance suite (spec §11) against the Turso store.

use tda_store_turso::TursoStore;

tda_conformance::conformance_suite!(TursoStore::open_memory().await);
