# 可行性调研

> 这是一份调研记录，不是接口契约。命令和数据模型以 [design.md](design.md)、[apply.md](apply.md)、[load-order.md](load-order.md) 为准。

## 结论

**可行，但价值排序要调整。**

真正有价值的是：

1. 按 pack format 选对版本；
2. 可复现的 lock + sha512；
3. 原子安装/回滚；
4. 明确区分包层与激活层；
5. 加载顺序的诚实报告。

依赖求解器（pubgrub）保留，但不应作为第一卖点：Modrinth 上多数 datapack 没有依赖声明，solver 早期大部分时间在无依赖图上空转。`dpp build-pack` 和私有静态源是让依赖元数据长出来的关键。

## Modrinth

### 项目类型不是 datapack

实测 `project_type:datapack` facet 的前 100 条命中，`project_type` 全部返回 `mod`。Modrinth 的 datapack 是 **version loader / category**，不是 primary project type。

因此：

- 搜索用 `categories:datapack`，不要用 `project_type:datapack`。
- 安装必须选择 `version.loaders` 包含 `datapack` 的版本。
- 文件必须是 `.zip`，且 zip 根目录有 `pack.mcmeta`；`.jar` 是 mod 版本，必须拒绝。
- 例：Incendium 的 datapack 版本 `loaders = ["datapack"]`，文件为 `.zip`；同一项目另有 mod 版本。

### 身份、hash、依赖

- 长期身份用 `project_id` / `version_id`；slug 会变。
- 文件 hash 只有 `sha512` + `sha1`，没有 sha256。lock 主校验用 sha512。
- 依赖字段：`required` / `optional` / `incompatible` / `embedded`。`embedded` 不单独安装。
- API 要求唯一 `User-Agent`；当前限制 300 请求/分钟/IP；需要缓存和退避。
- 依赖元数据稀疏。抽样 20 个下载量最高的 datapack 项目（约 420 个 datapack 版本）：
  - 5 个项目出现 `required` 依赖；
  - 96 个版本带任何依赖声明；
  - 绝大多数 `dependencies = []`。

这支持“先做包层，再做 solver”的顺序。

## 版本与 pack format

Minecraft 26.x 使用 calver；数据包兼容单位是 pack format，不是 MC 版本字符串。

| MC | data pack format |
|---|---|
| 26.3 | 121.0 |
| 26.2 | 107.1 |
| 26.1 | 101.1 |
| 1.21.11 | 94.1 |
| 1.21.9–1.21.10 | 88.0 |

26.3/26.2/26.1 的值已用 Mojang 官方 client jar 内 `version.json` 的 `pack_version.data_major/data_minor` 复核。

格式 schema：

- 1.21.9+（data format ≥82）：`min_format` / `max_format`，支持 major.minor。
- 更早：`pack_format` / `supported_formats`；只为兼容 <82 保留。
- `pack.mcmeta` 必须在 zip/文件夹根目录；外面套一层目录的包游戏不认。
- 1.13 才有数据包；dpp 最低支持 1.13（format 4），不支持 1.12。

目标格式来源优先级：

1. 本地 client/server jar 的 `version.json`（权威）；
2. dpp 随版本发布的内置映射表；
3. 世界清单显式覆盖。

26.1 世界存储把 `region/entities/poi/data` 移到了 `dimensions/...`，但 `world/datapacks/` 仍在世界根目录。目录设计不需要为 26.x 推翻，但不要假设其他目录没变。

## `/reload` 与激活

这是设计里最容易过度承诺的部分。完整决策见 [apply.md](apply.md)，关键事实：

- `/reload` 权限等级 2；单人未开作弊时权限 0，不能执行。
- `/reload` 是全局的；`/datapack enable/disable` 也会触发全量 reload；没有单包 reload。
- `damage_type`、`dimension`、`dimension_type`、`enchantment`、`jukebox_song`、`trim_material`、`trim_pattern`、`worldgen` 等需要重新进入世界/重启，不能靠 `/reload`。
- worldgen 只影响新区块；已有区块不会被重写。
- `pack.mcmeta.features.enabled` 需要世界创建时启用；已有世界不能靠重启补。
- 无效数据可能保留旧数据；26.3 起某些坏 JSON 甚至阻止世界加载/reload。
- `#minecraft:load` 会在每次 reload 时重新执行，可能有副作用。
- 纯原版没有 API 查询内存注册表/函数内容；`status` 只能做部署状态。

## pacman 参考

| 维度 | pacman / libalpm | dpp 采用 | 不采用 |
|---|---|---|---|
| 元数据 | sync DB | 源索引缓存 + API 缓存 | tar DB |
| 本地 DB | `desc` + `files` + `mtree` | `index.toml` + 每文件 sha512 | uid/gid |
| 求解 | satisfier 扫描，无 SAT solver | pubgrub + 显式 source priority | 扫到谁算谁 |
| 事务 | prepare/commit | journal + staging + rename | 逐文件覆盖 |
| 缓存 | `/var/cache/pacman/pkg` | 内容寻址缓存 | 自动删旧版本 |
| 孤儿 | `pacman -Qdt` | `dpp remove` 清理无用依赖 | — |
| 文件冲突 | 文件系统 ownership | `data/` 资源路径重叠检查 | 绝对路径冲突 |
| 配置保护 | `.pacnew` | 本地修改检测 + 备份 | — |
| 签名 | GPG | sha512 pinning；私有源可选 minisign（二期） | MVP 强上 GPG |
| 锁 | `db.lck` | `session.lock` + `.dpp/txn.lock` | 文件存在判断 |
| hooks | alpm hooks | 不做安装脚本/hook | — |

