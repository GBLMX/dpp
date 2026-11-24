# dpp 设计文档

状态：设计阶段，尚未实现。文档中的命令、字段和目录结构是当前契约；实现前可以修改，但修改后必须同步更新这里。

## 阅读顺序

1. [design.md](design.md) — 目标、三层模型、状态机、文件布局、源与求解、命令。
2. [apply.md](apply.md) — `/reload` 的真实边界、apply 决策矩阵、RCON/MSMP、卸载清理。
3. [load-order.md](load-order.md) — 加载顺序的权威来源与 MVP 取舍。
4. [feasibility.md](feasibility.md) — Modrinth、pack format、pacman 参考、风险与 MVP。

## 文档定位

| 文档 | 定位 | 是否规范 |
|---|---|---|
| [design.md](design.md) | 核心架构与数据模型 | 规范 |
| [apply.md](apply.md) | 运行时激活与状态语义 | 规范 |
| [load-order.md](load-order.md) | 加载顺序决策记录 | 规范 |
| [feasibility.md](feasibility.md) | 调研与可行性记录 | 参考（不是接口契约） |

## 术语

- **包层**：dpp 拥有的静态状态，包括 `dpp.toml`、`lock.toml`、`index.toml`、managed `datapacks/` 和缓存。
- **世界配置层**：MC 拥有的静态状态，包括 `level.dat` 的 `DataPacks`/`Version`/`allowCommands`/`enabled_features`、`server.properties` 和外来数据包。dpp 只读。
- **运行时 + 持久世界状态层**：内存注册表、函数、标签、tick，以及持久化的记分板、实体、region、`schedule` 等。dpp 不管理。
- **desired / staged / committed / applied / observed / unknown**：见 [design.md](design.md) 与 [apply.md](apply.md)。

## 变更规范

- 任何改变命令语义、文件格式或 `apply` 行为的改动，先改文档，再改实现。
- `feasibility.md` 中的结论如果被版本更新推翻，更新该文档并在相关规范文档中修正。
- 加载顺序相关决策以 [load-order.md](load-order.md) 为准，不要在其他文档里重新定义。
