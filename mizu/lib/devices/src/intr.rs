use crossbeam_queue::SegQueue;
use ksync::channel::mpmc::{Receiver, TryRecvError};

#[derive(Clone)]
pub struct Interrupt(pub Receiver<SegQueue<()>>);

impl Interrupt {
    pub async fn wait(&self) -> bool {
        self.0.recv().await.is_ok()
    }

    pub fn try_wait(&self) -> Option<bool> {
        match self.0.try_recv() {
            Ok(_) | Err(TryRecvError::Closed(Some(_))) => Some(true),
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Closed(None)) => Some(false),
        }
    }
}
