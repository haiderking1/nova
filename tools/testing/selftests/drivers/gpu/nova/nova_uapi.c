// SPDX-License-Identifier: GPL-2.0
/*
 * Tests of nova's uAPI, on the first render node driven by nova-drm, and of its
 * dumb buffers, on its primary node.
 */

#include <errno.h>
#include <fcntl.h>
#include <stdio.h>
#include <string.h>
#include <sys/ioctl.h>
#include <sys/mman.h>
#include <time.h>
#include <unistd.h>

#include <drm/drm.h>
#include <drm/nova_drm.h>

#include "../../../kselftest_harness.h"

/* Primary nodes are numbered from 0, render nodes from 128, at most 64 of each. */
#define PRIMARY_NODE_FIRST	0
#define RENDER_NODE_FIRST	128
#define NODE_COUNT		64

/* Opens the first /dev/dri/<@name><n> node nova-drm drives, from @first on, or returns -1. */
static int nova_open(const char *node, int first)
{
	for (int i = 0; i < NODE_COUNT; i++) {
		char path[32], name[32] = "";
		struct drm_version version = {
			.name = name,
			.name_len = sizeof(name) - 1,
		};
		int fd;

		snprintf(path, sizeof(path), "/dev/dri/%s%d", node, first + i);
		fd = open(path, O_RDWR | O_CLOEXEC);
		if (fd < 0)
			continue;
		if (!ioctl(fd, DRM_IOCTL_VERSION, &version) && !strcmp(name, "nova-drm"))
			return fd;
		close(fd);
	}

	return -1;
}

/* Queries the information @id into @data, of @size bytes; returns the size written. */
static int nova_info(int fd, __u32 id, void *data, __u32 size)
{
	struct drm_nova_info info = {
		.id = id,
		.size = size,
		.data = (__u64)(uintptr_t)data,
	};

	if (ioctl(fd, DRM_IOCTL_NOVA_INFO, &info))
		return -errno;

	return info.size;
}

FIXTURE(nova) {
	int fd;
};

FIXTURE_SETUP(nova)
{
	self->fd = nova_open("renderD", RENDER_NODE_FIRST);
	if (self->fd < 0)
		SKIP(return, "no render node driven by nova-drm");
}

FIXTURE_TEARDOWN(nova)
{
	if (self->fd >= 0)
		close(self->fd);
}

TEST_F(nova, info_gpu)
{
	struct drm_nova_info_gpu gpu = {};

	ASSERT_EQ(sizeof(gpu), nova_info(self->fd, DRM_NOVA_INFO_GPU, NULL, 0));
	ASSERT_EQ(sizeof(gpu), nova_info(self->fd, DRM_NOVA_INFO_GPU, &gpu, sizeof(gpu)));

	TH_LOG("%s (%s): architecture %#x, chip %#x, %llu MiB of VRAM, BAR1 of %llu MiB",
	       gpu.gpu_name, gpu.gpu_short_name, gpu.architecture, gpu.chipid,
	       gpu.vram_size >> 20, gpu.vram_bar_size >> 20);
	EXPECT_NE(0, gpu.chipid);
	EXPECT_NE(0, gpu.vram_size);
	EXPECT_NE(0, gpu.vram_bar_size);
}

TEST_F(nova, info_graphics)
{
	struct drm_nova_info_graphics gr = {};

	ASSERT_EQ(sizeof(gr), nova_info(self->fd, DRM_NOVA_INFO_GRAPHICS, NULL, 0));
	ASSERT_EQ(sizeof(gr), nova_info(self->fd, DRM_NOVA_INFO_GRAPHICS, &gr, sizeof(gr)));

	TH_LOG("SM %u.%u, %u GPCs, %u TPCs of %u SMs, %u warps per SM",
	       gr.sm_major, gr.sm_minor, gr.gpc_count, gr.tpc_count, gr.sm_per_tpc,
	       gr.max_warps_per_sm);
	TH_LOG("classes: gpfifo %#x, copy %#x, 2d %#x, 3d %#x, m2mf %#x, compute %#x",
	       gr.class_gpfifo, gr.class_copy, gr.class_2d, gr.class_3d, gr.class_m2mf,
	       gr.class_compute);
	EXPECT_NE(0, gr.sm_major);
	EXPECT_NE(0, gr.gpc_count);
	EXPECT_LE(gr.gpc_count, gr.tpc_count);
	EXPECT_NE(0, gr.sm_per_tpc);
	EXPECT_NE(0, gr.max_warps_per_sm);
	EXPECT_NE(0, gr.class_gpfifo);
	EXPECT_NE(0, gr.class_copy);
	EXPECT_NE(0, gr.class_2d);
	EXPECT_NE(0, gr.class_3d);
	EXPECT_NE(0, gr.class_m2mf);
	EXPECT_NE(0, gr.class_compute);
}

/* A buffer too small for the information gets as much of it as fits. */
TEST_F(nova, info_truncated)
{
	struct drm_nova_info_graphics gr = {};

	ASSERT_EQ(2, nova_info(self->fd, DRM_NOVA_INFO_GRAPHICS, &gr, 2));
	EXPECT_NE(0, gr.sm_major);
	EXPECT_EQ(0, gr.gpc_count);
}

TEST_F(nova, info_unknown)
{
	char data[64];

	EXPECT_EQ(-EINVAL, nova_info(self->fd, 0xffff, data, sizeof(data)));
}


/* Creates an object of @size bytes with @flags; returns its handle, or -errno. */
static int nova_gem_create(int fd, __u64 size, __u32 flags)
{
	struct drm_nova_gem_create create = {
		.flags = flags,
		.size = size,
	};

	if (ioctl(fd, DRM_IOCTL_NOVA_GEM_CREATE, &create))
		return -errno;

	return create.handle;
}

static void nova_gem_close(int fd, __u32 handle)
{
	struct drm_gem_close close = { .handle = handle };

	ioctl(fd, DRM_IOCTL_GEM_CLOSE, &close);
}

static int nova_gem_info(int fd, __u32 handle, struct drm_nova_gem_info *info)
{
	*info = (struct drm_nova_gem_info){ .handle = handle };

	return ioctl(fd, DRM_IOCTL_NOVA_GEM_INFO, info) ? -errno : 0;
}

/* Maps @size bytes of the object whose mmap offset is @offset, or returns NULL. */
static __u32 *nova_gem_mmap(int fd, __u64 offset, size_t size)
{
	void *map = mmap(NULL, size, PROT_READ | PROT_WRITE, MAP_SHARED, fd, offset);

	return map == MAP_FAILED ? NULL : map;
}

/*
 * Words read per 64 KiB: CPU reads of VRAM go uncached through the PCI BAR, at
 * a few MB/s, so large objects are sampled.
 */
#define READ_SAMPLES	64

/* Returns the stride between the words read of an object of @size bytes. */
static size_t read_stride(size_t size)
{
	return size > (1 << 20) ? (64 << 10) / 4 / READ_SAMPLES : 1;
}

/* Returns the index of the first word of @map, of @size bytes, that isn't @value, or -1. */
static ssize_t find_not(const __u32 *map, size_t size, __u32 value)
{
	for (size_t i = 0; i < size / 4; i += read_stride(size))
		if (map[i] != value)
			return i;

	return -1;
}

/* Writes a pattern of @size bytes into @map. */
static void write_pattern(__u32 *map, size_t size)
{
	for (size_t i = 0; i < size / 4; i++)
		map[i] = i * 0x9e3779b9u;
}

/* Returns the index of the first word of @map, of @size bytes, not as write_pattern() wrote it. */
static ssize_t check_pattern(const __u32 *map, size_t size)
{
	for (size_t i = 0; i < size / 4; i += read_stride(size))
		if (map[i] != (__u32)(i * 0x9e3779b9u))
			return i;

	return -1;
}

