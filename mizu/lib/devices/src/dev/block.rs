use alloc::boxed::Box;

use arsc_rs::Arsc;
use async_trait::async_trait;
use ksc::Error;

use crate::Interrupt;

#[async_trait]
pub trait Block: Send + Sync {
    fn ack_interrupt(&self);

    async fn read(&self, block: usize, buf: &mut [u8]) -> Result<usize, Error>;

    async fn write(&self, block: usize, buf: &[u8]) -> Result<usize, Error>;

    async fn intr_dispatch(self: Arsc<Self>, intr: Interrupt) {
        loop {
            if !intr.wait().await {
                break;
            }
            self.ack_interrupt()
        }
    }
}
