{
  description = "nova - soka's experiments with the Rust NVIDIA driver";

  # Locked to the same nixpkgs rev as mother-base, so the dev shell and the
  # boot kernel use the exact same rustc/gcc and modules stay compatible.
  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

  outputs = { self, nixpkgs }:
    let
      system = "x86_64-linux";
      pkgs = nixpkgs.legacyPackages.${system};
      lib = nixpkgs.lib;

      # Read the version from the Makefile, e.g. "7.3.0-rc1"
      makefile = builtins.readFile ./Makefile;
      makeVar = name: builtins.head (builtins.match ".*\n${name} = ([^\n]*)\n.*" makefile);
      version = "${makeVar "VERSION"}.${makeVar "PATCHLEVEL"}.${makeVar "SUBLEVEL"}${makeVar "EXTRAVERSION"}";
    in
    {
      # Full NixOS-compatible kernel built from this tree, with nova enabled.
      # mother-base boots it as the "nova" specialisation (second boot entry).
      packages.${system} = rec {
        # nix/kernel.config is nixpkgs' full NixOS config plus:
        #   RUST=y, DRM=y (nova-drm needs it built in), NOVA_CORE=m,
        #   NOVA_CORE_SELFTESTS=y, DRM_NOVA=m, DRM_NOUVEAU=n (keep it off the
        #   card), DRM_SIMPLEDRM=y + SYSFB_SIMPLEFB=y (UEFI framebuffer console),
        #   RUST_DEBUG_ASSERTIONS=y (Rust dev_dbg!/pr_debug! only print with it)
        # then trimmed with `make localmodconfig` to the modules this VM loads
        # (lsmod, the nova entry's initrd modules, nova, fuse/overlay/tun/loop):
        # ~200 modules instead of ~7300, so a full build takes minutes.
        kernel = pkgs.linuxKernel.manualConfig {
          # Our nix files aren't kernel source; editing them shouldn't rebuild it
          src = lib.fileset.toSource {
            root = ./.;
            fileset = lib.fileset.difference ./. (lib.fileset.unions [ ./flake.nix ./flake.lock ./nix ]);
          };
          inherit version;
          modDirVersion = version;
          configfile = ./nix/kernel.config;
          # What buildLinux declares by default; NixOS asserts on these.
          # All are enabled in kernel.config (EFI_STUB, IA32_EMULATION,
          # IP{,6}_NF_MATCH_RPFILTER).
          features = {
            efiBootStub = true;
            ia32Emulation = true;
            netfilterRPFilter = true;
          };
        };
        # GSP firmware in the TLV layout nova-core loads, repackaged from
        # linux-firmware. The version must match nova's GSP bindings
        # (drivers/gpu/nova-core/gsp/fw/r570_144).
        firmware = pkgs.runCommand "nova-firmware-570.144" {
          nativeBuildInputs = [ pkgs.python3 ];
        } ''
          python3 -I ${./nix/mktlv.py} ${pkgs.linux-firmware}/lib/firmware $out/lib/firmware 570.144
        '';

        default = kernel;
      };

      devShells.${system}.default = pkgs.mkShell {
        # Same toolchain nixpkgs' kernel builder uses
        packages = with pkgs; [
          gnumake
          gcc
          flex
          bison
          bc
          perl
          python3
          openssl
          elfutils
          zlib # resolve_btfids (BTF)
          ncurses # menuconfig
          pkg-config
          cpio
          kmod
          pahole

          rustc-unwrapped
          rust-bindgen-unwrapped
          rustfmt
          clippy
          rust-analyzer
        ];

        hardeningDisable = [ "all" ]; # nix's hardening flags break kernel builds
        RUST_LIB_SRC = pkgs.rustPlatform.rustLibSrc;
        LIBCLANG_PATH = "${pkgs.llvmPackages.libclang.lib}/lib";

        shellHook = ''
          export PATH="$(git rev-parse --show-toplevel)/nix:$PATH" # nova-dev
          echo "nova dev shell: kernel ${version}, rustc $(rustc --version | cut -d' ' -f2)"
        '';
      };
    };
}
