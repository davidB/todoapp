//! Real `Clock`/`IdGenerator` implementations and the `Services` builder,
//! shared by the headless CLI path and the TUI (always compiled, no `tui`
//! feature — the direct CLI path needs them without ratatui).

use todoapp_app::Services;
use todoapp_core::{Clock, Date, Id, IdGenerator, Timestamp};
use todoapp_store_turso::TursoStore;
use ulid::Ulid;

pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> Timestamp {
        use std::time::{SystemTime, UNIX_EPOCH};
        let ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_millis());
        #[allow(clippy::cast_possible_truncation)]
        Timestamp::from_millisecond(ms as i64)
    }
    fn today(&self) -> Date {
        Date(jiff::Zoned::now().date())
    }
}

pub struct UlidGen;

impl IdGenerator for UlidGen {
    fn next_id(&self) -> Id {
        Id::new(Ulid::new().to_string().to_lowercase())
    }
}

/// Build a `Services` bundle from individual field references so the borrow
/// checker can see exactly which fields are in use (field-level disjoint borrows).
pub fn make_svc<'a>(
    store: &'a TursoStore,
    clock: &'a SystemClock,
    ids: &'a UlidGen,
) -> Services<'a, TursoStore> {
    Services {
        store,
        links: store,
        collections: store,
        query: store,
        clock,
        ids,
        blobs: store,
    }
}
