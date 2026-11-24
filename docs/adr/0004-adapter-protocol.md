# ADR-0004: 宿主适配器是窄通道、低频、能力白名单

- 状态：Accepted
- 日期：2026-11-25
- 实现状态：部分。已实现 `dpp.events/1` 协议类型、manifest/event 校验、payload 限额、事件白名单、单调 ID 和 `dpp adapter validate/event`；未实现 Paper/KubeJS 传输适配器、调度、背压和负载测试。

## Context

“数据包发信号，后端做复杂计算，结果回写 storage”听起来像通用事件总线，但数据包和插件/mod 之间没有原生共享内存或统一事件 API：

- 数据包能执行命令、读写 scoreboard/storage、调用函数；
- Paper 插件能读 scoreboard、派发命令、监听 Bukkit 事件；
- Folia 的区域化线程不保证跨区域顺序；
- KubeJS 是 mod 侧脚本，和 Paper 插件不是同一宿主；
- 高频率事件逐条过桥会带来 TPS、命令解析和并发成本。

如果把它宣传成“任意插件事件 ↔ 任意数据包事件”的通用总线，协议会很快变成不可验证的 NMS/命令注入层。

## Decision

适配器采用**窄通道**：

1. **版本化协议**
   - 例如 `dpp.events/1`，有明确的 payload、事件 ID、超时、错误码。
   - 协议版本和数据包库版本、适配器版本分开记录。

2. **低频、可批处理**
   - 第一版只支持低频事件（BOSS 阶段、任务状态、菜单操作等）。
   - 高频事件由宿主侧聚合/采样；禁止默认每 tick 全量扫描所有 scoreboard/storage。

3. **有界 payload**
   - 事件字段有类型、长度和数量上限。
   - 复杂数据放 storage，但必须有 schema 和大小限制；不能把任意 NBT 当命令字符串执行。

4. **能力白名单**
   - 数据包只能调用适配器声明的事件类型。
   - 禁止数据包传入任意 JS、文件路径、shell 命令或任意命令文本。
   - KubeJS 适配器脚本是受信任的宿主配置，不是数据包可执行的代码。

5. **允许的实现手段**
   - 数据包 → 宿主：自定义命令（如 `/dpp:emit`）或 scoreboard 轮询。
   - 宿主 → 数据包：派发 `/function`；复杂 payload 需要 command storage 或 scoreboard，不能用不存在的通用 API。
   - 不承诺 Paper 有通用 score-change event；如果目标版本没有可订阅事件，就按轮询/自定义命令设计并实测。

6. **调度**
   - Paper：主线程/全局 scheduler。
   - Folia：全局命令和全局状态使用 global region；区域事件使用 region scheduler；跨区域顺序不保证。
   - Folia 不作为第一版适配器目标。

7. **幂等与背压**
   - 事件带单调 ID 或幂等键；重复投递可丢弃。
   - 队列满时明确丢弃/合并，不能无限堆积导致服务器卡死。

## Consequences

- 适配器不会成为“万能事件总线”，但可以稳定承载种子包真正需要的事件。
- 需要为每个宿主实现协议、集成测试和负载测试；不能只写一个转发 demo 就宣称支持。
- 高频率玩法逻辑应留在宿主侧，或者由数据包本地处理；跨边界只传状态变化。
- Paper 先做；KubeJS 作为独立 mod host；Folia 后置。

## Alternatives considered

- **全量 Bukkit 事件转发**：事件数量、线程模型、payload 映射和性能都不可控。
- **每 tick 扫描所有 scoreboard/storage**：简单但昂贵，且 Folia 下更复杂。
- **共享内存/NMS 直连**：版本脆弱、跨平台差、插件权限和崩溃风险高，不适合作为公共协议。
- **只做数据包，不做宿主适配器**：种子包无法利用插件算力，但可以作为 MVP 的安全回退。

## References

- [Paper scoreboard package](https://jd.papermc.io/paper/26.2/org/bukkit/scoreboard/package-summary.html)
- [Folia Overview](https://docs.papermc.io/folia/reference/overview/)
- [KubeJS Persistent Data](https://kubejs-wiki.tysontheember.dev/26.1/concepts/persistent-data/)
- [Smithed Namespacing](https://docs.smithed.dev/conventions/namespacing/)
