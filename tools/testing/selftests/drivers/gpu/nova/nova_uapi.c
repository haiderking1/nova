// SPDX-License-Identifier: GPL-2.0
/*
 * Tests of nova's uAPI, on the first render node driven by nova-drm.
 */

#include <errno.h>
#include <fcntl.h>
#include <stdio.h>
#include <string.h>
#include <sys/ioctl.h>
#include <unistd.h>

#include <drm/drm.h>
#include <drm/nova_drm.h>

#include "../../../kselftest_harness.h"

/* Render nodes are numbered from 128, at most 64 of them. */
#define RENDER_NODE_FIRST	128
#define RENDER_NODE_COUNT	64

/* Opens the first render node nova-drm drives, or returns -1. */
static int nova_open(void)
{
	for (int i = 0; i < RENDER_NODE_COUNT; i++) {
		char path[32], name[32] = "";
		struct drm_version version = {
			.name = name,
			.name_len = sizeof(name) - 1,
		};
		int fd;

		snprintf(path, sizeof(path), "/dev/dri/renderD%d", RENDER_NODE_FIRST + i);
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
	self->fd = nova_open();
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

TEST_HARNESS_MAIN
