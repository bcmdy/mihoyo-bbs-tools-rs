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

            # buildRustPackage 的 doCheck 默认为 true，会在 Nix 构建阶段执行
            # cargo test。本仓库的单元测试用 wiremock 起本地 HTTP 服务并写临时
            # 目录，Nix sandbox 内没有网络能力（含受限的 loopback），测试必然
            # 失败；而 nix 包只需验证“能编译 + 安装文件齐全”，单元测试由 Rust
            # CI 的 quality 任务覆盖，因此这里关闭 check。
            doCheck = false;

            postInstall = ''
              install -Dm644 docs/使用说明.md \
                "$out/share/doc/mihoyo-bbs-tools/使用说明.md"
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
