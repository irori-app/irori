{
  description = "Irori game launcher (Linux, prebuilt binary)";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

  outputs = { nixpkgs, ... }:
    let
      version = "1.0.0";

      sources = {
        x86_64-linux = {
          suffix = "";
          hash = "sha256-inEUFcnlQoCujNPYIBT+2JYJqxAtKKHb8C2S7SXgaLw=";
        };
        aarch64-linux = {
          suffix = "-arm64";
          hash = "sha256-w7xXXIWGBLuwQneWBpVEGPH9A30cpXbcghxoOVuLEI0=";
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
        irori = pkgs.stdenv.mkDerivation {
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
              --prefix PATH : ${pkgs.lib.makeBinPath [ pkgs.umu-launcher ]}
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
