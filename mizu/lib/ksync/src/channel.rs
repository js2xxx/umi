use arsc_rs::Arsc;
use crossbeam_queue::{ArrayQueue, SegQueue};

mod broadcast;
pub mod mpmc;
pub mod oneshot;

pub use self::broadcast::Broadcast;

pub fn oneshot<T>() -> (oneshot::Sender<T>, oneshot::Receiver<T>) {
    let packet = Arsc::new(oneshot::Packet::new());
    (
        oneshot::Sender::new(packet.clone()),
        oneshot::Receiver::new(packet),
    )
}

pub fn with_flavor<F: mpmc::Flavor>(queue: F) -> (mpmc::Sender<F>, mpmc::Receiver<F>) {
    let channel = Arsc::new(mpmc::Channel::new(queue));
    (
        mpmc::Sender::new(channel.clone()),
        mpmc::Receiver::new(channel),
    )
}

pub fn bounded<T>(capacity: usize) -> (mpmc::Sender<ArrayQueue<T>>, mpmc::Receiver<ArrayQueue<T>>) {
    self::with_flavor(ArrayQueue::new(capacity))
}

pub fn unbounded<T>() -> (mpmc::Sender<SegQueue<T>>, mpmc::Receiver<SegQueue<T>>) {
    self::with_flavor(SegQueue::new())
}
