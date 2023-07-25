use core::time::Duration;

use kmem::ID_OFFSET;
use volatile::{map_field, VolatilePtr};

use super::stall;
use crate::reg::{SdmmcRegs, UhsMode};

const TOP_BASE: usize = 0x300_0000 + ID_OFFSET;
const PINMUX_BASE: usize = TOP_BASE + 0x1000;

const TOP_SD_PWRSW_CTRL: usize = TOP_BASE + 0x1f4;

const PAD_SDIO0_CD_REG: usize = PINMUX_BASE + 0x34;
const PAD_SDIO0_PWR_EN_REG: usize = PINMUX_BASE + 0x38;
const PAD_SDIO0_CLK_REG: usize = PINMUX_BASE + 0x1C;
const PAD_SDIO0_CMD_REG: usize = PINMUX_BASE + 0x20;
const PAD_SDIO0_D0_REG: usize = PINMUX_BASE + 0x24;
const PAD_SDIO0_D1_REG: usize = PINMUX_BASE + 0x28;
const PAD_SDIO0_D2_REG: usize = PINMUX_BASE + 0x2C;
const PAD_SDIO0_D3_REG: usize = PINMUX_BASE + 0x30;

const REG_SDIO0_CD_PAD_REG: usize = PINMUX_BASE + 0x900;
const REG_SDIO0_PWR_EN_PAD_REG: usize = PINMUX_BASE + 0x904;
const REG_SDIO0_CLK_PAD_REG: usize = PINMUX_BASE + 0xA00;
const REG_SDIO0_CMD_PAD_REG: usize = PINMUX_BASE + 0xA04;
const REG_SDIO0_DAT0_PAD_REG: usize = PINMUX_BASE + 0xA08;
const REG_SDIO0_DAT1_PAD_REG: usize = PINMUX_BASE + 0xA0C;
const REG_SDIO0_DAT2_PAD_REG: usize = PINMUX_BASE + 0xA10;
const REG_SDIO0_DAT3_PAD_REG: usize = PINMUX_BASE + 0xA14;

const SDHCI_OFFSET: usize = 0x200;
const SDHCI_VENDOR_MSHC_CTRL_R: usize = SDHCI_OFFSET;
const SDHCI_PHY_TX_RX_DLY: usize = SDHCI_OFFSET + 0x40;
// const SDHCI_PHY_DS_DLY: usize = SDHCI_OFFSET + 0x44;
// const SDHCI_PHY_DLY_STS: usize = SDHCI_OFFSET + 0x48;
const SDHCI_PHY_CONFIG: usize = SDHCI_OFFSET + 0x4C;

macro_rules! map_vendor {
    ($regs:ident. $offset:ident) => {
        (($regs).as_raw_ptr().as_ptr() as usize) + $offset
    };
}

unsafe fn mmio_write(addr: usize, value: u32) {
    (addr as *mut u32).write_volatile(value)
}

unsafe fn mmio_read(addr: usize) -> u32 {
    (addr as *const u32).read_volatile()
}

unsafe fn mmio_clear_set(addr: usize, clear: u32, set: u32) {
    mmio_write(addr, (mmio_read(addr) & !clear) | set)
}

unsafe fn setup_pad(bunplug: bool) {
    let value = if bunplug { 3 } else { 0 };

    mmio_write(PAD_SDIO0_CD_REG, 0);
    mmio_write(PAD_SDIO0_PWR_EN_REG, 0);
    mmio_write(PAD_SDIO0_CLK_REG, value);
    mmio_write(PAD_SDIO0_CMD_REG, value);
    mmio_write(PAD_SDIO0_D0_REG, value);
    mmio_write(PAD_SDIO0_D1_REG, value);
    mmio_write(PAD_SDIO0_D2_REG, value);
    mmio_write(PAD_SDIO0_D3_REG, value);
}

unsafe fn setup_io(reset: bool) {
    let set = 1 << if reset { 3 } else { 2 };
    let clear = 1 << if reset { 2 } else { 3 };

    // Pad settings
    mmio_clear_set(REG_SDIO0_CD_PAD_REG, 1 << 3, 1 << 2);
    mmio_clear_set(REG_SDIO0_PWR_EN_PAD_REG, 1 << 2, 1 << 3);
    mmio_clear_set(REG_SDIO0_CLK_PAD_REG, 1 << 2, 1 << 3);

    mmio_clear_set(REG_SDIO0_CMD_PAD_REG, clear, set);
    mmio_clear_set(REG_SDIO0_DAT0_PAD_REG, clear, set);
    mmio_clear_set(REG_SDIO0_DAT1_PAD_REG, clear, set);
    mmio_clear_set(REG_SDIO0_DAT2_PAD_REG, clear, set);
    mmio_clear_set(REG_SDIO0_DAT3_PAD_REG, clear, set);
}

pub fn power_on_epilog(regs: &mut VolatilePtr<SdmmcRegs>) {
    unsafe { voltage_restore(false, regs) };
    unsafe { setup_pad(false) };
    unsafe { setup_io(false) };
}

pub fn power_off_prolog(regs: &mut VolatilePtr<SdmmcRegs>) {
    unsafe { voltage_restore(true, regs) };
    unsafe { setup_pad(true) };
    unsafe { setup_io(true) };
}

pub fn voltage_switch() {
    unsafe { mmio_clear_set(REG_SDIO0_CLK_PAD_REG, 0, (1 << 5) | (1 << 6) | (1 << 7)) };
    unsafe { mmio_write(TOP_SD_PWRSW_CTRL, 0b1011) }
}

unsafe fn voltage_restore(bunplug: bool, regs: &mut VolatilePtr<SdmmcRegs>) {
    mmio_clear_set(TOP_SD_PWRSW_CTRL, 0xf, if bunplug { 0xe } else { 0x9 });
    stall(Duration::from_millis(1));

    mmio_clear_set(
        map_vendor!(regs.SDHCI_VENDOR_MSHC_CTRL_R),
        0,
        (1 << 1) | (1 << 8) | (1 << 9),
    );
    mmio_write(map_vendor!(regs.SDHCI_PHY_TX_RX_DLY), 0x100_0100);
    mmio_write(map_vendor!(regs.SDHCI_PHY_CONFIG), 1);
}

pub fn reset(regs: &mut VolatilePtr<SdmmcRegs>) {
    let mut hc2 = map_field!(regs.host_control_2).read();
    if hc2.uhs_mode().get() == UhsMode::Sdr104 {
        unsafe {
            mmio_clear_set(map_vendor!(regs.SDHCI_VENDOR_MSHC_CTRL_R), 1 << 1, 0);
            mmio_clear_set(map_vendor!(regs.SDHCI_PHY_CONFIG), 1 << 0, 0);
            mmio_write(map_vendor!(regs.SDHCI_PHY_TX_RX_DLY), 1 << 8);
        }
    } else {
        unsafe {
            mmio_clear_set(map_vendor!(regs.SDHCI_VENDOR_MSHC_CTRL_R), 0, 1 << 1);
            mmio_clear_set(map_vendor!(regs.SDHCI_PHY_CONFIG), 0, 1 << 0);
            mmio_write(map_vendor!(regs.SDHCI_PHY_TX_RX_DLY), 0x100_0100);
        }
    }
}