pacman 的教训：事务前算完、校验完，再落地；本地数据库要能验证文件完整性。dpp 的 `check` 对应这两点。

## 竞品/先例

- **packwiz**：modpack 导向；datapack 支持依赖 loader mod 的 `datapack-folder`，不是纯原版世界管理器。
- **DPM**（npm）：datapack library/package manager，周下载约 50，未形成生态。
- **Vanilla Tweaks installer**： curated 包安装器，无依赖/锁文件/求解。

“纯原版 + world lock + 原子安装 + 激活语义”目前是空位。

## 风险

1. **元数据稀疏**：solver 早期收益有限，需要 `build-pack`/私有源引导。
2. **reload 语义**：全局、可能失败、保留旧数据、实验性资源不 reload。
3. **加载顺序**：权威在 `level.dat`；前缀只是提示；重命名改变身份。
4. **feature flag**：只能新世界；不能靠重启。
5. **worldgen**：只影响新区块；已有世界有硬边界。
6. **26.3 严格加载**：坏 JSON 可能阻止世界启动；需要语法检查、快照、回滚。
7. **可观测性**：纯原版无法读取内存资源版本；status 只能报告 applied/observed。
8. **RCON**：默认关闭、明文、需要重启服务端启用；单人用不了。
9. **快照范围**：不是世界备份；不能撤销运行时残留。
10. **安全**：Modrinth sha512 不是签名；私有源需要自己的签名方案（二期）。
11. **跨平台**：Windows 文件锁/rename/大小写；长路径。
12. **加载顺序写入**：`level.dat` NBT 往返风险高，不进 MVP。

## MVP

### 必须有

- `init`：读 `level.dat`、MC 版本、pack format，生成世界清单。
- Modrinth 源：搜索、datapack loader 过滤、sha512、缓存、限流。
- Pack Inspector：zip 安全、`pack.mcmeta` 双 schema、文件清单、路径重叠。
- lock/index + `install/remove/list/check`。
- staging + journal + 原子提交/回滚。
- `status` 五态 + `apply --dry-run` 分类。
- `apply` 对纯 reloadable 变更可选 RCON `/reload`；其他情况打印指引。
- `snapshot` 包状态快照（managed-only 默认）。
- `build-pack` + 静态源索引。
- 两阶段卸载（`uninstall_hook`）。
- JSON 语法检查 + apply 前自动包层快照。

### 暂不做

- TUI。
- 签名/透明日志。
- 写 `level.dat` 排序。
- RCON 重排。
- 自动重启服务端。
- MSMP 适配。
- 资源包、mod、workspace、features、patch。
- 世界数据备份。

## 建议顺序

1. 包层事务：lock/index、staging、journal、回滚。
2. Modrinth 客户端 + pack.mcmeta 校验。
3. `status` / `apply --dry-run` 分类。
4. RCON reload（opt-in）+ 日志/包列表验证。
5. 两阶段卸载 + snapshot。
6. build-pack + 私有源。
7. pubgrub 求解 + 冲突校验（在候选模型稳定之后）。
8. 顺序报告/`dpp order`。
9. 二期：签名、MSMP、TUI、level.dat 写入。

## 参考

- Modrinth API：https://docs.modrinth.com/api/
- Modrinth versions：https://docs.modrinth.com/api/operations/getprojectversions/
- Modrinth search：https://docs.modrinth.com/api/operations/searchprojects/
- Data pack：https://minecraft.wiki/w/Data_pack
- pack.mcmeta：https://minecraft.wiki/w/Pack.mcmeta
- Version.json：https://minecraft.wiki/w/Version.json
- Java Edition level format：https://minecraft.wiki/w/Java_Edition_level_format
- `/reload`：https://minecraft.wiki/w/Commands/reload
- 中文 `/reload`：https://zh.minecraft.wiki/w/命令/reload
- `/datapack`：https://minecraft.wiki/w/Commands/datapack
- Permission level：https://minecraft.wiki/w/Permission_level
- `#minecraft:load`：https://minecraft.wiki/w/Function_(Java_Edition)
- RCON / server.properties：https://minecraft.wiki/w/Server.properties
- MSMP：https://minecraft.wiki/w/Minecraft_Server_Management_Protocol
- 26.3 严格加载：https://mojira.dev/MC-310325
- reload 失败：https://mojira.dev/MC-277148
- worldgen 只影响新区块：https://www.fumora.net/lab/datapack-worldgen-server-basics
- pubgrub：https://docs.rs/pubgrub/latest/pubgrub/
- pacman（libalpm 内部）：https://pyshine.com/pacman/
- ArchWiki pacman：https://wiki.archlinux.org/title/Pacman
- packwiz datapack 选项：https://packwiz.infra.link/reference/additional-options/
- DPM：https://www.npmjs.com/package/@bigstonedevelopment/dpm
