// SPDX-License-Identifier: GPL-2.0

#include <linux/screen_info.h>

__rust_helper unsigned int rust_helper_screen_info_video_type(const struct screen_info *si)
{
	return screen_info_video_type(si);
}

__rust_helper u64 rust_helper___screen_info_lfb_base(const struct screen_info *si)
{
	return __screen_info_lfb_base(si);
}

__rust_helper u64 rust_helper___screen_info_lfb_size(const struct screen_info *si,
						     unsigned int type)
{
	return __screen_info_lfb_size(si, type);
}
