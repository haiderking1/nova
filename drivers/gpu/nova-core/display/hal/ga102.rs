// SPDX-License-Identifier: GPL-2.0

//! Display engine of GA10x GPUs (but GA100, which has none), with the classes
//! `NVC670_DISPLAY`, `NVC67D_CORE_CHANNEL_DMA` and `NVC67E_WINDOW_CHANNEL_DMA`.

use kernel::{
    device,
    io::{
        register::Array,
        Io, //
    },
    prelude::*, //
};

use crate::{
    driver::Bar0,
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
        disp::{
            core_c67d,
            window_c67e, //
        },
        rm::commands::{
            AllocCoreChannelGa102,
            AllocDisplayGa102,
            AllocWindowChannelGa102,
            DispChannelPushbuffer,
            RmAlloc,
            RmControl, //
        },
    },
};

use super::{
    super::{
        channel::PushBuffer,
        engine::{
            armed,
            assembly,
            InternalHandles,
            PresentMode, //
        },
        mode::HeadRaster,
        regs,
    },
    CoreNotifier,
    DisplayHal,
    HeadOutput,
    WindowImage, //
};

struct Ga102;

pub(super) const GA102_HAL: &dyn DisplayHal = &Ga102;

impl DisplayHal for Ga102 {
    fn alloc_display(
        &self,
        dev: &device::Device,
        cmdq: &Cmdq<'_>,
        client: Handle<Client>,
        device: Handle<Device>,
        handle: u32,
    ) -> Result<Handle<DisplayRoot>> {
        Ok(RmAlloc::new(client, device, Handle::new(handle), AllocDisplayGa102).send(dev, cmdq)?)
    }

