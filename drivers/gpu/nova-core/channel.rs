// SPDX-License-Identifier: GPL-2.0

//! GPU channels, through which Nova submits work to the GPU's engines.
//!
//! A channel runs on one engine. It fetches the command buffers listed in its GPFIFO, a ring of
//! GPU virtual addresses and lengths, from the address space it is bound to.

#[cfg(CONFIG_NOVA_CORE_SELFTESTS)]
pub(crate) mod compute_test;
pub(crate) mod copy;
pub(crate) mod gpfifo;
pub(crate) mod gr;
pub(crate) mod vaspace;

// IDs of Nova's channels. GSP-RM keeps channel ID 0 for itself.

/// Channel ID of the copy channel.
pub(crate) const COPY_CHID: u32 = 1;
/// Channel ID of the channel GSP-RM builds the golden context of the graphics engine on.
pub(crate) const GOLDEN_CHID: u32 = 2;
/// Channel ID of the compute channel.
pub(crate) const COMPUTE_CHID: u32 = 3;

use kernel::{
    device,
    prelude::*, //
};

use crate::gsp::{
    cmdq::Cmdq,
    rm::{
        commands::{
            FifoGetDeviceInfoTable,
            RmControl, //
        },
        device::RmDevice,
    },
    EngineInfo,
};

/// Returns the engines of the GPU, and the runlists they are on.
pub(crate) fn engines(
    dev: &device::Device,
    cmdq: &Cmdq<'_>,
    rm: &RmDevice,
) -> Result<KVec<EngineInfo>> {
    let mut engines = KVec::new();
    let mut base = 0;
    loop {
        let (page, more) = RmControl::new(
            rm.client(),
            rm.subdevice(),
            FifoGetDeviceInfoTable::new(base),
        )
        .send(dev, cmdq)?;
        engines.extend_from_slice(&page, GFP_KERNEL)?;
        if !more {
            return Ok(engines);
        }
        base += FifoGetDeviceInfoTable::PAGE_ENTRIES;
    }
}
