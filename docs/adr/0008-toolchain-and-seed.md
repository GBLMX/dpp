# ADR-0008: 与 Beet/Smithed 互补，种子库后置

- 状态：Accepted
- 日期：2026-11-25
- 实现状态：已实现 stdlib、`dpp new` 脚手架与 `dpp build-pack` 确定性打包。

## Context

构想中的生态策略是“官方 stdlib + 示范包 + 脚手架”。但外部已有：

- **Beet**：Python 数据包开发工具，负责构建、生成、检查，`beet build --link` 可直接链接到世界；
- **Smithed**：命名空间、pack ID、标签规范、Weld 合并等约定/工具；
- **Packwiz/DPM/mcpkg**：已有数据包/资源包安装或整合包管理能力；
- 数据包作者的构建需求与 dpp 的安装需求不是同一个问题。

如果 dpp 自己做构建、合并、包格式，会和现有工具重叠，且 Rust 工具还要维护 Python 生态的插件/生成器，成本很高。

## Decision

1. **dpp 不自建数据包构建器。**
   - Beet 负责源代码 → 数据包产物。
   - dpp 的 `build-pack`/`pack` 只做校验、打包、哈希、清单生成，不执行 Beet pipeline。
   - `dpp new` 可以生成 Beet 项目模板，但不能把 Python/Beet 变成 dpp 的运行时依赖。

2. **Smithed 是约定/工具，不是 dpp 的运行时信号协议。**
   - 采用其 namespacing、pack ID、tag 规范作为兼容参考。
   - Weld 作为可选离线合并工具；dpp 不默认把多个包 weld 成一个 zip，因为会丢失身份、更新来源和回滚能力。

3. **种子库在求解器之后。**
   - 先有 `dpp.toml` + lock + solver，stdlib 才有可声明的版本和依赖。
   - 第一版只做一个最小纯数据包 stdlib（事件/标签/storage schema + 版本握手）和一个 demo。
   - 不在 solver 稳定前承诺“2–3 个官方库 + 1–2 个示范包”。

4. **卸载钩子不作为第一版保证。**
   - 世界关闭时数据包无法执行清理；在线两阶段卸载需要 host/apply 支持。
   - 第一版文档必须说明：删除文件不等于清理游戏内状态。

5. **不重复 Packwiz。**
   - Packwiz 偏整合包/加载器，数据包需要 `datapack-folder` 和加载器 mod。

  - dpp 的差异点是直接管理 `world/datapacks/`、依赖求解、事务/快照和宿主预检；不是“又一个整合包导出器”。

## 实现

- `dpp new <name> [--dir <path>] [--mc-version <version>] [--pack-format <format>]`
  生成 Beet 项目骨架：`dpp.toml`、`beet.json`、`src/pack.mcmeta`、`src/data/<namespace>/function/` 和 `src/data/minecraft/tags/function/load.json`。
- `stdlib/` 是可直接打包的最小纯数据包：`dpp:stdlib` 0.1.0，提供版本握手、`dpp:meta` storage、`dpp:emit` 与空的 `#dpp:events` 监听器标签。
- `examples/demo/` 是依赖 `dpp:stdlib` 的最小示例，load 时调用 `dpp:emit`，通过 `#dpp:events` 注册监听器。
- `dpp new` 不调用 Beet/Python；作者仍需自行 `beet build`，dpp 只负责模板与产物校验。
- `dpp build-pack [PATH] [--output ZIP] [--url URL] [--json]` 校验 `dpp.toml`/`pack.mcmeta`，固定文件顺序、时间戳、权限与压缩参数生成确定性 zip，输出 sha512 和可粘贴到静态源的 JSON index 条目；支持直接包根或含 `src/` 的 Beet 项目根。

## Consequences

- dpp 保持 Rust 单二进制，不依赖 Python/Beet 才能安装数据包。
- 作者仍需安装 Beet 才能构建；dpp 只提供模板和产物校验。
- stdlib 的发布晚于 solver；短期生态内容少，但基础契约更可靠。
- 不会因为 Weld/合并丢失包身份和回滚粒度。

## Alternatives considered

- **内置 Beet/Python**：分发和跨平台成本高，且与 dpp 的核心问题无关。
- **把 Weld 作为默认安装步骤**：会改变包身份、更新来源和 load order，不可逆。
- **先做 stdlib 再做 solver**：stdlib 没有版本范围/依赖契约，容易变成不可升级的复制粘贴模板。
- **自动执行卸载脚本**：世界关闭/崩溃时不可靠，且扩大信任边界。

## References

- [Beet Get Started](https://mcbeet.dev/quick-start/get-started/)
- [Smithed Conventions](https://docs.smithed.dev/conventions/)
- [Smithed Weld](https://docs.smithed.dev/weld/)
- [Packwiz datapack options](https://packwiz.infra.link/reference/additional-options/)
- [设计文档](../design.md)
