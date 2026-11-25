# Architecture Decision Records

本目录记录 dpp 的架构决策。ADR 只记录**决策与后果**，不替代设计文档、用户文档或实现计划。

## 索引

| ADR | 决策 | 状态 | 实现 |
|---|---|---|---|
| [0001](0001-instance-root-and-layers.md) | 实例根与状态作用域 | Accepted | 部分：instance root / host.lock / `dpp host` 已实现；config 作用域未实现 |
| [0002](0002-pack-manifest-and-solving.md) | 数据包清单、包身份与求解策略 | Accepted | 部分：add/install/update 均走 pubgrub；已有 lock 的 add 优先锁版本；声明冲突仍是求解后校验 |
| [0003](0003-host-capability-boundary.md) | 宿主能力是预检约束，不是包管理 | Accepted | 部分：离线检测、host.lock、`add/install --server-root` 包级 requires/forbids 已接入；运行时握手未实现 |
| [0004](0004-adapter-protocol.md) | 宿主适配器是窄通道、低频、能力白名单 | Accepted | 协议类型/校验/CLI 已实现；Paper/KubeJS 传输适配器未实现 |
| [0005](0005-host-provided-datapacks.md) | 宿主注入数据包不进入 dpp 包图 | Accepted | 部分：host-provided 观测、host.lock、status 冲突警告已实现；Paper 自动采集未实现 |
| [0006](0006-config-patches.md) | 配置补丁延后，限定实例层 JSON/TOML | Accepted | JSON/TOML allowlist、merge/replace、基线冲突与备份已实现；package 绑定/YAML 未实现 |
| [0007](0007-transaction-and-snapshot.md) | 事务回滚与持久快照分离 | Accepted | 事务/journal 恢复、managed 包快照 save/restore/list 与 remove 自动快照已实现；config 快照未实现 |
| [0008](0008-toolchain-and-seed.md) | 与 Beet/Smithed 互补，种子库后置 | Accepted | 已实现：`stdlib/`、`dpp new`、`dpp build-pack` |
| [0009](0009-offline-and-locked-boundary.md) | 严格安装模式只覆盖 install | Accepted | 已实现 `install --locked/--offline`；add/update offline 仍等待 metadata cache |

## 约定

- `Accepted` 表示当前设计以此为准；实现状态单独标注。
- 改变已接受决策时新增 ADR，并标记旧 ADR 为 `Superseded by ADR-XXXX`，不要改写历史。
- 与 `docs/design.md`、`docs/apply.md`、`docs/load-order.md` 冲突时，先确认哪个是规范来源，再修实现或文档。
