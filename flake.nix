{
  description = "MihoyoBBSTools RS";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

  outputs =
    { self, nixpkgs }:
    let
      systems = [
        "x86_64-linux"
        "aarch64-linux"
      ];
      forAllSystems = nixpkgs.lib.genAttrs systems;
    in
    {
      packages = forAllSystems (
        system:
        let
          pkgs = import nixpkgs { inherit system; };
          manifest = builtins.fromTOML (builtins.readFile ./Cargo.toml);
        in
        {
          default = pkgs.rustPlatform.buildRustPackage {
            pname = "mihoyo-bbs-tools";
            version = manifest.package.version;
            src = pkgs.lib.cleanSource ./.;

            cargoLock.lockFile = ./Cargo.lock;
            nativeBuildInputs = [ pkgs.makeWrapper ];
            MIHOYO_BBS_TOOLS_VERSION = manifest.package.version;

            postInstall = ''
              install -Dm644 docs/使用说明.md \
                "$out/share/doc/mihoyo-bbs-tools/使用说明.md"
              install -Dm644 docs/快速开始.md \
                "$out/share/doc/mihoyo-bbs-tools/快速开始.md"
              install -Dm644 docs/configuration.md \
                "$out/share/doc/mihoyo-bbs-tools/configuration.md"
              install -Dm644 docs/security.md \
                "$out/share/doc/mihoyo-bbs-tools/security.md"
              install -Dm644 config/config.example.yaml \
                "$out/share/mihoyo-bbs-tools/config/config.example.yaml"
              install -Dm644 integrations/dacapo/template.yml \
                "$out/share/mihoyo-bbs-tools/dacapo/template.yml"
              wrapProgram "$out/bin/MihoyoBBSToolsRS" \
                --set-default SSL_CERT_FILE "${pkgs.cacert}/etc/ssl/certs/ca-bundle.crt"
            '';

            meta = {
              description = manifest.package.description;
              homepage = manifest.package.repository;
              license = pkgs.lib.licenses.mit;
              mainProgram = "MihoyoBBSToolsRS";
              platforms = pkgs.lib.platforms.linux;
            };
          };
        }
      );

      apps = forAllSystems (system: {
        default = {
          type = "app";
          program = "${self.packages.${system}.default}/bin/MihoyoBBSToolsRS";
        };
      });
    };
}
