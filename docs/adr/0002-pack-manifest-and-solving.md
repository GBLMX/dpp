# ADR-0002: 数据包清单、包身份与求解策略

- 状态：Accepted
- 日期：2026-11-25
- 实现状态：部分。包内 `dpp.toml` 解析与 pubgrub solver 已接入 `add`（无现成 lock）和 lock 缺失时的 `install`；已有 lock 的 `add` 仍只解析目标包，完整冲突回溯未实现。

## Context

Modrinth 提供数据包版本、`game_versions`、`loaders` 和每版本 `dependencies`，但：

- `version_number` 只是“理想情况下”遵循 SemVer；
- 存量数据包通常没有依赖声明；
- 数据包没有原生依赖图，版本范围也没有统一语义；
- 同名文件不一定是冲突：tag 会合并，函数/数据文件有覆盖规则；
- `pack.mcmeta` 的 `overlays` 可能让同一压缩包支持多个格式。

如果没有明确的包身份和版本策略，`pubgrub` 会被迫在不可比较的版本集合上工作。

## Decision

### 包内清单

数据包可以在 zip 根目录提供 `dpp.toml`：

```toml
[package]
id = "smithed:stdlib"
version = "0.3.1"
mc_versions = ["26.3"]
pack_format = [121, 0]

[dependencies]
"smithed:events" = ">=0.2.0, <0.4.0"

[conflicts]
"legacy:battle" = "*"

[host]
requires = ["dpp.events/1"]
```

- `dpp.toml` 与 `pack.mcmeta` 同级，位于 zip 根目录；嵌套目录中的同名文件不生效。
- 包内 `dpp.toml` 与 world 级 `world/dpp.toml` 同名但作用域不同：前者只描述数据包自身，后者描述 world 目标与直接依赖；解析器不得把两者混用。
- `[package].id` 是规范包 ID；必须稳定、全局唯一；依赖和冲突都使用规范包 ID。
- 来源定位（Modrinth project/version、静态源 URL）不进入规范包 ID；lock 单独记录。
- 依赖详情的 `project` 指定来源 locator；`source` 仍表示来源类型（例如 `modrinth`），两者不得混用。没有 `project` 时，依赖 key 作为 locator（legacy/world 直接依赖路径）。
- `[dependencies]`、`[conflicts]`、`[host]` 都是可选的。
- 没有 `dpp.toml` 的存量包视为 **legacy**：无依赖、无冲突、无 host 约束，允许精确版本安装。

### 版本与求解

- 只有带 `dpp.toml` 且 `package.version` 可解析为 SemVer 的包参加 SemVer 求解。
- 非 SemVer 版本只能精确锁定，不允许参与版本范围比较。
- 不信任 Modrinth `version_number`；它只作为展示和候选排序的辅助信息，最终以 lock 中的精确 version ID + sha512 为准。
- 求解按 world 的 MC 版本 + pack format 过滤候选后，再运行 `pubgrub`。
- 两阶段：
  1. 获取候选元数据与文件信息；
  2. 求解并生成精确 lock。
- lock 至少记录：规范包 ID、来源、project/version ID、版本号、sha512、install_name、文件清单哈希。
- 旧 lock 中把 Modrinth slug 当包 ID 的记录视为 legacy；在实现规范 ID 迁移前不能与新 lock 混用。

### Modrinth dependencies fallback

只有包内没有 `dpp.toml.dependencies` 时才使用 Modrinth 版本依赖：

- `required` → 作为依赖；
- `optional` → 默认不安装；
- `incompatible` → 作为冲突；
- `embedded` → 不安装，视为包已内置。

### 冲突与文件重叠

- 声明式 `[conflicts]` 是硬冲突。
- 路径重叠、tag 合并、`pack.mcmeta filter` 是静态审计信息；不能简单把所有同名路径判为错误。
- tag 文件会合并；同一函数/数据文件的覆盖必须单独定义规则，不能在 MVP 里假装是完整文件冲突检测。

### 宿主约束

`[host]` 不进入包求解图；它由独立的 host preflight 处理（ADR-0003）。包求解只回答“哪些数据包版本互相兼容”。

## Consequences

- 存量包仍可直接安装，但不会有依赖求解能力。
- 想做可依赖库的包必须提供 `dpp.toml` 并使用 SemVer。
- 版本号不规范的包不会被静默“修复”；它们只能精确安装。
- 求解器需要维护规范 ID ↔ 来源 locator 的映射，不能继续把 Modrinth slug 当唯一身份。
- 冲突检测在 MVP 中只能做到“声明冲突 + 静态审计”，不能承诺完整运行时冲突分析。

## Alternatives considered

- **只信任 Modrinth `dependencies`**：大量数据包没有依赖元数据，而且 project 依赖不表达版本范围。
- **把 `version_number` 全量当 SemVer**：会产生错误排序和错误求解；Modrinth 明确不保证。
- **不做求解器，只做 lock**：可以复现，但不能处理“升级一个公共库”的核心痛点，差异化不足。
- **把所有同名路径都判冲突**：会误报大量 tag 合并，破坏正常数据包组合。

## References

- [Modrinth List project's versions](https://docs.modrinth.com/api/operations/getprojectversions/)
- [设计文档](../design.md)
- [Smithed Namespacing](https://docs.smithed.dev/conventions/namespacing/)
