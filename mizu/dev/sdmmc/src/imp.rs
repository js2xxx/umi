mod mars;

use alloc::vec::Vec;
use core::{
    fmt, mem,
    ops::Range,
    ptr::NonNull,
    task::{ready, Context, Poll},
    time::Duration,
};

use bit_struct::{u12, u2, u3, u4, u6};
use devices::intr::Completion;
use futures_util::task::AtomicWaker;
use ksc::Error::{self, EILSEQ, EINVAL, EIO, ENODEV, ETIMEDOUT};
use ktime::Instant;
use sdio_host::{
    common_cmd::{self, stop_transmission, Resp, ResponseLen},
    sd_cmd, Cmd,
};
use volatile::{map_field, VolatilePtr};

use crate::{
    adma::DescTable,
    reg::{
        AdmaErrorStatus, AutoCmdError, BlockSize, Capabilities, ClockControl, Command, DmaSelect,
        Interrupt, PowerControl, RespType, SdmmcRegs, SoftwareReset, TimeoutControl, TransferMode,
    },
    Data, SdmmcInfo,
};

const fn resp_type(resp: ResponseLen, is_busy: bool) -> RespType {
    match resp {
        ResponseLen::Zero => RespType::Zero,
        ResponseLen::R48 if is_busy => RespType::L48Busy,
        ResponseLen::R48 => RespType::L48,
        ResponseLen::R136 => RespType::L136,
    }
}

fn stall(dur: Duration) {
    let start = Instant::now();
    while start.elapsed() < dur {}
}

pub trait RespExt: Resp {
    type Repr: Copy = u32;

    fn from_raw(resp: [u32; 4]) -> Self::Repr;
}

impl RespExt for common_cmd::Rz {
    type Repr = ();

    fn from_raw(_: [u32; 4]) -> Self::Repr {}
}
impl RespExt for common_cmd::R1 {
    fn from_raw([r0, ..]: [u32; 4]) -> Self::Repr {
        r0
    }
}
impl RespExt for common_cmd::R2 {
    type Repr = [u32; 4];
    fn from_raw(resp: [u32; 4]) -> Self::Repr {
        resp
    }
}
impl RespExt for common_cmd::R3 {
    fn from_raw([r0, ..]: [u32; 4]) -> Self::Repr {
        r0
    }
}
impl RespExt for sd_cmd::R6 {
    fn from_raw([r0, ..]: [u32; 4]) -> Self::Repr {
        r0
    }
}
impl RespExt for sd_cmd::R7 {
    fn from_raw([r0, ..]: [u32; 4]) -> Self::Repr {
        r0
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub enum CapacityType {
    #[default]
    Standard,
    High,
}

#[derive(Debug)]
pub struct Inner {
    regs: VolatilePtr<'static, SdmmcRegs>,
    caps: Capabilities,

    pub info: SdmmcInfo,
    pub block_shift: u32,

    dma_table: DescTable,

    intr_enable: Interrupt,
    clock_div: u64,
    timeout_clock: u64,

    working_cmd: Option<WorkingCmd>,
    resp_slot: Option<Result<[u32; 4], Error>>,
    data_slot: Option<DataSlot>,

    cmd_idle: AtomicWaker,
    data_idle: AtomicWaker,
    cmd_finished: AtomicWaker,
    data_finished: AtomicWaker,
}

struct WorkingCmd {
    index: u8,
    resp: ResponseLen,
    has_data: bool,
    is_busy: bool,
}

impl fmt::Debug for WorkingCmd {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let resp = match self.resp {
            ResponseLen::Zero => &"Zero",
            ResponseLen::R48 => &"R48",
            ResponseLen::R136 => &"R136",
        };
        f.debug_struct("WorkingCmd")
            .field("index", &self.index)
            .field("resp", resp)
            .field("has_data", &self.has_data)
            .field("is_busy", &self.is_busy)
            .finish()
    }
}

#[derive(Debug)]
struct DataSlot {
    buffer: Vec<u8>,
    bytes_transfered: usize,
    is_read: bool,
    res: Option<Result<usize, Error>>,
}

unsafe impl Send for Inner {}

impl Inner {
    pub unsafe fn new(base: NonNull<()>) -> Result<Self, Error> {
        Ok(Inner {
            regs: VolatilePtr::new(base.cast()),
            caps: Capabilities::of_defaults(),
            info: Default::default(),
            block_shift: 0,
            dma_table: DescTable::new()?,
            intr_enable: Interrupt::empty(),
            clock_div: 0,
            timeout_clock: 0,
            working_cmd: None,
            resp_slot: None,
            data_slot: None,
            cmd_idle: Default::default(),
            data_idle: Default::default(),
            cmd_finished: Default::default(),
            data_finished: Default::default(),
        })
    }

