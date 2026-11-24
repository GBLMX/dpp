# ADR-0003: 宿主能力是预检约束，不是包管理

- 状态：Accepted
- 日期：2026-11-25
- 实现状态：部分。已实现 `--server-root`、离线 host 检测、`host.lock`、`dpp host status/check`，以及 `add/install` 的包级 `[host] requires/forbids` 预检；运行时适配器握手和 jar 管理按边界未实现。

## Context

数据包可以脱离 mod/插件独立运行，但生产服务器常依赖宿主能力：

- Paper/Purpur 插件 API；
- Folia 的 regionized threading；
- NeoForge/Fabric + KubeJS 脚本；
- 插件/mod 提供的命令、storage、事件或持久化。

同时，dpp 的边界是“不管理 mods/plugins 的 jar”。如果把宿主插件当包依赖求解，会直接违反边界，并且无法保证插件版本、加载器和运行时行为。

KubeJS 是 mod，不是 Paper 插件；Paper、Folia 和 KubeJS 不能放在同一个宿主类型里。

## Decision

### 宿主能力模型

包可以在 `dpp.toml` 声明：

```toml
[host]
requires = ["dpp.events/1", "paper"]
forbids = ["folia"]
```

- 这是**预检约束**，不是依赖包。
- 不进入 `pubgrub` 包图；不安装、不升级、不删除任何 jar/mod/plugin。
- 能力使用版本化 token，例如 `dpp.events/1`；不要用“某个插件名”代替能力。
- 包作者可以用 `requires` 声明最低能力；`forbids` 用于明确不兼容的宿主类型。

### host.lock

`host.lock` 记录实例层观测结果，至少包括：

```toml
[host]
kind = "paper"          # vanilla | paper | purpur | folia | neoforge | fabric
version = "26.3"
capabilities = ["dpp.events/1"]
adapters = []
```

状态区分：

- `declared`：来自包或用户配置；
- `observed`：dpp 离线扫描文件/目录得到；
- `verified`：适配器在运行时握手确认；
- `unknown`：无法判断，不能当作成功。

没有 server root 时，host 层禁用。

### 检测边界

- 离线检测只能看 server root、jar 文件名/元数据、`plugins/`、`mods/`、`kubejs/`、`server.properties` 等。
- 不能从文件名可靠推断所有插件版本；不能保证插件实际启用。
- RCON/运行时探测是可选的，不是默认依赖；没有 RCON 不应让普通数据包安装失败。
- `--strict-host` 可以把缺失的 required 能力升级为错误；默认对无法验证的能力给 warning/unknown。

### 适配器边界

MVP 中适配器是**外部 companion**：

- Paper 插件由用户手动安装，dpp 只检查/记录；
- KubeJS 脚本由用户手动放置，dpp 只检查/记录；
- Folia 适配器标记 experimental，后置；
- 如果未来要管理适配器 jar，必须新增独立的 `host component` 领域和威胁模型，不能偷偷塞进数据包 solver。

## Consequences

- 数据包安装不会被宿主插件管理绑架；纯数据包用户零额外依赖。
- 宿主环境不能像数据包一样完全由 dpp 复现；`host.lock` 是记录/预检，不是安装保证。
- 需要明确哪些命令在缺少 server root 或无法检测宿主时失败/警告。
- “dpp-adapter-paper 是 dpp 组件”只能指 companion，不是 solver 包。

## Alternatives considered

- **把插件也纳入包管理**：违反边界，且需要处理 jar、插件依赖、加载器、版本和卸载，产品膨胀。
- **完全忽略宿主**：会让依赖宿主的数据包装完才失败，失去“环境可复现”的价值。
- **只按插件文件名判断能力**：文件名可改、插件可禁用、版本不可靠；只能作为 observed 信息。

## References

- [KubeJS 官方](https://kubejs.com/)
- [KubeJS GitHub](https://github.com/KubeJS-Mods/KubeJS)
- [Paper Datapack Discovery](https://docs.papermc.io/paper/dev/lifecycle/datapacks/index.md)
- [Folia Overview](https://docs.papermc.io/folia/reference/overview/)
- [设计文档](../design.md)
