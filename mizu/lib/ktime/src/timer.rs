
use core::time::Duration;

use event_listener::Event;
use futures_lite::{stream, Stream};
use ktime_core::Instant;

#[derive(Debug)]
pub struct Timer {
    deadline: Instant,
    period: Duration,
}

impl Timer {
    pub fn deadline(deadline: Instant) -> Self {
        Timer {
            deadline,
            period: Duration::MAX,
        }
    }

    pub fn after(duration: Duration) -> Self {
        Timer::deadline(Instant::now() + duration)
    }

    pub fn period(period: Duration) -> Self {
        Timer {
            deadline: Instant::now() + period,
            period,
        }
    }

    pub fn set_deadline(&mut self, deadline: Instant) {
        self.deadline = deadline;
        self.period = Duration::MAX;
    }

    pub fn set_after(&mut self, duration: Duration) {
        self.set_deadline(Instant::now() + duration)
    }

    pub fn set_period(&mut self, period: Duration) {
        self.deadline = Instant::now() + period;
        self.period = period;
    }

    pub async fn wait(&mut self) -> Instant {
        loop {
            let now = Instant::now();
            if now >= self.deadline {
                if self.period != Duration::MAX {
                    self.deadline += self.period;
                }
                break now;
            }
            TIMER.listen().await
        }
    }

    async fn wait_period(&mut self, waited: bool) -> Option<(Instant, bool)> {
        if waited {
            return None;
        }
        loop {
            let now = Instant::now();
            if now >= self.deadline {
                let mut waited = false;
                if self.period != Duration::MAX {
                    self.deadline += self.period;
                } else {
                    waited = true;
                }
                break Some((now, waited));
            }
            TIMER.listen().await
        }
    }

    pub fn iter(&mut self) -> impl Stream<Item = Instant> + '_ {
        stream::unfold((self, false), |(timer, waited)| async move {
            let ret = timer.wait_period(waited).await;
            ret.map(|(ret, waited)| (ret, (timer, waited)))
        })
    }
}

static TIMER: Event = Event::new();

pub fn notify_timer() {
    TIMER.notify(usize::MAX)
}
