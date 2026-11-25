# ADR-0009: 严格安装模式只覆盖 install

- 状态：Accepted
- 日期：2026-11-25
- 实现状态：已实现 `dpp install --locked`、`--offline` 及组合；`add/update --offline` 明确不支持。

## Context

dpp 需要同时满足两类需求：

- CI / 分享环境需要只按 `lock.toml` 精确复现，不因 manifest 或网络变化而改写包层。
- 断网环境需要只使用本地 archive cache 安装；手写 lock 或旧 lock 可能没有 `url`。

但 `add` / `update` 与 `install` 的信息来源不同：

- `install` 已有 lock，只需要归档和逐文件校验。
- `add` / `update` 需要 Modrinth 的版本列表、依赖、`game_versions`、loaders、pack format 等元数据；仅有 archive cache 不足以求解。

如果声称 `add/update --offline`，会出现“归档已缓存但解析仍需网络”的假离线。

Cargo 的语义也支持拆开：

- `--locked` 断言使用现有 lock，缺失或不匹配时失败。
- `--offline` 禁止网络，只使用本地可用内容。
- 两者可组合；不额外引入 `--frozen` 别名。

参考：[Cargo build](https://doc.rust-lang.org/cargo/commands/cargo-build.html)。

## Decision

1. **`--locked` 与 `--offline` 只属于 `dpp install`。**
   - `add` / `update` 不提供这两个参数。
   - 未来只有在元数据缓存完成后，才重新评估 `--offline`。

2. **严格模式要求显式 `lock.toml`。**
   - 没有 lock 时不能退化为在线求解。
   - 错误信息必须说明缺少 lock，而不是尝试联网。

3. **`--locked` 校验并冻结 lock。**
   - 校验 manifest 直接依赖及其版本范围。
   - 校验世界 MC 版本 / pack format、install name、sha512。
   - 不自动回填 lock 的 `package_id` 或 metadata。
   - lock 不满足 manifest 时失败，提示运行 `dpp update`。

4. **`--offline` 只使用 archive cache。**
   - 以 lock 的 sha512 定位 `~/.cache/dpp/archives/<sha512>.zip`。
   - 命中且哈希正确才使用；损坏缓存不静默回退网络。
   - 缺失或损坏时失败，不写 `datapacks/`。
   - 离线安装不要求 lock 提供 `url`；URL 只在需要下载时必需。

5. **不声明“假离线”。**
   - `add/update` 继续需要 Modrinth API。
   - 文档必须明确离线边界，避免用户把 archive cache 误当成完整元数据缓存。

6. **保留显式组合。**
   - 使用 `dpp install --locked --offline`。
   - 不增加 `--frozen`，避免把两个语义不同的开关隐藏成一个。

## Consequences

- CI 可以用 `install --locked --offline` 做可复现、无网络安装。
- 手写/旧 lock 只要 sha512 和 cache 正确即可离线安装，即使没有 URL。
- `add/update` 仍受网络和 Modrinth rate limit 影响。
- 未来 metadata cache 需要单独 ADR；不能复用它来声称当前 `add/update --offline` 已实现。
- 严格模式失败时包层保持不变；这是可预期行为。

## Alternatives considered

- **让 `add` / `update` 复用 archive cache 做离线**：拒绝。版本和依赖元数据不在 archive cache 中。
- **没有 lock 时在线求解后继续离线安装**：拒绝。这违反 `--offline` 的禁网语义和可复现目标。
- **增加 `--frozen`**：拒绝。当前设计文档已明确只保留两个显式语义，避免混淆。
- **离线时忽略 URL 缺失并静默用空 URL**：仅用于离线 cache 命中；在线缺 URL 仍失败。

## Implementation

- `InstallOptions { stage, locked, offline }`
- `install_world_with_options`
- `ensure_cached_with_mode`
- `prepare_lock` 在离线模式下允许缺失 `url`
- 测试覆盖：
  - 缓存命中且 `url = None`
  - 缓存缺失且 `url = None`
  - 无 lock 时两种严格模式失败
  - lock 版本不满足 manifest 范围失败
  - CLI 参数失败路径

## References

- [Cargo build: --locked / --offline / --frozen](https://doc.rust-lang.org/cargo/commands/cargo-build.html)
- [uv CLI: --offline](https://docs.astral.sh/uv/reference/cli/)
- [Modrinth API: rate limits and User-Agent](https://docs.modrinth.com/api/)
- [Modrinth List project's versions](https://docs.modrinth.com/api/operations/getprojectversions/)
- [设计文档](../design.md)
- [ADR-0002](0002-pack-manifest-and-solving.md)
