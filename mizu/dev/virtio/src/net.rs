use core::{mem, task::Context};

use arsc_rs::Arsc;
use atomic_refcell::AtomicRefCell;
use devices::{
    net::{Features, Net, NetRx, NetTx},
    Token,
};
use futures_util::task::AtomicWaker;
use virtio_drivers::{
    device::net::{VirtIONetRaw, VirtioNetHdr},
    transport::mmio::MmioTransport,
};

use crate::HalImpl;

const NET_HDR_SIZE: usize = core::mem::size_of::<VirtioNetHdr>();

const TX_BUFFER_LEN: usize = 1514;
const RX_BUFFER_LEN: usize = 1536;

pub struct VirtioNet<const LEN: usize> {
    tx: TxRing<LEN>,
    rx: RxRing<LEN>,
    available: Arsc<AtomicWaker>,
    device: Arsc<AtomicRefCell<VirtIONetRaw<HalImpl, MmioTransport, LEN>>>,
}
unsafe impl<const LEN: usize> Send for VirtioNet<LEN> {}
unsafe impl<const LEN: usize> Sync for VirtioNet<LEN> {}

const FREE_END: usize = isize::MAX as usize;

pub struct TxRing<const LEN: usize> {
    buffers: [[u8; TX_BUFFER_LEN]; LEN],
    indices: [usize; LEN],
    free_head: usize,
    available: Arsc<AtomicWaker>,
    device: Arsc<AtomicRefCell<VirtIONetRaw<HalImpl, MmioTransport, LEN>>>,
}
unsafe impl<const LEN: usize> Send for TxRing<LEN> {}

pub struct RxRing<const LEN: usize> {
    buffers: [[u8; RX_BUFFER_LEN]; LEN],
    len: [(usize, usize); LEN],
    available: Arsc<AtomicWaker>,
    device: Arsc<AtomicRefCell<VirtIONetRaw<HalImpl, MmioTransport, LEN>>>,
}
unsafe impl<const LEN: usize> Send for RxRing<LEN> {}

impl<const LEN: usize> VirtioNet<LEN> {
    pub fn new(transport: MmioTransport) -> virtio_drivers::Result<Self> {
        let device = Arsc::new(AtomicRefCell::new(VirtIONetRaw::new(transport)?));
        let available = Arsc::new(AtomicWaker::new());

        let mut tx_buffer = [[0; TX_BUFFER_LEN]; LEN];
        for (index, buf) in tx_buffer.iter_mut().enumerate() {
            buf[..mem::size_of::<usize>()].copy_from_slice(&(index + 1).to_le_bytes())
        }
        let tx = TxRing {
            buffers: tx_buffer,
            indices: [0; LEN],
            free_head: 0,
            available: available.clone(),
            device: device.clone(),
        };

        let rx = RxRing {
            buffers: [[0; RX_BUFFER_LEN]; LEN],
            len: [(0, 0); LEN],
            available: available.clone(),
            device: device.clone(),
        };

        Ok(VirtioNet {
            tx,
            rx,
            available,
            device,
        })
    }

    pub fn startup(&mut self) {
        ksync::critical(|| {
            let mut device = self.device.borrow_mut();
            for index in 0..LEN {
                let token = unsafe { device.receive_begin(&mut self.rx.buffers[index]) };
                assert_eq!(token, Ok(index as u16))
            }
        })
    }
}

impl<const LEN: usize> Net for VirtioNet<LEN> {
    fn features(&self) -> Features {
        let mut features = Features::default();
        features.max_unit = TX_BUFFER_LEN;
        features
    }

    fn address(&self) -> [u8; 6] {
        self.device.borrow().mac_address()
    }

    fn ack_interrupt(&self) {
        self.available.wake()
    }

    fn is_link_up(&self) -> bool {
        true
    }

    fn queues(&mut self) -> (&mut dyn NetTx, &mut dyn NetRx) {
        (&mut self.tx, &mut self.rx)
    }
}

impl<const LEN: usize> NetTx for TxRing<LEN> {
    fn tx_peek(&mut self, cx: &mut Context<'_>) -> Option<Token> {
        ksync::critical(|| {
            let mut device = self.device.borrow_mut();
            while let Some(token) = device.poll_transmit() {
                let index = self.indices[usize::from(token)];
                let res = unsafe { device.transmit_complete(token, &self.buffers[index]) };
                if res.is_ok() {
                    self.buffers[index][..mem::size_of::<usize>()]
                        .copy_from_slice(&self.free_head.to_le_bytes());
                    self.free_head = index;
                }
            }
        });

        if self.free_head != FREE_END {
            Some(Token(self.free_head))
        } else {
            self.available.register(cx.waker());
            None
        }
    }

    fn tx_buffer(&mut self, token: Token) -> &mut [u8] {
        if self.free_head == token.0 {
            let (bytes, _) = self.buffers[self.free_head].split_array_mut();
            self.free_head = usize::from_le_bytes(*bytes);
            bytes.fill(0);
            ksync::critical(|| {
                self.device
                    .borrow()
                    .fill_buffer_header(&mut self.buffers[token.0])
            })
            .unwrap();
        }
        &mut self.buffers[token.0][NET_HDR_SIZE..]
    }

    fn transmit(&mut self, token: Token, len: usize) {
        let raw = ksync::critical(|| unsafe {
            let mut device = self.device.borrow_mut();
            device.transmit_begin(&self.buffers[token.0][..(len + NET_HDR_SIZE)])
        })
        .unwrap();
        self.indices[usize::from(raw)] = token.0;
    }
}

impl<const RX: usize> NetRx for RxRing<RX> {
    fn rx_peek(&mut self, cx: &mut Context<'_>) -> Option<Token> {
        ksync::critical(|| {
            let mut device = self.device.borrow_mut();
            while let Some(token) = device.poll_receive() {
                let index = usize::from(token);
                let res = unsafe { device.receive_complete(token, &mut self.buffers[index]) };
                if let Ok(len) = res {
                    self.len[index] = len;
                    return Some(Token(index));
                }
            }
            self.available.register(cx.waker());
            None
        })
    }

    fn rx_buffer(&mut self, token: Token) -> &mut [u8] {
        let (start, len) = self.len[token.0];
        &mut self.buffers[token.0][start..][..len]
    }

    fn receive(&mut self, token: Token) {
        let raw = ksync::critical(|| unsafe {
            let mut device = self.device.borrow_mut();
            device.receive_begin(&mut self.buffers[token.0])
        });
        assert_eq!(raw, Ok(token.0 as u16))
    }
}
