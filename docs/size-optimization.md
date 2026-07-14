# 构建体积优化记录

本文记录 `codex/size-optimization` 独立实验分支的构建体积优化。优化尚未合并到 `main`，以下数据不代表主分支当前产物。

## 实施范围

- Release profile 使用 `opt-level = "z"`、完整 LTO、单 codegen unit、`panic = "abort"` 和符号剥离。
- Tokio 改用 current-thread runtime，仅保留 `macros`、`rt` 和 `time`。
- reqwest 使用 `rustls-no-provider`，由项目显式安装 `ring` provider；CI 阻止 `aws-lc-rs` 和 `aws-lc-sys` 回归。
- 昵称查询改为异步请求并复用统一 HTTP 客户端，移除 reqwest blocking client。
- 日志只保留全局级别过滤，移除 `env-filter` 及其正则依赖。
- 移除未使用的 `anyhow` 和 `secrecy`。
- 提交 `Cargo.lock`，CI、Release 和 Docker 均使用锁定依赖。

## 远程实测

所有数据均来自 GitHub Actions Summary 中的文件字节数，不是 Actions 外层 Artifact 大小。

| 阶段 | GitHub Actions | Windows EXE | Windows ZIP | Linux 程序 | Linux tar.gz |
|---|---|---:|---:|---:|---:|
| Release profile + Tokio | [29343540132](https://github.com/bcmdy/mihoyo-bbs-tools-rs/actions/runs/29343540132) | 3,731,968 | 1,951,124 | 5,533,688 | 2,348,653 |
| 切换到 ring | [29345265583](https://github.com/bcmdy/mihoyo-bbs-tools-rs/actions/runs/29345265583) | 3,015,168 | 1,571,992 | 3,547,696 | 1,719,709 |
| 异步昵称 + 日志依赖精简 | [29346204731](https://github.com/bcmdy/mihoyo-bbs-tools-rs/actions/runs/29346204731) | 2,677,760 | 1,406,803 | 3,148,752 | 1,538,781 |

从第一阶段到最终阶段：

- Windows EXE 减少 1,054,208 字节，约 28.2%。
- Windows ZIP 减少 544,321 字节，约 27.9%。
- Linux 程序减少 2,384,936 字节，约 43.1%。
- Linux tar.gz 减少 809,872 字节，约 34.5%。

优化前 `ci-45` 的 Windows 3,716,703 字节和 Linux 3,877,371 字节是 Actions 外层 Artifact 大小，统计口径不同，不能与上表的程序或内层发布包直接比较。

## 验证结果

- 最终精简阶段的格式、Clippy、单元测试、Ubuntu Release 和 Windows Release 均通过：[Rust CI 29346204731](https://github.com/bcmdy/mihoyo-bbs-tools-rs/actions/runs/29346204731)。
- `ring`、平台证书验证器和生产 HTTPS 证书已通过无凭据的米游社公开端点测试：[TLS 真实握手测试 29346657444](https://github.com/bcmdy/mihoyo-bbs-tools-rs/actions/runs/29346657444)。
- 加入默认忽略的 TLS smoke 测试后，分支最终普通 CI 再次全部通过：[Rust CI 29346657535](https://github.com/bcmdy/mihoyo-bbs-tools-rs/actions/runs/29346657535)。
- `tests/tls_smoke.rs` 默认忽略，普通离线测试不会访问生产网络；需要复核时由远程 CI 显式执行 ignored test。

## 取舍

- 完整 LTO 和单 codegen unit 可能延长 Release 编译时间。
- `opt-level = "z"` 优先体积，CPU 密集型代码的执行速度可能低于默认 Release 优化。
- `panic = "abort"` 发生 panic 时直接终止进程，不执行栈展开。
- `RUST_LOG` 仍支持 `trace`、`debug`、`info`、`warn`、`error` 等全局级别，但不再支持模块级复杂过滤表达式。
- 当前未采用 UPX。它能进一步压缩发布文件，但会增加杀毒软件误报概率和启动时解压成本，不适合作为默认分发方案。
