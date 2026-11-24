# dpp

Minecraft Java Edition 数据包（datapack）包管理器。只管理原版数据包：不管理 mod、不管理资源包、不注入游戏、不执行包内脚本。

> 状态：Rust workspace 骨架已建立；核心包管理功能尚未实现。`docs/` 是当前设计、激活语义和可行性结论；实现以这些文档为准。
> 状态：Rust workspace 已实现 `new` / `init` / `add` / `remove` / `install` / `update` / `apply` / `status` / `check` / `search` / `inspect` / `build-pack` / `host` / `snapshot` / `config` / `adapter`。`docs/` 是当前设计、激活语义和可行性结论；实现以这些文档为准。

## 解决的问题

- 手动下载/解压，依赖靠人肉维护。
- 数据包版本与 `pack_format` 适配混乱。
- 加载顺序踩 `level.dat` / 字典序的坑。
- 更新把 `datapacks/` 写半残，缺少回滚。
- 分享一套数据包环境无法复现。

## 核心原则

1. **纯原版**：生成的世界不需要任何加载器或 mod。
2. **期望状态**：`dpp.toml` + `lock.toml` 记录精确版本、sha512、依赖和来源；可复现。
3. **包层与激活分离**：`install/update/remove` 只改包层；`apply` 负责 `/reload`、重启或重进世界的指引。
4. **原子优先**：staging + journal + rename；失败回滚，不把 `datapacks/` 写半残。
5. **诚实的状态**：`status` 只报告 desired/staged/committed/applied/observed，不假装能读取 MC 内存注册表。
6. **顺序是 advisory**：MVP 不写 `level.dat`，不自动重排；`dpp order` 只报告并打印原生命令。

## 文档

| 文档 | 内容 |
|---|---|
| [docs/README.md](docs/README.md) | 文档索引与阅读顺序 |
| [docs/design.md](docs/design.md) | 三层模型、状态机、文件布局、源与求解、命令 |
| [docs/apply.md](docs/apply.md) | `/reload` 边界、apply 决策、RCON/MSMP、卸载清理 |
| [docs/load-order.md](docs/load-order.md) | 加载顺序权威与 MVP 取舍 |
| [docs/feasibility.md](docs/feasibility.md) | Modrinth / pack format / pacman 调研、风险与 MVP |
| [docs/research-toolchain.md](docs/research-toolchain.md) | 现代开发工具链、lock/cache/供应链/Rust 工具调研 |
| [docs/research-datapack.md](docs/research-datapack.md) | MC 数据包生态、Smithed、beet/Spyglass/GameTest |

## 当前实现

Rust workspace 骨架已建立；目前可运行：

- `dpp --version`
- `dpp new <name> [--dir <PATH>] [--mc-version <VERSION>] [--pack-format <FORMAT>]`
- `dpp init [--mc-version <VERSION>] [--pack-format <FORMAT>]`
- `dpp status`
- `dpp order [--json] [--print-commands]`
- `dpp check`
- `dpp search <query> [--mc-version <VERSION>] [--limit <N>]`
- `dpp build-pack [PATH] [--output <ZIP>] [--url <URL>] [--json]`
- `dpp inspect <zip> --pack-format <FORMAT>`

`init` 解析 `level.dat` 的 `Data.Version` / `DataVersion`，自动识别 MC 版本与 data pack format；`--mc-version` / `--pack-format` 可覆盖。快照版本若找不到本地 client/server jar，会要求显式指定 format。`status` 显示检测结果；`check` 校验 manifest / world / lock 一致性；`inspect` 校验本地 zip 根目录的 `pack.mcmeta` 与目标格式。

`dpp order` 在疑似关闭的世界里读取 `level.dat` 的 `DataPacks.Enabled`，按 `(load_weight, id, install_name)` 报告期望顺序；`session.lock` 存在时报告 `unknown`，不猜 `/datapack list` 的顺序。`--print-commands` 只打印 `/datapack enable ... before ...` 建议，`--json` 输出结构化报告；不写 `level.dat`，不执行命令。

