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
        kernel = pkgs.buildLinux {
          src = self;
          inherit version;
          modDirVersion = version;
          # rc kernels have options nixpkgs' common config doesn't know yet
          ignoreConfigErrors = true;

          structuredExtraConfig = with lib.kernel; {
            RUST = yes;
            DRM = yes; # DRM_NOVA requires built-in DRM
            # nixpkgs' common config forces these off (WIP), so override it
            NOVA_CORE = lib.mkForce module;
            NOVA_CORE_SELFTESTS = yes;
            DRM_NOVA = lib.mkForce module;

            # Keep nouveau off the card so nova gets it
            DRM_NOUVEAU = no;

            # Firmware (UEFI GOP) framebuffer, so the console shows something
            # until nova can drive the display itself
            DRM_SIMPLEDRM = yes;
            SYSFB_SIMPLEFB = yes;
          };
        };
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
          echo "nova dev shell: kernel ${version}, rustc $(rustc --version | cut -d' ' -f2)"
        '';
      };
    };
}
