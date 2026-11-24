# 加载顺序

## 结论

**MVP：加载顺序是 advisory，不是 dpp 能强制保证的状态。**

- 权威顺序在 `level.dat` 的 `Data.DataPacks.Enabled`，或游戏内 `/datapack enable ... first|last|before|after`。
- 文件夹前缀 `0010_` 只是首次安装时的排序提示；已有世界存储顺序后，重命名不保证改变实际顺序。
- dpp 在 MVP 不写 `level.dat`，不自动重命名，不自动 `/datapack enable` 重排。
- `dpp order` 只读：报告期望/实际顺序，打印原生命令；由用户在游戏内执行。

这样避免“看似自动排序，实际没生效”的假保证。

## 权威来源

Minecraft 的加载顺序：

- 存在 `level.dat` 的 `Data.DataPacks.Enabled`。
- 可以通过 `/datapack enable <name> [first|last|before <existing>|after <existing>]` 调整。
- `/datapack enable/disable` 会重新加载全部数据包。
- 世界数据包在命令中的名字是 `file/<文件夹或 zip 名>`，含特殊字符时需要引号。
- `DataPacks.Disabled` 记录被禁用的包名。

因此：

- 前缀只是“默认发现/插入”的提示，不是权威排序机制。
- 重命名会改变 `file/<name>` 身份；`Disabled` 里旧名字不会自动跟随，可能导致包被重新启用。
- 已有世界如果用户在游戏内调整过顺序，前缀重命名不会自动纠正。

## MVP 决策

| 操作 | MVP | 说明 |
|---|---|---|
| 首次安装生成 `{weight:04}_{id}` 前缀 | ✅ | 只作为初始提示 |
| 更新时保持 `install_name` 不变 | ✅ | 避免身份变化和 `Disabled` 状态丢失 |
| 权重变化自动重命名 | ❌ | 不做；`check` 报 drift |
| 写 `level.dat` 的 `DataPacks.Enabled` | ❌ | 权威但风险高；二期 opt-in |
| RCON 自动 `datapack enable ... before/after` | ❌ | 每条命令触发 reload；二期 opt-in |
| `dpp order` 只读报告 + 打印命令 | ✅ | 用户手动执行 |
| `dpp apply` 自动 enable/reorder | ❌ | 只验证 `/datapack list enabled`，缺失则打印命令 |
| `dpp check` 报告顺序偏差 | ✅ | 不自动修 |

### `load_weight` 的含义

- 数值越小越靠前。
- 同权重按 `id` / `install_name` 稳定排序。
- 用于：首次安装前缀、覆盖/冲突警告中的“谁在后”、推荐顺序。
- 不用于：强制运行层顺序、自动重排、保证 `/reload` 后的顺序。

### `install_name`

```text
0010_terralith
0020_incendium
```

- 只在首次安装时根据 `load_weight` 生成。
- 更新版本不改变名称。
- 权重变化不自动改名；这是有意的：改名会改变 `file/<name>` 身份，并可能重置 `Disabled`。
- 如果用户确实要重排，走 `dpp order` 打印的原生命令，或二期显式迁移工具。

## `dpp order`

```text
dpp order [--json] [--print-commands]
```

只读行为：

1. 期望顺序：按 `(load_weight, id, install_name)` 排序。
2. 实际顺序：
   - 世界关闭：读 `level.dat` 的 `DataPacks.Enabled`（最后一次保存状态）。
   - 世界运行：优先 `/datapack list enabled`；如果该输出不保证顺序，标记 `unknown`，不要猜。
3. 输出差异和原生命令，例如：

```text
Expected: 0010_terralith, 0020_incendium
Observed: 0020_incendium, 0010_terralith
Action (run in game/console):
/datapack enable "file/0010_terralith" before "file/0020_incendium"
```

注意：

- `/datapack enable` 需要权限 2，并触发全量 reload。
- 对已启用的包执行 `enable` 可能被游戏拒绝；不同版本行为需按目标版本验证。
- 不要对 `Disabled` 的包自动 enable；先让用户确认。
- 执行后 `level.dat` 由游戏写入；dpp 不碰 NBT。

## 二期选项（不在 MVP）

### A. `dpp order --write-level-dat`

适用：世界关闭、需要确定性的离线顺序。

要求：

- 先备份 `level.dat` 和 `level.dat_old`。
- 解析 gzip NBT，只改 `Data.DataPacks.Enabled`，保留所有未知 tag/字段。
- 原子写临时文件 → rename；再读回校验。
- 检测世界未打开（`session.lock`）。
- 有完整测试：未知字段保真、快照、崩溃恢复、不同 MC 版本。
- 失败必须可回滚；不能把 `level.dat` 写坏。

风险：NBT 往返保真、MC schema 变化、用户并发启动、跨版本差异。默认关闭，显式 opt-in。

### B. `dpp order --rcon`

适用：世界运行、已有 RCON。

要求：

- 权限 2+；只操作已启用包。
- 每条 `/datapack enable ... before/after` 都会触发一次全量 reload，包多时开销大。
- 失败时可能停在部分顺序；需要重新计算并继续，或提示用户手动处理。
- 不自动 enable 被禁用的包。

风险：reload 抖动、部分成功、玩家在线时的卡顿。默认关闭，显式 opt-in。

### C. 保持 advisory

只打印命令，由用户在游戏内执行。最安全，也是 MVP 的选择。

## 与 `apply` 的关系

- `dpp apply` 会验证 `/datapack list enabled` 是否包含期望的 `file/<install_name>`。
- 缺失时打印 `/datapack enable "file/<name>"`，**不自动执行**。
- 顺序偏差不阻塞 apply；`dpp status`/`check` 单独报告。
- 如果用户手动执行了 `/datapack enable`，它会触发 reload；dpp 可通过 `dpp apply --manual` 或下一次 `status` 记录。

## 测试清单

- 新世界首次安装：前缀是否只影响初始发现。
- 已有世界、用户游戏内调整顺序后：`dpp order` 是否正确报告 drift。
- 权重变化不重命名：`Disabled` 状态是否保留。
- 重命名实验：确认 `file/<name>` 身份变化和 `Disabled` 影响，仅用于文档/二期测试。
- 世界运行 vs 关闭：实际顺序读取来源是否正确标注。
- `/datapack list enabled` 是否提供顺序；若不提供，不得假装知道。
- RCON 重排（二期）：权限、reload 次数、失败恢复。

## 参考

- Data pack（加载顺序、覆盖/合并）：https://minecraft.wiki/w/Data_pack
- `/datapack`（`file/<name>`、enable before/after、reload）：https://minecraft.wiki/w/Commands/datapack
- level.dat（`DataPacks.Enabled/Disabled`）：https://minecraft.wiki/w/Java_Edition_level_format
- 权限等级：https://minecraft.wiki/w/Permission_level
