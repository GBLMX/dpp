# dpp

Minecraft Java Edition 数据包（datapack）包管理器。只管理原版数据包：不管理 mod、不管理资源包、不注入游戏、不执行包内脚本。

> 状态：Rust workspace 已实现 `new` / `init` / `add` / `remove` / `install` / `update` / `apply` / `status` / `order` / `check` / `search` / `inspect` / `build-pack` / `host` / `snapshot` / `config` / `adapter`。`docs/` 是当前设计、激活语义和可行性结论；实现以这些文档为准。

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
| [docs/adr/README.md](docs/adr/README.md) | 已接受的架构决策记录 |

## 当前实现

Rust workspace 已建立；目前可运行：

- `dpp --version`
- `dpp new <name> [--dir <PATH>] [--mc-version <VERSION>] [--pack-format <FORMAT>]`
- `dpp init [--mc-version <VERSION>] [--pack-format <FORMAT>]`
- `dpp add <modrinth-project>`
- `dpp remove <modrinth-project> [--stage]`
- `dpp install [--stage] [--locked] [--offline]`
- `dpp update [--stage]`
- `dpp apply [--dry-run] [--manual] [--rcon] [--log <PATH>] [--abort] [--rollback]`
- `dpp snapshot save <name>`
- `dpp snapshot restore <name>`
- `dpp snapshot list`
- `dpp config allow <target>`
- `dpp config apply <patch>`
- `dpp config status`
- `dpp adapter validate <manifest>`
- `dpp adapter event <manifest> <event.json>`
- `dpp status`
- `dpp order [--json] [--print-commands]`
- `dpp check`
- `dpp search <query> [--mc-version <VERSION>] [--limit <N>]`
- `dpp host status --server-root <PATH>`
- `dpp host check --server-root <PATH>`
- `dpp build-pack [PATH] [--output <ZIP>] [--url <URL>] [--json]`
- `dpp inspect <zip> --pack-format <FORMAT>`

`init` 解析 `level.dat` 的 `Data.Version` / `DataVersion`，自动识别 MC 版本与 data pack format；`--mc-version` / `--pack-format` 可覆盖。快照版本若找不到本地 client/server jar，会要求显式指定 format。`add` 从 Modrinth 选择最新且 `pack.mcmeta` 兼容当前世界格式的 datapack，下载、校验 sha512、写入 `lock.toml` / `index.toml` 并通过 staging + rename 安装。首次安装按 `load_weight` 生成 `0050_<slug>` 目录；更新保持 `install_name` 不变，不自动重排，也不写 `level.dat`。`install` 以 `lock.toml` 为权威重建缺失的数据包；已安装且哈希一致的包会跳过。`remove` 删除直接依赖后，从剩余直接依赖出发遍历已安装包内 `dpp.toml`，删除不可达的 lock/index 条目和 managed 目录；仍被其他包需要的目标保留。`status` 显示检测结果；`check` 校验 manifest / world / lock/index 一致性、逐文件 sha512、managed 目录缺失/额外文件和外来 datapacks 条目；`inspect` 校验本地 zip 根目录的 `pack.mcmeta` 与目标格式。

`add`、lock 缺失时的 `install` 与 `update` 都使用 `pubgrub` 求解包内 `dpp.toml` 依赖；已有 lock 的 `add` 会把锁定版本作为偏好，只有新约束不兼容时才升级共享依赖，并保留已有 `install_name`。`dpp host status/check` 可离线检测宿主并写入 `.dpp/host.lock`；`add/install` 传入 `--server-root` 时会校验包内 `[host] requires/forbids`。jar 管理仍不做，`inspect` 会显示包内清单元数据。
`[conflicts]` 与 Modrinth `incompatible` 先正常求解；发现冲突对后禁用候选并有界重试，已有 lock 版本作为偏好。超过重试上限或无替代候选时失败，不写包层。

`dpp install --locked` 要求 lock 与 manifest/world 匹配，且不改写 lock；`dpp install --offline` 只使用全局 archive cache，缺归档即失败。两者可组合。`add` / `update` 仍需要 Modrinth 版本元数据，因此不支持 `--offline`，也不支持 `--locked`。

