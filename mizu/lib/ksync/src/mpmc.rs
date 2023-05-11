use core::{
    fmt,
    future::Future,
    pin::Pin,
    sync::atomic::{AtomicBool, AtomicUsize, Ordering::*},
    task::{ready, Context, Poll},
};

use arsc_rs::Arsc;
use crossbeam_queue::{ArrayQueue, SegQueue};
use event_listener::{Event, EventListener};
use futures_lite::{stream, FutureExt, Stream};

pub trait Flavor {
    type Item;

    fn push(&self, data: Self::Item) -> Option<Self::Item>;

    fn pop(&self) -> Option<Self::Item>;

    fn is_empty(&self) -> bool;

    fn is_full(&self) -> bool;

    fn len(&self) -> usize;

    fn capacity(&self) -> usize;
}

impl<T> Flavor for SegQueue<T> {
    type Item = T;

    fn push(&self, data: T) -> Option<T> {
        self.push(data);
        None
    }

    fn pop(&self) -> Option<T> {
        self.pop()
    }

    fn is_empty(&self) -> bool {
        self.is_empty()
    }

    fn is_full(&self) -> bool {
        false
    }

    fn len(&self) -> usize {
        self.len()
    }

    fn capacity(&self) -> usize {
        usize::MAX
    }
}

impl<T> Flavor for ArrayQueue<T> {
    type Item = T;

    fn push(&self, data: T) -> Option<T> {
        self.push(data).err()
    }

    fn pop(&self) -> Option<T> {
        self.pop()
    }

    fn is_empty(&self) -> bool {
        self.is_empty()
    }

    fn is_full(&self) -> bool {
        self.is_full()
    }

    fn len(&self) -> usize {
        self.len()
    }

    fn capacity(&self) -> usize {
        self.capacity()
    }
}

struct Channel<F: Flavor> {
    queue: F,
    send: Event,
    recv: Event,
    closed: AtomicBool,
    sender: AtomicUsize,
    receiver: AtomicUsize,
}

impl<F: Flavor> Channel<F> {
    fn close(&self) -> bool {
        if !self.closed.swap(true, SeqCst) {
            self.send.notify(usize::MAX);
            self.recv.notify(usize::MAX);
            true
        } else {
            false
        }
    }

    fn is_closed(&self) -> bool {
        self.closed.load(SeqCst)
    }
}

pub struct Sender<F: Flavor> {
    channel: Arsc<Channel<F>>,
}

impl<F: Flavor> Sender<F> {
    pub fn try_send(&self, data: F::Item) -> Result<(), TrySendError<F::Item>> {
        if self.channel.is_closed() {
            return Err(TrySendError { data, closed: true });
        }
        if let Some(data) = self.channel.queue.push(data) {
            return Err(TrySendError {
                data,
                closed: false,
            });
        }
        self.channel.recv.notify_additional(1);
        Ok(())
    }

    pub fn send(&self, data: F::Item) -> Send<'_, F> {
        Send {
            sender: self,
            data: Some(data),
            listener: None,
        }
    }

    pub fn close(&self) -> bool {
        self.channel.close()
    }

    pub fn is_closed(&self) -> bool {
        self.channel.is_closed()
    }

    pub fn is_empty(&self) -> bool {
        self.channel.queue.is_empty()
    }

    pub fn is_full(&self) -> bool {
        self.channel.queue.is_full()
    }

    pub fn len(&self) -> usize {
        self.channel.queue.len()
    }

    pub fn capacity(&self) -> usize {
        self.channel.queue.capacity()
    }

    pub fn receiver_count(&self) -> usize {
        self.channel.receiver.load(SeqCst)
    }

    pub fn sender_count(&self) -> usize {
        self.channel.sender.load(SeqCst)
    }
}

#[must_use = "futures do nothing unless polled"]
pub struct Send<'a, F: Flavor> {
    sender: &'a Sender<F>,
    data: Option<F::Item>,
    listener: Option<EventListener>,
}

impl<F: Flavor> Unpin for Send<'_, F> {}

impl<F: Flavor> Future for Send<'_, F> {
    type Output = Result<(), SendError<F::Item>>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        loop {
            let data = match self.sender.try_send(self.data.take().unwrap()) {
                Ok(()) => break Poll::Ready(Ok(())),
                Err(err) if err.is_full() => err.data,
                Err(err) => break Poll::Ready(Err(SendError { data: err.data })),
            };
            self.data = Some(data);
            match self.listener.as_mut() {
                Some(listener) => {
                    ready!(listener.poll(cx));
                    self.listener = None;
                }
                None => self.listener = Some(self.sender.channel.send.listen()),
            }
        }
    }
}