    pub fn init_bus(&mut self, bus_width: usize, clock_freqs: Range<u64>) -> Result<u32, Error> {
        let regs = &mut self.regs;

        Self::power_impl(regs, PowerControl::empty());

        Self::reset(regs, SoftwareReset::all());
        mars::reset(regs);

        Self::power_impl(regs, PowerControl::VOLTAGE_3_0V | PowerControl::POWER_ON);

        self.caps = map_field!(regs.capabilities).read();
        log::info!("SD card {:?}", self.caps);

        // Use ADMA2 & 64-bit addressing v4 by default.
        assert!(
            self.caps.adma2_support().get(),
            "SDMMC doesn't support ADMA2"
        );
        assert!(
            self.caps.system_address64_support_v4().get(),
            "SDMMC doesn't support 64-bit addressing version 4"
        );

        map_field!(regs.host_control_2).update(|mut hc2| {
            hc2.host_v4_enable().set(true);
            hc2
        });

        map_field!(regs.host_control_1).update(|mut hc1| {
            hc1.high_speed_enable().set(true);
            match bus_width {
                4 => hc1.bus_width_4bit().set(true),
                8 => hc1.bus_width_8bit().set(true),
                _ => {}
            }
            hc1
        });

        self.block_shift = self.caps.max_block_len().get().value() as u32 + 9;

        self.intr_enable =
            Interrupt::CMD_MASK | Interrupt::DATA_MASK | Interrupt::CURRENT_LIMIT_ERR;
        map_field!(regs.intr_status_enable).write(self.intr_enable);
        map_field!(regs.intr_signal_enable).write(self.intr_enable);

        self.clock_div =
            (self.caps.sd_clock_base_freq().get() as u64) * 1_000_000 / (2 * clock_freqs.start);
        Self::clock(regs, Some(self.clock_div));

        let value = self.caps.timeout_clock_freq().get().value() as u64;
        self.timeout_clock = if self.caps.timeout_clock_unit().get() {
            value * 1_000
        } else {
            value
        };

        Ok(self.block_shift)
    }
}

impl Inner {
    pub fn is_present(&mut self) -> bool {
        let regs = &mut self.regs;
        map_field!(regs.present_state).read().card_inserted().get()
    }

    pub fn reset(regs: &mut VolatilePtr<'_, SdmmcRegs>, reset: SoftwareReset) {
        map_field!(regs.software_reset).write(reset);
        let dur = Duration::from_millis(100);
        let now = Instant::now();
        while map_field!(regs.software_reset).read().contains(reset) {
            if now.elapsed() > dur {
                log::error!("Reset {reset:?} on {:p} failed", regs.as_raw_ptr());
                break;
            }
        }
    }

    fn power_impl(regs: &mut VolatilePtr<'_, SdmmcRegs>, power: PowerControl) {
        if power.contains(PowerControl::POWER_ON) {
            map_field!(regs.power_control).write(power);
            mars::power_on_epilog(regs);
            stall(Duration::from_millis(5));
        } else {
            mars::power_off_prolog(regs);
            map_field!(regs.power_control).write(power);
            stall(Duration::from_millis(20));
        }
    }

    fn clock(regs: &mut VolatilePtr<'_, SdmmcRegs>, div: Option<u64>) {
        let mut clock = ClockControl::of_defaults();
        let Some(div) = div else {
            map_field!(regs.clock_control).write(clock);
            return;
        };

        clock.internal_clock_enable().set(true);
        map_field!(regs.clock_control).write(clock);

        loop {
            let mut read = map_field!(regs.clock_control).read();
            if read.internal_clock_stable().get() {
                break;
            }
            core::hint::spin_loop()
        }

        clock.frequency_select_lo().set(div as u8);
        clock
            .frequency_select_hi()
            .set(u2::new((div >> 8) as u8).unwrap());

        clock.sd_clock_enable().set(true);
        map_field!(regs.clock_control).write(clock);
        stall(Duration::from_micros(50));
    }