    fn alloc_core(
        &self,
        dev: &device::Device,
        cmdq: &Cmdq<'_>,
        internal: &InternalHandles,
        client: Handle<Client>,
        root: Handle<DisplayRoot>,
        handle: u32,
        push: &PushBuffer<'_>,
    ) -> Result<Handle<DisplayCoreChannel>> {
        RmControl::new(
            internal.client,
            internal.subdevice,
            DispChannelPushbuffer::core_ga102(push.dma_address()),
        )
        .send(dev, cmdq)?;

        Ok(RmAlloc::new(
            client,
            root,
            Handle::new(handle),
            AllocCoreChannelGa102::new(0),
        )
        .send(dev, cmdq)?)
    }

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
    ) -> Result<Handle<DisplayWindowChannel>> {
        RmControl::new(
            internal.client,
            internal.subdevice,
            DispChannelPushbuffer::window_ga102(window, push.dma_address()),
        )
        .send(dev, cmdq)?;

        Ok(RmAlloc::new(
            client,
            root,
            Handle::new(handle),
            AllocWindowChannelGa102::new(window, 0),
        )
        .send(dev, cmdq)?)
    }

    fn push_core_nop(&self, bar: Bar0<'_>, push: &mut PushBuffer<'_>) -> Result {
        // No notifier for the next update.
        push.push(bar, core_c67d::SET_CONTEXT_DMA_NOTIFIER, &[0])
    }

    fn push_window_update(
        &self,
        bar: Bar0<'_>,
        push: &mut PushBuffer<'_>,
        image: &WindowImage<'_>,
    ) -> Result {
        use window_c67e::*;

        let surface = image.surface;
        let size = surface.width | surface.height << 16;
        let offset = u32::try_from(surface.addr.into_raw() >> 8)?;

        /// Format conversion matrix leaving colors as they are, in S5.16 fixed point.
        const IDENTITY: [u32; 12] = [
            0x10000, 0, 0, 0, //
            0, 0x10000, 0, 0, //
            0, 0, 0x10000, 0,
        ];

        let methods = [
            // The update reports its progress in the notifier, whose offset is in units of 16
            // bytes from bit 4.
            (SET_CONTEXT_DMA_NOTIFIER, image.notifier_ctxdma),
            (
                SET_NOTIFIER_CONTROL,
                NOTIFIER_MODE_WRITE | image.notifier_offset,
            ),
            (
                SET_PRESENT_CONTROL,
                match image.present {
                    PresentMode::Vsync => PRESENT_VSYNC,
                    PresentMode::Immediate => PRESENT_IMMEDIATE,
                },
            ),
            (SET_SIZE, size),
            // Pitch-linear.
            (SET_STORAGE, 0),
            (SET_PARAMS, FORMAT_X8R8G8B8),
            (set_planar_storage(0), surface.pitch >> 6),
            (set_context_dma_iso(0), image.ctxdma),
            (set_offset(0), offset),
            (set_point_in(0), 0),
            (SET_SIZE_IN, size),
            (SET_SIZE_OUT, size),
            // Shown as it is, without the input LUT that composition would need.
            (SET_CONTEXT_DMA_ILUT, 0),
            (SET_COMPOSITION_CONTROL, COMPOSITION_BYPASS),
            // The update waits for no other channel, or for the core channel's next update
            // (`INTERLOCK_WITH_CORE`, bit 0).
            (SET_INTERLOCK_FLAGS, u32::from(image.interlock_with_core)),
            (SET_WINDOW_INTERLOCK_FLAGS, 0),
        ];

        for (method, value) in methods {
            push.push(bar, method, &[value])?;
        }
        push.push(bar, SET_FMT_COEFFICIENT_C00, &IDENTITY)?;
        push.push(bar, UPDATE, &[1])
    }

    fn set_vblank_interrupt(&self, bar: Bar0<'_>, head: usize, enable: bool) -> Result {
        let enables = regs::NV_PDISP_FE_RM_INTR_EN_HEAD_TIMING::try_at(head).ok_or(EINVAL)?;
        let drop_pending = || -> Result {
            bar.write(
                regs::NV_PDISP_FE_EVT_STAT_HEAD_TIMING::try_at(head).ok_or(EINVAL)?,
                regs::NV_PDISP_FE_EVT_STAT_HEAD_TIMING::zeroed().with_last_data(true),
            );
            Ok(())
        };

        // Like RM, drop the vblank the head may have had pending before enabling its interrupt,
        // and after disabling it.
        if enable {
            drop_pending()?;
        }
        bar.update(enables, |reg| reg.with_last_data(enable));
        if !enable {
            drop_pending()?;
        }

        Ok(())
    }

    fn take_vblanks(&self, bar: Bar0<'_>) -> u32 {
        let dispatch = bar
            .read(regs::NV_PDISP_FE_RM_INTR_DISPATCH)
            .head_timing()
            .get();
        let mut heads = 0;

        for head in (0..8).filter(|head| dispatch & 1 << head != 0) {
            let (Some(stat), Some(events)) = (
                regs::NV_PDISP_FE_RM_INTR_STAT_HEAD_TIMING::try_at(head),
                regs::NV_PDISP_FE_EVT_STAT_HEAD_TIMING::try_at(head),
            ) else {
                continue;
            };

            if bar.read(stat).last_data() {
                bar.write(
                    events,
                    regs::NV_PDISP_FE_EVT_STAT_HEAD_TIMING::zeroed().with_last_data(true),
                );
                heads |= 1 << head;
            }
        }

        heads
    }

    fn push_head_mode(
        &self,
        bar: Bar0<'_>,
        push: &mut PushBuffer<'_>,
        head: u32,
        display_id: u32,
        output: &HeadOutput,
        raster: &HeadRaster,
    ) -> Result {
        use core_c67d::*;

        // Like NVIDIA's driver on each head enable (`nvkms-evo.c`): the SOR the head drives gets
        // its control, sent again, and GSP-RM is told the display the head drives, which it
        // programs the output for in the modeset's supervisor steps.
        push.push(bar, sor_set_control(output.sor), &[output.control])?;
        push.push(bar, head_set_display_id(head), &[display_id])?;

        let position = |(x, y): (u16, u16)| u32::from(x) | u32::from(y) << 16;
        // The output stays as it is but for the sync polarities, in bits 2 and 3.
        let output = armed(
            bar,
            usize::try_from(head_set_control_output_resource(head))?,
        )
        .ok_or(EINVAL)?;
        let output = output & !0b1100
            | u32::from(raster.sync_negative.0) << 2
            | u32::from(raster.sync_negative.1) << 3;

        push.push(bar, head_set_control_output_resource(head), &[output])?;
        push.push(
            bar,
            head_set_pixel_clock_frequency(head),
            &[raster.pixel_clock],
        )?;
        // Not driver-managed, no spread spectrum.
        push.push(bar, head_set_pixel_clock_configuration(head), &[0])?;
        push.push(
            bar,
            head_set_pixel_clock_frequency_max(head),
            &[raster.pixel_clock],
        )?;
        push.push(
            bar,
            head_set_raster_size(head),
            &[
                position(raster.total),
                position(raster.sync_end),
                position(raster.blank_end),
                position(raster.blank_start),
            ],
        )?;
        // No second vertical blanking, as for progressive rasters in nouveau.
        push.push(bar, head_set_raster_vert_blank2(head), &[1])?;

        // The viewport is the whole active area, unscaled.
        let active = position((
            raster.blank_start.0 - raster.blank_end.0,
            raster.blank_start.1 - raster.blank_end.1,
        ));
        push.push(bar, head_set_viewport_point_in(head), &[0])?;
        push.push(bar, head_set_viewport_size_in(head), &[active])?;
        push.push(bar, head_set_viewport_size_out(head), &[active])
    }

    fn push_window_bounds(&self, bar: Bar0<'_>, push: &mut PushBuffer<'_>, window: u32) -> Result {
        use core_c67d::*;

        let method = window_set_window_usage_bounds(window);
        let bounds = armed(bar, usize::try_from(method)?).ok_or(EINVAL)?;
        // `MAX_PIXELS_FETCHED_PER_LINE`, bits 14:0.
        push.push(bar, method, &[bounds | 0x7fff])
    }

    fn armed_output(&self, bar: Bar0<'_>, head: u32) -> Option<HeadOutput> {
        use core_c67d::*;

        let head_bit = 1u32.checked_shl(head)?;
        (0..SORS).find_map(|sor| {
            let control = armed(bar, usize::try_from(sor_set_control(sor)).ok()?)?;
            // The heads driving the SOR are in bits 7:0.
            (control & 0xff & head_bit != 0).then_some(HeadOutput { sor, control })
        })
    }

    fn armed_output_bpp(&self, bar: Bar0<'_>, head: u32) -> Option<u32> {
        use core_c67d::*;

        let output = armed(
            bar,
            usize::try_from(head_set_control_output_resource(head)).ok()?,
        )?;
        output_resource_bpp(output)
    }

    fn push_detach(
        &self,
        bar: Bar0<'_>,
        push: &mut PushBuffer<'_>,
        head: u32,
        output: &HeadOutput,
    ) -> Result {
        use core_c67d::*;

        // Like nouveau: the head stops driving the SOR, which is all off if no other head drives
        // it, and drives no display.
        let control = output.control & !1u32.checked_shl(head).ok_or(EINVAL)?;
        let control = if control & 0xff == 0 {
            SOR_CONTROL_NONE
        } else {
            control
        };
        push.push(bar, sor_set_control(output.sor), &[control])?;
        push.push(bar, head_set_display_id(head), &[0])
    }

    fn push_core_update(
        &self,
        bar: Bar0<'_>,
        push: &mut PushBuffer<'_>,
        windows: u32,
        notifier: Option<CoreNotifier>,
    ) -> Result {
        use core_c67d::*;

        match notifier {
            Some(notifier) => {
                // The offset is in units of 16 bytes, in bits 11:4.
                push.push(bar, SET_CONTEXT_DMA_NOTIFIER, &[notifier.ctxdma])?;
                push.push(
                    bar,
                    SET_NOTIFIER_CONTROL,
                    &[NOTIFIER_MODE_WRITE | notifier.offset & 0xff0 | NOTIFIER_NOTIFY],
                )?;
            }
            None => push.push(bar, SET_CONTEXT_DMA_NOTIFIER, &[0])?,
        }
        push.push(bar, SET_INTERLOCK_FLAGS, &[0])?;
        push.push(bar, SET_WINDOW_INTERLOCK_FLAGS, &[windows])?;
        push.push(bar, UPDATE, &[UPDATE_RELEASE])?;
        if notifier.is_some() {
            // Like nouveau: the following updates don't write the notifier.
            push.push(bar, SET_NOTIFIER_CONTROL, &[0])?;
        }

        Ok(())
    }

    fn armed_raster(&self, bar: Bar0<'_>, head: u32) -> Option<HeadRaster> {
        use core_c67d::*;

        let read = |method: u32| armed(bar, usize::try_from(method).ok()?);
        // CAST: positions are 15-bit fields.
        let position = |value: u32| ((value & 0x7fff) as u16, (value >> 16 & 0x7fff) as u16);

        Some(HeadRaster {
            total: position(read(head_set_raster_size(head))?),
            sync_end: position(read(head_set_raster_sync_end(head))?),
            blank_end: position(read(head_set_raster_blank_end(head))?),
            blank_start: position(read(head_set_raster_blank_start(head))?),
            pixel_clock: read(head_set_pixel_clock_frequency(head))? & 0x7fff_ffff,
            sync_negative: {
                let output = read(head_set_control_output_resource(head))?;
                (output & 1 << 2 != 0, output & 1 << 3 != 0)
            },
        })
    }

    fn log_armed_state(&self, dev: &device::Device, bar: Bar0<'_>, window: usize) {
        // What the first core update would change is logged too: it arms all of the assembly
        // state.
        let log = |name: &str, method: usize| {
            let pending = assembly(bar, method);
            if let Some(value) = armed(bar, method) {
                if pending.is_some_and(|pending| pending != value) {
                    dev_info!(
                        dev,
                        "display: armed {} ({:#06x}) = {:#010x}, assembly {:#010x?}\n",
                        name,
                        method,
                        value,
                        pending
                    );
                } else {
                    dev_dbg!(
                        dev,
                        "display: armed {} ({:#06x}) = {:#010x}\n",
                        name,
                        method,
                        value
                    );
                }
            }
        };

        log("SET_CONTEXT_DMA_NOTIFIER", 0x208);
        for sor in 0..4 {
            log("SOR_SET_CONTROL", 0x300 + sor * 0x20);
        }
        for window in 0..8 {
            log("WINDOW_SET_CONTROL", 0x1000 + window * 0x80);
        }
        // What the window Nova uses is allowed to do.
        let window = window * 0x80;
        log("WINDOW_SET_WINDOW_FORMAT_USAGE_BOUNDS", 0x1004 + window);
        log(
            "WINDOW_SET_WINDOW_ROTATED_FORMAT_USAGE_BOUNDS",
            0x1008 + window,
        );
        log("WINDOW_SET_MAX_INPUT_SCALE_FACTOR", 0x100c + window);
        log("WINDOW_SET_WINDOW_USAGE_BOUNDS", 0x1010 + window);
        for (name, method) in LOGGED_HEAD_METHODS {
            log(name, method);
        }
    }
}

