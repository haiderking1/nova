// SPDX-License-Identifier: GPL-2.0

//! Nova-core auxbus data. Contains all the methods used by the auxbus drivers
//! to interact with nova-core.

use core::pin::Pin;

use kernel::{
    auxiliary,
    device::Bound,
    pci,
    prelude::*,
    screen_info::PixelFormat,
    types::ForLt, //
};

pub use crate::gpu::Spec;

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
        self.gpu.boot_display.as_ref().map(|display| BootDisplayMode {
            width: display.width,
            height: display.height,
            stride: display.stride,
            format: display.format,
        })
    }

    /// Writes `data` at `offset` into the framebuffer of the boot display, which shows on screen.
    ///
    /// `offset` is in bytes from the start of the framebuffer (see [`BootDisplayMode`] for its
    /// layout). `offset` and the length of `data` must be multiples of 4. Fails with `ENODEV`
    /// without a usable boot display.
    pub fn write_boot_display(&self, offset: usize, data: &[u8]) -> Result {
        self.gpu.write_boot_display(offset, data)
    }

    /// Returns the size of the PCIe BAR used for accessing VRAM, typically
    /// BAR1.
    pub fn bar1_size(&self) -> Result<u64> {
        let bar1 = driver::bar1_resource_index(self.pdev)?;

        self.pdev.resource_len(bar1)
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