/*
 * Creates an object of @size bytes with @flags, of the size @expected_size, checks that it maps
 * zeroed, and that a pattern written through a mapping reads back through another.
 */
static void gem_roundtrip(struct __test_metadata *_metadata, int fd, __u64 size, __u32 flags,
			  __u64 expected_size)
{
	struct drm_nova_gem_info info;
	__u32 *map;
	int handle;

	handle = nova_gem_create(fd, size, flags);
	ASSERT_LE(0, handle);
	ASSERT_EQ(0, nova_gem_info(fd, handle, &info));
	EXPECT_EQ(expected_size, info.size);
	EXPECT_EQ(flags | DRM_NOVA_GEM_CPU_VISIBLE, info.flags);
	ASSERT_NE(0, info.mmap_offset);

	map = nova_gem_mmap(fd, info.mmap_offset, info.size);
	ASSERT_NE(NULL, map);
	EXPECT_EQ(-1, find_not(map, info.size, 0));
	write_pattern(map, info.size);
	ASSERT_EQ(0, munmap(map, info.size));

	map = nova_gem_mmap(fd, info.mmap_offset, info.size);
	ASSERT_NE(NULL, map);
	EXPECT_EQ(-1, check_pattern(map, info.size));
	ASSERT_EQ(0, munmap(map, info.size));

	nova_gem_close(fd, handle);
}

TEST_F(nova, gem_system)
{
	gem_roundtrip(_metadata, self->fd, 5000, 0, 8192);
}

TEST_F(nova, gem_vram)
{
	gem_roundtrip(_metadata, self->fd, 100000, DRM_NOVA_GEM_VRAM | DRM_NOVA_GEM_CPU_VISIBLE,
		      128 << 10);
}

/* Large enough to span many blocks of VRAM. */
TEST_F(nova, gem_vram_large)
{
	gem_roundtrip(_metadata, self->fd, 64 << 20, DRM_NOVA_GEM_VRAM | DRM_NOVA_GEM_CPU_VISIBLE,
		      64 << 20);
}

/* VRAM the CPU can't see has no mmap offset. */
TEST_F(nova, gem_vram_private)
{
	struct drm_nova_gem_info info;
	int handle;

	handle = nova_gem_create(self->fd, 4096, DRM_NOVA_GEM_VRAM);
	ASSERT_LE(0, handle);
	ASSERT_EQ(0, nova_gem_info(self->fd, handle, &info));
	EXPECT_EQ(DRM_NOVA_GEM_VRAM, info.flags);
	EXPECT_EQ(64 << 10, info.size);
	EXPECT_EQ(0, info.mmap_offset);

	nova_gem_close(self->fd, handle);
}

/* A mapping can't be larger than its object. */
TEST_F(nova, gem_mmap_too_large)
{
	struct drm_nova_gem_info info;
	int handle;

	handle = nova_gem_create(self->fd, 4096, 0);
	ASSERT_LE(0, handle);
	ASSERT_EQ(0, nova_gem_info(self->fd, handle, &info));
	EXPECT_EQ(NULL, nova_gem_mmap(self->fd, info.mmap_offset, 2 * info.size));

	nova_gem_close(self->fd, handle);
}

TEST_F(nova, gem_invalid)
{
	EXPECT_EQ(-EINVAL, nova_gem_create(self->fd, 4096, 1 << 7));
	EXPECT_EQ(-EINVAL, nova_gem_create(self->fd, 0, 0));
}

/*
 * Shares an object with @flags from @fd with another file as a dma-buf: the importer gets the same
 * object, under one handle however often it imports it, and keeps it once the exporter's handle
 * is closed.
 */
static void gem_share(struct __test_metadata *_metadata, int fd, __u32 flags)
{
	struct drm_prime_handle export = { .flags = DRM_CLOEXEC | DRM_RDWR };
	struct drm_prime_handle import = {}, again = {};
	struct drm_nova_gem_info info, imported;
	__u32 *map;
	int handle, other;

	other = nova_open("renderD", RENDER_NODE_FIRST);
	ASSERT_LE(0, other);

	handle = nova_gem_create(fd, 64 << 10, flags);
	ASSERT_LE(0, handle);
	ASSERT_EQ(0, nova_gem_info(fd, handle, &info));
	map = nova_gem_mmap(fd, info.mmap_offset, info.size);
	ASSERT_NE(NULL, map);
	write_pattern(map, info.size);
	ASSERT_EQ(0, munmap(map, info.size));

	export.handle = handle;
	ASSERT_EQ(0, ioctl(fd, DRM_IOCTL_PRIME_HANDLE_TO_FD, &export));
	EXPECT_EQ((off_t)info.size, lseek(export.fd, 0, SEEK_END));
	nova_gem_close(fd, handle);

	import.fd = export.fd;
	ASSERT_EQ(0, ioctl(other, DRM_IOCTL_PRIME_FD_TO_HANDLE, &import));
	again.fd = export.fd;
	ASSERT_EQ(0, ioctl(other, DRM_IOCTL_PRIME_FD_TO_HANDLE, &again));
	EXPECT_EQ(import.handle, again.handle);
	close(export.fd);

	ASSERT_EQ(0, nova_gem_info(other, import.handle, &imported));
	EXPECT_EQ(info.size, imported.size);
	EXPECT_EQ(info.flags, imported.flags);
	map = nova_gem_mmap(other, imported.mmap_offset, imported.size);
	ASSERT_NE(NULL, map);
	EXPECT_EQ(-1, check_pattern(map, imported.size));
	ASSERT_EQ(0, munmap(map, imported.size));

	nova_gem_close(other, import.handle);
	close(other);
}

TEST_F(nova, gem_share_system)
{
	gem_share(_metadata, self->fd, 0);
}

TEST_F(nova, gem_share_vram)
{
	gem_share(_metadata, self->fd, DRM_NOVA_GEM_VRAM | DRM_NOVA_GEM_CPU_VISIBLE);
}

/* Dumb buffers, which the CPU draws into for display, are on the primary node. */
TEST(dumb)
{
	struct drm_mode_create_dumb create = {
		.width = 1920,
		.height = 1080,
		.bpp = 32,
	};
	struct drm_mode_map_dumb map_dumb = {};
	struct drm_mode_destroy_dumb destroy = {};
	__u32 *map;
	int fd;

	fd = nova_open("card", PRIMARY_NODE_FIRST);
	if (fd < 0)
		SKIP(return, "no primary node driven by nova-drm");

	ASSERT_EQ(0, ioctl(fd, DRM_IOCTL_MODE_CREATE_DUMB, &create));
	EXPECT_LE(1920 * 4, create.pitch);
	EXPECT_LE((__u64)create.pitch * 1080, create.size);

	map_dumb.handle = create.handle;
	ASSERT_EQ(0, ioctl(fd, DRM_IOCTL_MODE_MAP_DUMB, &map_dumb));
	map = nova_gem_mmap(fd, map_dumb.offset, create.size);
	ASSERT_NE(NULL, map);
	EXPECT_EQ(-1, find_not(map, create.size, 0));
	write_pattern(map, create.size);
	EXPECT_EQ(-1, check_pattern(map, create.size));
	ASSERT_EQ(0, munmap(map, create.size));

	destroy.handle = create.handle;
	EXPECT_EQ(0, ioctl(fd, DRM_IOCTL_MODE_DESTROY_DUMB, &destroy));
	close(fd);
}


/* The kernel managed range of the address spaces of the tests, like NVK's on nouveau. */
#define KERNEL_START	(1ull << 39)
#define KERNEL_SIZE	(1ull << 39)