impl<F: Flavor> Clone for Sender<F> {
    fn clone(&self) -> Self {
        let count = self.channel.sender.fetch_add(1, SeqCst);
        assert!(
            count <= usize::MAX / 2,
            "too many senders (potential leaks / overflow)"
        );
        Sender {
            channel: self.channel.clone(),
        }
    }
}

impl<F: Flavor> Drop for Sender<F> {
    fn drop(&mut self) {
        if self.channel.sender.fetch_sub(1, SeqCst) == 1 {
            self.channel.close();
        }
    }
}

pub struct Receiver<F: Flavor> {
    channel: Arsc<Channel<F>>,
}

impl<F: Flavor> Receiver<F> {
    pub fn try_recv(&self) -> Result<F::Item, TryRecvError<F::Item>> {
        let data = self.channel.queue.pop();
        if self.channel.is_closed() {
            Err(TryRecvError::Closed(data))
        } else {
            let data = data.ok_or(TryRecvError::Empty)?;
            self.channel.send.notify_additional(1);
            Ok(data)
        }
    }

    pub fn recv(&self) -> Recv<'_, F> {
        Recv {
            receiver: self,
            listener: None,
        }
    }

    pub fn recv_once(self) -> RecvOnce<F> {
        RecvOnce {
            receiver: self,
            listener: None,
        }
    }

    pub fn streamed(self) -> impl Stream<Item = F::Item> {
        stream::unfold(self, |this| async move {
            match this.recv().await {
                Ok(data) => Some((data, this)),
                Err(RecvError { data }) => data.map(|data| (data, this)),
            }
        })
    }

    pub fn close(&self) -> bool {
        self.channel.close()
    }

    pub fn is_closed(&self) -> bool {
        self.channel.is_closed()
    }

    pub fn is_empty(&self) -> bool {
        self.channel.queue.is_empty()
    }

    pub fn is_full(&self) -> bool {
        self.channel.queue.is_full()
    }

    pub fn len(&self) -> usize {
        self.channel.queue.len()
    }

    pub fn capacity(&self) -> usize {
        self.channel.queue.capacity()
    }

    pub fn receiver_count(&self) -> usize {
        self.channel.receiver.load(SeqCst)
    }

    pub fn sender_count(&self) -> usize {
        self.channel.sender.load(SeqCst)
    }
}

#[must_use = "futures do nothing unless polled"]
pub struct Recv<'a, F: Flavor> {
    receiver: &'a Receiver<F>,
    listener: Option<EventListener>,
}

impl<F: Flavor> Future for Recv<'_, F> {
    type Output = Result<F::Item, RecvError<F::Item>>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        loop {
            match self.receiver.try_recv() {
                Ok(data) => break Poll::Ready(Ok(data)),
                Err(TryRecvError::Closed(data)) => break Poll::Ready(Err(RecvError { data })),
                Err(TryRecvError::Empty) => match self.listener.as_mut() {
                    Some(listener) => {
                        ready!(listener.poll(cx));
                        self.listener = None;
                    }
                    None => self.listener = Some(self.receiver.channel.recv.listen()),
                },
            }
        }
    }
}

#[must_use = "futures do nothing unless polled"]
pub struct RecvOnce<F: Flavor> {
    receiver: Receiver<F>,
    listener: Option<EventListener>,
}

impl<F: Flavor> Future for RecvOnce<F> {
    type Output = Result<F::Item, RecvError<F::Item>>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        loop {
            match self.receiver.try_recv() {
                Ok(data) => break Poll::Ready(Ok(data)),
                Err(TryRecvError::Closed(data)) => break Poll::Ready(Err(RecvError { data })),
                Err(TryRecvError::Empty) => match self.listener.as_mut() {
                    Some(listener) => {
                        ready!(listener.poll(cx));
                        self.listener = None;
                    }
                    None => self.listener = Some(self.receiver.channel.recv.listen()),
                },
            }
        }
    }
}

impl<F: Flavor> Clone for Receiver<F> {
    fn clone(&self) -> Self {
        let count = self.channel.receiver.fetch_add(1, SeqCst);
        assert!(
            count <= usize::MAX / 2,
            "too many senders (potential leaks / overflow)"
        );
        Receiver {
            channel: self.channel.clone(),
        }
    }
}

