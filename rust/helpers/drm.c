// SPDX-License-Identifier: GPL-2.0

#include <drm/drm_atomic.h>
#include <drm/drm_crtc.h>
#include <drm/drm_gem_atomic_helper.h>
#include <drm/drm_gem.h>
#include <drm/drm_gem_shmem_helper.h>
#include <drm/drm_managed.h>
#include <drm/drm_modeset_helper_vtables.h>
#include <drm/drm_rect.h>
#include <drm/drm_vma_manager.h>

#ifdef CONFIG_DRM

__rust_helper void rust_helper_drm_gem_object_get(struct drm_gem_object *obj)
{
	drm_gem_object_get(obj);
}

__rust_helper void rust_helper_drm_gem_object_put(struct drm_gem_object *obj)
{
	drm_gem_object_put(obj);
}

__rust_helper __u64
rust_helper_drm_vma_node_offset_addr(struct drm_vma_offset_node *node)
{
	return drm_vma_node_offset_addr(node);
}

#ifdef CONFIG_DRM_GEM_SHMEM_HELPER
__rust_helper void
rust_helper_drm_gem_shmem_object_free(struct drm_gem_object *obj)
{
	return drm_gem_shmem_object_free(obj);
}

__rust_helper void
rust_helper_drm_gem_shmem_object_print_info(struct drm_printer *p, unsigned int indent,
					    const struct drm_gem_object *obj)
{
	drm_gem_shmem_object_print_info(p, indent, obj);
}

__rust_helper int
rust_helper_drm_gem_shmem_object_pin(struct drm_gem_object *obj)
{
	return drm_gem_shmem_object_pin(obj);
}

__rust_helper void
rust_helper_drm_gem_shmem_object_unpin(struct drm_gem_object *obj)
{
	drm_gem_shmem_object_unpin(obj);
}

__rust_helper struct sg_table *
rust_helper_drm_gem_shmem_object_get_sg_table(struct drm_gem_object *obj)
{
	return drm_gem_shmem_object_get_sg_table(obj);
}

__rust_helper int
rust_helper_drm_gem_shmem_object_vmap(struct drm_gem_object *obj,
				      struct iosys_map *map)
{
	return drm_gem_shmem_object_vmap(obj, map);
}

__rust_helper void
rust_helper_drm_gem_shmem_object_vunmap(struct drm_gem_object *obj,
					struct iosys_map *map)
{
	drm_gem_shmem_object_vunmap(obj, map);
}

__rust_helper int
rust_helper_drm_gem_shmem_object_mmap(struct drm_gem_object *obj, struct vm_area_struct *vma)
{
	return drm_gem_shmem_object_mmap(obj, vma);
}

#endif /* CONFIG_DRM_GEM_SHMEM_HELPER */

/* Kernel mode setting (KMS) */

__rust_helper void *rust_helper_drmm_kzalloc(struct drm_device *dev, size_t size, gfp_t gfp)
{
	return drmm_kzalloc(dev, size, gfp);
}

__rust_helper struct drm_plane_state *
rust_helper_drm_atomic_get_new_plane_state(const struct drm_atomic_commit *state,
					   struct drm_plane *plane)
{
	return drm_atomic_get_new_plane_state(state, plane);
}

__rust_helper struct drm_plane_state *
rust_helper_drm_atomic_get_old_plane_state(const struct drm_atomic_commit *state,
					   struct drm_plane *plane)
{
	return drm_atomic_get_old_plane_state(state, plane);
}

__rust_helper struct drm_crtc_state *
rust_helper_drm_atomic_get_new_crtc_state(const struct drm_atomic_commit *state,
					  struct drm_crtc *crtc)
{
	return drm_atomic_get_new_crtc_state(state, crtc);
}

__rust_helper struct drm_crtc_state *
rust_helper_drm_atomic_get_old_crtc_state(const struct drm_atomic_commit *state,
					  struct drm_crtc *crtc)
{
	return drm_atomic_get_old_crtc_state(state, crtc);
}

__rust_helper struct drm_connector_state *
rust_helper_drm_atomic_get_new_connector_state(const struct drm_atomic_commit *state,
					       struct drm_connector *connector)
{
	return drm_atomic_get_new_connector_state(state, connector);
}

__rust_helper u32 rust_helper_drm_crtc_mask(const struct drm_crtc *crtc)
{
	return drm_crtc_mask(crtc);
}

__rust_helper void rust_helper_drm_plane_helper_add(struct drm_plane *plane,
						    const struct drm_plane_helper_funcs *funcs)
{
	drm_plane_helper_add(plane, funcs);
}

__rust_helper void rust_helper_drm_crtc_helper_add(struct drm_crtc *crtc,
						   const struct drm_crtc_helper_funcs *funcs)
{
	drm_crtc_helper_add(crtc, funcs);
}

__rust_helper void
rust_helper_drm_encoder_helper_add(struct drm_encoder *encoder,
				   const struct drm_encoder_helper_funcs *funcs)
{
	drm_encoder_helper_add(encoder, funcs);
}

__rust_helper void
rust_helper_drm_connector_helper_add(struct drm_connector *connector,
				     const struct drm_connector_helper_funcs *funcs)
{
	drm_connector_helper_add(connector, funcs);
}

__rust_helper struct drm_shadow_plane_state *
rust_helper_to_drm_shadow_plane_state(struct drm_plane_state *state)
{
	return to_drm_shadow_plane_state(state);
}

__rust_helper int rust_helper_drm_rect_width(const struct drm_rect *r)
{
	return drm_rect_width(r);
}

__rust_helper int rust_helper_drm_rect_height(const struct drm_rect *r)
{
	return drm_rect_height(r);
}

__rust_helper void rust_helper_drm_rect_init(struct drm_rect *r, int x, int y, int width,
					     int height)
{
	drm_rect_init(r, x, y, width, height);
}

__rust_helper void rust_helper_iosys_map_set_vaddr(struct iosys_map *map, void *vaddr)
{
	iosys_map_set_vaddr(map, vaddr);
}

__rust_helper void rust_helper_iosys_map_set_vaddr_iomem(struct iosys_map *map,
							 void __iomem *vaddr_iomem)
{
	iosys_map_set_vaddr_iomem(map, vaddr_iomem);
}

__rust_helper void rust_helper_iosys_map_incr(struct iosys_map *map, size_t incr)
{
	iosys_map_incr(map, incr);
}
#endif /* CONFIG_DRM */