static int nova_vm_init(int fd, __u64 addr, __u64 size)
{
	struct drm_nova_vm_init init = {
		.kernel_managed_addr = addr,
		.kernel_managed_size = size,
	};

	return ioctl(fd, DRM_IOCTL_NOVA_VM_INIT, &init) ? -errno : 0;
}

static int nova_vm_bind(int fd, struct drm_nova_vm_bind_op *ops, __u32 count)
{
	struct drm_nova_vm_bind bind = {
		.op_count = count,
		.op_ptr = (__u64)(uintptr_t)ops,
	};

	return ioctl(fd, DRM_IOCTL_NOVA_VM_BIND, &bind) ? -errno : 0;
}

static int nova_vm_map(int fd, __u64 addr, __u32 handle, __u64 offset, __u64 range, __u32 kind)
{
	struct drm_nova_vm_bind_op op = {
		.op = DRM_NOVA_VM_BIND_OP_MAP,
		.handle = handle,
		.kind = kind,
		.addr = addr,
		.bo_offset = offset,
		.range = range,
	};

	return nova_vm_bind(fd, &op, 1);
}

static int nova_vm_unmap(int fd, __u64 addr, __u64 range)
{
	struct drm_nova_vm_bind_op op = {
		.op = DRM_NOVA_VM_BIND_OP_UNMAP,
		.addr = addr,
		.range = range,
	};

	return nova_vm_bind(fd, &op, 1);
}

TEST_F(nova, info_vm)
{
	struct drm_nova_info_vm vm = {};

	ASSERT_EQ(sizeof(vm), nova_info(self->fd, DRM_NOVA_INFO_VM, &vm, sizeof(vm)));
	TH_LOG("address spaces of %llu GiB", vm.va_size >> 30);
	EXPECT_LE(KERNEL_START + KERNEL_SIZE, vm.va_size);
}

TEST_F(nova, vm_create)
{
	EXPECT_EQ(-EINVAL, nova_vm_unmap(self->fd, 1 << 20, 4096));
	EXPECT_EQ(-EINVAL, nova_vm_init(self->fd, KERNEL_START, 0));
	EXPECT_EQ(-EINVAL, nova_vm_init(self->fd, KERNEL_START + 1, KERNEL_SIZE));
	EXPECT_EQ(-EINVAL, nova_vm_init(self->fd, KERNEL_START, 1ull << 60));
	ASSERT_EQ(0, nova_vm_init(self->fd, KERNEL_START, KERNEL_SIZE));
	EXPECT_EQ(-EEXIST, nova_vm_init(self->fd, KERNEL_START, KERNEL_SIZE));
}

TEST_F(nova, vm_mappings)
{
	int vram, system;

	ASSERT_EQ(0, nova_vm_init(self->fd, KERNEL_START, KERNEL_SIZE));
	vram = nova_gem_create(self->fd, 1 << 20, DRM_NOVA_GEM_VRAM);
	ASSERT_LE(0, vram);
	system = nova_gem_create(self->fd, 1 << 20, 0);
	ASSERT_LE(0, system);

	/* Both kinds of memory, block-linear and pitch-linear. */
	EXPECT_EQ(0, nova_vm_map(self->fd, 1 << 20, vram, 0, 1 << 20, 0x06));
	EXPECT_EQ(0, nova_vm_map(self->fd, 4 << 20, system, 0, 1 << 20, 0x00));

	/* Over a mapping, splitting it, then unmapping parts of mappings. */
	EXPECT_EQ(0, nova_vm_map(self->fd, (1 << 20) + (64 << 10), system, 64 << 10, 64 << 10, 0));
	EXPECT_EQ(0, nova_vm_unmap(self->fd, (1 << 20) + (512 << 10), 4096));
	EXPECT_EQ(0, nova_vm_unmap(self->fd, (4 << 20) - (64 << 10), 128 << 10));

	/* Compressible kinds are mapped uncompressed. */
	EXPECT_EQ(0, nova_vm_map(self->fd, 8 << 20, vram, 0, 1 << 20, 0x08));

	/* A part of the object, the end of the address space below the kernel's range. */
	EXPECT_EQ(0, nova_vm_map(self->fd, KERNEL_START - 4096, vram, (1 << 20) - 4096, 4096, 0));

	EXPECT_EQ(0, nova_vm_unmap(self->fd, 0, KERNEL_START));

	nova_gem_close(self->fd, vram);
	nova_gem_close(self->fd, system);
}

TEST_F(nova, vm_mappings_invalid)
{
	int bo;

	ASSERT_EQ(0, nova_vm_init(self->fd, KERNEL_START, KERNEL_SIZE));
	bo = nova_gem_create(self->fd, 1 << 20, DRM_NOVA_GEM_VRAM);
	ASSERT_LE(0, bo);

	/* In the kernel's range, and past the end of the address space. */
	EXPECT_EQ(-EINVAL, nova_vm_map(self->fd, KERNEL_START, bo, 0, 4096, 0));
	EXPECT_EQ(-EINVAL, nova_vm_map(self->fd, KERNEL_START - 4096, bo, 0, 8192, 0));
	EXPECT_EQ(-EINVAL, nova_vm_unmap(self->fd, KERNEL_START, 4096));
	EXPECT_EQ(-EINVAL, nova_vm_map(self->fd, 1ull << 41, bo, 0, 4096, 0));
	/* Unaligned, empty, past the end of the object. */
	EXPECT_EQ(-EINVAL, nova_vm_map(self->fd, (1 << 20) + 1, bo, 0, 4096, 0));
	EXPECT_EQ(-EINVAL, nova_vm_map(self->fd, 1 << 20, bo, 1, 4096, 0));
	EXPECT_EQ(-EINVAL, nova_vm_map(self->fd, 1 << 20, bo, 0, 0, 0));
	EXPECT_EQ(-EINVAL, nova_vm_map(self->fd, 1 << 20, bo, 4096, 1 << 20, 0));
	/* Not a kind, and not an object. */
	EXPECT_EQ(-EINVAL, nova_vm_map(self->fd, 1 << 20, bo, 0, 4096, 0x07));
	EXPECT_EQ(-EINVAL, nova_vm_map(self->fd, 1 << 20, bo, 0, 4096, 0x100));
	EXPECT_EQ(-ENOENT, nova_vm_map(self->fd, 1 << 20, bo + 1000, 0, 4096, 0));

	nova_gem_close(self->fd, bo);
}

/* A mapping keeps its object, and closing the file unmaps everything. */
TEST_F(nova, vm_keeps_objects)
{
	int bo;

	ASSERT_EQ(0, nova_vm_init(self->fd, KERNEL_START, KERNEL_SIZE));
	bo = nova_gem_create(self->fd, 64 << 20, DRM_NOVA_GEM_VRAM);
	ASSERT_LE(0, bo);
	for (int i = 0; i < 64; i++)
		ASSERT_EQ(0, nova_vm_map(self->fd, (__u64)(i + 1) << 30, bo, 0, 64 << 20, 0));

	nova_gem_close(self->fd, bo);
	EXPECT_EQ(0, nova_vm_unmap(self->fd, 1ull << 30, 32ull << 30));
}

/*
 * Push buffer methods, from NVIDIA's class headers: clc56f.h (host), clc7b5.h
 * (copy), clc797.h (3D) and cla140.h (inline-to-memory), as Mesa ships them.
 */