/// Core channel methods of head 0 whose armed state is logged, with their names in `clc67d.h`.
const LOGGED_HEAD_METHODS: [(&str, usize); 26] = [
    ("PROCAMP", 0x2000),
    ("CONTROL_OUTPUT_RESOURCE", 0x2004),
    ("CONTROL", 0x2008),
    ("PIXEL_CLOCK_FREQUENCY", 0x200c),
    ("PIXEL_CLOCK_CONFIGURATION", 0x201c),
    ("DISPLAY_ID(0)", 0x2020),
    ("PIXEL_CLOCK_FREQUENCY_MAX", 0x2028),
    ("HEAD_USAGE_BOUNDS", 0x2030),
    ("VIEWPORT_POINT_IN", 0x2048),
    ("VIEWPORT_SIZE_IN", 0x204c),
    ("VIEWPORT_SIZE_OUT", 0x2058),
    ("RASTER_SIZE", 0x2064),
    ("RASTER_SYNC_END", 0x2068),
    ("RASTER_BLANK_END", 0x206c),
    ("RASTER_BLANK_START", 0x2070),
    ("0x2074", 0x2074),
    ("CONTEXT_DMA_CURSOR(0)", 0x2088),
    ("OFFSET_CURSOR(0)", 0x2090),
    ("CONTROL_CURSOR", 0x209c),
    ("PRESENT_CONTROL", 0x218c),
    ("DISPLAY_RATE", 0x21a8),
    ("OLUT_CONTROL", 0x2280),
    ("OLUT_FP_NORM_SCALE", 0x2284),
    ("CONTEXT_DMA_OLUT", 0x2288),
    ("OFFSET_OLUT", 0x228c),
    ("DSC_CONTROL", 0x22d4),
];
