// SPDX-License-Identifier: GPL-2.0

//! Formats and methods of GA10x channels: the GPFIFO channel class and the copy engine class.

use core::mem::offset_of;

use kernel::prelude::*;

use super::bindings;

/// The GA10x GPFIFO channel class (`AMPERE_CHANNEL_GPFIFO_A`, `clc56f.h`): its USERD, GPFIFO
/// entries and method headers.
pub(crate) mod host_c56f {
    use super::*;

    /// Offset in the USERD of `GPGet`, the GPFIFO entry the channel fetches next.
    pub(crate) const USERD_GP_GET: usize = offset_of!(bindings::Nvc56fControl, GPGet);
    /// Offset in the USERD of `GPPut`, the GPFIFO entry the channel stops fetching at.
    pub(crate) const USERD_GP_PUT: usize = offset_of!(bindings::Nvc56fControl, GPPut);

    /// Size of a GPFIFO entry in bytes.
    pub(crate) const GP_ENTRY_SIZE: usize = bindings::NVC56F_GP_ENTRY__SIZE as usize;

    /// Sets the class of the object a subchannel runs the methods of.
    pub(crate) const SET_OBJECT: u32 = bindings::NVC56F_SET_OBJECT;

    /// Returns the GPFIFO entry of the push buffer of `words` 32-bit words at GPU virtual address
    /// `addr`, which must be 4-byte aligned.
    pub(crate) fn gp_entry(addr: u64, words: u32) -> Result<[u32; 2]> {
        // `NVC56F_GP_ENTRY1_LENGTH` is bits 30:10, `NVC56F_GP_ENTRY1_GET_HI` bits 7:0.
        if addr % 4 != 0 || addr >> 40 != 0 || words >= 1 << 21 {
            return Err(EINVAL);
        }

        // CAST: `NVC56F_GP_ENTRY0_GET` holds the low 32 bits of the address, and
        // `NVC56F_GP_ENTRY1_GET_HI` the 8 bits above, checked above to be all there is.
        Ok([
            addr as u32,
            (addr >> 32) as u32 | bindings::NVC56F_GP_ENTRY1_LEVEL_MAIN << 9 | words << 10,
        ])
    }

    /// Returns the header of `count` methods of `subchannel` from `method` on (a byte offset in
    /// the subchannel's class), each taking the data word that follows.
    pub(crate) fn methods(subchannel: u32, method: u32, count: u32) -> Result<u32> {
        // `NVC56F_DMA_METHOD_ADDRESS` (in words) is bits 11:0, `_SUBCHANNEL` 15:13, `_COUNT` 28:16
        // and `NVC56F_DMA_SEC_OP` 31:29.
        if method % 4 != 0 || method >> 14 != 0 || subchannel >= 1 << 3 || count >= 1 << 13 {
            return Err(EINVAL);
        }

        Ok(bindings::NVC56F_DMA_SEC_OP_INC_METHOD << 29
            | count << 16
            | subchannel << 13
            | method >> 2)
    }
}

/// Methods of the GA10x copy engine class (`AMPERE_DMA_COPY_B`, `clc7b5.h`).
pub(crate) mod copy_c7b5 {
    use super::*;

    /// The class, for [`host_c56f::SET_OBJECT`].
    pub(crate) const CLASS: u32 = bindings::AMPERE_DMA_COPY_B;

    /// Semaphore of the next launch: bits 48:32 of its GPU virtual address, then bits 31:0 and
    /// the value released, in the 2 following methods.
    pub(crate) const SET_SEMAPHORE_A: u32 = bindings::NVC7B5_SET_SEMAPHORE_A;
    /// Starts a copy, and releases the semaphore once it's done.
    pub(crate) const LAUNCH_DMA: u32 = bindings::NVC7B5_LAUNCH_DMA;
    /// Destination: bits 48:32 of its GPU virtual address, then bits 31:0 in the next method.
    pub(crate) const OFFSET_OUT_UPPER: u32 = bindings::NVC7B5_OFFSET_OUT_UPPER;
    /// Pitches of the source and the destination in bytes, then the length of a line in elements
    /// and the number of lines, in the 3 following methods.
    pub(crate) const PITCH_IN: u32 = bindings::NVC7B5_PITCH_IN;
    /// The constant A written by remapping, then constant B and the remapping of the components,
    /// in the 2 following methods.
    pub(crate) const SET_REMAP_CONST_A: u32 = bindings::NVC7B5_SET_REMAP_CONST_A;

    /// Remapping writing constant A as a single 4-byte component: a 32-bit fill.
    pub(crate) const REMAP_FILL_32: u32 = bindings::NVC7B5_SET_REMAP_COMPONENTS_DST_X_CONST_A
        | bindings::NVC7B5_SET_REMAP_COMPONENTS_COMPONENT_SIZE_FOUR << 16
        | bindings::NVC7B5_SET_REMAP_COMPONENTS_NUM_DST_COMPONENTS_ONE << 24;

    /// [`LAUNCH_DMA`] filling pitch-linear lines with the remapped constants, with the transfer
    /// flushed, then releasing a 1-word semaphore. Field positions are those of
    /// `NVC7B5_LAUNCH_DMA_*`.
    pub(crate) const LAUNCH_FILL_AND_RELEASE: u32 =
        bindings::NVC7B5_LAUNCH_DMA_DATA_TRANSFER_TYPE_NON_PIPELINED
            | bindings::NVC7B5_LAUNCH_DMA_FLUSH_ENABLE_TRUE << 2
            | bindings::NVC7B5_LAUNCH_DMA_SEMAPHORE_TYPE_RELEASE_ONE_WORD_SEMAPHORE << 3
            | bindings::NVC7B5_LAUNCH_DMA_SRC_MEMORY_LAYOUT_PITCH << 7
            | bindings::NVC7B5_LAUNCH_DMA_DST_MEMORY_LAYOUT_PITCH << 8
            | bindings::NVC7B5_LAUNCH_DMA_MULTI_LINE_ENABLE_TRUE << 9
            | bindings::NVC7B5_LAUNCH_DMA_REMAP_ENABLE_TRUE << 10
            // Like NVIDIA's drivers on this class.
            | bindings::NVC7B5_LAUNCH_DMA_DISABLE_PLC_TRUE << 26;
}
