// SPDX-License-Identifier: GPL-2.0

#include <drm/drm_syncobj.h>

__rust_helper void rust_helper_drm_syncobj_get(struct drm_syncobj *obj)
{
	drm_syncobj_get(obj);
}

__rust_helper void rust_helper_drm_syncobj_put(struct drm_syncobj *obj)
{
	drm_syncobj_put(obj);
}
