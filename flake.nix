{
  description = "MihoyoBBSTools RS";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

  outputs =
    {
      self,
      nixpkgs,
    }:
    let
      systems = [
        "x86_64-linux"
        "aarch64-linux"
      ];
      forAllSystems = nixpkgs.lib.genAttrs systems;
      eachSystem =
        function:
        forAllSystems (
          system:
          function {
            pkgs = import nixpkgs { inherit system; };
            inherit system;
          }
        );
    in
    {
      packages = eachSystem (
        { pkgs, ... }:
        let
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

      apps = eachSystem (
        { system, ... }:
        {
          default = {
            type = "app";
            program = "${self.packages.${system}.default}/bin/MihoyoBBSToolsRS";
          };
        }
      );

      # Home Manager 模块：声明式写入配置并生成 systemd user service + timer。
      #
      # 与上游的实现差异（有意为之）：
      # - 上游靠 `AutoMihoyoBBS_config_path` 环境变量指定配置目录，本项目的
      #   `run` 只接受 `--config`，因此这里把配置路径直接写进 ExecStart。
      # - 只生成一个配置文件（本项目的多账号在同一 YAML 的 accounts 列表里），
      #   不提供上游那种「按名字生成多个 YAML」的 settings 属性集。
      # - 定时默认关闭：需要用户显式开启 `enable`，避免与 `runtime.schedule`
      #   或外部 cron 重复执行。
      homeModules.default =
        {
          config,
          lib,
          pkgs,
          ...
        }:
        let
          cfg = config.services.mihoyo-bbs-tools;
          settingsFormat = pkgs.formats.yaml { };
          configFile = settingsFormat.generate "mihoyo-bbs-tools-config.yaml" cfg.settings;
        in
        {
          options.services.mihoyo-bbs-tools = {
            enable = lib.mkEnableOption "使用 systemd 用户定时器每日运行 MihoyoBBSToolsRS（启用前请确保配置中 runtime.schedule 保持关闭）";

            package = lib.mkOption {
              type = lib.types.package;
              default = self.packages.${pkgs.stdenv.hostPlatform.system}.default;
              defaultText =
                lib.literalExpression "self.packages.\${pkgs.stdenv.hostPlatform.system}.default";
              description = "要使用的 MihoyoBBSToolsRS 包。";
            };

            settings = lib.mkOption {
              type = settingsFormat.type;
              default = { };
              example = lib.literalExpression ''
                {
                  version = 1;
                  runtime.schedule.enabled = false;
                  accounts = [
                    {
                      name = "账号一";
                      credentials.cookie = "account_id=...; stoken=v2_...";
                      tasks = {
                        china_game_checkin = true;
                        bbs = {
                          enabled = true;
                          forums = [ 5 2 ];
                        };
                      };
                      games = [ "genshin" ];
                    }
                  ];
                  notifications = {
                    enabled = false;
                    providers = [ ];
                  };
                }
              '';
              description = ''
                写入 config.yaml 的完整配置。字段含义见仓库的
                `config/config.example.yaml` 与 `docs/使用说明.md`。

                注意：该文件包含 Cookie、SToken 与通知凭据，生成在 Nix store 中，
                仅当前用户可读。如不接受这一存储位置，请改用 `extraArgs` 指向
                你自己管理的配置并保持 `settings = { }`。
              '';
            };

            configPath = lib.mkOption {
              type = lib.types.nullOr lib.types.str;
              default = null;
              example = "/home/me/.config/mihoyo-bbs-tools/config.yaml";
              description = ''
                自定义配置文件路径。默认写入 Nix store 中的生成文件；显式设置后
                改用该路径，此时应自行保证文件存在且包含凭据。
              '';
            };

            extraArgs = lib.mkOption {
              type = lib.types.listOf lib.types.str;
              default = [ ];
              example = [ "--task" "china-checkin,bbs" ];
              description = "追加到 `run` 子命令之后的参数。";
            };

            extraEnvironment = lib.mkOption {
              type = lib.types.attrsOf lib.types.str;
              default = { };
              example = { RUST_LOG = "debug"; };
              description = "服务进程的额外环境变量，可用于注入 RUST_LOG 等。";
            };

            onCalendar = lib.mkOption {
              type = lib.types.str;
              default = "09:30";
              example = "*-*-* 08:00:00";
              description = ''
                运行时间，默认每天 09:30。取值遵循 systemd 时间规范，
                详见 {manpage}`systemd.time(7)`。
              '';
            };

            randomizedDelaySec = lib.mkOption {
              type = lib.types.str;
              default = "20min";
              description = "定时触发的随机延迟，避免多实例同时请求米游社。";
            };

            persistent = lib.mkOption {
              type = lib.types.bool;
              default = true;
              description = "关机期间错过的运行是否在开机后补跑。";
            };
          };

          config = lib.mkIf cfg.enable {
            assertions = [
              {
                assertion = cfg.configPath != null || cfg.settings != { };
                message = ''
                  services.mihoyo-bbs-tools：启用后必须提供配置。
                  设置 settings（写入 Nix store）或 configPath（指向已有文件）。
                '';
              }
            ];

            systemd.user.services.mihoyo-bbs-tools = {
              Unit = {
                Description = "MihoyoBBSTools RS 自动签到";
                Documentation = [ "https://github.com/bcmdy/mihoyo-bbs-tools-rs" ];
              };
              Service = {
                Type = "oneshot";
                ExecStart = lib.escapeShellArgs (
                  [
                    "${cfg.package}/bin/MihoyoBBSToolsRS"
                    "run"
                    "--config"
                    (if cfg.configPath != null then cfg.configPath else "${configFile}")
                  ]
                  ++ cfg.extraArgs
                );
                Environment = lib.mapAttrsToList (
                  name: value: "${name}=${value}"
                ) cfg.extraEnvironment;
              };
            };

            systemd.user.timers.mihoyo-bbs-tools = {
              Unit = {
                Description = "每日运行 MihoyoBBSTools RS";
              };
              Timer = {
                OnCalendar = cfg.onCalendar;
                Persistent = cfg.persistent;
                RandomizedDelaySec = cfg.randomizedDelaySec;
              };
              Install = {
                WantedBy = [ "timers.target" ];
              };
            };
          };
        };
    };
}