    pub fn switch_voltage(&mut self) -> Result<(), Error> {
        let regs = &mut self.regs;
        Self::clock(regs, None);

        let mut state = map_field!(regs.present_state).read();
        if state.data_line_signal_level_lo().get().value() != 0 {
            return Err(EIO);
        }

        map_field!(regs.host_control_2).update(|mut hc2| {
            hc2.signaling_1_8v_enable().set(true);
            hc2
        });
        mars::voltage_switch();
        stall(Duration::from_millis(10));

        let mut hc2 = map_field!(regs.host_control_2).read();
        if !hc2.signaling_1_8v_enable().get() {
            return Err(EIO);
        }
        map_field!(regs.host_control_2).update(|mut hc2| {
            hc2.uhs_mode().set(crate::reg::UhsMode::Sdr104);
            hc2
        });

        Self::clock(regs, Some(self.clock_div));
        stall(Duration::from_millis(1));

        state = map_field!(regs.present_state).read();
        if state.data_line_signal_level_lo().get().value() != 0b1111 {
            return Err(EIO);
        }

        Ok(())
    }

    fn poll_inhibit(&mut self, cx: &mut Context<'_>, occupies_data_line: bool) -> Poll<()> {
        let regs = &mut self.regs;

        let mut state = map_field!(regs.present_state).read();
        let inhibit_cmd = state.inhibit_cmd().get();
        let inhibit_data =
            (occupies_data_line && state.inhibit_data().get()) || self.data_slot.is_some();
        if inhibit_cmd || inhibit_data {
            if inhibit_cmd {
                self.cmd_idle.register(cx.waker());
            }
            if inhibit_data {
                self.data_idle.register(cx.waker());
            }
            return Poll::Pending;
        }
        Poll::Ready(())
    }

    fn send_data(&mut self, data: &mut Data) -> Result<(), Error> {
        let regs = &mut self.regs;

        let block_shift = data.block_shift.unwrap_or(self.block_shift);
        let block_size = u12::new(1 << block_shift).ok_or(EINVAL)?;

        let len = ((data.block_count as usize) << block_shift).min(DescTable::MAX_LEN);

        let buffer = data.buffer.get_mut(..len).ok_or(EINVAL)?;
        let filled = unsafe { self.dma_table.fill(buffer, data.is_read) };

        self.data_slot = Some(DataSlot {
            buffer: mem::take(&mut data.buffer),
            bytes_transfered: filled,
            is_read: data.is_read,
            res: None,
        });
        map_field!(regs.adma_system_address).write(*self.dma_table.dma_addr() as u64);
        map_field!(regs.host_control_1).update(|mut hc1| {
            hc1.dma_select().set(DmaSelect::Adma2);
            hc1.bus_width_4bit().set(true);
            hc1
        });

        self.intr_enable &= !(Interrupt::BUFFER_READ_READY | Interrupt::BUFFER_WRITE_READY);
        self.intr_enable |= Interrupt::ADMA_ERR | Interrupt::DMA;
        self.intr_enable |= Interrupt::AUTO_CMD_ERR;
        map_field!(regs.intr_status_enable).write(self.intr_enable);
        map_field!(regs.intr_signal_enable).write(self.intr_enable);

        map_field!(regs.block_size).write(BlockSize::new(false, u3!(7), block_size));
        map_field!(regs.block_count).write(data.block_count);

        Ok(())
    }

    fn set_transfer(
        &mut self,
        use_auto_cmd: bool,
        data: Option<&mut Data>,
    ) -> Result<TransferMode, Error> {
        Ok(if let Some(data) = data {
            self.send_data(data)?;
            let regs = &mut self.regs;

            let mut hc2 = map_field!(regs.host_control_2).read();
            hc2.cmd23_enable().set(use_auto_cmd);
            hc2.addressing_64bit().set(true);
            map_field!(regs.host_control_2).write(hc2);

            let mut transfer_mode = TransferMode::empty();
            transfer_mode.set(TransferMode::BLOCK_COUNT_ENABLE, true);
            transfer_mode.set(TransferMode::IS_MULTI_BLOCK, data.block_count > 1);
            transfer_mode.set(TransferMode::IS_READ, data.is_read);
            transfer_mode.set(TransferMode::AUTO_CMD23_ENABLE, use_auto_cmd);
            transfer_mode |= TransferMode::DMA_ENABLE;
            transfer_mode
        } else {
            let regs = &mut self.regs;
            map_field!(regs.transfer_mode).read() & !TransferMode::AUTO_CMD_MASK
        })
    }

