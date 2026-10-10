// SPDX-License-Identifier: GPL-2.0

//! Nova-core auxbus data. Contains all the methods used by the auxbus drivers
//! to interact with nova-core.

use core::{
    ops::Range,
    pin::Pin, //
};

use kernel::{
    auxiliary,
    device::Bound,
    dma_buf::Fence,
    pci,
    prelude::*,
    screen_info::PixelFormat,
    sync::{
        aref::ARef,
        Arc, //
    },
    types::ForLt, //
};

pub use crate::{
    buffer::{
        SystemBuffer,
        VramBuffer,
        VRAM_BUFFER_ALIGN, //
    },
    channel::client::{
        ChannelEngine,
        GraphicsObjects,
        PushBuffer,
        PUSH_MAX, //
    },
    channel::{
        gr::GraphicsInfo,
        hal::Classes, //
    },
    display::{
        DisplayCopy,
        DisplaySource,
        ModeTimings,
        PresentMode,
        ScanoutMode,
        VblankHandler, //
    },
    gpu::Spec,
    gsp::ConnectorType, //
    vm::{
        BufferRef,
        ClientVm,
        VA_SIZE, //
    },
};

use crate::{
    driver,
    gpu::Gpu,
    gsp::commands::GetGspStaticInfoReply, //
};

/// Mode of the display the system firmware set up on the GPU (e.g. through UEFI GOP), which the
/// display engine keeps scanning out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BootDisplayMode {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// Distance between the starts of two lines of the framebuffer, in bytes.
    pub stride: u32,
    /// Pixel layout of the framebuffer.
    pub format: PixelFormat,
}

/// API handle for the auxiliary bus child drivers to interact with nova-core.
pub struct NovaCoreApi<'bound> {
    pub(crate) gpu: Pin<&'bound Gpu<'bound>>,
    pub(crate) pdev: &'bound pci::Device<Bound>,
}