`dpp order` 在疑似关闭的世界里读取 `level.dat` 的 `DataPacks.Enabled`，按 `(load_weight, id, install_name)` 报告期望顺序；`session.lock` 存在时报告 `unknown`，不猜 `/datapack list` 的顺序。`--print-commands` 只打印 `/datapack enable ... before ...` 建议，`--json` 输出结构化报告；不写 `level.dat`，不执行命令。

`dpp new` 生成 Beet 兼容骨架（`dpp.toml`、`beet.json`、`src/pack.mcmeta`、函数与 load 标签），不调用 Beet/Python；构建仍由作者执行 `beet build`。仓库内 `stdlib/` 是最小官方纯数据包 `dpp:stdlib` 0.1.0，提供 `dpp:meta` 版本握手、`dpp:emit` 和 `#dpp:events` 监听器标签；`examples/demo/` 演示依赖 stdlib 的最小用法。
`dpp build-pack` 读取包根（或含 `src/` 的项目根）的 `dpp.toml` 与 `pack.mcmeta`，校验声明的 pack format，按固定文件顺序、时间戳、权限和压缩参数生成确定性 zip；输出 sha512 与可粘贴到静态源的 JSON index 条目。
`dpp update` 丢弃现有 lock 的版本选择，按 `dpp.toml` + 当前 MC/format 重新求解；保留已有 `install_name`，自动创建 `pre-update-*` 快照，原子提交新 lock/index 并清理不再需要的包。
`install/update/remove --stage` 只写 `.dpp/staging/<txid>/plan.toml` 和 staged 包目录，不修改 `datapacks/`、lock/index；随后由 `dpp apply` 提交。
`dpp apply` 记录当前 lock 的激活标记到 `.dpp/applied.toml`；若存在 `staging/<txid>/plan.toml`，先原子提交 staged plan。默认 `next_start`，`--manual` 标记人工重启/重进，`--rcon` 通过 localhost RCON 发送 `/reload` 并检查 `datapack list enabled`（密码只从 `DPP_RCON_PASSWORD` 读取），`--log` 检查追加日志，`--abort` 删除 staged plan，`--rollback` 恢复最近的 `pre-apply-*` 快照。

适配器可把宿主注入的包写入 `.dpp/host-provided.toml`；`dpp host check` 会记录到 `host.lock`，`dpp status --server-root` 会显示 `host-provided` 并警告与 managed 包的冲突，但不会自动删除或重命名。

`dpp snapshot save/restore/list` 保存/恢复 managed 数据包目录、`dpp.toml`、`lock.toml` 和 `index.toml`；`remove` 和 `snapshot restore` 前会自动创建保护快照。快照不包含 world region、实体、scoreboard 或 command storage。

`dpp config allow/apply/status` 在实例层应用 JSON/TOML 补丁；目标必须显式 allowlist，写入前记录备份与 sha512 基线，用户修改后冲突会生成 `.dppnew` 而不是静默覆盖。

`dpp adapter validate/event` 校验 `dpp.events/1` 适配器清单和事件 JSON：有界 payload、声明事件白名单、单调事件 ID；真正的 Paper/KubeJS 传输适配器仍需单独实现。

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
- `crates/dpp-ops`：Modrinth 解析、下载缓存、staging/rename 事务、lock/index 更新。
- `crates/dpp-cli`：`dpp` 二进制与 CLI 集成测试。
- `stdlib/`：官方最小纯数据包 `dpp:stdlib`。
- `examples/demo/`：依赖 `dpp:stdlib` 的最小示例包。

## 计划中的命令

```text
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

Rust workspace 已实现 `new`/`init`/`add`/`remove`/`install`/`update`/`apply`/`status`/`order`/`check`/`search`/`inspect`/`build-pack`/`host`/`snapshot`/`config`/`adapter`，以及包内 `dpp.toml` 依赖求解。尚未实现自动重启、完整语义日志分析和包发布；这些按 [docs/feasibility.md](docs/feasibility.md) 的顺序推进。`dpp order` 只读报告期望/实际顺序，不写 `level.dat`，不执行 `/datapack enable`。
