// SPDX-License-Identifier: GPL-2.0

//! GPU virtual address spaces of userspace.
//!
//! Userspace places its mappings itself, at the addresses it chooses, as Vulkan drivers do, but
//! in the kernel range it gives when creating its address space, where nova-core maps what it
//! needs for userspace. GPUVM tracks the mappings: mapping over existing mappings replaces the
//! parts it covers, and unmapping part of a mapping keeps the rest mapped.

use core::ops::Range;

use kernel::{
    drm::gem::BaseObject,
    drm::gpuvm::{
        DriverGpuVm,
        GpuVaAlloc,
        GpuVm,
        OpMap,
        OpMapRequest,
        OpMapped,
        OpRemap,
        OpRemapped,
        OpUnmap,
        OpUnmapped,
        UniqueRefGpuVm, //
    },
    page,
    prelude::*, //
};

use nova_core::api::{
    BufferRef,
    ChannelEngine,
    ClientVm,
    NovaCoreApi,
    VA_SIZE, //
};

use crate::{
    driver::{
        NovaDevice,
        NovaDriver, //
    },
    gem::{
        NovaGemObject,
        NovaObject,
        Placement, //
    },
};

/// The nova-core side of an address space of userspace.
pub(crate) struct NovaVm {
    /// Taken when the address space is freed.
    vm: Option<ClientVm>,
}

/// What is known of a mapping.
pub(crate) struct VaData {
    /// The PTE kind of the mapping.
    kind: u8,
}

/// Number of [`GpuVaAlloc`]s an operation may need: one for a new mapping, and two for the parts
/// of a mapping it splits that stay mapped.
const VAS_PER_OP: usize = 3;

/// What the steps of operations on address spaces need of nova-core.
pub(crate) trait VmApi {
    /// See [`NovaCoreApi::vm_map`].
    fn vm_map(
        &self,
        vm: &mut ClientVm,
        addr: u64,
        buffer: BufferRef<'_>,
        offset: u64,
        size: u64,
        kind: u8,
    ) -> Result;

    /// See [`NovaCoreApi::vm_unmap`].
    fn vm_unmap(&self, vm: &mut ClientVm, addr: u64, size: u64) -> Result;
}

impl VmApi for NovaCoreApi<'_> {
    fn vm_map(
        &self,
        vm: &mut ClientVm,
        addr: u64,
        buffer: BufferRef<'_>,
        offset: u64,
        size: u64,
        kind: u8,
    ) -> Result {
        NovaCoreApi::vm_map(self, vm, addr, buffer, offset, size, kind)
    }

    fn vm_unmap(&self, vm: &mut ClientVm, addr: u64, size: u64) -> Result {
        NovaCoreApi::vm_unmap(self, vm, addr, size)
    }
}

/// What the steps of an operation on an address space need.
pub(crate) struct BindContext<'ctx> {
    api: &'ctx dyn VmApi,
    /// The PTE kind of the mapping created, if any.
    kind: u8,
    /// [`GpuVaAlloc`]s allocated before the operation, which can't fail allocating them.
    vas: KVec<GpuVaAlloc<NovaVm>>,
}

impl BindContext<'_> {
    /// Takes one of the [`GpuVaAlloc`]s allocated before the operation.
    fn va(&mut self) -> Result<GpuVaAlloc<NovaVm>> {
        self.vas.pop().ok_or(EINVAL)
    }
}

impl NovaVm {
    fn vm(&mut self) -> Result<&mut ClientVm> {
        self.vm.as_mut().ok_or(ENODEV)
    }
}

impl DriverGpuVm for NovaVm {
    type Driver = NovaDriver;
    type Object = NovaGemObject;
    type VaData = VaData;
    type VmBoData = ();
    type SmContext<'ctx>
        = BindContext<'ctx>
    where
        Self: 'ctx;

    fn sm_step_map<'op, 'ctx>(
        &mut self,
        op: OpMap<'op, Self>,
        context: &mut BindContext<'ctx>,
    ) -> Result<OpMapped<'op, Self>> {
        context.api.vm_map(
            self.vm()?,
            op.addr(),
            op.obj().buffer()?,
            op.gem_offset(),
            op.length(),
            context.kind,
        )?;

        let kind = context.kind;
        Ok(op.insert(context.va()?, VaData { kind }))
    }

    fn sm_step_unmap<'op, 'ctx>(
        &mut self,
        op: OpUnmap<'op, Self>,
        context: &mut BindContext<'ctx>,
    ) -> Result<OpUnmapped<'op, Self>> {
        context
            .api
            .vm_unmap(self.vm()?, op.va().addr(), op.va().length())?;

        let (unmapped, _va) = op.remove();
        Ok(unmapped)
    }

    fn sm_step_remap<'op, 'ctx>(
        &mut self,
        op: OpRemap<'op, Self>,
        context: &mut BindContext<'ctx>,
    ) -> Result<OpRemapped<'op, Self>> {
        // The part of the mapping between those that stay mapped goes.
        let va = op.va_to_unmap();
        let start = op
            .prev()
            .map_or(va.addr(), |prev| prev.addr() + prev.length());
        let end = op
            .next()
            .map_or(va.addr() + va.length(), |next| next.addr());
        if start < end {
            context.api.vm_unmap(self.vm()?, start, end - start)?;
        }

        let kind = va.data().kind;
        let vas = [context.va()?, context.va()?];
        let (remapped, ret) = op.remap(vas, VaData { kind }, VaData { kind });
        if let Some(unused) = ret.unused_va {
            context.vas.push(unused, GFP_KERNEL)?;
        }

        Ok(remapped)
    }
}

