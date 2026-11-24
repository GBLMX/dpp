# dpp

Minecraft Java Edition 数据包（datapack）包管理器。只管理原版数据包：不管理 mod、不管理资源包、不注入游戏、不执行包内脚本。

> 状态：Rust workspace 骨架已建立；核心包管理功能尚未实现。`docs/` 是当前设计、激活语义和可行性结论；实现以这些文档为准。

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
- `dpp init [--mc-version <VERSION>] [--pack-format <FORMAT>]`
- `dpp status`
- `dpp check`

`init` 目前只检查 `level.dat` 是否存在并创建 `dpp.toml` 与 `.dpp/`；尚不解析 NBT，MC 版本/format 需要手动指定。其余命令见下方“计划中的命令”。

## 开发

```sh
cargo build
cargo test
cargo clippy --all-targets -- -D warnings
cargo run -p dpp-cli -- --help
```

布局：

- `crates/dpp-core`：manifest/lock/pack format/path 等核心类型。
- `crates/dpp-cli`：`dpp` 二进制与 CLI 集成测试。

## 计划中的命令

```text
dpp init
dpp search <query>
dpp add <pkg>
dpp remove <pkg>
dpp install
dpp update [pkg...]
dpp apply [--dry-run] [--rcon]
dpp status [--json]
dpp check [--strict]
dpp snapshot save|restore|list
dpp order [--print-commands]
dpp build-pack
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
