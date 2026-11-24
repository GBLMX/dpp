# 激活：apply、reload 与状态

## 1. 为什么分离

Minecraft 没有“安装后自动生效”的包管理器语义。磁盘上的 `datapacks/` 只有在以下时机才会进入运行层：

- 世界/服务端启动；
- `/reload`；
- `/datapack enable` / `/datapack disable`（也会触发全量 reload）；
- 退出世界后重新进入（单人）。

因此 dpp 把变更拆成两个阶段：

- `install/update/remove`：只改包层。
- `apply`：提交 staged plan，并给出/执行激活动作。

`apply` 不能保证运行层一定变成新版本；它只能执行动作、记录结果、提示验证。

## 2. `/reload` 的真实边界

### 2.1 权限与单人限制

- `/reload` 需要权限等级 2。
- 单人世界开启作弊时，host 权限为 4，可以执行。
- 单人世界未开启作弊时，host 权限为 0，**不能执行 `/reload`**。
- Java 26.2 起，host 可以在 World Options 中切换 `Allow Commands`（世界是否允许命令）。注意：Open to LAN 的 “Settings for Other Players → Allow Commands” 不影响 host，二者不要混用。
- 专用服务器：控制台/RCON 可执行；玩家需要 op 等级 2+。
- dpp 不自动改 `level.dat` 来开作弊；只打印指引。

### 2.2 全量，不是单包

- `/reload` 永远是全局数据包 reload；没有单包增量 reload。
- `/datapack enable` / `disable` 也会 reload 全部数据包。
- 不存在“只重启某个包”的机制。

### 2.3 可重载范围

按当前 Wiki，典型可 reload 的资源包括：

- `function/`
- `tags/`
- `loot_table/`
- `recipe/`
- `advancement/`
- `predicate/`
- `item_modifier/`
- `structure/`

实验性/动态注册项不能靠 `/reload`，必须重新进入世界（单人）或重启服务端（专用服）。当前 Wiki 列出的例子包括：

- `damage_type`
- `dimension`
- `dimension_type`
- `enchantment`
- `jukebox_song`
- `trim_material`
- `trim_pattern`
- `worldgen`

这个集合会随版本变化。dpp 必须维护版本化分类表；未知路径保守判为 restart/rejoin，不要猜“可以 reload”。

### 2.4 Worldgen 只影响新区块

Worldgen 变更在区块生成时生效：

- 已有区块不会被重写；
- 新生成的区块使用新数据；
- 已有世界可能出现旧地形与新地形的硬边界；
- 重启也不能让旧区块重新生成。

因此 worldgen 是独立分类 `new_chunks_only`，不能和普通 reload 混在一起承诺“已完整生效”。

### 2.5 Feature flag 只能创建世界时启用

`pack.mcmeta` 的 `features.enabled` 需要世界在创建时启用对应 feature flag。已有世界缺少该 flag 时：

- `/reload` 不生效；
- 重启也不一定解决；
- `/datapack enable` 会失败。

dpp 读取 `level.dat` 的 `enabled_features` 并与包的 `pack.mcmeta.features.enabled` 对比；缺失时判为 `new_world_required`，拒绝把“重启”当作解决方案。

### 2.6 无效数据与 reload 失败

- 如果数据包含有无效数据，游戏会保留旧数据；reload 的结果在不同版本文档中存在差异（有“总是成功”和“执行失败”两种说法），不要只依赖命令返回码。
- 26.3 起，`advancement`、`loot_table`、`recipe`、`number_provider`、`slot_source`、`predicate`、`item_modifier` 等文件出错可能直接阻止世界加载或 reload。
- 服务端日志可能出现 `Reload failed; keeping old data`。
- dpp 在 install/apply 前做 JSON 语法检查；语义错误无法完全预检，必须提示风险并在 apply 前自动创建包层快照。

### 2.7 `#minecraft:load` 会重新执行

数据包 reload 时，`#minecraft:load` 函数标签会重新执行。它可能有副作用：

- 重建/重置记分板；
- 生成实体；
- 修改 storage；
- 重设 gamerule；
- 触发其他初始化逻辑。

`apply` 必须提示用户检查 load 函数是否幂等。

### 2.8 新增包是否自动启用

`/reload` 可能重新扫描 `datapacks/`，但“新增包一定被启用”不是稳定契约。dpp 必须：

