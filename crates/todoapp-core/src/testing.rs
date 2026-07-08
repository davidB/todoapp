//! Deterministic `Clock`/`IdGenerator` fixtures, store- and app-agnostic, so
//! every crate's tests (app, conformance, adapters) can share one definition.

use std::cell::RefCell;

use crate::{Clock, Date, Id, IdGenerator, Timestamp};

/// Sequential id generator: `t1`, `t2`, … Deterministic for tests.
#[derive(Default)]
pub struct SeqIds {
    n: RefCell<u64>,
}

impl IdGenerator for SeqIds {
    fn next_id(&self) -> Id {
        let mut n = self.n.borrow_mut();
        *n += 1;
        Id::new(format!("t{n}"))
    }
}

/// Clock pinned to a fixed instant and date — deterministic for tests.
pub struct FixedClock {
    pub now: Timestamp,
    pub today: Date,
}

impl Default for FixedClock {
    fn default() -> Self {
        Self {
            now: Timestamp::from_millisecond(0),
            today: Date::parse("2026-06-22").unwrap(),
        }
    }
}

impl Clock for FixedClock {
    fn now(&self) -> Timestamp {
        self.now
    }
    fn today(&self) -> Date {
        self.today
    }
}