impl NovaCoreApi<'_> {
    /// Obtain a [`NovaCoreApi`] handle from an auxiliary device registered
    /// by nova-core.
    pub fn of(adev: &auxiliary::Device<Bound>) -> Result<NovaCoreApiHandle<'_>> {
        NovaCoreApiHandle::of(adev)
    }

    /// Returns the GPU [`GetGspStaticInfoReply`].
    pub fn gsp_static_info(&self) -> &GetGspStaticInfoReply {
        &self.gpu.gsp_static_info
    }

    /// Returns the GPU [`Spec`].
    pub fn spec(&self) -> &Spec {
        &self.gpu.spec
    }

    /// Returns the mode of the display the system firmware set up on the GPU, if it set one up.
    pub fn boot_display_mode(&self) -> Option<BootDisplayMode> {
        self.gpu
            .boot_display
            .as_ref()
            .map(|display| BootDisplayMode {
                width: display.width,
                height: display.height,
                stride: display.stride,
                format: display.format,
            })
    }

    /// Returns the size and layout of the framebuffers of Nova's own scanout of the boot display's
    /// mode, if it is set up.
    ///
    /// Nova's scanout replaces the boot display's framebuffer: frames are copied into its back
    /// buffer with [`NovaCoreApi::copy_to_scanout()`] and shown with
    /// [`NovaCoreApi::flip_scanout()`].
    pub fn scanout_mode(&self) -> Option<ScanoutMode> {
        self.gpu.scanout_mode()
    }

    /// Shows the back buffer of Nova's own scanout from the next frame on, or right away with
    /// [`PresentMode::Immediate`], tearing, and returns once the framebuffer shown before is the
    /// back buffer: it then holds the frame before the one just flipped.
    ///
    /// Fails with `ENODEV` if the scanout isn't set up.
    pub fn flip_scanout(&self, present: PresentMode) -> Result {
        self.gpu.flip_scanout(present)
    }

    /// Maps `buffer` for the GPU's copy engine, to copy it to the display with
    /// [`NovaCoreApi::copy_to_scanout()`] or [`NovaCoreApi::copy_to_boot_display()`].
    ///
    /// The mapping lasts until given back to [`NovaCoreApi::unmap_display_source()`], which must
    /// be done before `buffer` is freed. Fails with `ENODEV` if the copy engine isn't set up.
    pub fn map_display_source(&self, buffer: BufferRef<'_>) -> Result<DisplaySource> {
        self.gpu.map_display_source(buffer)
    }

    /// Tears down `source`, a mapping made by [`NovaCoreApi::map_display_source()`].
    pub fn unmap_display_source(&self, source: DisplaySource) {
        self.gpu.unmap_display_source(source)
    }

    /// Copies the `rects` of the XRGB8888 pixels of `source`, whose lines are `pitch` bytes apart
    /// from byte `offset` on, into the back buffer of Nova's own scanout. The GPU's copy engine
    /// copies them; returns once it's done.
    ///
    /// Fails with `ENODEV` if the scanout isn't set up, and with `EINVAL` if a rectangle isn't
    /// within `source` or the scanout.
    pub fn copy_to_scanout(
        &self,
        source: &DisplaySource,
        offset: u64,
        pitch: u32,
        rects: &[DisplayCopy],
    ) -> Result {
        self.gpu.copy_to_scanout(source, offset, pitch, rects)
    }

    /// Like [`NovaCoreApi::copy_to_scanout()`], into the framebuffer of the boot display, which
    /// shows on screen as it is written: its pixels must be XRGB8888 (see [`BootDisplayMode`]).
    ///
    /// Fails with `ENODEV` without a usable boot display.
    pub fn copy_to_boot_display(
        &self,
        source: &DisplaySource,
        offset: u64,
        pitch: u32,
        rects: &[DisplayCopy],
    ) -> Result {
        self.gpu.copy_to_boot_display(source, offset, pitch, rects)
    }

    /// Enables the vblank interrupt of head `head` if `enable`, or disables it. Doesn't sleep, so
    /// it may be called in atomic context.
    ///
    /// The vblanks of the heads whose interrupt is enabled are passed to the handler set with
    /// [`NovaCoreApi::set_vblank_handler()`]. Fails with `ENODEV` if Nova can't service the display
    /// interrupt.
    pub fn set_vblank_interrupt(&self, head: u32, enable: bool) -> Result {
        self.gpu.set_vblank_interrupt(head, enable)
    }

    /// Checks that nova-core can show `mode` on the boot display, through its scanout: fails with
    /// `ERANGE` if the display's link can't carry it, `EINVAL` if its timings are invalid.
    pub fn check_mode(&self, mode: &ModeTimings) -> Result {
        self.gpu.check_mode(mode)
    }

    /// Has head `head` scan out `mode`, and returns once it does: whether the head had another
    /// mode. A mode of another resolution replaces nova-core's scanout with one of its size,
    /// cleared: see [`NovaCoreApi::scanout_mode()`].
    pub fn set_mode(&self, head: u32, mode: &ModeTimings) -> Result<bool> {
        self.gpu.set_mode(head, mode)
    }

    /// Sets the handler of vblanks, replacing any, or removes it with `None`.
    ///
    /// The handler must be removed before the client unbinds.
    pub fn set_vblank_handler(&self, handler: Option<Arc<dyn VblankHandler>>) {
        self.gpu.set_vblank_handler(handler)
    }

    /// Returns the type of connector the boot display is shown through.
    ///
    /// Fails with `ENODEV` if the display engine reports no active display.
    pub fn boot_display_connector(&self) -> Result<ConnectorType> {
        self.gpu.boot_display_connector()
    }

    /// Reads the EDID of the monitor the boot display is shown on, over the display's DDC channel.
    ///
    /// Fails with `ENODEV` if the display engine reports no active display.
    pub fn boot_display_edid(&self) -> Result<KVec<u8>> {
        self.gpu.boot_display_edid()
    }

    /// Returns the size of the PCIe BAR used for accessing VRAM, typically
    /// BAR1.
    pub fn bar1_size(&self) -> Result<u64> {
        let bar1 = driver::bar1_resource_index(self.pdev)?;

        self.pdev.resource_len(bar1)
    }

    /// Returns what the graphics engine is made of and which classes its channels have.
    ///
    /// Fails with `ENODEV` if the graphics engine isn't set up.
    pub fn graphics_info(&self) -> Result<GraphicsInfo> {
        self.gpu.graphics_info()
    }

    /// Allocates a buffer of at least `size` bytes of VRAM, zeroed, with a window in BAR1 for the
    /// CPU if `cpu_visible`. Its size is rounded up to [`VRAM_BUFFER_ALIGN`].
    ///
    /// It must be given back to [`NovaCoreApi::free_vram`].
    pub fn alloc_vram(&self, size: usize, cpu_visible: bool) -> Result<VramBuffer> {
        self.gpu.alloc_vram(size, cpu_visible)
    }

    /// Frees `buffer`, tearing down its window in BAR1.
    pub fn free_vram(&self, buffer: VramBuffer) {
        self.gpu.free_vram(buffer)
    }

    /// Allocates an empty GPU virtual address space of [`VA_SIZE`] bytes for a client, which maps
    /// its buffers itself but in `kernel`, the range where nova-core maps the buffers it needs for
    /// the client.
    ///
    /// It must be given back to [`NovaCoreApi::free_vm`].
    pub fn create_vm(&self, kernel: Range<u64>) -> Result<ClientVm> {
        self.gpu.create_vm(kernel)
    }

    /// Frees `vm`, which nothing may use anymore.
    pub fn free_vm(&self, vm: ClientVm) {
        self.gpu.free_vm(vm)
    }

    /// Maps the `size` bytes of `buffer` from `offset` at the virtual address `addr` of `vm`, as
    /// memory of the PTE kind `kind`, replacing what was mapped there.
    ///
    /// The addresses, `offset` and `size` must be page aligned, and outside the kernel range of
    /// `vm`. Compressible kinds are mapped as their uncompressed equivalent.
    pub fn vm_map(
        &self,
        vm: &mut ClientVm,
        addr: u64,
        buffer: BufferRef<'_>,
        offset: u64,
        size: u64,
        kind: u8,
    ) -> Result {
        self.gpu.vm_map(vm, addr, buffer, offset, size, kind)
    }

    /// Tears down the `size` bytes mapped from the virtual address `addr` of `vm`.
    pub fn vm_unmap(&self, vm: &mut ClientVm, addr: u64, size: u64) -> Result {
        self.gpu.vm_unmap(vm, addr, size)
    }

    /// Allocates a channel on `engine` running in `vm`, and returns its ID.
    ///
    /// The channel maps its memory in the kernel range of `vm`. It must be freed with
    /// [`NovaCoreApi::destroy_channel`] before `vm` is.
    pub fn create_channel(&self, vm: &mut ClientVm, engine: ChannelEngine) -> Result<u32> {
        self.gpu.create_channel(vm, engine)
    }

    /// Frees the channel `chid`, which runs in `vm`, once the work it runs is done.
    pub fn destroy_channel(&self, vm: &mut ClientVm, chid: u32) -> Result {
        self.gpu.destroy_channel(vm, chid)
    }

    /// Has the channel `chid` run the push buffers `pushes`, in order, after the work submitted
    /// before, and returns the fence that signals once they are done.
    ///
    /// Waits for room on the channel, as the work submitted before completes. Fails with
    /// `ETIMEDOUT` if that work makes no progress in time, and with `ENODEV` once the channel runs
    /// no more work, such as after that.
    pub fn channel_exec(&self, chid: u32, pushes: &[PushBuffer]) -> Result<ARef<Fence>> {
        self.gpu.channel_exec(chid, pushes)
    }

    /// Returns the fence of the last work submitted to the channel `chid` that is not done yet, if
    /// any.
    ///
    /// Fails with `ENODEV` once the channel runs no more work.
    pub fn channel_last_fence(&self, chid: u32) -> Result<Option<ARef<Fence>>> {
        self.gpu.channel_last_fence(chid)
    }

    /// Returns the GPU's timer, a count of nanoseconds that work on the GPU timestamps with.
    pub fn timestamp(&self) -> u64 {
        self.gpu.timestamp()
    }

    /// Allocates a zeroed buffer of at least `size` bytes of system memory, which the GPU
    /// accesses coherently with the CPU's caches. Its size is rounded up to the page size.
    pub fn alloc_system(&self, size: usize) -> Result<SystemBuffer> {
        self.gpu.alloc_system(size)
    }
}

/// Expose a handle to nova-core API
pub struct NovaCoreApiHandle<'a> {
    adev: &'a auxiliary::Device<Bound>,
}

impl<'a> NovaCoreApiHandle<'a> {
    fn of(adev: &'a auxiliary::Device<Bound>) -> Result<Self> {
        adev.registration_data_with::<ForLt!(NovaCoreApi<'_>), ()>(|_| ())?;
        Ok(Self { adev })
    }

    /// Access the [`NovaCoreApi`] through a closure.
    pub fn with<R>(&self, f: impl for<'b> FnOnce(Pin<&'a NovaCoreApi<'b>>) -> R) -> R {
        self.adev
            .registration_data_with::<ForLt!(NovaCoreApi<'_>), R>(f)
            .expect("TypeId was validated in NovaCoreApiHandle::of()")
    }
}
