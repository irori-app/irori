{
  description = "Irori game launcher (Linux, prebuilt binary)";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

  outputs = { nixpkgs, ... }:
    let
      version = "1.1.0";

      sources = {
        x86_64-linux = {
          suffix = "";
          hash = "sha256-sT4V8ZUA0j0/FpG7N373Uk+6YdEQaDKCp8GhPDi/JB0=";
        };
        aarch64-linux = {
          suffix = "-arm64";
          hash = "sha256-W7EBCZY1F3NVK0Qq2CCDJOipjCuUlCm88siy6n42aJE=";
        };
      };

      systems = nixpkgs.lib.attrNames sources;
      forAllSystems = f: nixpkgs.lib.genAttrs systems (system: f nixpkgs.legacyPackages.${system});

      libs = pkgs: with pkgs; [
        gtk3
        webkitgtk_4_1
        libsoup_3
        glib
        glib-networking
        libayatana-appindicator
        libxkbcommon
        wayland
        vulkan-loader
        libGL
        fontconfig
        freetype
        libx11
        libxcb
        udev
      ];
    in {
      packages = forAllSystems (pkgs: rec {
        default = irori;
        irori =
          let
            bwrapFix = pkgs.writeShellScriptBin "bwrap" ''
              args=()
              home=""
              while [ $# -gt 0 ]; do
                if [ "$1" = -- ]; then
                  cfg="$home/.config/MangoHud"
                  if [ -n "$home" ] && [ -d "$cfg" ] && [ ! -L "$cfg" ]; then
                    args+=(--ro-bind "$cfg" "$cfg")
                  fi
                  args+=("$@")
                  break
                fi
                if [ "$1" = --setenv ] && [ "$3" = --setenv ]; then
                  args+=("$1" "$2" "")
                  shift 2
                  continue
                fi
                if [ "$1" = --setenv ] && [ "$2" = HOME ]; then
                  home="$3"
                fi
                args+=("$1")
                shift
                if [ "''${args[-1]}" = --tmpfs ] && [ "$1" = /run ]; then
                  args+=("$1" --ro-bind-try /run/current-system /run/current-system)
                  shift
                fi
              done
              exec ${pkgs.bubblewrap}/bin/bwrap "''${args[@]}"
            '';
          in
          pkgs.stdenv.mkDerivation {
            pname = "irori";
            inherit version;

            src = pkgs.fetchurl (
              let source = sources.${pkgs.stdenv.hostPlatform.system};
              in {
                url = "https://github.com/irori-app/irori/releases/download/v${version}/Irori-${version}-linux${source.suffix}.tar.gz";
                inherit (source) hash;
              }
            );

            sourceRoot = "Irori";

            nativeBuildInputs = with pkgs; [ autoPatchelfHook wrapGAppsHook3 ];

            buildInputs = libs pkgs ++ [ pkgs.stdenv.cc.cc.lib ] ++ (with pkgs.gst_all_1; [
              gstreamer
              gst-plugins-base
              gst-plugins-good
              gst-plugins-bad
              gst-libav
            ]);

            runtimeDependencies = map pkgs.lib.getLib (libs pkgs);

            dontConfigure = true;
            dontBuild = true;

            installPhase = ''
              runHook preInstall

              install -Dm755 irori irori-sign-in -t $out/bin
              install -Dm644 irori.desktop -t $out/share/applications
              install -Dm644 irori.svg -t $out/share/icons/hicolor/scalable/apps
              install -Dm644 app.irori.Irori.metainfo.xml -t $out/share/metainfo
              install -Dm644 LICENSE -t $out/share/licenses/irori

              runHook postInstall
            '';

            preFixup = ''
              gappsWrapperArgs+=(
                --prefix PATH : ${pkgs.lib.makeBinPath [ pkgs.umu-launcher bwrapFix ]}
                --prefix LD_LIBRARY_PATH : ${pkgs.lib.makeLibraryPath [ pkgs.vulkan-loader ]}
              )
            '';

            doInstallCheck = true;
            installCheckPhase = ''
              runHook preInstallCheck
              HOME=$(mktemp -d) $out/bin/irori --version | grep -F "${version}"
              runHook postInstallCheck
            '';

            meta = {
              description = "A native game launcher that gathers your game libraries";
              homepage = "https://irori.app";
              license = pkgs.lib.licenses.mit;
              sourceProvenance = [ pkgs.lib.sourceTypes.binaryNativeCode ];
              platforms = systems;
              mainProgram = "irori";
            };
          };
      });
    };
}
