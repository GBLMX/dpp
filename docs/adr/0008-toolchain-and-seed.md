- 实现状态：已实现 stdlib、`dpp new` 脚手架与 `dpp build-pack` 确定性打包。

  - dpp 的差异点是直接管理 `world/datapacks/`、依赖求解、事务/快照和宿主预检；不是“又一个整合包导出器”。

## 实现

- `dpp new <name> [--dir <path>] [--mc-version <version>] [--pack-format <format>]`
  生成 Beet 项目骨架：`dpp.toml`、`beet.json`、`src/pack.mcmeta`、`src/data/<namespace>/function/` 和 `src/data/minecraft/tags/function/load.json`。
- `stdlib/` 是可直接打包的最小纯数据包：`dpp:stdlib` 0.1.0，提供版本握手、`dpp:meta` storage、`dpp:emit` 与空的 `#dpp:events` 监听器标签。
- `examples/demo/` 是依赖 `dpp:stdlib` 的最小示例，load 时调用 `dpp:emit`，通过 `#dpp:events` 注册监听器。
- `dpp new` 不调用 Beet/Python；作者仍需自行 `beet build`，dpp 只负责模板与产物校验。
- `dpp build-pack [PATH] [--output ZIP] [--url URL] [--json]` 校验 `dpp.toml`/`pack.mcmeta`，固定文件顺序、时间戳、权限与压缩参数生成确定性 zip，输出 sha512 和可粘贴到静态源的 JSON index 条目；支持直接包根或含 `src/` 的 Beet 项目根。