    fn set_timeout(&mut self, is_busy: bool) {
        let regs = &mut self.regs;
        if is_busy {
            map_field!(regs.timeout_control).write(TimeoutControl::new(u4!(0), u4!(14)));
        }
        const MICROS: u64 = 1_000_000;

        // Default: 1 second timeout.
        let mut timeout = (1 << 13) * 1000 / self.timeout_clock;
        let mut count = 0;
        while timeout < MICROS && count < 15 {
            count += 1;
            timeout <<= 1;
        }

        let counter_value = if count >= 15 {
            u4!(14)
        } else {
            u4::new(count).unwrap()
        };
        map_field!(regs.timeout_control).write(TimeoutControl::new(u4!(0), counter_value));
    }

    pub fn send_cmd<R: Resp>(
        &mut self,
        cx: &mut Context<'_>,
        cmd: Cmd<R>,
        require_crc: bool,
        use_auto_cmd: bool,
        data: Option<&mut Data>,
    ) -> Poll<Result<(), Error>> {
        if !self.is_present() {
            return Poll::Ready(Err(ENODEV));
        }
        // log::trace!("Sending cmd {}, argument = {:#x}", cmd.cmd, cmd.arg);

        let has_data = data.is_some();
        let is_busy = cmd.cmd == stop_transmission().cmd;
        let index = u6::new(cmd.cmd).ok_or(EINVAL)?;

        ready!(self.poll_inhibit(cx, has_data || is_busy));

        let transfer_mode = self.set_transfer(use_auto_cmd, data)?;
        self.set_timeout(is_busy);

        let regs = &mut self.regs;

        map_field!(regs.argument).write(cmd.arg);
        map_field!(regs.transfer_mode).write(transfer_mode);

        let mut cmd_reg = Command::of_defaults();
        cmd_reg.resp_type().set(resp_type(R::LENGTH, is_busy));
        cmd_reg.crc_check().set(require_crc);
        cmd_reg.has_data().set(has_data);
        cmd_reg.cmd_index().set(index);

        map_field!(regs.command).write(cmd_reg);
        self.working_cmd = Some(WorkingCmd {
            index: index.value(),
            resp: R::LENGTH,
            has_data,
            is_busy,
        });
        Poll::Ready(Ok(()))
    }

    pub fn get_resp(&mut self, cx: &mut Context<'_>) -> Poll<Result<[u32; 4], Error>> {
        if !self.is_present() {
            return Poll::Ready(Err(ENODEV));
        }
        match self.resp_slot.take() {
            Some(resp) => {
                self.cmd_idle.wake();
                Poll::Ready(resp)
            }
            None => {
                self.cmd_finished.register(cx.waker());
                Poll::Pending
            }
        }
    }

    pub fn get_data(&mut self, cx: &mut Context<'_>) -> Poll<(Vec<u8>, Result<usize, Error>)> {
        if !self.is_present() {
            return Poll::Ready((
                match self.data_slot.take() {
                    Some(s) => s.buffer,
                    None => Default::default(),
                },
                Err(ENODEV),
            ));
        }
        match self.data_slot.as_mut() {
            Some(slot) => match slot.res.take() {
                Some(res) => {
                    let buffer = mem::take(&mut slot.buffer);
                    self.data_slot = None;
                    self.data_idle.wake();
                    Poll::Ready((buffer, res))
                }
                None => {
                    self.data_finished.register(cx.waker());
                    Poll::Pending
                }
            },
            None => Poll::Ready((Default::default(), Err(EILSEQ))),
        }
    }
}

impl Inner {
    fn ack_cmd_intr(&mut self, intr: &mut Interrupt) {
        fn complete_cmd(
            regs: &mut VolatilePtr<SdmmcRegs>,
            cmd: WorkingCmd,
            intr: &mut Interrupt,
        ) -> Option<Result<[u32; 4], Error>> {
            if intr.intersects(
                Interrupt::TIMEOUT
                    | Interrupt::CRC_ERR
                    | Interrupt::END_BIT_ERR
                    | Interrupt::INDEX_ERR,
            ) {
                let ret = if intr.contains(Interrupt::TIMEOUT) {
                    ETIMEDOUT
                } else {
                    EILSEQ
                };

                if cmd.has_data
                    && (*intr & (Interrupt::CRC_ERR | Interrupt::TIMEOUT)) == Interrupt::CRC_ERR
                {
                    *intr |= Interrupt::DATA_CRC_ERR;
                    return None;
                }

                return Some(Err(ret));
            }

            if intr.contains(Interrupt::AUTO_CMD_ERR) {
                let status = map_field!(regs.auto_cmd_error_status).read();
                return Some(Err(if status.contains(AutoCmdError::TIMEOUT) {
                    ETIMEDOUT
                } else {
                    EILSEQ
                }));
            }

            if !intr.contains(Interrupt::CMD_COMPLETE) {
                return None;
            }

            let [r0, r1, r2, r3] = map_field!(regs.resp).read();
            map_field!(regs.resp).write([0; 4]);
            let resp = match cmd.resp {
                ResponseLen::Zero => [0; 4],
                ResponseLen::R48 => [r0, 0, 0, 0],
                ResponseLen::R136 => [
                    r0 << 8,
                    r1 << 8 | r0 >> 24,
                    r2 << 8 | r1 >> 24,
                    r3 << 8 | r2 >> 24,
                ],
            };
            Some(Ok(resp))
        }

        let regs = &mut self.regs;

        let Some(cmd) = self.working_cmd.take() else {
            log::warn!("Unexpected SDMMC command completion: {intr:?}");
            return;
        };

        let res = complete_cmd(regs, cmd, intr);
        if let Some(res) = res {
            self.resp_slot = Some(res);
            self.cmd_finished.wake();
        }
    }

