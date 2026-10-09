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

    /// Returns the methods waiting for the channel's engine to be idle, then for the memory it
    /// wrote to be visible to the rest of the system, CPU included.
    pub(crate) fn wait_idle_and_flush() -> Result<[u32; 7]> {
        Ok([
            // Any subchannel runs the channel's own methods.
            methods(0, bindings::NVC56F_WFI, 1)?,
            bindings::NVC56F_WFI_SCOPE_ALL,
            methods(0, bindings::NVC56F_MEM_OP_A, 4)?,
            0,
            0,
            // `NVC56F_MEM_OP_C_MEMBAR_TYPE`, bits 2:0.
            bindings::NVC56F_MEM_OP_C_MEMBAR_TYPE_SYS_MEMBAR,
            // `NVC56F_MEM_OP_D_OPERATION`, bits 31:27.
            bindings::NVC56F_MEM_OP_D_OPERATION_MEMBAR << 27,
        ])
    }

    /// Returns the methods releasing the 32-bit semaphore at GPU virtual address `addr`, 4-byte
    /// aligned, with `payload`, once the channel's engine is idle: once the work sent before is
    /// done.
    pub(crate) fn semaphore_release(addr: u64, payload: u32) -> Result<[u32; 6]> {
        // `NVC56F_SEM_ADDR_LO_OFFSET` is bits 31:2, `NVC56F_SEM_ADDR_HI_OFFSET` bits 7:0.
        if addr % 4 != 0 || addr >> 40 != 0 {
            return Err(EINVAL);
        }

        // CAST: the low 32 bits and the 8 bits above are set separately, checked above to be all
        // there is.
        Ok([
            // Any subchannel runs the channel's own methods.
            methods(0, bindings::NVC56F_SEM_ADDR_LO, 5)?,
            addr as u32,
            (addr >> 32) as u32,
            payload,
            // `NVC56F_SEM_PAYLOAD_HI`, unused by 32-bit semaphores.
            0,
            // `NVC56F_SEM_EXECUTE`: operation in bits 2:0, `RELEASE_WFI` bit 20, `PAYLOAD_SIZE`
            // bit 24.
            bindings::NVC56F_SEM_EXECUTE_OPERATION_RELEASE
                | bindings::NVC56F_SEM_EXECUTE_RELEASE_WFI_EN << 20
                | bindings::NVC56F_SEM_EXECUTE_PAYLOAD_SIZE_32BIT << 24,
        ])
    }

    /// Returns the GPFIFO entry of the push buffer of `words` 32-bit words at GPU virtual address
    /// `addr`, which must be 4-byte aligned. If `wait`, the channel fetches it once the work
    /// before is done, rather than ahead.
    pub(crate) fn gp_entry(addr: u64, words: u32, wait: bool) -> Result<[u32; 2]> {
        // `NVC56F_GP_ENTRY1_LENGTH` is bits 30:10, `NVC56F_GP_ENTRY1_GET_HI` bits 7:0.
        if addr % 4 != 0 || addr >> 40 != 0 || words >= 1 << 21 {
            return Err(EINVAL);
        }

        // CAST: `NVC56F_GP_ENTRY0_GET` holds the low 32 bits of the address, and
        // `NVC56F_GP_ENTRY1_GET_HI` the 8 bits above, checked above to be all there is.
        Ok([
            addr as u32,
            (addr >> 32) as u32
                | bindings::NVC56F_GP_ENTRY1_LEVEL_MAIN << 9
                | words << 10
                // `NVC56F_GP_ENTRY1_SYNC` is bit 31.
                | if wait {
                    bindings::NVC56F_GP_ENTRY1_SYNC_WAIT << 31
                } else {
                    0
                },
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

    /// Starts a copy.
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
    /// flushed. Field positions are those of `NVC7B5_LAUNCH_DMA_*`.
    pub(crate) const LAUNCH_FILL: u32 = bindings::NVC7B5_LAUNCH_DMA_DATA_TRANSFER_TYPE_NON_PIPELINED
            | bindings::NVC7B5_LAUNCH_DMA_FLUSH_ENABLE_TRUE << 2
            | bindings::NVC7B5_LAUNCH_DMA_SRC_MEMORY_LAYOUT_PITCH << 7
            | bindings::NVC7B5_LAUNCH_DMA_DST_MEMORY_LAYOUT_PITCH << 8
            | bindings::NVC7B5_LAUNCH_DMA_MULTI_LINE_ENABLE_TRUE << 9
            | bindings::NVC7B5_LAUNCH_DMA_REMAP_ENABLE_TRUE << 10
            // Like NVIDIA's drivers on this class.
            | bindings::NVC7B5_LAUNCH_DMA_DISABLE_PLC_TRUE << 26;
}

/// Methods of the GA10x compute class (`AMPERE_COMPUTE_B`, `clc7c0.h`), and its work descriptors.
pub(crate) mod compute_c7c0 {
    use super::*;

    /// The class, for [`host_c56f::SET_OBJECT`].
    pub(crate) const CLASS: u32 = bindings::AMPERE_COMPUTE_B;

    /// Invalidates the caches of the scheduler of compute work.
    pub(crate) const INVALIDATE_SKED_CACHES: u32 = bindings::NVC7C0_INVALIDATE_SKED_CACHES;
    /// Window of shared memory in the generic address space: bits 63:32 of its address, then bits
    /// 31:0 in the next method.
    pub(crate) const SET_SHADER_SHARED_MEMORY_WINDOW_A: u32 =
        bindings::NVC7C0_SET_SHADER_SHARED_MEMORY_WINDOW_A;
    /// Window of local memory in the generic address space, like
    /// [`SET_SHADER_SHARED_MEMORY_WINDOW_A`].
    pub(crate) const SET_SHADER_LOCAL_MEMORY_WINDOW_A: u32 =
        bindings::NVC7C0_SET_SHADER_LOCAL_MEMORY_WINDOW_A;
    /// Work descriptor to run: its GPU virtual address shifted right by 8.
    pub(crate) const SEND_PCAS_A: u32 = bindings::NVC7C0_SEND_PCAS_A;
    /// What to do with the work descriptor given with [`SEND_PCAS_A`].
    pub(crate) const SEND_SIGNALING_PCAS2_B: u32 = bindings::NVC7C0_SEND_SIGNALING_PCAS2_B;
    /// [`SEND_SIGNALING_PCAS2_B`]: schedule the work.
    pub(crate) const PCAS_ACTION_INVALIDATE_COPY_SCHEDULE: u32 =
        bindings::NVC7C0_SEND_SIGNALING_PCAS2_B_PCAS_ACTION_INVALIDATE_COPY_SCHEDULE;

    /// Size of a work descriptor (QMD) in bytes, and the alignment of its GPU virtual address.
    pub(crate) const QMD_SIZE: usize = 256;

    /// A version 3.0 work descriptor (QMD) of the compute class, used by GA10x.
    ///
    /// Field positions are those of `NVC6C0_QMDV03_00_*` in NVIDIA's `clc6c0qmd.h`.
    pub(crate) struct Qmd([u32; QMD_SIZE / size_of::<u32>()]);

    impl Qmd {
        /// Creates the descriptor of a grid of `grid` blocks of `block` threads each, running the
        /// program at GPU virtual address `program`, which uses `registers` registers per thread
        /// and no shared memory.
        pub(crate) fn new(program: u64, grid: [u32; 3], block: [u16; 3], registers: u8) -> Self {
            let mut dw = [0u32; QMD_SIZE / size_of::<u32>()];

            // `SM_GLOBAL_CACHING_ENABLE`, bit 134.
            dw[4] |= 1 << 6;
            // `INVALIDATE_SHADER_DATA_CACHE`, `_INSTRUCTION_CACHE` and `_SHADER_CONSTANT_CACHE`,
            // bits 189 to 191.
            dw[5] |= 0b111 << 29;
            // `API_VISIBLE_CALL_LIMIT` `NO_CHECK`, bit 378.
            dw[11] |= 1 << 26;
            // `CTA_RASTER_WIDTH`, `_HEIGHT` and `_DEPTH`: bits 415:384, 431:416 and 463:448.
            dw[12] = grid[0];
            dw[13] = grid[1] & 0xffff;
            dw[14] = grid[2] & 0xffff;
            // No shared memory (`SHARED_MEMORY_SIZE`, bits 561:544). `MIN_SM_CONFIG_SHARED_MEM_SIZE`
            // (bits 567:562) of 1 and `MAX_SM_CONFIG_SHARED_MEM_SIZE` (bits 574:569) of 26, in
            // units of 4 KiB plus one: 0 to 100 KiB, what GA10x SMs offer.
            dw[17] = 1 << 18 | 26 << 25;
            // `QMD_VERSION` 0 and `QMD_MAJOR_VERSION` 3 (bits 579:576 and 583:580), then
            // `CTA_THREAD_DIMENSION0` (bits 607:592).
            dw[18] = 3 << 4 | u32::from(block[0]) << 16;
            // `CTA_THREAD_DIMENSION1` and `_2`: bits 623:608 and 639:624.
            dw[19] = u32::from(block[1]) | u32::from(block[2]) << 16;
            // `REGISTER_COUNT_V` (bits 656:648), and `TARGET_SM_CONFIG_SHARED_MEM_SIZE` (bits
            // 662:657) of 1: no shared memory.
            dw[20] = u32::from(registers) << 8 | 1 << 17;
            // `PROGRAM_ADDRESS_LOWER` and `_UPPER`: bits 1567:1536 and 1584:1568.
            // CAST: the address is split in its low 32 bits and the 17 bits above.
            dw[48] = program as u32;
            dw[49] = (program >> 32) as u32 & 0x1_ffff;

            Self(dw)
        }

        /// Has the work, once done and its memory writes visible to the system, write `payload` to
        /// the 32-bit word at GPU virtual address `addr`, 4-byte aligned.
        pub(crate) fn release(mut self, addr: u64, payload: u32) -> Result<Self> {
            if addr % 4 != 0 || addr >> 40 != 0 {
                return Err(EINVAL);
            }

            // CAST: the address is split in its low 32 bits and the 8 bits above, checked above
            // to be all there is.
            // `RELEASE0_ADDRESS_LOWER`: bits 799:768.
            self.0[24] = addr as u32;
            // `RELEASE0_ADDRESS_UPPER` (bits 807:800), `RELEASE0_MEMBAR_TYPE` `FE_SYSMEMBAR` (bit
            // 819), `RELEASE0_ENABLE` (bit 823) and `RELEASE0_STRUCTURE_SIZE`
            // `SEMAPHORE_ONE_WORD` (bits 831:830).
            self.0[25] = (addr >> 32) as u32 | 1 << 19 | 1 << 23 | 1 << 30;
            // `RELEASE0_PAYLOAD_LOWER`: bits 863:832.
            self.0[26] = payload;

            Ok(self)
        }

        /// Returns the words of the descriptor.
        pub(crate) fn words(&self) -> &[u32] {
            &self.0
        }
    }
}