#define NVC56F_SET_OBJECT			0x0000
#define NVC56F_SEM_ADDR_LO			0x005c
#define NVC7B5_LAUNCH_DMA			0x0300
#define NVC7B5_OFFSET_OUT_UPPER			0x0408
#define NVC7B5_PITCH_IN				0x0410
#define NVC7B5_SET_REMAP_CONST_A		0x0700
#define NVC797_SET_REPORT_SEMAPHORE_A		0x1b00
#define NVA140_LINE_LENGTH_IN			0x0180
#define NVA140_LAUNCH_DMA			0x01b0
#define NVA140_LOAD_INLINE_DATA			0x01b4

/*
 * NVC56F_SEM_EXECUTE: wait for a 32-bit semaphore to hold the payload, letting
 * other channels run meanwhile.
 */
#define SEM_ACQUIRE		(0x0 | 1 << 12 | 0 << 24)
/* NVC7B5_SET_REMAP_COMPONENTS: constant A as a single 4-byte component. */
#define REMAP_FILL_32		(0x4 | 0x3 << 16 | 0x0 << 24)
/*
 * NVC7B5_LAUNCH_DMA: a non-pipelined, flushed fill of pitch-linear lines with
 * the remapped constant, without PLC.
 */
#define LAUNCH_FILL		(0x2 | 1 << 2 | 1 << 7 | 1 << 8 | 1 << 9 | 1 << 10 | 1 << 26)
/*
 * NVC797_SET_REPORT_SEMAPHORE_D: release a one-word payload, once all the
 * pipeline is done.
 */
#define REPORT_RELEASE		(0x0 | 0xf << 12 | 1 << 28)
/* NVA140_LAUNCH_DMA: a flushed write of pitch-linear lines. */
#define LAUNCH_INLINE		(0x1 | 0x1 << 4)

/* Header of @count methods of @subc from @method on. */
static __u32 mthd(__u32 subc, __u32 method, __u32 count)
{
	return 1u << 29 | count << 16 | subc << 13 | method >> 2;
}

/* Header of @count values for the method @method of @subc. */
static __u32 mthd_noninc(__u32 subc, __u32 method, __u32 count)
{
	return 3u << 29 | count << 16 | subc << 13 | method >> 2;
}

/* Subchannels, as NVK binds them. */
#define SUBC_3D		0
#define SUBC_COMPUTE	1
#define SUBC_M2MF	2
#define SUBC_2D		3
#define SUBC_COPY	4

/* Addresses the tests map at. */
#define PUSH_VA		(1ull << 30)
#define DST_VA		(2ull << 30)

#define ALL_CLASSES	(DRM_NOVA_CLASS_3D | DRM_NOVA_CLASS_COMPUTE | DRM_NOVA_CLASS_2D | \
			 DRM_NOVA_CLASS_M2MF)

static int nova_channel_create_classes(int fd, __u32 engine, __u32 classes)
{
	struct drm_nova_channel_create create = { .engine = engine, .classes = classes };

	if (ioctl(fd, DRM_IOCTL_NOVA_CHANNEL_CREATE, &create))
		return -errno;

	return create.channel;
}

/* A channel with all the classes of its engine. */
static int nova_channel_create(int fd, __u32 engine)
{
	return nova_channel_create_classes(fd, engine,
					   engine == DRM_NOVA_ENGINE_GRAPHICS ? ALL_CLASSES : 0);
}

static int nova_channel_destroy(int fd, __u32 channel)
{
	struct drm_nova_channel_destroy destroy = { .channel = channel };

	return ioctl(fd, DRM_IOCTL_NOVA_CHANNEL_DESTROY, &destroy) ? -errno : 0;
}

static int nova_exec_sync(int fd, __u32 channel, struct drm_nova_exec_push *pushes,
			  __u32 count, struct drm_nova_sync *waits, __u32 wait_count,
			  struct drm_nova_sync *sigs, __u32 sig_count)
{
	struct drm_nova_exec exec = {
		.channel = channel,
		.push_count = count,
		.wait_count = wait_count,
		.sig_count = sig_count,
		.wait_ptr = (__u64)(uintptr_t)waits,
		.sig_ptr = (__u64)(uintptr_t)sigs,
		.push_ptr = (__u64)(uintptr_t)pushes,
	};

	return ioctl(fd, DRM_IOCTL_NOVA_EXEC, &exec) ? -errno : 0;
}

/* Submits work, without waiting for it. */
static int nova_exec(int fd, __u32 channel, struct drm_nova_exec_push *pushes, __u32 count)
{
	return nova_exec_sync(fd, channel, pushes, count, NULL, 0, NULL, 0);
}

static int syncobj_create(int fd)
{
	struct drm_syncobj_create create = {};

	if (ioctl(fd, DRM_IOCTL_SYNCOBJ_CREATE, &create))
		return -errno;

	return create.handle;
}

static void syncobj_destroy(int fd, __u32 handle)
{
	struct drm_syncobj_destroy destroy = { .handle = handle };

	ioctl(fd, DRM_IOCTL_SYNCOBJ_DESTROY, &destroy);
}

/* The absolute CLOCK_MONOTONIC time @ms milliseconds from now, in ns. */
static __s64 deadline_ms(__s64 ms)
{
	struct timespec now;

	clock_gettime(CLOCK_MONOTONIC, &now);
	return (__s64)now.tv_sec * 1000000000 + now.tv_nsec + ms * 1000000;
}

/* Waits for point @point of @handle, or for its fence if 0, for at most @ms ms. */
static int syncobj_wait(int fd, __u32 handle, __u64 point, __s64 ms)
{
	struct drm_syncobj_timeline_wait wait = {
		.handles = (__u64)(uintptr_t)&handle,
		.points = (__u64)(uintptr_t)&point,
		.timeout_nsec = deadline_ms(ms),
		.count_handles = 1,
	};

	return ioctl(fd, DRM_IOCTL_SYNCOBJ_TIMELINE_WAIT, &wait) ? -errno : 0;
}

/* Submits work and waits for it to be done. */
static int nova_exec_wait(int fd, __u32 channel, struct drm_nova_exec_push *pushes,
			  __u32 count)
{
	struct drm_nova_sync sig = { .flags = DRM_NOVA_SYNC_SYNCOBJ };
	int handle, ret;

	handle = syncobj_create(fd);
	if (handle < 0)
		return handle;
	sig.handle = handle;

	ret = nova_exec_sync(fd, channel, pushes, count, NULL, 0, &sig, 1);
	if (!ret)
		ret = syncobj_wait(fd, handle, 0, 5000);
	syncobj_destroy(fd, handle);

	return ret;
}

/* An object mapped for the CPU and at @va for the GPU. */
struct mapped_bo {
	int handle;
	__u32 *map;
	__u64 size;
};

static void mapped_bo_create(struct __test_metadata *_metadata, int fd, struct mapped_bo *bo,
			     __u64 size, __u32 flags, __u64 va)
{
	struct drm_nova_gem_info info;

	bo->handle = nova_gem_create(fd, size, flags | DRM_NOVA_GEM_CPU_VISIBLE);
	ASSERT_LE(0, bo->handle);
	ASSERT_EQ(0, nova_gem_info(fd, bo->handle, &info));
	bo->size = info.size;
	bo->map = nova_gem_mmap(fd, info.mmap_offset, info.size);
	ASSERT_NE(NULL, bo->map);
	ASSERT_EQ(0, nova_vm_map(fd, va, bo->handle, 0, info.size, 0));
}

static void mapped_bo_destroy(int fd, struct mapped_bo *bo)
{
	munmap(bo->map, bo->size);
	nova_gem_close(fd, bo->handle);
}

static struct drm_nova_info_graphics graphics_info(int fd)
{
	struct drm_nova_info_graphics gr = {};

	nova_info(fd, DRM_NOVA_INFO_GRAPHICS, &gr, sizeof(gr));
	return gr;
}

