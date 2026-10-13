# nova, soka's fork

An experimental fork of **nova**, the Rust kernel driver for NVIDIA GPUs
(`nova-core` + `nova-drm`, from
[drm-rust-next](https://gitlab.freedesktop.org/drm/rust/kernel)), extended
until it runs a full desktop: Hyprland, browsers and games, through Mesa's
NVK (Vulkan) and Zink (OpenGL), with no `nvidia.ko` and no nouveau.

**This is not upstream nova and not supported.** Upstream nova doesn't do
display or rendering yet; everything here beyond it is my own work in
progress, tested on one machine. The uAPI (`include/uapi/drm/nova_drm.h`) is
this fork's own and will change.

## What it adds

On top of nova-core's GSP boot (GSP-RM firmware r570.144):

- **Display**: modesetting through GSP-RM and the display engine, including
  refresh rate changes, DisplayPort link training, atomic KMS (built on
  Lyude Paul's Rust KMS bindings), scanout of clients' buffers and an fbdev
  console.
- **Channels**: GPU channels on the copy engines, the graphics/compute engine
  (golden context, context promotion) and the video decoders (NVDEC), with
  per-client address spaces (GPUVM), fences and a hang watchdog.
- **Memory**: VRAM and system memory GEM objects, mapped into each client's
  GPU address space.
- **A DRM uAPI** for userspace drivers: the matching NVK backend is in
  [mesa-nova](https://github.com/haiderking1/mesa-nova).
- Fixes found along the way, e.g. GSP command queue elements not zero-padded
  (sent upstream to the nova-gpu list).

## Tested on

- RTX 3070 (GA104), passed through to a NixOS VM (QEMU Q35).
- Kernel 7.3.0-rc1 (drm-rust-next), Rust 1.98.
- Only GA10x with GSP-RM r570.144 is tested. Other generations may not work.

## Building

`flake.nix` has a dev shell and the kernel package, and `nix/kernel.config`
is the config I run. Outside Nix, build it like any kernel with
`CONFIG_NOVA_CORE=m` and `CONFIG_DRM_NOVA=m`. The GSP firmware comes from
linux-firmware (`nvidia/ga104/gsp/*-570.144.bin`), not from this repo.

Userspace needs:

- [mesa-nova](https://github.com/haiderking1/mesa-nova): NVK with the nova
  backend, Zink, and Vulkan Video VP9/AV1 decode (`NVK_EXPERIMENTAL=video`).
- [libdrm-nova](https://github.com/haiderking1/libdrm-nova): libdrm
  describing auxiliary-bus DRM devices (nova-drm) by their PCI parent,
  without which NVK finds no GPU.

## Credits

nova is the work of its upstream developers (Danilo Krummrich, Alexandre
Courbot and many others). Commits taken from others, such as Lyude Paul's KMS
bindings and Eliot Courtney's nova-core changes, keep their authors. Much of
this fork was written with the help of an AI assistant (Claude), which the
commits say.

## License

GPL-2.0, like the kernel. See `COPYING`.