impl<F: Flavor> Drop for Receiver<F> {
    fn drop(&mut self) {
        if self.channel.receiver.fetch_sub(1, SeqCst) == 1 {
            self.channel.close();
        }
    }
}

impl<F: Flavor> Unpin for Receiver<F> {}

pub struct TrySendError<T> {
    data: T,
    closed: bool,
}

impl<T> TrySendError<T> {
    pub fn data(self) -> T {
        self.data
    }

    pub fn is_closed(&self) -> bool {
        self.closed
    }

    pub fn is_full(&self) -> bool {
        !self.closed
    }
}

impl<T> fmt::Debug for TrySendError<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.closed {
            write!(f, "TrySendError(Closed)")
        } else {
            write!(f, "TrySendError(Full)")
        }
    }
}

impl<T> fmt::Display for TrySendError<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.closed {
            write!(f, "sending into a closed channel")
        } else {
            write!(f, "sending into a full channel")
        }
    }
}

pub enum TryRecvError<T> {
    Empty,
    Closed(Option<T>),
}

impl<T> fmt::Debug for TryRecvError<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => write!(f, "TryRecvError(Empty)"),
            Self::Closed(_) => write!(f, "TryRecvError(Closed)"),
        }
    }
}

impl<T> fmt::Display for TryRecvError<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => write!(f, "receiving from an empty channel"),
            Self::Closed(_) => write!(f, "receiving from a closed channel"),
        }
    }
}

pub struct SendError<T> {
    data: T,
}

impl<T> SendError<T> {
    pub fn data(self) -> T {
        self.data
    }
}

impl<T> fmt::Debug for SendError<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "SendError")
    }
}

impl<T> fmt::Display for SendError<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "sending into a closed channel")
    }
}

pub struct RecvError<T> {
    data: Option<T>,
}

impl<T> RecvError<T> {
    pub fn data(self) -> Option<T> {
        self.data
    }
}

impl<T> fmt::Debug for RecvError<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "RecvError")
    }
}

impl<T> fmt::Display for RecvError<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "receiving from a closed channel")
    }
}

pub fn with_flavor<F: Flavor>(queue: F) -> (Sender<F>, Receiver<F>) {
    let channel = Arsc::new(Channel {
        queue,
        send: Event::new(),
        recv: Event::new(),
        closed: AtomicBool::new(false),
        sender: AtomicUsize::new(1),
        receiver: AtomicUsize::new(1),
    });
    (
        Sender {
            channel: channel.clone(),
        },
        Receiver { channel },
    )
}

pub fn bounded<T>(capacity: usize) -> (Sender<ArrayQueue<T>>, Receiver<ArrayQueue<T>>) {
    self::with_flavor(ArrayQueue::new(capacity))
}

pub fn unbounded<T>() -> (Sender<SegQueue<T>>, Receiver<SegQueue<T>>) {
    self::with_flavor(SegQueue::new())
}

// #[cfg(test)]
// mod tests {
//     use core::time::Duration;
//     use std::{sync::mpsc, thread};

//     use futures_lite::StreamExt;
//     use ktime::{sleep, timer_tick, Instant};

//     use super::*;
//     #[test]
//     fn test_channel() {
//         let (ticker_tx, rx) = mpsc::channel();
//         let ticker = thread::spawn(move || loop {
//             let try_recv = rx.try_recv();
//             if try_recv.is_ok() {
//                 break;
//             }
//             timer_tick()
//         });
//         let duration = Duration::from_millis(10);
//         spin_on::spin_on(async {
//             let (tx, rx) = bounded(1);
//             let instant = Instant::now();
//             assert!(tx.send(()).await.is_ok());
//             let rx = smol::spawn(async move {
//                 sleep(duration).await;
//                 let count = rx.streamed().count().await;
//                 assert_eq!(count, 3);
//             });
//             assert!(tx.send(()).await.is_ok());
//             let delta = instant.elapsed() - duration;
//             // CI executes tests very slow, so stop checking its value.
//             assert!(delta > Duration::ZERO);
//             assert!(tx.send(()).await.is_ok());
//             drop(tx);
//             rx.await;
//         });
//         ticker_tx.send(()).unwrap();
//         ticker.join().unwrap();
//     }
// }
