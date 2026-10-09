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

TEST_HARNESS_MAIN