/* Writes the methods of a copy engine fill of @size bytes at @va with @value. */
static __u32 push_copy_fill(__u32 *push, __u32 class_copy, __u64 va, __u32 size, __u32 value)
{
	__u32 n = 0;
	__u32 line = 4096;

	push[n++] = mthd(SUBC_COPY, NVC56F_SET_OBJECT, 1);
	push[n++] = class_copy;
	push[n++] = mthd(SUBC_COPY, NVC7B5_SET_REMAP_CONST_A, 3);
	push[n++] = value;
	push[n++] = 0;
	push[n++] = REMAP_FILL_32;
	push[n++] = mthd(SUBC_COPY, NVC7B5_OFFSET_OUT_UPPER, 2);
	push[n++] = va >> 32;
	push[n++] = va;
	/* Pitches in and out, line length in 4-byte components, line count. */
	push[n++] = mthd(SUBC_COPY, NVC7B5_PITCH_IN, 4);
	push[n++] = line;
	push[n++] = line;
	push[n++] = line / 4;
	push[n++] = size / line;
	push[n++] = mthd(SUBC_COPY, NVC7B5_LAUNCH_DMA, 1);
	push[n++] = LAUNCH_FILL;

	return n;
}

TEST_F(nova, info_exec)
{
	struct drm_nova_info_exec exec = {};

	ASSERT_EQ(sizeof(exec), nova_info(self->fd, DRM_NOVA_INFO_EXEC, &exec, sizeof(exec)));
	TH_LOG("at most %u push buffers per submission", exec.push_max);
	EXPECT_LE(1, exec.push_max);
}

/* The copy engine fills memory at the addresses userspace mapped it at. */
TEST_F(nova, exec_copy)
{
	struct drm_nova_info_graphics gr = graphics_info(self->fd);
	struct mapped_bo push, system, vram;
	struct drm_nova_exec_push exec[2];
	__u32 words;
	int channel;

	ASSERT_EQ(0, nova_vm_init(self->fd, KERNEL_START, KERNEL_SIZE));
	mapped_bo_create(_metadata, self->fd, &push, 4096, 0, PUSH_VA);
	mapped_bo_create(_metadata, self->fd, &system, 1 << 20, 0, DST_VA);
	mapped_bo_create(_metadata, self->fd, &vram, 4 << 20, DRM_NOVA_GEM_VRAM, DST_VA + (1 << 30));
	channel = nova_channel_create(self->fd, DRM_NOVA_ENGINE_COPY);
	ASSERT_LE(0, channel);

	/* Two push buffers: one filling system memory, the other VRAM. */
	words = push_copy_fill(push.map, gr.class_copy, DST_VA, system.size, 0xc0ffee00);
	exec[0] = (struct drm_nova_exec_push){ .va = PUSH_VA, .range = words * 4 };
	exec[1].va = PUSH_VA + words * 4;
	exec[1].range = 4 * push_copy_fill(push.map + words, gr.class_copy, DST_VA + (1 << 30),
					   vram.size, 0x5eed5eed);
	exec[1].flags = DRM_NOVA_EXEC_PUSH_NO_PREFETCH;
	ASSERT_EQ(0, nova_exec_wait(self->fd, channel, exec, 2));

	EXPECT_EQ(-1, find_not(system.map, system.size, 0xc0ffee00));
	EXPECT_EQ(-1, find_not(vram.map, vram.size, 0x5eed5eed));

	EXPECT_EQ(0, nova_channel_destroy(self->fd, channel));
	mapped_bo_destroy(self->fd, &push);
	mapped_bo_destroy(self->fd, &system);
	mapped_bo_destroy(self->fd, &vram);
}

/*
 * A graphics channel has the objects userspace binds: the 3D engine and the
 * inline-to-memory engine write data from the push buffer to memory.
 */
TEST_F(nova, exec_graphics)
{
	static const __u32 data[] = { 0x6e6f7661, 0x2e2e2e21, 0x12345678, 0xfeedface };
	struct drm_nova_info_graphics gr = graphics_info(self->fd);
	struct drm_nova_exec_push exec;
	struct mapped_bo push, dst;
	__u32 *p;
	int channel;

	ASSERT_EQ(0, nova_vm_init(self->fd, KERNEL_START, KERNEL_SIZE));
	mapped_bo_create(_metadata, self->fd, &push, 4096, 0, PUSH_VA);
	mapped_bo_create(_metadata, self->fd, &dst, 64 << 10, DRM_NOVA_GEM_VRAM, DST_VA);
	channel = nova_channel_create(self->fd, DRM_NOVA_ENGINE_GRAPHICS);
	ASSERT_LE(0, channel);

	p = push.map;
	/* Each object the channel has, bound like NVK binds them. */
	*p++ = mthd(SUBC_3D, NVC56F_SET_OBJECT, 1);
	*p++ = gr.class_3d;
	*p++ = mthd(SUBC_COMPUTE, NVC56F_SET_OBJECT, 1);
	*p++ = gr.class_compute;
	*p++ = mthd(SUBC_M2MF, NVC56F_SET_OBJECT, 1);
	*p++ = gr.class_m2mf;
	*p++ = mthd(SUBC_2D, NVC56F_SET_OBJECT, 1);
	*p++ = gr.class_2d;
	/* The 3D pipeline writes each word once the work before is done. */
	for (int i = 0; i < 4; i++) {
		__u64 va = DST_VA + i * 4;

		*p++ = mthd(SUBC_3D, NVC797_SET_REPORT_SEMAPHORE_A, 4);
		*p++ = va >> 32;
		*p++ = va;
		*p++ = data[i];
		*p++ = REPORT_RELEASE;
	}
	/* The inline-to-memory engine writes the same words further. */
	*p++ = mthd(SUBC_M2MF, NVA140_LINE_LENGTH_IN, 4);
	*p++ = sizeof(data);
	*p++ = 1;
	*p++ = (DST_VA + 64) >> 32;
	*p++ = DST_VA + 64;
	*p++ = mthd(SUBC_M2MF, NVA140_LAUNCH_DMA, 1);
	*p++ = LAUNCH_INLINE;
	*p++ = mthd_noninc(SUBC_M2MF, NVA140_LOAD_INLINE_DATA, 4);
	for (int i = 0; i < 4; i++)
		*p++ = data[i];

	exec = (struct drm_nova_exec_push){
		.va = PUSH_VA,
		.range = (p - push.map) * 4,
	};
	ASSERT_EQ(0, nova_exec_wait(self->fd, channel, &exec, 1));
	for (int i = 0; i < 4; i++)
		EXPECT_EQ(data[i], dst.map[i]);
	EXPECT_EQ(0, dst.map[4]);
	for (int i = 0; i < 4; i++)
		EXPECT_EQ(data[i], dst.map[16 + i]);
	EXPECT_EQ(0, dst.map[20]);

	EXPECT_EQ(0, nova_channel_destroy(self->fd, channel));
	mapped_bo_destroy(self->fd, &push);
	mapped_bo_destroy(self->fd, &dst);
}

