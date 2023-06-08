use alloc::boxed::Box;

use async_trait::async_trait;

use crate::IntoAny;

bitflags::bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
    pub struct Event: u16 {
        const READABLE = 1 << 0;
        const WRITABLE = 1 << 2;
        const ERROR   = 1 << 3;
        const HANG_UP = 1 << 4;
        const INVALID = 1 << 5;
    }
}

#[async_trait]
pub trait IoPoll: IntoAny {
    async fn event(&self, expected: Event) -> Event {
        let _ = expected;
        Event::READABLE | Event::WRITABLE
    }
}
