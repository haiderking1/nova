/* SPDX-License-Identifier: MIT */

#ifndef DRM_FBDEV_TTM_H
#define DRM_FBDEV_TTM_H

#include <drm/drm_fbdev_shadow.h>

/* TTM drivers use the generic shadow-buffered fbdev emulation. */
#define drm_fbdev_ttm_driver_fbdev_probe drm_fbdev_shadow_driver_fbdev_probe

#define DRM_FBDEV_TTM_DRIVER_OPS DRM_FBDEV_SHADOW_DRIVER_OPS

#endif