/* Submissions in a row on a channel, timed. */
TEST_F(nova, exec_latency)
{
	struct drm_nova_info_graphics gr = graphics_info(self->fd);
	struct drm_nova_exec_push exec;
	struct mapped_bo push, dst;
	struct timespec start, end;
	const int runs = 200;
	int channel;

	ASSERT_EQ(0, nova_vm_init(self->fd, KERNEL_START, KERNEL_SIZE));
	mapped_bo_create(_metadata, self->fd, &push, 4096, 0, PUSH_VA);
	mapped_bo_create(_metadata, self->fd, &dst, 4096, 0, DST_VA);
	channel = nova_channel_create(self->fd, DRM_NOVA_ENGINE_COPY);
	ASSERT_LE(0, channel);

	clock_gettime(CLOCK_MONOTONIC, &start);
	for (int i = 0; i < runs; i++) {
		exec = (struct drm_nova_exec_push){
			.va = PUSH_VA,
			.range = 4 * push_copy_fill(push.map, gr.class_copy, DST_VA, 4096, i),
		};
		ASSERT_EQ(0, nova_exec_wait(self->fd, channel, &exec, 1));
		ASSERT_EQ(i, dst.map[1023]);
	}
	clock_gettime(CLOCK_MONOTONIC, &end);
	TH_LOG("%d submissions, %ld us each", runs,
	       ((end.tv_sec - start.tv_sec) * 1000000 + (end.tv_nsec - start.tv_nsec) / 1000) / runs);

	EXPECT_EQ(0, nova_channel_destroy(self->fd, channel));
	mapped_bo_destroy(self->fd, &push);
	mapped_bo_destroy(self->fd, &dst);
}

/* EXEC returns once the work is submitted, before it is done. */
TEST_F(nova, exec_async)
{
	struct drm_nova_info_graphics gr = graphics_info(self->fd);
	struct drm_nova_sync sig = { .flags = DRM_NOVA_SYNC_SYNCOBJ };
	const __u64 size = 512ull << 20;
	struct drm_nova_exec_push exec;
	struct mapped_bo push;
	int big, channel;

	ASSERT_EQ(0, nova_vm_init(self->fd, KERNEL_START, KERNEL_SIZE));
	mapped_bo_create(_metadata, self->fd, &push, 4096, 0, PUSH_VA);
	big = nova_gem_create(self->fd, size, DRM_NOVA_GEM_VRAM);
	ASSERT_LE(0, big);
	ASSERT_EQ(0, nova_vm_map(self->fd, DST_VA, big, 0, size, 0));
	channel = nova_channel_create(self->fd, DRM_NOVA_ENGINE_COPY);
	ASSERT_LE(0, channel);
	sig.handle = syncobj_create(self->fd);
	ASSERT_LT(0, sig.handle);

	/* A fill of 512 MiB takes a millisecond or more. */
	exec = (struct drm_nova_exec_push){
		.va = PUSH_VA,
		.range = 4 * push_copy_fill(push.map, gr.class_copy, DST_VA, size, 0x5a5a5a5a),
	};
	ASSERT_EQ(0, nova_exec_sync(self->fd, channel, &exec, 1, NULL, 0, &sig, 1));
	EXPECT_EQ(-ETIME, syncobj_wait(self->fd, sig.handle, 0, 0));
	EXPECT_EQ(0, syncobj_wait(self->fd, sig.handle, 0, 5000));

	syncobj_destroy(self->fd, sig.handle);
	EXPECT_EQ(0, nova_channel_destroy(self->fd, channel));
	mapped_bo_destroy(self->fd, &push);
	nova_gem_close(self->fd, big);
}

/*
 * Work waits for the synchronization objects it is given: a fill of a buffer
 * on one channel, then a fill of its start on another, which would be
 * overwritten if it ran first.
 */
TEST_F(nova, exec_wait_other_channel)
{
	struct drm_nova_info_graphics gr = graphics_info(self->fd);
	struct drm_nova_sync first = { .flags = DRM_NOVA_SYNC_SYNCOBJ };
	struct drm_nova_sync second = { .flags = DRM_NOVA_SYNC_SYNCOBJ };
	struct drm_nova_exec_push exec;
	struct mapped_bo push, dst;
	int a, b;

	ASSERT_EQ(0, nova_vm_init(self->fd, KERNEL_START, KERNEL_SIZE));
	mapped_bo_create(_metadata, self->fd, &push, 4096, 0, PUSH_VA);
	mapped_bo_create(_metadata, self->fd, &dst, 4 << 20, 0, DST_VA);
	a = nova_channel_create(self->fd, DRM_NOVA_ENGINE_COPY);
	b = nova_channel_create(self->fd, DRM_NOVA_ENGINE_COPY);
	ASSERT_LE(0, a);
	ASSERT_LE(0, b);
	first.handle = syncobj_create(self->fd);
	second.handle = syncobj_create(self->fd);
	ASSERT_LT(0, first.handle);
	ASSERT_LT(0, second.handle);

	exec = (struct drm_nova_exec_push){
		.va = PUSH_VA,
		.range = 4 * push_copy_fill(push.map, gr.class_copy, DST_VA, dst.size, 0x11111111),
	};
	ASSERT_EQ(0, nova_exec_sync(self->fd, a, &exec, 1, NULL, 0, &first, 1));
	exec = (struct drm_nova_exec_push){
		.va = PUSH_VA + 1024,
		.range = 4 * push_copy_fill(push.map + 256, gr.class_copy, DST_VA, 4096, 0x22222222),
	};
	ASSERT_EQ(0, nova_exec_sync(self->fd, b, &exec, 1, &first, 1, &second, 1));
	ASSERT_EQ(0, syncobj_wait(self->fd, second.handle, 0, 5000));

	EXPECT_EQ(-1, find_not(dst.map, 4096, 0x22222222));
	EXPECT_EQ(-1, find_not(dst.map + 1024, dst.size - 4096, 0x11111111));

	syncobj_destroy(self->fd, first.handle);
	syncobj_destroy(self->fd, second.handle);
	EXPECT_EQ(0, nova_channel_destroy(self->fd, a));
	EXPECT_EQ(0, nova_channel_destroy(self->fd, b));
	mapped_bo_destroy(self->fd, &push);
	mapped_bo_destroy(self->fd, &dst);
}

/* Work signals points of timelines, and waits for them. */
TEST_F(nova, exec_timeline)
{
	struct drm_nova_info_graphics gr = graphics_info(self->fd);
	struct drm_nova_sync point[2] = {
		{ .flags = DRM_NOVA_SYNC_TIMELINE_SYNCOBJ, .timeline_value = 1 },
		{ .flags = DRM_NOVA_SYNC_TIMELINE_SYNCOBJ, .timeline_value = 2 },
	};
	struct drm_nova_sync done = { .flags = DRM_NOVA_SYNC_SYNCOBJ };
	struct drm_nova_exec_push exec;
	struct mapped_bo push, dst;
	int a, b, timeline;

	ASSERT_EQ(0, nova_vm_init(self->fd, KERNEL_START, KERNEL_SIZE));
	mapped_bo_create(_metadata, self->fd, &push, 4096, 0, PUSH_VA);
	mapped_bo_create(_metadata, self->fd, &dst, 12288, 0, DST_VA);
	a = nova_channel_create(self->fd, DRM_NOVA_ENGINE_COPY);
	b = nova_channel_create(self->fd, DRM_NOVA_ENGINE_COPY);
	ASSERT_LE(0, a);
	ASSERT_LE(0, b);
	timeline = syncobj_create(self->fd);
	done.handle = syncobj_create(self->fd);
	ASSERT_LT(0, timeline);
	ASSERT_LT(0, done.handle);
	point[0].handle = timeline;
	point[1].handle = timeline;

	/* Points 1 and 2 on one channel, then work on another waiting for point 2. */
	for (int i = 0; i < 2; i++) {
		exec = (struct drm_nova_exec_push){
			.va = PUSH_VA + i * 256,
			.range = 4 * push_copy_fill(push.map + i * 64, gr.class_copy,
						    DST_VA + i * 4096, 4096, i + 1),
		};
		ASSERT_EQ(0, nova_exec_sync(self->fd, a, &exec, 1, NULL, 0, &point[i], 1));
	}
	exec = (struct drm_nova_exec_push){
		.va = PUSH_VA + 512,
		.range = 4 * push_copy_fill(push.map + 128, gr.class_copy, DST_VA + 8192, 4096, 3),
	};
	ASSERT_EQ(0, nova_exec_sync(self->fd, b, &exec, 1, &point[1], 1, &done, 1));

	ASSERT_EQ(0, syncobj_wait(self->fd, done.handle, 0, 5000));
	EXPECT_EQ(0, syncobj_wait(self->fd, timeline, 1, 0));
	EXPECT_EQ(0, syncobj_wait(self->fd, timeline, 2, 0));
	for (int i = 0; i < 3; i++)
		EXPECT_EQ(-1, find_not(dst.map + i * 1024, 4096, i + 1));

	syncobj_destroy(self->fd, timeline);
	syncobj_destroy(self->fd, done.handle);
	EXPECT_EQ(0, nova_channel_destroy(self->fd, a));
	EXPECT_EQ(0, nova_channel_destroy(self->fd, b));
	mapped_bo_destroy(self->fd, &push);
	mapped_bo_destroy(self->fd, &dst);
}

