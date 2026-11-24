# ADR-0007: 事务回滚与持久快照分离

- 状态：Accepted
- 日期：2026-11-25
- 实现状态：部分。事务回滚、journal（prepared/committing/committed + 自动恢复）、managed 包 snapshot save/restore/list、remove 自动快照已实现；config/instance 快照未实现。

## Context

当前实现已经有：

- staging 解压；
- trash 备份；
- rename 提交；
- lock/index 原子写；
- 已处理错误时恢复旧目录和旧文件。

但这不是用户可长期使用的 snapshot。事务回滚只处理“本次操作失败”；用户升级后想回到上一版本、或配置被改坏时，需要持久快照。

另外，kill -9 或断电可能发生在 rename 之后、lock 写入之前。没有 journal 时，不能宣称崩溃安全。

## Decision

### 事务

- 一次 `add/remove/install/update/config` 是一个事务。
- 事务失败时恢复到操作前状态。
- 提交顺序：staging → trash 备份 → rename 新目录 → 原子写 lock/index → 清理 trash。
- 没有实现 journal 前，不承诺 kill -9/断电后的自动恢复；崩溃后最多留下可诊断的 staging/trash。

### 持久快照

- `snapshot` 是独立机制，不是事务的别名。
- 自动快照时机：`update`、`remove`、配置补丁、显式 `snapshot save`；`add` 可按策略自动快照。
- 快照范围：
  - `world/datapacks/` 中 dpp managed 的目录；
  - `world/dpp.toml` 的期望状态；
  - `world/.dpp/lock.toml`、`index.toml`；
  - 已实现的受管配置文件；
  - host/config 层有 server root 时，记录 `host.lock`。
- 不包含：region、实体、scoreboard、command storage、完整世界数据。
- 实现优先使用 hardlink/copy；快照 manifest 记录路径、sha512、来源事务 ID。
- 保留策略显式配置；默认保留最近 N 个，不能无限增长。

### 状态

`status`/`snapshot` 必须区分：

- `transaction rollback available`；
- `snapshot available`；
- `crash recovery unknown`。

不能把“事务回滚”宣传成“完整世界备份”。

## Consequences

- 用户获得可恢复的包环境，而不是只有错误时回滚。
- 快照会占用磁盘；需要保留策略和清理命令。
- 配置文件快照需要 ADR-0006 的实例根和所有权模型。
- 没有 journal 前，崩溃一致性仍是已知限制。

## Alternatives considered

- **只保留事务回滚**：无法处理升级后回退和用户误操作。
- **每次备份完整世界**：太慢、太大，且不是 dpp 的职责。
- **用 git 做快照**：二进制、权限、大文件和 staging 不适合；可以作为用户外部方案，不作核心。
- **先做 journal 再做快照**：journal 解决崩溃恢复，不解决用户回滚；两者可以并行设计，但不应混淆。

## References

- [设计文档](../design.md)
- [Apply 语义](../apply.md)
