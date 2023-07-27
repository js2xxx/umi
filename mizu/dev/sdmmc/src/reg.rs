use ::bit_struct::enums;
use bit_struct::*;
use bitflags::bitflags;

#[derive(Debug, Clone, Copy)]
#[repr(C)]
pub struct SdmmcRegs {
    pub sdma_system_address: u32,

    pub block_size: BlockSize,
    pub block_count: u16,

    pub argument: u32,

    pub transfer_mode: TransferMode,
    pub command: Command,

    pub resp: [u32; 4],
    pub buf_data: u32,
    pub present_state: PresentState,

    pub host_control_1: HostControl1,
    pub power_control: PowerControl,
    pub block_gap_control: BlockGapControl,
    pub wakeup_control: WakeupControl,

    pub clock_control: ClockControl,
    pub timeout_control: TimeoutControl,
    pub software_reset: SoftwareReset,

    pub intr_status: Interrupt,

    pub intr_status_enable: Interrupt,

    pub intr_signal_enable: Interrupt,

    pub auto_cmd_error_status: AutoCmdError,
    pub host_control_2: HostControl2,

    pub capabilities: Capabilities,
    pub max_current_capabilities: [u32; 2],

    pub force_event_for_acmd_error_status: u16,
    pub force_event_for_error_intr_status: u16,

    pub adma_error_status: AdmaErrorStatus,
    pub _reserved1: [u8; 3],

    pub adma_system_address: u64,
}

// NOTE: Remember the macro implementation follows the big-endian direction.
bit_struct! {
    pub struct BlockSize(u16) {
        _reserved: bool,
        sdma_buffer_boundary: u3,
        block_size: u12,
    }
}

bitflags! {
    #[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
    pub struct TransferMode: u16 {
        const DMA_ENABLE            = 1 << 0;
        const BLOCK_COUNT_ENABLE    = 1 << 1;

        const AUTO_CMD12_ENABLE     = 1 << 2;
        const AUTO_CMD23_ENABLE     = 1 << 3;
        const AUTO_CMD_AUTO_SELECT  =
              Self::AUTO_CMD12_ENABLE.bits()
            | Self::AUTO_CMD23_ENABLE.bits();
        const AUTO_CMD_MASK = Self::AUTO_CMD_AUTO_SELECT.bits();

        const IS_READ               = 1 << 4;
        const IS_MULTI_BLOCK        = 1 << 5;
        const RESP_TYPE             = 1 << 6;
        const RESP_ERR_CHECK_ENABLE = 1 << 7;
        const RESP_INTR_DISABLE     = 1 << 8;
    }
}

bit_struct! {
    pub struct Command(u16) {
        _reserved: u2,
        cmd_index: u6,

        cmd_type: CmdType,
        has_data: bool,
        index_check: bool,
        crc_check: bool,
        is_sub_cmd: bool,
        resp_type: RespType,
    }
}

enums! {
    pub CmdType { Normal, Suspend, Resume, Abort }
    pub RespType { Zero, L136, L48, L48Busy }
}

bit_struct! {
    pub struct PresentState(u32) {
        uhs2_if_detection: bool,
        lane_synchronization: bool,
        is_dormant: bool,
        sub_cmd_status: bool,
        is_cmd_not_issued_by_error: bool,
        _reserved2: bool,
        host_regulator_voltage_stable: bool,
        cmd_line_signal_level: bool,

        data_line_signal_level_lo: u4,
        write_protect_switch_pin_level: bool,
        card_detect_pin_level: bool,
        card_state_stable: bool,
        card_inserted: bool,

        _reserved1: u4,
        buffer_read_enable: bool,
        buffer_write_enable: bool,
        read_transfer_active: bool,
        write_transfer_active: bool,

        data_line_signal_level_hi: u4,
        retuning_request: bool,
        data_line_active: bool,
        inhibit_data: bool,
        inhibit_cmd: bool,
    }
}

enums! {
    pub DmaSelect { Sdma, NotUsed, Adma2, Adma3 }
}

bit_struct! {
    pub struct HostControl1(u8) {
        card_detect_signal_select: bool,
        card_detect_test_level: bool,
        bus_width_8bit: bool,
        dma_select: DmaSelect,
        high_speed_enable: bool,
        bus_width_4bit: bool,
        led: bool,
    }
}

bitflags! {
    #[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
    pub struct PowerControl: u8 {
        const POWER_ON      = 0b0001;
        const VOLTAGE_1_8V  = 0b1010;
        const VOLTAGE_3_0V  = 0b1100;
    }
}

bit_struct! {
    pub struct BlockGapControl(u8) {
        _reserved: u4,
        intr_at_block_gap: bool,
        read_wait_control: bool,
        continue_request: bool,
        stop_at_block_gap_request: bool
    }

    pub struct WakeupControl(u8) {
        _reserved: u5,
        wakeup_on_removal: bool,
        wakeup_on_insertion: bool,
        wakeup_on_card_intr: bool,
    }

    pub struct ClockControl(u16) {
        frequency_select_lo: u8,
        frequency_select_hi: u2,
        clock_gen_select: bool,
        _reserved: bool,
        pll_enable: bool,
        sd_clock_enable: bool,
        internal_clock_stable: bool,
        internal_clock_enable: bool,
    }

    pub struct TimeoutControl(u8) {
        _reserved: u4,
        counter_value: u4
    }
}

