// SPDX-License-Identifier: GPL-2.0

//! GPU channels, through which Nova submits work to the GPU's engines.
//!
//! A channel runs on one engine. It fetches the command buffers listed in its GPFIFO, a ring of
//! GPU virtual addresses and lengths, from the address space it is bound to.

pub(crate) mod copy;
pub(crate) mod vaspace;
