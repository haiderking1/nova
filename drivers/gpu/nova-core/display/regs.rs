// SPDX-License-Identifier: GPL-2.0

//! Display engine registers.

use kernel::io::register;

use crate::driver::NovaRegisters;

// UDISP: the user area of the display channels (`NV_UDISP_FE_CHN_ASSY_BASEADR_*` in
// `dev_disp.h`), through which the CPU tells a channel how far its push buffer was written, and
// learns how far the display engine read it (`NvC67DDispControlDma` in `clc67d.h`).

register! {
    base: NovaRegisters;

    /// PUT pointer of the core channel: the push buffer offset up to which the CPU wrote methods.
    pub(super) NV_UDISP_CORE_PUT(u32) @ 0x00680000 {
        /// Offset in 32-bit words.
        11:2    ptr;
    }

    /// GET pointer of the core channel: the push buffer offset up to which the display engine
    /// read methods.
    pub(super) NV_UDISP_CORE_GET(u32) @ 0x00680004 {
        /// Offset in 32-bit words.
        11:2    ptr;
    }

    /// PUT pointers of the window channels (`NV_UDISP_FE_CHN_ASSY_BASEADR_WIN`).
    pub(super) NV_UDISP_WIN_PUT(u32)[32, stride = 0x1000] @ 0x00690000 {
        /// Offset in 32-bit words.
        11:2    ptr;
    }

    /// GET pointers of the window channels.
    pub(super) NV_UDISP_WIN_GET(u32)[32, stride = 0x1000] @ 0x00690004 {
        /// Offset in 32-bit words.
        11:2    ptr;
    }
}

register! {
    base: NovaRegisters;

    /// Armed state of the core channel: the values of its methods the display engine currently
    /// uses, each at the offset of its method (`NV_UDISP_FE_CHN_ARMED_BASEADR_CORE`).
    pub(super) NV_UDISP_CORE_ARMED(u32)[0x2000] @ 0x00688000 {
        31:0    value;
    }
}

// Head timing interrupts (`NV_PDISP_FE_*_HEAD_TIMING` in `dev_disp.h`, v03_00, and v04_00 for the
// enables). A head's LAST_DATA event fires once it read the last pixel of a frame: at the start of
// vertical blanking.

register! {
    base: NovaRegisters;

    /// Heads with a head timing interrupt pending, one bit per head.
    pub(super) NV_PDISP_FE_RM_INTR_DISPATCH(u32) @ 0x00611ec0 {
        7:0     head_timing;
    }

    /// Head timing interrupts pending for the CPU, per head.
    pub(super) NV_PDISP_FE_RM_INTR_STAT_HEAD_TIMING(u32)[8, stride = 4] @ 0x00611c00 {
        1:1     last_data => bool;
    }

    /// Head timing events, per head: writing 1 to an event clears it.
    pub(super) NV_PDISP_FE_EVT_STAT_HEAD_TIMING(u32)[8, stride = 4] @ 0x00611800 {
        1:1     last_data => bool;
    }

    /// Enables of the head timing interrupts for the CPU, per head.
    pub(super) NV_PDISP_FE_RM_INTR_EN_HEAD_TIMING(u32)[8, stride = 4] @ 0x00611d80 {
        1:1     last_data => bool;
    }
}

register! {
    base: NovaRegisters;

    /// Assembly state of the core channel: the values of its methods the next update makes take
    /// effect, each at the offset of its method (`NV_UDISP_FE_CHN_ASSY_BASEADR_CORE`). The first
    /// words, below the core channel's first method, are the PUT and GET pointers.
    pub(super) NV_UDISP_CORE_ASSEMBLY(u32)[0x2000] @ 0x00680000 {
        31:0    value;
    }
}

// Exceptions of the display channels (`gv100_disp_exception()` in nouveau), one slot per channel:
// the core channel's is 0, window `w`'s is `w + 1`. GSP-RM services and clears them; Nova only
// reads them, to report why an update failed.

register! {
    base: NovaRegisters;

    /// The exception of a channel: the method it happened on, and why.
    pub(super) NV_PDISP_FE_EXCEPT(u32)[33, stride = 12] @ 0x00611020 {
        /// Offset of the method, in 32-bit words.
        11:0    method;
        /// Reason of the exception.
        14:12   reason;
    }

    /// Data of the method an exception happened on.
    pub(super) NV_PDISP_FE_EXCEPTARG(u32)[33, stride = 12] @ 0x00611024 {
        31:0    data;
    }

    /// Error code of an exception.
    pub(super) NV_PDISP_FE_EXCEPTERR(u32)[33, stride = 12] @ 0x00611028 {
        31:0    code;
    }
}
