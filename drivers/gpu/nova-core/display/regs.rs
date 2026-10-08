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
}