1. 发送 `/reload`；
2. 用 `/datapack list enabled` 验证 `file/<install_name>` 是否在列表；
3. 缺失时打印 `/datapack enable "file/<install_name>"`，由用户决定是否执行。

不要自动 enable/reorder：那会改变加载顺序并触发额外 reload。顺序问题见 [load-order.md](load-order.md)。

## 3. 变更分类

| 类别 | 识别 | 动作 | 备注 |
|---|---|---|---|
| `reloadable` | function/tags/loot_table/recipe/advancement/predicate/item_modifier/structure | `/reload` | load 函数会重跑；运行时状态保留 |
| `experimental_registry` | damage_type/dimension/dimension_type/enchantment/jukebox_song/trim_material/trim_pattern/worldgen 等 | 单人退出重进；专用服重启 | `/reload` 不生效 |
| `worldgen_only` | worldgen/biome/structure/dimension | 重启/重进 | 只影响新区块 |
| `new_world_required` | `pack.mcmeta.features.enabled` 不在世界 `enabled_features` | 需要新世界 | 重启无效 |
| `pack_add_remove` | 对比 lock/index | `/reload` 后验证 enabled 列表 | 缺失则打印 enable 命令，不自动重排 |
| `unknown` | 未在版本化分类表中 | 保守 restart/rejoin | 不猜 reload |
| `invalid` | JSON 语法错误 / 已知语义风险 | 阻止或要求 `--force` | 26.3 某些类型可阻止世界加载 |

变更同时包含多类时，取最高要求：`new_world_required` > `restart/rejoin` > `worldgen_only` > `reloadable`。

## 4. 状态模型

| 状态 | 来源 | 含义 |
|---|---|---|
| `desired` | `dpp.toml` + `lock.toml` | 期望的精确包集合 |
| `staged` | `.dpp/staging/plan.toml` | 运行中安装但尚未提交 |
| `committed` | `datapacks/` + `index.toml` | 磁盘上已落地的包 |
| `applied` | `.dpp/applied.toml` | dpp 上次成功发送 reload / 记录手动确认 |
| `observed` | RCON / `session.lock` / 日志 | 服务端运行状态、MC 版本、enabled 列表、reload 错误 |
| `unknown` | — | 内存资源内容，纯原版不可查询 |

`applied.toml` 示例：

```toml
lock_hash = "sha512:..."
applied_at = "2026-11-24T12:00:00Z"
method = "rcon"          # rcon | manual | next_start
verification = "pack_list_ok"  # sent | log_ok | pack_list_ok | unverified
```

`dpp status` 输出示例：

```text
Desired:   lock 8f3a...  (26.3, format 121.0)
Committed: 8f3a...       (matches)
Staged:    none
Applied:   8f3a...       via rcon, pack list verified
Observed:  server running, 12/12 packs enabled, no foreign packs
Unknown:   in-memory resource contents (vanilla has no API)
Pending:   none
```

允许的结论只有：

- `reload sent`：命令已发送，未验证内容；
- `reload verified`：日志或 pack 列表检查通过；
- `unverified`：无法检查；
- `manual required`：单人作弊关闭、RCON 关闭、需要重启/重进；
- `failed`：日志明确报错，运行层仍是旧数据。

## 5. `apply` 流程

```text
dpp apply [--dry-run] [--rcon] [--log PATH] [--yes] [--rollback]
dpp apply --manual
dpp apply --abort
```

### 5.1 世界关闭

- 若有 staged plan：提交到 `datapacks/`，更新 lock/index；
- 标记 `method = "next_start"`；
- 不做 reload；
- 提示“下次启动生效”。

### 5.2 世界运行中

1. 若有 staged plan，先原子提交；
2. 计算变更分类；
3. `--dry-run` 时只打印计划；
4. 纯 reloadable + RCON 可用：发送 `reload`；
5. 纯 reloadable + RCON 不可用：打印控制台命令/单人操作；
6. 需要重启/重进：打印步骤，不自动重启；
7. 需要新世界：拒绝，并指出缺失的 feature flag；
8. 验证：可选 `/datapack list enabled`、日志检查；通过才写 `applied`。

### 5.3 单人 vs 专用服

