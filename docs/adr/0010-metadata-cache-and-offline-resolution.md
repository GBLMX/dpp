# ADR-0010: 版本元数据缓存与离线求解

- 状态：Accepted
- 日期：2026-11-25
- 取代：[ADR-0009](0009-offline-and-locked-boundary.md) 中“add/update 不支持 --offline”的部分；install 的 archive-only 边界不变。

## Context

`add` / `update` 求解需要 Modrinth 版本列表（版本号、`game_versions`、`loaders`、文件、依赖）。只缓存归档 zip 不够，因此 ADR-0009 当时拒绝 `add/update --offline`。

现在需要在不伪造“完整离线环境”的前提下，支持已查询过的项目离线求解：

- 在线查询成功后缓存版本列表。
- `--offline` 时不发网络请求，只读缓存；缺失或损坏即失败。
- 缓存不是 lock，不能承诺跨时间可复现。

## Decision

1. **缓存版本列表，不缓存求解结果。**
   - 路径：`~/.cache/dpp/metadata/<sha512(base_url|project|mc_version)>.json`。
   - 内容：`fetched_at` + `Vec<ProjectVersion>`。
   - 键包含 Modrinth base URL、项目 ID/slug、目标 MC 版本；不同 MC 版本不串用。

2. **在线成功才写缓存。**
   - TTL 300 秒；TTL 内在线请求直接复用。
   - 写缓存是 best effort：创建目录或写入失败不影响在线求解。
   - 不缓存错误响应、空结果或解析失败。

3. **`--offline` 只读元数据缓存。**
   - 忽略 TTL；缓存存在即使用。
   - 缓存缺失、读失败或 JSON 损坏时返回错误，不回退网络。
   - 仍会读取 archive cache 完成包下载/校验；两条缓存职责分离。

4. **`--locked` 仍只属于 `install`。**
   - `add/update --offline` 只表示“求解不联网”，不冻结版本。
   - 需要可复现环境时仍使用 `install --locked --offline`。

5. **CLI 边界。**
   - `dpp add <project> --offline`
   - `dpp update [--stage] --offline`

## Consequences

- 离线 `add` / `update` 要求此前对同一项目、同一 MC 版本做过在线查询，或缓存由其他流程写入。
- 缓存过期但离线时继续使用；这是可用性优先，不代表元数据最新。
- 缓存损坏不会静默忽略，避免把空候选误报成“无兼容版本”。
- `~/.cache/dpp/metadata` 可安全删除；删除后离线求解失败，在线求解会重建。
- 归档缓存和元数据缓存都不等于 lock；可复现性仍由 `lock.toml` + `install --locked --offline` 提供。

## Implementation

- `dpp-modrinth::ModrinthClient::with_metadata_cache`
- `ModrinthClient::with_offline`
- `project_versions` 缓存读写与 TTL
- `dpp add --offline` / `dpp update --offline`