    fn ack_data_intr(&mut self, intr: Interrupt) {
        let Some(data) = self.data_slot.as_mut() else {
            log::error!("Unexpected transfer completion: {intr:?}");
            return;
        };

        if intr.contains(Interrupt::DATA_TIMEOUT) && !intr.contains(Interrupt::TRANSFER_COMPLETE) {
            data.res = Some(Err(ETIMEDOUT));
        } else if intr.intersects(Interrupt::DATA_END_BIT_ERR | Interrupt::DATA_CRC_ERR) {
            data.res = Some(Err(EILSEQ));
        } else if intr.contains(Interrupt::ADMA_ERR) {
            let regs = &mut self.regs;
            map_field!(regs.adma_error_status).write(AdmaErrorStatus::of_defaults());
            data.res = Some(Err(EIO));
        }

        if data.res.map_or(false, |r| r.is_err()) {
            Self::reset(&mut self.regs, SoftwareReset::DATA);
            data.bytes_transfered = 0;
            self.data_finished.wake();
            return;
        }

        assert!(
            !intr.intersects(Interrupt::BUFFER_READ_READY | Interrupt::BUFFER_WRITE_READY),
            "Unexpected non-DMA operation"
        );

        if intr.contains(Interrupt::TRANSFER_COMPLETE) {
            unsafe { self.dma_table.extract(&mut data.buffer, data.is_read) };
            data.res = Some(Ok(data.bytes_transfered));
            self.data_finished.wake();
        }
    }

    pub fn ack_interrupt(&mut self, completion: &Completion) -> bool {
        loop {
            let regs = &mut self.regs;
            let mut intr = map_field!(regs.intr_status).read();
            if matches!(intr.bits(), 0 | u32::MAX) {
                completion();
                break true;
            }
            // log::trace!("Sd card: {intr:?}");

            let mask =
                intr & (Interrupt::CMD_MASK | Interrupt::DATA_MASK | Interrupt::CURRENT_LIMIT_ERR);
            map_field!(regs.intr_status).write(mask);

            if intr.intersects(Interrupt::INSERTION | Interrupt::REMOVAL) {
                let present = map_field!(regs.present_state).read().card_inserted().get();

                self.intr_enable = if present {
                    (self.intr_enable | Interrupt::REMOVAL) & !Interrupt::INSERTION
                } else {
                    (self.intr_enable | Interrupt::INSERTION) & !Interrupt::REMOVAL
                };

                map_field!(regs.intr_status_enable).write(self.intr_enable);
                map_field!(regs.intr_signal_enable).write(self.intr_enable);

                self.cmd_idle.wake();
                self.data_idle.wake();
                self.cmd_finished.wake();
                self.data_finished.wake();
            }

            if intr.intersects(Interrupt::CMD_MASK) {
                self.ack_cmd_intr(&mut intr)
            }
            if intr.intersects(Interrupt::DATA_MASK) {
                self.ack_data_intr(intr)
            }
        }
    }
}