| 环境 | 可行路径 |
|---|---|
| 单人，作弊开启 | `/reload`，或退出重进 |
| 单人，作弊关闭 | 退出到标题并重进；26.2+ 可在 World Options 开 Allow Commands |
| 专用服，RCON 开启 | dpp 发送 `reload`；可选验证 |
| 专用服，RCON 关闭 | 用户在控制台输入 `reload` |
| 专用服需要重启 | 用户自行重启；dpp 不执行 systemctl/kill |

### 5.4 RCON

- 默认关闭；`enable-rcon=true` + 非空 `rcon.password` 才启动。
- 明文协议，只建议 localhost；不要在公网使用。
- 密码从环境变量/系统钥匙串读取；不写入 `dpp.toml`/lock，不打印。
- RCON 只用于 `/reload`、`/datapack list`、`/list` 等只读/激活命令；不自动执行 `stop` 或重启。
- RCON 失败不写 applied；状态保持 pending/manual。

### 5.5 MSMP

Minecraft Server Management Protocol 是 JSON-RPC over WebSocket，可查状态、保存、停止、系统消息、玩家/白名单/gamerule 和通知。

它**没有**执行任意命令或 reload 的方法，因此不能替代 RCON。未来可以作为：

- 服务端运行/保存/世界升级状态检测；
- 自动标记 `next_start`；
- 更安全的 TLS 管理通道。

不进 MVP。

### 5.6 失败与回滚

- reload 失败/日志报错：不写 applied；提示 `dpp apply --rollback`。
- 世界无法启动：需要停服后用 `dpp snapshot restore <pre-apply>` 回滚包层，再启动。
- 自动 pre-apply 快照只覆盖包层，不覆盖记分板/实体/region；不能撤销运行时残留。

## 6. 卸载 hook

`uninstall_hook` 是作者提供的清理函数路径，不是脚本。dpp 只打印，不执行。

正确流程：

1. `dpp remove <pkg>` 检测到 hook → **staging 删除**，包仍安装且可运行；
2. 提示用户运行 `/function ns:cleanup`；包被禁用时先 enable；
3. 用户执行完；
4. `dpp apply` 才真正删除并 reload；
5. `dpp remove --force` 可跳过清理，但标记 `cleanup=skipped`，status 持续警告。

不能在删除后提示函数：包没了，函数也没了。

运行时残留不在 dpp 自动管理范围：记分板、实体 tag、storage、schedule、bossbar、team、gamerule、已生成区块。dpp 可以提供静态扫描清单，但不承诺清理。

## 7. 快照与 apply

`snapshot` 只保存包层：

- `dpp.toml`
- `lock.toml`
- `index.toml`
- managed `datapacks/`（硬链接/reflink 或复制）
- 文件 sha512 清单
- `level.dat` 的 `DataPacks.Enabled/Disabled` 只读快照（用于警告）

不保存：

- `applied.toml`（restore 后必须重新标记 pending）
- staged plan（有 plan 时禁止快照）
- 内存状态、scoreboard、region、实体数据

restore 后：

- 世界关闭 → 下次启动生效；
- 世界运行 → 需要 `dpp apply`；
- 清空 applied marker；
- 跨 MC 版本、feature flag 缺失、install name 顺序不一致时警告/拒绝。

明确：**snapshot 不是世界备份**。

## 8. 不做什么

- 不自动重启服务端。
- 不自动执行 `uninstall_hook` 或任何数据包内容。
- 不自动写 `level.dat`。
- 不声称能读取内存注册表/函数内容。
- 不把 worldgen 变更描述成“重启后完整生效”。
- 不在缺 feature flag 时建议“重启试试”。

## 参考

- `/reload`：https://minecraft.wiki/w/Commands/reload
- 中文 `/reload`（实验性注册项列表）：https://zh.minecraft.wiki/w/命令/reload
- `/datapack`：https://minecraft.wiki/w/Commands/datapack
- 权限等级：https://minecraft.wiki/w/Permission_level
- `#minecraft:load`：https://minecraft.wiki/w/Function_(Java_Edition)
- World Options / Allow Commands：https://minecraft.wiki/w/World_Options
- `server.properties` / RCON：https://minecraft.wiki/w/Server.properties
- MSMP：https://minecraft.wiki/w/Minecraft_Server_Management_Protocol
- 26.3 严格数据包加载：https://mojira.dev/MC-310325
- reload 失败保留旧数据：https://mojira.dev/MC-277148
- worldgen 只影响新区块：https://www.fumora.net/lab/datapack-worldgen-server-basics