/// An address space of userspace.
pub(crate) struct Vm {
    gpuvm: UniqueRefGpuVm<NovaVm>,
    /// Where nova-core maps what it needs for userspace.
    kernel: Range<u64>,
    /// The IDs of the channels running in the address space.
    channels: KVec<u32>,
}

/// An operation on the mappings of an address space.
pub(crate) enum BindOp<'a> {
    /// Maps `size` bytes of `obj` from `offset` at `addr`, as memory of the PTE kind `kind`.
    Map {
        addr: u64,
        size: u64,
        obj: &'a NovaGemObject,
        offset: u64,
        kind: u8,
    },
    /// Unmaps `size` bytes from `addr`.
    Unmap { addr: u64, size: u64 },
}

impl Vm {
    /// Creates an empty address space whose kernel range is `kernel`.
    pub(crate) fn new(dev: &NovaDevice, api: &NovaCoreApi<'_>, kernel: Range<u64>) -> Result<Self> {
        // GPUVM shares the reservation of the objects only mapped into the address space with
        // this one.
        let resv = NovaObject::new(dev, page::PAGE_SIZE, Placement::System)?;

        let vm = api.create_vm(kernel.clone())?;
        // GPUVM doesn't map anything in the kernel range.
        match GpuVm::new::<Error, _>(
            c"nova",
            dev,
            &*resv,
            0..VA_SIZE,
            kernel.clone(),
            NovaVm { vm: None },
        ) {
            Ok(mut gpuvm) => {
                gpuvm.data().vm = Some(vm);
                Ok(Self {
                    gpuvm,
                    kernel,
                    channels: KVec::new(),
                })
            }
            Err(err) => {
                api.free_vm(vm);
                Err(err)
            }
        }
    }

    /// Carries out `op`.
    ///
    /// Fails with `EINVAL` if the addresses aren't page aligned, or reach out of the address space
    /// or into its kernel range.
    pub(crate) fn bind(&mut self, api: &NovaCoreApi<'_>, op: BindOp<'_>) -> Result {
        let (addr, size) = match op {
            BindOp::Map { addr, size, .. } | BindOp::Unmap { addr, size } => (addr, size),
        };
        let page_size = page::PAGE_SIZE as u64;
        let end = addr.checked_add(size).ok_or(EINVAL)?;
        if size == 0
            || addr % page_size != 0
            || size % page_size != 0
            || end > VA_SIZE
            || (addr < self.kernel.end && self.kernel.start < end)
        {
            return Err(EINVAL);
        }

        let mut vas = KVec::with_capacity(VAS_PER_OP, GFP_KERNEL)?;
        for _ in 0..VAS_PER_OP {
            vas.push(GpuVaAlloc::new(GFP_KERNEL)?, GFP_KERNEL)?;
        }
        let mut context = BindContext { api, kind: 0, vas };

        let result = match op {
            BindOp::Map {
                addr,
                size,
                obj,
                offset,
                kind,
            } => {
                let end = offset.checked_add(size).ok_or(EINVAL)?;
                if offset % page_size != 0 || end > obj.size() as u64 {
                    return Err(EINVAL);
                }

                context.kind = kind;
                let vm_bo = self.gpuvm.obtain(obj, ())?;
                self.gpuvm.sm_map(OpMapRequest {
                    addr,
                    range: size,
                    gem_offset: offset,
                    vm_bo: &vm_bo,
                    context: &mut context,
                })
            }
            BindOp::Unmap { addr, size } => self.gpuvm.sm_unmap(addr, size, &mut context),
        };
        // Drops the objects nothing maps anymore.
        self.gpuvm.deferred_cleanup();

        result
    }

    /// Unmaps everything, and frees the address space.
    pub(crate) fn free(mut self, api: &NovaCoreApi<'_>) {
        // The channels first, which use the address space.
        while let Some(chid) = self.channels.last().copied() {
            let _ = self.destroy_channel(api, chid);
        }

        // Below the kernel range, then above it.
        for range in [0..self.kernel.start, self.kernel.end..VA_SIZE] {
            if !range.is_empty() {
                let _ = self.bind(
                    api,
                    BindOp::Unmap {
                        addr: range.start,
                        size: range.end - range.start,
                    },
                );
            }
        }

        if let Some(vm) = self.gpuvm.data().vm.take() {
            api.free_vm(vm);
        }
    }

    /// Creates a channel on `engine` running in the address space, and returns its ID.
    pub(crate) fn create_channel(
        &mut self,
        api: &NovaCoreApi<'_>,
        engine: ChannelEngine,
    ) -> Result<u32> {
        self.channels.reserve(1, GFP_KERNEL)?;
        let chid = api.create_channel(self.gpuvm.data().vm()?, engine)?;
        self.channels.push(chid, GFP_KERNEL)?;

        Ok(chid)
    }

    /// Destroys the channel `chid` running in the address space, once the work it runs is done.
    pub(crate) fn destroy_channel(&mut self, api: &NovaCoreApi<'_>, chid: u32) -> Result {
        let index = self
            .channels
            .iter()
            .position(|&channel| channel == chid)
            .ok_or(ENOENT)?;
        self.channels.remove(index)?;

        api.destroy_channel(self.gpuvm.data().vm()?, chid)
    }

    /// Returns whether the channel `chid` runs in the address space.
    pub(crate) fn has_channel(&self, chid: u32) -> bool {
        self.channels.contains(&chid)
    }
}
