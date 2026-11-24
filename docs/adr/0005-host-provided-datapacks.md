# ADR-0005: 宿主注入数据包不进入 dpp 包图

- 状态：Accepted
- 日期：2026-11-25
- 实现状态：部分。已实现 `.dpp/host-provided.toml` 观测、host.lock 记录、`host status` 展示和 managed/host-provided 冲突警告；Paper lifecycle API 和自动采集适配器未实现。

## Context

Paper 的实验性 Datapack Discovery API 允许插件在启动时发现、移除、注入数据包。插件注入的包 ID 形如：

```text
<PluginName>/<id>
```

它不是 `world/datapacks/` 下的普通目录，也不受 dpp 的 `lock.toml` / `index.toml` 管理。适配器可能想用这个能力携带自己的协议数据包，但这会和 dpp 的包所有权、快照、卸载语义冲突。

## Decision

1. **dpp 不用 Paper lifecycle API 管理用户数据包。**
   - 用户数据包只通过 `world/datapacks/` 安装。
   - dpp 不通过插件 API 注入/移除用户的普通数据包。

2. **宿主注入包标记为 `host-provided`。**
   - 不进入 `lock.toml` / `index.toml`。
   - `dpp status` 可以显示其存在，但不能 `dpp remove` / `dpp update`。
   - 其版本、启用状态、加载顺序由宿主/适配器负责。

3. **适配器 companion 包也属于宿主。**
   - 如果 Paper 适配器需要自带协议数据包，它由插件 JAR 提供，不归 dpp 管理。
   - dpp 只记录适配器版本和观测到的能力；不能把它写入包 lock。

4. **不自动去重。**
   - 如果用户同时安装普通 world 数据包和插件注入的同名包，dpp 不能静默删除其中一个；只报告冲突/重复风险。
   - 用户要切换到 dpp 管理，必须手动禁用插件注入或移除插件，不能由 dpp 越过宿主所有权。

5. **Paper lifecycle API 仍是实验性。**
   - 不把它作为 dpp 的核心依赖；适配器必须锁定 Paper 版本并做集成测试。

## Consequences

- dpp 的包图保持完整、可快照、可回滚；不会出现“lock 里没有但游戏里加载了”的隐形包。
- 适配器自带的数据包不能被 dpp 单独升级；升级适配器就是升级插件，属于宿主运维。
- 需要 status 明确区分 `managed` 与 `host-provided`。
- 如果未来要统一管理，必须新增 ADR 并定义所有权转移流程。

## Alternatives considered

- **把插件注入包纳入 lock**：无法通过文件快照恢复，卸载/升级由插件控制，dpp 无法保证一致性。
- **自动移除重复包**：危险，可能破坏适配器协议包或用户数据。
- **完全忽略宿主包**：会导致“看起来没装、游戏里却生效”的状态不可见，因此至少要在 status 中报告。

## References

- [Paper Datapack Discovery](https://docs.papermc.io/paper/dev/lifecycle/datapacks/index.md)
- [设计文档](../design.md)
