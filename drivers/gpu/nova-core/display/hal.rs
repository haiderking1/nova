// SPDX-License-Identifier: GPL-2.0

//! What differs between the display engines of GPU families: the classes of the display object
//! and channels, and the methods of those classes.

use kernel::{
    device,
    prelude::*, //
};

use crate::{
    driver::Bar0,
    gpu::Chipset,
    gsp::{
        cmdq::Cmdq,
        commands::{
            Client,
            Device,
            DisplayCoreChannel,
            DisplayRoot,
            DisplayWindowChannel,
            Handle, //
        },
    },
};

use super::{
    channel::PushBuffer,
    engine::{
        InternalHandles,
        PresentMode,
        Surface, //
    },
    mode::HeadRaster,
};

mod ga102;

/// What a window update shows, and where it reports its progress.
pub(crate) struct WindowImage<'a> {
    /// The surface shown, 1:1.
    pub(super) surface: &'a Surface,
    /// Handle of the context DMA the surface is in.
    pub(super) ctxdma: u32,
    /// Handle of the context DMA of the notifier the update writes.
    pub(super) notifier_ctxdma: u32,
    /// Offset of the notifier in its context DMA, in bytes: a multiple of 16.
    pub(super) notifier_offset: u32,
    /// Whether the update waits for the next core channel update, to take effect with it.
    pub(super) interlock_with_core: bool,
    /// When the update takes effect.
    pub(super) present: PresentMode,
}

/// The output a head drives: the SOR it owns.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct HeadOutput {
    /// Index of the SOR.
    pub(crate) sor: u32,
    /// Control of the SOR, as when the head drives it: the heads driving it and its protocol.
    pub(crate) control: u32,
}

/// Where a core channel update reports its progress.
#[derive(Clone, Copy, Debug)]
pub(crate) struct CoreNotifier {
    /// Handle of the context DMA of the notifier.
    pub(crate) ctxdma: u32,
    /// Offset of the notifier in its context DMA, in bytes: a multiple of 16.
    pub(crate) offset: u32,
}

pub(crate) trait DisplayHal: Sync {
    /// Allocates the display object `handle` under `device`, the parent of the display channels.
    fn alloc_display(
        &self,
        dev: &device::Device,
        cmdq: &Cmdq<'_>,
        client: Handle<Client>,
        device: Handle<Device>,
        handle: u32,
    ) -> Result<Handle<DisplayRoot>>;

    /// Sets `push` as the push buffer of the core channel, then allocates the core channel
    /// `handle` under the display object `root`.
    #[expect(clippy::too_many_arguments)]
    fn alloc_core(
        &self,
        dev: &device::Device,
        cmdq: &Cmdq<'_>,
        internal: &InternalHandles,
        client: Handle<Client>,
        root: Handle<DisplayRoot>,
        handle: u32,
        push: &PushBuffer<'_>,
    ) -> Result<Handle<DisplayCoreChannel>>;

    /// Sets `push` as the push buffer of the channel of window `window`, then allocates that
    /// channel `handle` under the display object `root`.
    #[expect(clippy::too_many_arguments)]
    fn alloc_window(
        &self,
        dev: &device::Device,
        cmdq: &Cmdq<'_>,
        internal: &InternalHandles,
        client: Handle<Client>,
        root: Handle<DisplayRoot>,
        window: u32,
        handle: u32,
        push: &PushBuffer<'_>,
    ) -> Result<Handle<DisplayWindowChannel>>;

    /// Pushes a core channel method changing nothing on screen: it only takes effect on the next
    /// update of the core channel.
    fn push_core_nop(&self, bar: Bar0<'_>, push: &mut PushBuffer<'_>) -> Result;

    /// Pushes the methods pointing a window at `image`, bypassing composition, and updating it.
    ///
    /// The window is shown as it is, which needs neither an input LUT on the window nor an output
    /// LUT on the head, and the update isn't interlocked with any other channel, so it only
    /// changes the window.
    fn push_window_update(
        &self,
        bar: Bar0<'_>,
        push: &mut PushBuffer<'_>,
        image: &WindowImage<'_>,
    ) -> Result;

    /// Enables the vblank interrupt of head `head` if `enable`, or disables it, dropping any
    /// pending vblank of the head.
    fn set_vblank_interrupt(&self, bar: Bar0<'_>, head: usize, enable: bool) -> Result;

    /// Acknowledges the vblank interrupts pending, and returns the heads they are of, one bit per
    /// head.
    fn take_vblanks(&self, bar: Bar0<'_>) -> u32;

    /// Returns the output head `head` drives, as the core channel has it armed, if any.
    fn armed_output(&self, bar: Bar0<'_>, head: u32) -> Option<HeadOutput>;

    /// Returns the bits per pixel head `head` sends its output, as the core channel has it armed.
    fn armed_output_bpp(&self, bar: Bar0<'_>, head: u32) -> Option<u32>;

    /// Pushes the core channel methods giving head `head` the raster `raster`, driving the display
    /// `display_id` (an RM display ID) on `output`. The head keeps the windows it shows.
    fn push_head_mode(
        &self,
        bar: Bar0<'_>,
        push: &mut PushBuffer<'_>,
        head: u32,
        display_id: u32,
        output: &HeadOutput,
        raster: &HeadRaster,
    ) -> Result;

    /// Pushes the core channel methods detaching head `head` from `output`, which then drives no
    /// display, as for a modeset in NVIDIA's driver. The head keeps its raster.
    fn push_detach(
        &self,
        bar: Bar0<'_>,
        push: &mut PushBuffer<'_>,
        head: u32,
        output: &HeadOutput,
    ) -> Result;

    /// Pushes the core channel method letting window `window` fetch lines of any width, as
    /// nouveau does, keeping the rest of its usage bounds: the firmware set them for its mode.
    fn push_window_bounds(&self, bar: Bar0<'_>, push: &mut PushBuffer<'_>, window: u32) -> Result;

    /// Pushes a core channel update, which takes effect with the updates of the windows in
    /// `windows`, a mask, already sent interlocked with it, and reports its progress in `notifier`
    /// if any.
    fn push_core_update(
        &self,
        bar: Bar0<'_>,
        push: &mut PushBuffer<'_>,
        windows: u32,
        notifier: Option<CoreNotifier>,
    ) -> Result;

    /// Returns the raster head `head` scans out with, as the core channel has it armed.
    fn armed_raster(&self, bar: Bar0<'_>, head: u32) -> Option<HeadRaster>;

    /// Logs the parts of the armed state of the core channel that taking over head 0 and window
    /// `window` depends on.
    fn log_armed_state(&self, dev: &device::Device, bar: Bar0<'_>, window: usize);
}

/// Returns the HAL of the display engine of `chipset`, if Nova supports it.
pub(crate) fn display_hal(chipset: Chipset) -> Option<&'static dyn DisplayHal> {
    match chipset {
        Chipset::GA102 | Chipset::GA103 | Chipset::GA104 | Chipset::GA106 | Chipset::GA107 => {
            Some(ga102::GA102_HAL)
        }
        _ => None,
    }
}
