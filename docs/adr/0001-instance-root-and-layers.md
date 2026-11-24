# ADR-0001: 实例根与状态作用域

- 状态：Accepted
- 日期：2026-11-25
- 实现状态：部分。world 包作用域和 instance/host 作用域已实现；config 作用域未实现。

## Context

dpp 当前以单个 Minecraft world 为工作单元：`--world` 指向世界目录，`.dpp/lock.toml`、`index.toml` 和 `datapacks/` 都在 world 内。这适合纯数据包管理，但新构想加入了：

- 宿主插件 / mod / 脚本（`plugins/`、`mods/`、`kubejs/`）；
- 服务器配置；
- 适配器与 `host.lock`；
- 多实例同步。

这些对象位于 server root，不在 world 内。如果继续把所有状态塞进 world，路径推断、配置补丁和宿主预检都会变得不可靠。

## Decision

不改变 `docs/design.md` 已有的“包层 / 世界配置层 / 运行时与持久世界状态层”模型；在其上增加**作用域**：

1. **Package 作用域（world）**
   - 作用域：一个 world。
   - 管理 `world/datapacks/`、`world/.dpp/lock.toml`、`index.toml`、staging/trash、包快照。
   - 依赖求解、安装、卸载、包级回滚只在这一层发生。

2. **Host / Instance 作用域（server root）**
   - 作用域：server root。
   - 管理 `host.lock`、宿主能力声明、适配器观测记录、实例级快照。
   - 不下载、不替换、不删除 jar / mod / 插件；详见 ADR-0003。

3. **Config 作用域（server root）**
   - 未来的配置补丁、宿主配置目标。
   - 在 ADR-0006 之前不实现配置补丁。

建议目录：

```text
<server-root>/
├── plugins/ mods/ kubejs/
├── world/
│   ├── level.dat
│   ├── datapacks/
│   └── .dpp/
│       ├── lock.toml
│       ├── index.toml
│       ├── staging/
│       └── snapshots/
└── .dpp/
    ├── instance.toml
    ├── host.lock
    └── snapshots/
```

约束：

- `--world` 仍是包操作的必需参数。
- 新增可选的 `--server-root`。没有 server root 时，host/config 层禁用；单机存档和纯数据包模式不受影响。
- 包操作不得写 server root 的 host/config 文件，除非是显式的预检读取。
- host/config 操作不得写 `world/datapacks/`。
- 现有 world 内 `.dpp` 保持不变，不强制迁移。
- 快照分域：包快照在 world，config/instance 快照在 server root。

## Consequences

- 纯 world 模式继续可用，不会因为宿主功能而强制重构。
- 需要 server root 的命令在没有 `--server-root` 时必须明确报错，而不是猜测父目录。
- 所有涉及配置、适配器、host.lock 的功能默认受实例根约束。
- 多世界服务器的包环境仍是 per-world；宿主环境是 per-instance。两者不能混成一个 lock。

## Alternatives considered

- **全部状态放到 server root**：对单机存档和纯数据包用户过重，也会破坏现有 world 内状态。
- **全部状态继续放在 world**：host 配置和插件目录不在 world 内，长期会产生路径猜测和跨层写。
- **强制所有用户提供 server root**：能简化实现，但不符合“存量数据包直接可用”的低门槛目标。

## References

- [设计文档](../design.md)
- [可行性记录](../feasibility.md)
