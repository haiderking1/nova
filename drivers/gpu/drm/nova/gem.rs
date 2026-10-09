// SPDX-License-Identifier: GPL-2.0

use kernel::{
    drm,
    drm::gem::{
        self,
        BaseObject,
        KernelMapping, //
    },
    mm::virt::VmaNew,
    page,
    prelude::*,
    sync::aref::ARef,
};

use nova_core::api::{
    BufferRef,
    SystemBuffer,
    VramBuffer,
    VRAM_BUFFER_ALIGN, //
};

use crate::{
    driver::{NovaDevice, NovaDriver},
    file::File,
    kms::with_api,
};

/// GEM object of nova-drm, whose memory nova-core provides.
pub(crate) type NovaGemObject = gem::Object<NovaObject>;

/// Where the memory of a GEM object is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Placement {
    /// VRAM, with a window in BAR1 for the CPU if `cpu_visible`.
    Vram { cpu_visible: bool },
    /// System memory, which the GPU accesses coherently with the CPU's caches.
    System,
}

/// The memory of a GEM object.
enum Memory {
    /// Given back to nova-core when the object is freed.
    Vram(Option<VramBuffer>),
    System(SystemBuffer),
}

/// GEM Object inner driver data
#[pin_data(PinnedDrop)]
pub(crate) struct NovaObject {
    /// The device, to give the object's memory back to nova-core.
    drm: ARef<NovaDevice>,
    memory: Memory,
}

impl gem::DriverObject for NovaObject {
    type Driver = NovaDriver;
    type Args = Placement;

    const PRIVATE: bool = true;

    fn new(dev: &NovaDevice, size: usize, placement: Placement) -> impl PinInit<Self, Error> {
        try_pin_init!(NovaObject {
            drm: dev.into(),
            memory: with_api(dev, |api| {
                let memory = match placement {
                    Placement::Vram { cpu_visible } => {
                        Memory::Vram(Some(api.alloc_vram(size, cpu_visible)?))
                    }
                    Placement::System => Memory::System(api.alloc_system(size)?),
                };
                // The object's size is that of its memory.
                if memory.size() != size {
                    memory.free(api);
                    return Err(EINVAL);
                }

                Ok(memory)
            })?,
        })
    }

    fn mmap(obj: &NovaGemObject, vma: &VmaNew) -> Result {
        match &obj.memory {
            Memory::Vram(buffer) => buffer.as_ref().ok_or(ENODEV)?.mmap(vma),
            Memory::System(buffer) => buffer.mmap(vma),
        }
    }

    fn vmap(obj: &NovaGemObject) -> Result<KernelMapping> {
        match &obj.memory {
            // The kernel doesn't map VRAM buffers.
            Memory::Vram(_) => Err(EOPNOTSUPP),
            Memory::System(buffer) => Ok(KernelMapping::System(buffer.vaddr())),
        }
    }

    fn dumb_args(_dev: &NovaDevice, _size: usize) -> Result<Placement> {
        // The CPU copies dumb buffers to the display.
        Ok(Placement::System)
    }
}

impl Memory {
    /// Returns the size of the memory in bytes.
    fn size(&self) -> usize {
        match self {
            Self::Vram(buffer) => buffer.as_ref().map_or(0, VramBuffer::size),
            Self::System(buffer) => buffer.size(),
        }
    }

    /// Gives the memory back to nova-core.
    fn free(self, api: &nova_core::api::NovaCoreApi<'_>) {
        if let Self::Vram(Some(buffer)) = self {
            api.free_vram(buffer);
        }
    }
}

#[pinned_drop]
impl PinnedDrop for NovaObject {
    fn drop(self: Pin<&mut Self>) {
        // SAFETY: `memory` isn't structurally pinned.
        let this = unsafe { self.get_unchecked_mut() };

        if let Memory::Vram(buffer) = &mut this.memory {
            if let Some(buffer) = buffer.take() {
                // Without nova-core, the buffer just goes away, as its memory did.
                let _ = with_api(&this.drm, |api| {
                    api.free_vram(buffer);
                    Ok(())
                });
            }
        }
    }
}

impl NovaObject {
    /// Creates a new DRM GEM object of at least `size` bytes, in `placement`.
    pub(crate) fn new(
        dev: &NovaDevice,
        size: usize,
        placement: Placement,
    ) -> Result<ARef<NovaGemObject>> {
        if size == 0 {
            return Err(EINVAL);
        }
        // The size of the memory nova-core allocates.
        let size = match placement {
            Placement::Vram { .. } => size.checked_next_multiple_of(VRAM_BUFFER_ALIGN),
            Placement::System => page::page_align(size),
        }
        .ok_or(EINVAL)?;

        gem::Object::new(dev, size, placement)
    }

    /// Returns the memory of the object, for nova-core to map it.
    pub(crate) fn buffer(&self) -> Result<BufferRef<'_>> {
        Ok(match &self.memory {
            Memory::Vram(buffer) => BufferRef::Vram(buffer.as_ref().ok_or(ENODEV)?),
            Memory::System(buffer) => BufferRef::System(buffer),
        })
    }

    /// Returns where the memory of the object is.
    pub(crate) fn placement(&self) -> Placement {
        match &self.memory {
            Memory::Vram(buffer) => Placement::Vram {
                cpu_visible: buffer.as_ref().is_some_and(VramBuffer::cpu_visible),
            },
            Memory::System(_) => Placement::System,
        }
    }

    /// Look up a GEM object handle for a `File` and return an `ObjectRef` for it.
    #[inline]
    pub(crate) fn lookup_handle(
        file: &drm::File<File>,
        handle: u32,
    ) -> Result<ARef<NovaGemObject>> {
        NovaGemObject::lookup_handle(file, handle)
    }
}
