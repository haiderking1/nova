/* SPDX-License-Identifier: MIT */

#ifndef DRM_FBDEV_SHADOW_H
#define DRM_FBDEV_SHADOW_H

#include <linux/stddef.h>

struct drm_fb_helper;
struct drm_fb_helper_surface_size;

/*
 * fbdev emulation for drivers whose buffer objects the CPU maps with
 * &drm_gem_object_funcs.vmap: fbdev draws into a shadow buffer in system
 * memory, whose damage is blitted into the framebuffer's buffer object.
 */
#ifdef CONFIG_DRM_FBDEV_EMULATION
int drm_fbdev_shadow_driver_fbdev_probe(struct drm_fb_helper *fb_helper,
					struct drm_fb_helper_surface_size *sizes);

#define DRM_FBDEV_SHADOW_DRIVER_OPS \
	.fbdev_probe = drm_fbdev_shadow_driver_fbdev_probe
#else
#define DRM_FBDEV_SHADOW_DRIVER_OPS \
	.fbdev_probe = NULL
#endif

#endif
