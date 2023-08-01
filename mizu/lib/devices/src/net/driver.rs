use core::task::Context;

use smoltcp::phy;

use crate::Token;

pub trait Net: Send + Sync {
    fn features(&self) -> Features;

    fn address(&self) -> [u8; 6];

    fn ack_interrupt(&self);

    fn is_link_up(&self) -> bool;

    fn queues(&mut self) -> (&mut dyn NetTx, &mut dyn NetRx);
}

impl<T: Net + ?Sized> Net for &'_ mut T {
    fn features(&self) -> Features {
        (**self).features()
    }

    fn address(&self) -> [u8; 6] {
        (**self).address()
    }

    fn ack_interrupt(&self) {
        (**self).ack_interrupt()
    }

    fn is_link_up(&self) -> bool {
        (**self).is_link_up()
    }

    fn queues(&mut self) -> (&mut dyn NetTx, &mut dyn NetRx) {
        (**self).queues()
    }
}

pub trait NetTx: Send {
    fn tx_peek(&mut self, cx: &mut Context<'_>) -> Option<Token>;
    fn tx_buffer(&mut self, token: &Token) -> &mut [u8];
    fn transmit(&mut self, token: Token, len: usize);
}

pub trait NetRx: Send {
    fn rx_peek(&mut self, cx: &mut Context<'_>) -> Option<Token>;
    fn rx_buffer(&mut self, token: &Token) -> &mut [u8];
    fn receive(&mut self, token: Token);
}

pub(in crate::net) trait NetExt: Net {
    fn with_cx<'device, 'cx>(
        &'device mut self,
        cx: Option<&'device mut Context<'cx>>,
    ) -> WithCx<'device, 'cx, Self> {
        WithCx { cx, device: self }
    }
}
impl<T: Net + ?Sized> NetExt for T {}

#[derive(Debug, Clone, Copy, Default)]
#[non_exhaustive]
pub struct Features {
    pub max_unit: usize,
}

pub(in crate::net) struct WithCx<'device, 'cx, T: Net + ?Sized> {
    cx: Option<&'device mut Context<'cx>>,
    device: &'device mut T,
}

pub(in crate::net) struct TxToken<'driver> {
    device: &'driver mut dyn NetTx,
    token: Token,
}

pub(in crate::net) struct RxToken<'driver> {
    device: &'driver mut dyn NetRx,
    token: Token,
}

impl<'device, 'cx, T: Net + ?Sized> phy::Device for WithCx<'device, 'cx, T> {
    type RxToken<'a> = RxToken<'a>
    where
        Self: 'a;

    type TxToken<'a> = TxToken<'a>
    where
        Self: 'a;

    fn receive(
        &mut self,
        _: smoltcp::time::Instant,
    ) -> Option<(Self::RxToken<'_>, Self::TxToken<'_>)> {
        let (tx, rx) = self.device.queues();
        match (
            tx.tx_peek(self.cx.as_mut().unwrap()),
            rx.rx_peek(self.cx.as_mut().unwrap()),
        ) {
            (Some(tx_token), Some(rx_token)) => Some((
                RxToken {
                    device: rx,
                    token: rx_token,
                },
                TxToken {
                    device: tx,
                    token: tx_token,
                },
            )),
            _ => None,
        }
    }

    fn transmit(&mut self, _: smoltcp::time::Instant) -> Option<Self::TxToken<'_>> {
        let net_tx = self.device.queues().0;
        let token = net_tx.tx_peek(self.cx.as_mut().unwrap())?;
        Some(TxToken {
            device: net_tx,
            token,
        })
    }

    fn capabilities(&self) -> phy::DeviceCapabilities {
        let Features { max_unit } = self.device.features();

        let mut ret = phy::DeviceCapabilities::default();
        ret.max_transmission_unit = max_unit;
        ret
    }
}

impl phy::TxToken for TxToken<'_> {
    fn consume<R, F>(self, len: usize, f: F) -> R
    where
        F: FnOnce(&mut [u8]) -> R,
    {
        let buf = self.device.tx_buffer(&self.token);
        let res = f(&mut buf[..len]);
        self.device.transmit(self.token, len);
        res
    }
}

impl phy::RxToken for RxToken<'_> {
    fn consume<R, F>(self, f: F) -> R
    where
        F: FnOnce(&mut [u8]) -> R,
    {
        let res = f(self.device.rx_buffer(&self.token));
        self.device.receive(self.token);
        res
    }
}