`dpp new` 生成 Beet 兼容骨架（`dpp.toml`、`beet.json`、`src/pack.mcmeta`、函数与 load 标签），不调用 Beet/Python；构建仍由作者执行 `beet build`。仓库内 `stdlib/` 是最小官方纯数据包 `dpp:stdlib` 0.1.0，提供 `dpp:meta` 版本握手、`dpp:emit` 和 `#dpp:events` 监听器标签；`examples/demo/` 演示依赖 stdlib 的最小用法。
`dpp build-pack` 读取包根（或含 `src/` 的项目根）的 `dpp.toml` 与 `pack.mcmeta`，校验声明的 pack format，按固定文件顺序、时间戳、权限和压缩参数生成确定性 zip；输出 sha512 与可粘贴到静态源的 JSON index 条目。
`dpp update` 丢弃现有 lock 的版本选择，按 `dpp.toml` + 当前 MC/format 重新求解；保留已有 `install_name`，自动创建 `pre-update-*` 快照，原子提交新 lock/index 并清理不再需要的包。
`install/update/remove --stage` 只写 `.dpp/staging/<txid>/plan.toml` 和 staged 包目录，不修改 `datapacks/`、lock/index；随后由 `dpp apply` 提交。
`dpp apply` 记录当前 lock 的激活标记到 `.dpp/applied.toml`；若存在 `staging/<txid>/plan.toml`，先原子提交 staged plan。默认 `next_start`，`--manual` 标记人工重启/重进，`--rcon` 通过 localhost RCON 发送 `/reload` 并检查 `datapack list enabled`（密码只从 `DPP_RCON_PASSWORD` 读取），`--log` 检查追加日志，`--abort` 删除 staged plan，`--rollback` 恢复最近的 `pre-apply-*` 快照。
适配器可把宿主注入的包写入 `.dpp/host-provided.toml`；`dpp host check` 会记录到 `host.lock`，`dpp status --server-root` 会显示 `host-provided` 并警告与 managed 包的冲突，但不会自动删除或重命名。

## 开发

```sh
cargo build
cargo test
cargo clippy --all-targets -- -D warnings
cargo run -p dpp-cli -- --help
```

布局：

- `crates/dpp-core`：manifest/lock/pack format/pack.mcmeta/path 等核心类型。
- `crates/dpp-modrinth`：Modrinth API 客户端与响应类型。
- `crates/dpp-cli`：`dpp` 二进制与 CLI 集成测试。
- `stdlib/`：官方最小纯数据包 `dpp:stdlib`。
- `examples/demo/`：依赖 `dpp:stdlib` 的最小示例包。

## 计划中的命令

```text
dpp add <pkg>
dpp remove <pkg>
dpp install
dpp order [--print-commands]
dpp source list|add|remove
dpp cache clean
```

具体语义见 [docs/design.md](docs/design.md) 与 [docs/apply.md](docs/apply.md)。

## 非目标

- 任何 jar / mod 管理。
- 资源包管理。
- 游戏注入、补丁、mixins。
- 自动执行数据包内容、安装脚本或 `uninstall_hook`。
- 自动重启服务端或自动写 `level.dat`（MVP）。
- 世界数据备份（region、实体、记分板等）；`snapshot` 只备份包状态。

## 仓库状态

Rust workspace 骨架已建立，当前只实现 `init`/`status`/`check`。尚未实现 Modrinth 源、lock 求解、安装事务、snapshot、apply、RCON 和加载顺序报告；这些按 [docs/feasibility.md](docs/feasibility.md) 的顺序推进。
Rust workspace 已实现 `new`/`init`/`add`/`remove`/`install`/`update`/`apply`/`status`/`order`/`check`/`search`/`inspect`/`build-pack`/`host`/`snapshot`/`config`/`adapter`，以及包内 `dpp.toml` 依赖求解。尚未实现自动重启、完整语义日志分析和包发布；这些按 [docs/feasibility.md](docs/feasibility.md) 的顺序推进。`dpp order` 只读报告期望/实际顺序，不写 `level.dat`，不执行 `/datapack enable`。
