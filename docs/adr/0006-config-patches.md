# ADR-0006: 配置补丁延后，限定实例层 JSON/TOML

- 状态：Accepted
- 日期：2026-11-25
- 实现状态：部分。已实现实例层 JSON/TOML allowlist、merge/replace、sha512 基线冲突检测、`.dppnew` 和 config backup；package 绑定与 YAML 未实现。

## Context

“配置片段随包版本分发、自动合并、可回滚”很吸引人，但数据包生态并没有统一的配置文件模型：

- 大量数据包把配置放在游戏内 storage / scoreboard；
- 插件配置常见 YAML、JSON、TOML，路径和 schema 由插件决定；
- 数据包 zip 通常不携带服务器配置文件；
- 用户手改配置后，升级覆盖会产生冲突；
- 配置属于 server root，不是 world 包层。

如果把“任意配置文件补丁”放进数据包 MVP，会把 dpp 变成通用服务器配置管理器，并扩大信任边界。

## Decision

1. **配置补丁不进入 MVP。**
   - 先完成包求解、lock、事务、快照和 host 预检。
   - 配置补丁作为实例层能力，独立设计。

2. **只支持显式声明的 JSON/TOML 补丁。**
   - 目标路径必须在允许列表内。
   - 每份补丁声明 `target`、`format`、`merge`、所有权标记。
   - 不支持 YAML；需要时再单独 ADR。
   - 不支持任意脚本、模板执行或 shell 命令。

3. **必须有三方/冲突语义。**
   - 记录安装基线哈希。
   - 用户修改与包更新冲突时拒绝覆盖或生成 `.dppnew` 类文件；不能静默覆盖。
   - 补丁与包版本绑定，但只对 dpp 管理的 host companion 配置生效。

4. **游戏内配置不属于文件补丁。**
   - storage / scoreboard 配置属于世界数据；不能伪装成“配置文件”。
   - 是否快照命令存储/计分板需要单独的 world-data 决策，不在本 ADR 范围。

5. **每次配置变更前必须快照。**
   - 配置补丁和包变更共享事务边界，但快照范围包含目标配置文件。
   - 事务失败回滚；成功后保留可恢复快照。

## Consequences

- 第一阶段不解决“所有配置随包迁移”的愿景；这是有意的范围收缩。
- 适配器/宿主配置仍需用户手动维护；dpp 只能记录和校验。
- 需要先定义允许列表和所有权，否则不能安全地写 server root。
- 未来若支持 YAML，必须单独评估 schema、注释保留和冲突合并。

## Alternatives considered

- **通用文件覆盖/补丁器**：无法验证语义，容易破坏插件配置。
- **只做 YAML/JSON 简单 merge**：数组、注释、删除语义和冲突无法统一。
- **把配置放进数据包 zip 并让游戏读取**：只适用于自定义数据包，不是服务器/插件配置。
- **不做配置管理**：可以接受；用户仍可手动配置，dpp 保持包管理器定位。

## References

- [KubeJS Persistent Data](https://kubejs-wiki.tysontheember.dev/26.1/concepts/persistent-data/)
- [Smithed Namespacing](https://docs.smithed.dev/conventions/namespacing/)
- [设计文档](../design.md)