/* Invalid synchronization objects fail EXEC before anything runs. */
TEST_F(nova, exec_sync_invalid)
{
	struct drm_nova_info_graphics gr = graphics_info(self->fd);
	struct drm_nova_sync sync = { .flags = DRM_NOVA_SYNC_SYNCOBJ };
	struct drm_nova_exec_push exec;
	struct mapped_bo push, dst;
	int channel, empty;

	ASSERT_EQ(0, nova_vm_init(self->fd, KERNEL_START, KERNEL_SIZE));
	mapped_bo_create(_metadata, self->fd, &push, 4096, 0, PUSH_VA);
	mapped_bo_create(_metadata, self->fd, &dst, 4096, 0, DST_VA);
	channel = nova_channel_create(self->fd, DRM_NOVA_ENGINE_COPY);
	ASSERT_LE(0, channel);
	empty = syncobj_create(self->fd);
	ASSERT_LT(0, empty);
	exec = (struct drm_nova_exec_push){
		.va = PUSH_VA,
		.range = 4 * push_copy_fill(push.map, gr.class_copy, DST_VA, 4096, 0x0bad0bad),
	};

	/* No such object, to wait for or to signal. */
	sync.handle = empty + 100;
	EXPECT_EQ(-ENOENT, nova_exec_sync(self->fd, channel, &exec, 1, &sync, 1, NULL, 0));
	EXPECT_EQ(-ENOENT, nova_exec_sync(self->fd, channel, &exec, 1, NULL, 0, &sync, 1));
	/* An object without a fence to wait for. */
	sync.handle = empty;
	EXPECT_EQ(-EINVAL, nova_exec_sync(self->fd, channel, &exec, 1, &sync, 1, NULL, 0));
	/* A binary object with a point, a point 0, unknown flags. */
	sync.timeline_value = 1;
	EXPECT_EQ(-EINVAL, nova_exec_sync(self->fd, channel, &exec, 1, NULL, 0, &sync, 1));
	sync = (struct drm_nova_sync){ .flags = DRM_NOVA_SYNC_TIMELINE_SYNCOBJ, .handle = empty };
	EXPECT_EQ(-EINVAL, nova_exec_sync(self->fd, channel, &exec, 1, NULL, 0, &sync, 1));
	sync = (struct drm_nova_sync){ .flags = 1 << 8, .handle = empty };
	EXPECT_EQ(-EINVAL, nova_exec_sync(self->fd, channel, &exec, 1, NULL, 0, &sync, 1));

	/* None of these ran: the channel's first work does. */
	exec = (struct drm_nova_exec_push){
		.va = PUSH_VA,
		.range = 4 * push_copy_fill(push.map, gr.class_copy, DST_VA, 4096, 0x600d600d),
	};
	ASSERT_EQ(0, nova_exec_wait(self->fd, channel, &exec, 1));
	EXPECT_EQ(-1, find_not(dst.map, 4096, 0x600d600d));

	syncobj_destroy(self->fd, empty);
	EXPECT_EQ(0, nova_channel_destroy(self->fd, channel));
	mapped_bo_destroy(self->fd, &push);
	mapped_bo_destroy(self->fd, &dst);
}

/*
 * Submissions queue up without waiting, more than the ring and the kernel's
 * push buffer hold at once, and run in order.
 */
TEST_F(nova, exec_queue)
{
	struct drm_nova_info_graphics gr = graphics_info(self->fd);
	struct drm_nova_exec_push exec;
	struct mapped_bo push, dst;
	struct timespec start, end;
	const int runs = 1000;
	__u32 words = 0;
	int channel;

	ASSERT_EQ(0, nova_vm_init(self->fd, KERNEL_START, KERNEL_SIZE));
	mapped_bo_create(_metadata, self->fd, &push, 256 << 10, 0, PUSH_VA);
	mapped_bo_create(_metadata, self->fd, &dst, 4096, 0, DST_VA);
	channel = nova_channel_create(self->fd, DRM_NOVA_ENGINE_COPY);
	ASSERT_LE(0, channel);

	clock_gettime(CLOCK_MONOTONIC, &start);
	for (int i = 0; i < runs; i++) {
		__u32 n = push_copy_fill(push.map + words, gr.class_copy, DST_VA, 4096, i);

		exec = (struct drm_nova_exec_push){ .va = PUSH_VA + words * 4, .range = n * 4 };
		words += n;
		if (i < runs - 1) {
			ASSERT_EQ(0, nova_exec(self->fd, channel, &exec, 1));
		} else {
			ASSERT_EQ(0, nova_exec_wait(self->fd, channel, &exec, 1));
		}
	}
	clock_gettime(CLOCK_MONOTONIC, &end);
	TH_LOG("%d queued submissions, %ld us each", runs,
	       ((end.tv_sec - start.tv_sec) * 1000000 + (end.tv_nsec - start.tv_nsec) / 1000) / runs);
	EXPECT_EQ(-1, find_not(dst.map, 4096, runs - 1));

	EXPECT_EQ(0, nova_channel_destroy(self->fd, channel));
	mapped_bo_destroy(self->fd, &push);
	mapped_bo_destroy(self->fd, &dst);
}

/* The GPU's timer counts nanoseconds. */
TEST_F(nova, info_timestamp)
{
	struct drm_nova_info_timestamp before = {}, after = {};
	struct timespec pause = { .tv_nsec = 10 * 1000 * 1000 };
	__u64 ns;

	ASSERT_EQ(sizeof(before), nova_info(self->fd, DRM_NOVA_INFO_TIMESTAMP, &before,
					    sizeof(before)));
	nanosleep(&pause, NULL);
	ASSERT_EQ(sizeof(after), nova_info(self->fd, DRM_NOVA_INFO_TIMESTAMP, &after,
					   sizeof(after)));

	ns = after.timestamp - before.timestamp;
	TH_LOG("GPU timer advanced %llu ns over a 10 ms sleep", (unsigned long long)ns);
	EXPECT_LE(10ull * 1000 * 1000, ns);
	EXPECT_GT(1000ull * 1000 * 1000, ns);
}

