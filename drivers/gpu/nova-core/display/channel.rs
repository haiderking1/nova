// SPDX-License-Identifier: GPL-2.0

//! Display channels driven through a push buffer.
//!
//! The CPU writes methods to a push buffer in system memory and advances the channel's PUT
//! pointer; the display engine reads them up to PUT and advances GET. The push buffer is a ring:
//! when the end is near, a jump back to its start is written.

use kernel::{
    device,
    dma::Coherent,
    io::{
        io_write,
        poll::read_poll_timeout,
        Io, //
    },
    prelude::*,
    time::Delta, //
};

use crate::{
    driver::Bar0,
    gsp::DISP_PUSHBUFFER_SIZE,
    num, //
};

use super::regs;

/// Number of 32-bit words in a push buffer.
const PUSHBUFFER_WORDS: usize = num::u64_as_usize(DISP_PUSHBUFFER_SIZE) / size_of::<u32>();

/// Last word that may hold a method: the one after it is kept for the jump back to the start.
const MAX: usize = PUSHBUFFER_WORDS - 1;

/// How long to wait for the display engine to read methods.
const TIMEOUT: Delta = Delta::from_secs(2);

/// Method header for `count` data words starting at method `method` (a byte offset in the
/// channel class).
fn method_header(method: u32, count: usize) -> Result<u32> {
    let count = u32::try_from(count).map_err(|_| EINVAL)?;
    if count >= 1 << 11 || method & !0xfffc != 0 {
        return Err(EINVAL);
    }

    Ok(count << 18 | method)
}

/// Jump to the start of the push buffer.
const JUMP_TO_START: u32 = 0x2000_0000;

/// The push buffer of a display channel, and the channel's PUT and GET pointers.
///
/// Only the core channel exists so far.
pub(super) struct PushBuffer<'a> {
    buffer: Coherent<'a, [u32; PUSHBUFFER_WORDS]>,
    /// Word up to which methods were written.
    cur: usize,
    /// Word up to which the display engine was told to read, i.e. the last PUT written.
    put: usize,
}

impl<'a> PushBuffer<'a> {
    /// Allocates an empty push buffer.
    pub(super) fn new(dev: &'a device::Device<device::Bound>) -> Result<Self> {
        Ok(Self {
            buffer: Coherent::zeroed(dev, GFP_KERNEL)?,
            cur: 0,
            put: 0,
        })
    }

    /// Returns the DMA address of the push buffer, for the display engine.
    pub(super) fn dma_address(&self) -> u64 {
        self.buffer.dma_address()
    }

    /// Writes method `method` with the data words `data`, without kicking: see
    /// [`PushBuffer::kick`].
    pub(super) fn push(&mut self, bar: Bar0<'_>, method: u32, data: &[u32]) -> Result {
        let header = method_header(method, data.len())?;
        let words = 1 + data.len();
        self.wait_space(bar, words)?;

        io_write!(self.buffer, [try: self.cur], header);
        for (i, word) in data.iter().enumerate() {
            io_write!(self.buffer, [try: self.cur + 1 + i], *word);
        }
        self.cur += words;

        Ok(())
    }

    /// Makes the display engine read the methods written since the last kick.
    pub(super) fn kick(&mut self, bar: Bar0<'_>) -> Result {
        if self.cur != self.put {
            // MMIO writes are ordered after the preceding writes to the (coherent) push buffer.
            bar.write_reg(regs::NV_UDISP_CORE_PUT::zeroed().try_with_ptr(self.cur)?);
            self.put = self.cur;
        }

        Ok(())
    }

    /// Waits until the display engine read every method kicked.
    pub(super) fn wait_idle(&self, bar: Bar0<'_>) -> Result {
        read_poll_timeout(
            || Ok(self.get(bar)),
            |&get| get == self.put,
            Delta::from_micros(10),
            TIMEOUT,
        )
        .map(|_| ())
    }

    /// Returns the word up to which the display engine read the push buffer.
    pub(super) fn get(&self, bar: Bar0<'_>) -> usize {
        num::u32_as_usize(bar.read(regs::NV_UDISP_CORE_GET).ptr().get())
    }

    /// Number of words that can be written at `cur` without overtaking the display engine.
    fn free(&self, bar: Bar0<'_>) -> usize {
        let get = self.get(bar);
        if get > self.cur {
            // Like NVIDIA's driver, stay 5 words away from GET.
            (get - self.cur).saturating_sub(5)
        } else {
            MAX - self.cur
        }
    }

    /// Waits until `words` words can be written at `cur`, going back to the start of the push
    /// buffer if they don't fit before its end.
    fn wait_space(&mut self, bar: Bar0<'_>, words: usize) -> Result {
        if words > MAX {
            return Err(EINVAL);
        }

        if self.cur + words >= MAX {
            self.wind(bar)?;
        }

        read_poll_timeout(
            || Ok(self.free(bar)),
            |&free| free >= words,
            Delta::from_micros(10),
            TIMEOUT,
        )
        .map(|_| ())
    }

    /// Continues the push buffer at its start.
    fn wind(&mut self, bar: Bar0<'_>) -> Result {
        // With GET at the start, PUT would become equal to GET, which the hardware ignores: wait
        // until the display engine left the start.
        if self.get(bar) == 0 {
            // Unless nothing kicked made it leave yet.
            if self.put == 0 {
                self.kick(bar)?;
            }

            read_poll_timeout(
                || Ok(self.get(bar)),
                |&get| get > 0,
                Delta::from_micros(10),
                TIMEOUT,
            )?;
        }

        io_write!(self.buffer, [try: self.cur], JUMP_TO_START);
        self.cur = 0;
        self.kick(bar)
    }
}