// impl PresentState {
//     pub fn data_line_signal_level(&mut self) -> u8 {
//         self.data_line_signal_level_lo().get().inner_raw()
//             | (self.data_line_signal_level_hi().get().inner_raw() << 4)
//     }

//     pub fn set_data_line_signal_level(&mut self, level: u8) {
//         self.data_line_signal_level_lo()
//             .set(unsafe { u4::new_unchecked(level & 0xf) });
//         self.data_line_signal_level_hi()
//             .set(unsafe { u4::new_unchecked(level >> 4) })
//     }
// }

bitflags! {
    #[derive(Default, Debug, Clone, Copy, PartialEq, Eq)]
    pub struct SoftwareReset: u8 {
        const ALL = 1 << 0;
        const CMD = 1 << 1;
        const DATA = 1 << 2;
    }

    #[derive(Default, Debug, Clone, Copy, PartialEq, Eq)]
    pub struct Interrupt: u32 {
        const ADMA_ERR            = 1 << 25;
        const AUTO_CMD_ERR        = 1 << 24;
        const CURRENT_LIMIT_ERR   = 1 << 23;
        const DATA_END_BIT_ERR    = 1 << 22;
        const DATA_CRC_ERR        = 1 << 21;
        const DATA_TIMEOUT        = 1 << 20;
        const INDEX_ERR           = 1 << 19;
        const END_BIT_ERR         = 1 << 18;
        const CRC_ERR             = 1 << 17;
        const TIMEOUT             = 1 << 16;
        const ERROR               = 1 << 15;

        const FX                  = 1 << 13;
        const RETUNING            = 1 << 12;
        const INTR_C              = 1 << 11;
        const INTR_B              = 1 << 10;
        const INTR_A              = 1 << 9;
        const CARD_INTR           = 1 << 8;
        const REMOVAL             = 1 << 7;
        const INSERTION           = 1 << 6;
        const BUFFER_READ_READY   = 1 << 5;
        const BUFFER_WRITE_READY  = 1 << 4;
        const DMA                 = 1 << 3;
        const BLOCK_GAP           = 1 << 2;
        const TRANSFER_COMPLETE   = 1 << 1;
        const CMD_COMPLETE        = 1 << 0;

        const CMD_MASK = Self::CMD_COMPLETE
            .union(Self::TIMEOUT)
            .union(Self::CRC_ERR)
            .union(Self::INDEX_ERR)
            .union(Self::AUTO_CMD_ERR)
            .bits();

        const DATA_MASK = Self::TRANSFER_COMPLETE
            .union(Self::DMA)
            .union(Self::BUFFER_READ_READY)
            .union(Self::BUFFER_WRITE_READY)
            .union(Self::DATA_TIMEOUT)
            .union(Self::DATA_CRC_ERR)
            .union(Self::DATA_END_BIT_ERR)
            .union(Self::ADMA_ERR)
            .union(Self::BLOCK_GAP)
            .bits();
    }
}

bitflags! {
    #[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
    pub struct AutoCmdError: u16 {
        const NOT_EXECUTED  = 1 << 0;
        const TIMEOUT       = 1 << 1;
        const CRC           = 1 << 2;
        const END_BIT       = 1 << 3;
        const INDEX         = 1 << 4;
        const RESP          = 1 << 5;
        const NOT_ISSUED_BY_AUTO_CMD12  = 1 << 7;
    }
}

bit_struct! {
    pub struct HostControl2(u16) {
        preset_value_enable: bool,
        async_intr_enable: bool,
        addressing_64bit: bool,
        host_v4_enable: bool,
        cmd23_enable: bool,
        adma2_length_enable: bool,
        _reserved: bool,
        uhs2_interface_enable: bool,

        sampling_clock_select: bool,
        execute_tuning: bool,
        driver_strength_select: u2,
        signaling_1_8v_enable: bool,
        uhs_mode: UhsMode,
    }
}

enums! { pub UhsMode { Sdr12, Sdr25, Sdr50, Sdr104, Ddr50, Reserved1, Reserved2, Uhs2 } }

bit_struct! {
    pub struct Capabilities(u64) {
        _reserved1: u3,
        vdd2_1_8v_support: bool,
        adma3_support: bool,
        _reserved2: u3,
        clock_multiplier: u8,

        retuning_modes: u2,
        use_tuning_for_sdr50: bool,
        _reserved3: bool,
        timer_count_for_retuning: u4,
        _reserved4: bool,
        driver_type_d_support: bool,
        driver_type_c_support: bool,
        driver_type_a_support: bool,
        uhs2_support: bool,
        ddr50_support: bool,
        sdr104_support: bool,
        sdr50_support: bool,

        slot_type: u2,
        async_intr_support: bool,
        system_address64_support_v3: bool,
        system_address64_support_v4: bool,
        voltage_1_8v_support: bool,
        voltage_3_0v_support: bool,
        voltage_3_3v_support: bool,
        suspend_resume_support: bool,
        sdma_support: bool,
        high_speed_support: bool,
        _reserved5: bool,
        adma2_support: bool,
        embedded_8b_support: bool,
        max_block_len: u2,

        sd_clock_base_freq: u8,
        timeout_clock_unit: bool,
        _reserved6: bool,
        timeout_clock_freq: u6,
    }
}

bit_struct! {
    pub struct AdmaErrorStatus(u8) {
        _reserved: u5,
        length_mismatch: bool,
        error_state: AdmaErrorState,
    }
}

enums! { pub AdmaErrorState { Stop, Fetch, NotUsed, Transfering } }