/* EXEC without push buffers signals once the work submitted before is done. */
TEST_F(nova, exec_empty)
{
	struct drm_nova_info_graphics gr = graphics_info(self->fd);
	struct drm_nova_sync sig = { .flags = DRM_NOVA_SYNC_SYNCOBJ };
	const __u64 size = 512ull << 20;
	struct drm_nova_exec_push exec;
	struct mapped_bo push;
	int big, channel;

	ASSERT_EQ(0, nova_vm_init(self->fd, KERNEL_START, KERNEL_SIZE));
	mapped_bo_create(_metadata, self->fd, &push, 4096, 0, PUSH_VA);
	big = nova_gem_create(self->fd, size, DRM_NOVA_GEM_VRAM);
	ASSERT_LE(0, big);
	ASSERT_EQ(0, nova_vm_map(self->fd, DST_VA, big, 0, size, 0));
	channel = nova_channel_create(self->fd, DRM_NOVA_ENGINE_COPY);
	ASSERT_LE(0, channel);
	sig.handle = syncobj_create(self->fd);
	ASSERT_LT(0, sig.handle);

	/* With nothing submitted before, it signals right away. */
	ASSERT_EQ(0, nova_exec_sync(self->fd, channel, NULL, 0, NULL, 0, &sig, 1));
	EXPECT_EQ(0, syncobj_wait(self->fd, sig.handle, 0, 0));

	/* After a fill of 512 MiB, once it is done. */
	exec = (struct drm_nova_exec_push){
		.va = PUSH_VA,
		.range = 4 * push_copy_fill(push.map, gr.class_copy, DST_VA, size, 0x5a5a5a5a),
	};
	ASSERT_EQ(0, nova_exec(self->fd, channel, &exec, 1));
	ASSERT_EQ(0, nova_exec_sync(self->fd, channel, NULL, 0, NULL, 0, &sig, 1));
	EXPECT_EQ(-ETIME, syncobj_wait(self->fd, sig.handle, 0, 0));
	EXPECT_EQ(0, syncobj_wait(self->fd, sig.handle, 0, 5000));

	syncobj_destroy(self->fd, sig.handle);
	EXPECT_EQ(0, nova_channel_destroy(self->fd, channel));
	mapped_bo_destroy(self->fd, &push);
	nova_gem_close(self->fd, big);
}

/*
 * Work that makes no progress fails: its fences signal once the watchdog
 * gives up on it, and the channel runs no more work. Other channels still do.
 */
TEST_F(nova, exec_hang)
{
	struct drm_nova_info_graphics gr = graphics_info(self->fd);
	struct drm_nova_sync sig = { .flags = DRM_NOVA_SYNC_SYNCOBJ };
	struct drm_nova_exec_push exec;
	struct mapped_bo push, sem;
	struct timespec start, end;
	int channel, other;
	long ms;
	__u32 *p;

	ASSERT_EQ(0, nova_vm_init(self->fd, KERNEL_START, KERNEL_SIZE));
	mapped_bo_create(_metadata, self->fd, &push, 4096, 0, PUSH_VA);
	mapped_bo_create(_metadata, self->fd, &sem, 4096, 0, DST_VA);
	channel = nova_channel_create(self->fd, DRM_NOVA_ENGINE_COPY);
	ASSERT_LE(0, channel);
	sig.handle = syncobj_create(self->fd);
	ASSERT_LT(0, sig.handle);

	/* Wait for the semaphore to hold 1: it holds 0, and nothing releases it. */
	p = push.map;
	*p++ = mthd(0, NVC56F_SEM_ADDR_LO, 5);
	*p++ = (__u32)DST_VA;
	*p++ = DST_VA >> 32;
	*p++ = 1;
	*p++ = 0;
	*p++ = SEM_ACQUIRE;
	exec = (struct drm_nova_exec_push){ .va = PUSH_VA, .range = (p - push.map) * 4 };

	clock_gettime(CLOCK_MONOTONIC, &start);
	ASSERT_EQ(0, nova_exec_sync(self->fd, channel, &exec, 1, NULL, 0, &sig, 1));
	ASSERT_EQ(0, syncobj_wait(self->fd, sig.handle, 0, 20000));
	clock_gettime(CLOCK_MONOTONIC, &end);
	ms = (end.tv_sec - start.tv_sec) * 1000 + (end.tv_nsec - start.tv_nsec) / 1000000;
	TH_LOG("hung work failed after %ld ms", ms);
	EXPECT_LE(4000, ms);
	EXPECT_EQ(-ENODEV, nova_exec(self->fd, channel, &exec, 1));
	EXPECT_EQ(-ENODEV, nova_exec(self->fd, channel, NULL, 0));
	EXPECT_EQ(0, nova_channel_destroy(self->fd, channel));

	other = nova_channel_create(self->fd, DRM_NOVA_ENGINE_COPY);
	ASSERT_LE(0, other);
	exec = (struct drm_nova_exec_push){
		.va = PUSH_VA,
		.range = 4 * push_copy_fill(push.map, gr.class_copy, DST_VA, 4096, 0xa11ae),
	};
	ASSERT_EQ(0, nova_exec_wait(self->fd, other, &exec, 1));
	EXPECT_EQ(-1, find_not(sem.map, 4096, 0xa11ae));

	syncobj_destroy(self->fd, sig.handle);
	EXPECT_EQ(0, nova_channel_destroy(self->fd, other));
	mapped_bo_destroy(self->fd, &push);
	mapped_bo_destroy(self->fd, &sem);
}

TEST_F(nova, exec_invalid)
{
	struct drm_nova_exec_push exec = { .va = PUSH_VA, .range = 16 };
	int channel;

	/* Channels need the address space. */
	EXPECT_EQ(-EINVAL, nova_channel_create(self->fd, DRM_NOVA_ENGINE_COPY));
	ASSERT_EQ(0, nova_vm_init(self->fd, KERNEL_START, KERNEL_SIZE));
	EXPECT_EQ(-EINVAL, nova_channel_create(self->fd, 0));
	EXPECT_EQ(-EINVAL, nova_channel_create(self->fd, 3));
	/* Graphics channels have objects, copy channels their copy object. */
	EXPECT_EQ(-EINVAL, nova_channel_create_classes(self->fd, DRM_NOVA_ENGINE_GRAPHICS, 0));
	EXPECT_EQ(-EINVAL, nova_channel_create_classes(self->fd, DRM_NOVA_ENGINE_GRAPHICS, 1 << 7));
	EXPECT_EQ(-EINVAL, nova_channel_create_classes(self->fd, DRM_NOVA_ENGINE_COPY,
						       DRM_NOVA_CLASS_3D));
	channel = nova_channel_create(self->fd, DRM_NOVA_ENGINE_COPY);
	ASSERT_LE(0, channel);

	EXPECT_EQ(-ENOENT, nova_exec(self->fd, channel + 1, &exec, 1));
	/* Without push buffers, nothing to wait for or signal. */
	EXPECT_EQ(0, nova_exec(self->fd, channel, &exec, 0));
	EXPECT_EQ(-EINVAL, nova_exec(self->fd, channel, &exec, 100000));
	exec.range = 6;
	EXPECT_EQ(-EINVAL, nova_exec(self->fd, channel, &exec, 1));
	exec.range = 16;
	exec.flags = 1 << 7;
	EXPECT_EQ(-EINVAL, nova_exec(self->fd, channel, &exec, 1));

	EXPECT_EQ(0, nova_channel_destroy(self->fd, channel));
	EXPECT_EQ(-ENOENT, nova_channel_destroy(self->fd, channel));
}

/* Channels left are destroyed with the file. */
TEST_F(nova, exec_channels_left)
{
	ASSERT_EQ(0, nova_vm_init(self->fd, KERNEL_START, KERNEL_SIZE));
	for (int i = 0; i < 4; i++) {
		ASSERT_LE(0, nova_channel_create(self->fd, DRM_NOVA_ENGINE_GRAPHICS));
		ASSERT_LE(0, nova_channel_create(self->fd, DRM_NOVA_ENGINE_COPY));
	}
}

TEST_HARNESS_MAIN
