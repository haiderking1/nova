// SPDX-License-Identifier: GPL-2.0 or MIT

#ifdef CONFIG_RUST_DRM_DISPLAY_DP_HELPER

#include <drm/display/drm_dp_helper.h>

__rust_helper
int rust_helper_drm_dp_dpcd_read_data(struct drm_dp_aux *aux, unsigned int offset,
				      void *buffer, size_t size)
{
	return drm_dp_dpcd_read_data(aux, offset, buffer, size);
}

__rust_helper
int rust_helper_drm_dp_dpcd_write_data(struct drm_dp_aux *aux, unsigned int offset,
				       void *buffer, size_t size)
{
	return drm_dp_dpcd_write_data(aux, offset, buffer, size);
}

#endif
