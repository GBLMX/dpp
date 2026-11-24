# Minecraft 数据包生态与工具链调研

> 研究记录，不是接口契约。版本/激活语义以 [design.md](design.md)、[apply.md](apply.md)、[load-order.md](load-order.md) 为准。

## 结论

dpp 的定位是**数据包消费者/部署管理器**，不是作者工具链。beet、Spyglass、Misode、GameTest 解决“写和测”，dpp 解决“锁定版本、安装、激活、回滚”。

生态里最接近 dpp 的是 **Smithed**：它有 pack 数据库、launcher、兼容性 conventions、Weld 合并工具和 v2 API。公开材料里没有看到 world 级 lock/原子安装/`apply` 语义；Weld 的“合并成一个 zip”也与 dpp 保持包身份的目标不同。

## 1. 版本与 pack format

| MC | data pack format |
|---|---|
| 26.3 | 121.0 |
| 26.2 | 107.1 |
| 26.1 | 101.1 |
| 1.21.11 | 94.1 |
| 1.21.9–1.21.10 | 88.0 |
| 1.21.7–1.21.8 | 81 |
| 1.21.6 | 80 |
| 1.21.5 | 71 |
| 1.21.4 | 61 |
| 1.21.2–1.21.3 | 57 |
| 1.21–1.21.1 | 48 |
| 1.20.5–1.20.6 | 41 |
| 1.20.2 | 18 |
| 1.20–1.20.1 | 15 |
| 1.19.4 | 12 |
| 1.19–1.19.3 | 10 |
| 1.18.2 | 9 |
| 1.13–1.14.4 | 4 |

- 26.x 使用 calver；数据包从 1.13 才开始，dpp 最低支持 1.13（format 4）。
- 1.21.9+（data format ≥82）：`pack.mcmeta` 用 `min_format`/`max_format`，支持 major.minor。
- 更早的包用 `pack_format` / `supported_formats`；只为兼容 <82 保留。
- 26.3/26.2/26.1 的值已用 Mojang 官方 client jar 内 `version.json` 的 `pack_version.data_major/data_minor` 复核。

`pack.mcmeta` 规则：

- 必须在 zip/文件夹**根目录**；外面套一层目录的包游戏不认。
- `overlays`：按 format 选择子目录，原生做多版本适配。
- `filter`：用正则屏蔽低优先级包的文件；是原生冲突解决机制。
- `features`：实验特性 flags；已有世界缺少 flag 时不能靠重启启用。
- 1.21 起数据目录改为单数：`function/`、`tags/function/`、`loot_table/`、`recipe/`、`advancement/`、`predicate/`、`item_modifier/`、`structure/`。
- 26.1 世界存储把 `region/entities/poi/data` 移到 `dimensions/...`，但 `world/datapacks/` 仍在世界根目录。

## 2. 激活语义

关键事实（细节见 [apply.md](apply.md)）：

- `/reload` 权限等级 2；单人未开作弊时权限 0，不能执行。
- `/reload` 全局；`/datapack enable/disable` 也触发全量 reload；没有单包 reload。
- `damage_type`、`dimension`、`dimension_type`、`enchantment`、`jukebox_song`、`trim_material`、`trim_pattern`、`worldgen` 等需要重进世界/重启，不能靠 `/reload`。
- worldgen 只影响新区块；已有区块不重写。
- `pack.mcmeta.features.enabled` 需要世界创建时启用；已有世界不能靠重启补。
- 无效数据可能保留旧数据；26.3 起某些坏 JSON 会阻止世界加载/reload。
- `#minecraft:load` 在每次 reload 时重新执行，可能有副作用。
- 世界包在命令中的名字是 `file/<目录名>`；rename 会改变身份。
- 加载顺序权威在 `level.dat` 的 `DataPacks.Enabled` / `/datapack enable ... before|after`，不是文件夹前缀。

这些限制决定了 dpp 的 `apply` 只能做“分类 + 指引 + 记录”，不能声称运行层已同步。

## 3. Smithed

### 生态

Smithed 官方定位：

- pack 数据库；
- launcher；
- 一套 conventions（pack id、命名空间、enabled tag、tag specification 等），提高 pack 互操作性；
- Weld：把多个 pack 合并成一个 zip 的工具，实现 vanilla 合并规则 + `__smithed__` 自定义规则。

Weld：

- 安装：`pipx install smithed`，命令 `weld`。
- Vanilla 合并规则：`tag` 的 `values` 忽略重复合并，顺序按 load order；model/language/font/atlas 等按各自规则合并。
- 自定义：JSON 文件可写 `__smithed__`（`override`、`rules`、`priority`、conditions）；只有 Weld 处理，vanilla 不认。
- `pack.mcmeta` 也可写 `__smithed__.override`，用于 Smithed bundle 的破坏性覆盖。

`pack.mcmeta.id`：

- Smithed convention 建议顶级 `id` 字段，稳定、唯一，用于识别 pack。
- Weld 用 id 做 priority/conditions；这比目录名可靠。

Pack enabled tag：

- 用 predicate tag `#ns:installed` 表示包已安装/启用；
- 用 context int provider tags `#ns:major/minor/patch` 暴露版本；
- consumer 用 predicate/number provider 检查，避免缺失依赖导致 load error。
- 这是运行时探测约定；dpp 可以作为可选检查，但不能替代文件/哈希校验。

### Smithed API v2

实际探测：

```http
GET https://api.smithed.dev/v2/packs
GET https://api.smithed.dev/v2/packs/{id}
```

响应结构（节选）：

```json
{
  "id": "myriad",
  "display": {
    "name": "Myriad",
    "description": "...",
    "hidden": false
  },
  "versions": [
    {
      "name": "1.0.1",
      "supports": ["1.18.2"],
      "downloads": {
        "datapack": "https://github.com/.../TCC.1.0.1.Datapack.zip",
        "resourcepack": "https://github.com/.../TCC.1.0.1.Resourcepack.zip"
      },
      "dependencies": [
        { "id": "5G34ouRxSiXQ3X3odZHc", "version": "0.0.1" }
      ]
    }
  ]
}
```

观察：

- 有 pack id、版本名、支持 MC 版本、datapack/resourcepack 下载 URL、依赖（精确 `id + version`）。
- 在 v2 `/packs/{id}` 响应中**没有看到 checksum/hash 字段**；不能把 Smithed 当校验和源。
- 依赖是精确版本，不是范围；dpp 的 solver 可以把它转成 `=version` 约束。
- 资源包下载也在同一 version 里；dpp 只取 datapack，资源包字段忽略。

对 dpp 的启示：

- **二期可加 `smithed` source adapter**：拉 pack/version/dependencies，映射到 dpp 候选；下载后本地计算 sha512 并 pin；因为源没有 checksum，首次信任要显式标记 `verification = "computed"`。
- 不能把 Smithed 的 `dependencies` 当完整图；它可能只覆盖 Smithed 生态内的依赖。
- `pack.mcmeta.id` 可作为身份 hint；但 lock 的 `source + project_id + version_id` 仍是权威。
- Weld 的合并规则可用于 dpp 的冲突分析（tags 合并、JSON override），但 dpp 不应默认把多个包 weld 成一个 zip：那会丢失包身份、更新粒度和回滚能力。

## 4. 作者工具链

### beet

- Python 写的 Minecraft pack 开发工具包；`beet.json` 描述项目，`beet build` 构建，`beet watch` 监听，`beet build --link "World"` 链接到存档。
- 定位是 authoring layer：把生成器、预处理器、框架整合到一个 pipeline；输出仍是标准数据包/资源包。
- 对 dpp：beet 负责“产出 zip”，dpp 负责“消费 zip”；`dpp build-pack` 可在 beet 输出后做 manifest/pack.mcmeta/哈希校验。

### Spyglass

- 数据包 LSP/编辑器工具：补全、实时诊断、语义着色。
- 支持 `.mcdoc`、`.mcfunction`、`.mcmeta`、`.snbt`、数据包/资源包 JSON。
- 通过 `npm i -g @spyglassmc/language-server` 安装；VS Code / Open VSX 扩展。
- 对 dpp：CI 可把 Spyglass 作为独立 lint 步骤；dpp 不应把 Node LSP 嵌进 Rust 二进制。MVP 的 `check` 只做 JSON 语法/结构/路径/哈希检查。

### Misode generators

- `misode.github.io` 提供 pack.mcmeta、worldgen、loot table 等生成器。
- 对 dpp：作者用它生成 JSON，`dpp build-pack` 负责校验和打包；dpp 不重复实现生成器。

### GameTest

- Java 版 E2E 测试框架；1.21.5 起可用于数据包。
- 数据包内定义 `test_instance`、`test_environment`；测试结构是 `.nbt`。
- `/test (run|runmultiple|runclosest|...)` 执行；test instance block 可编辑结构。
- `server.jar` 内置入口：

```text
java -DbundlerMainClass="net.minecraft.gametest.Main" -jar server.jar \
  --packs <path> \
  --report <junit.xml> \
  --tests <selection> \
  --universe <path> \
  --verify <bool>
```

- `--packs` 接受含 `pack.mcmeta` 的 zip/文件夹；`--report` 输出 JUnit 风格 XML；退出码：全过 0，失败数为正，崩溃 1，未开始 -1。
- 对 dpp/RedstoneVerif：CI 可先用 dpp 锁定并导出 pack 集合，再交给 GameTest 跑 E2E；测试世界是一次性 `--universe`，不碰真实世界。

### 其他先例

- **packwiz**：modpack 导向；datapack 支持依赖 loader mod 的 `datapack-folder`，不是纯原版世界管理器。
- **DPM**（npm）：datapack library/package manager，周下载约 50，未形成生态。
- **Sandstone / Kore**：TypeScript/Kotlin DSL，属于 authoring，不是 package manager。

## 5. 与 dpp 的集成方式

推荐流水线：

```text
beet build
  -> Spyglass lint (可选)
  -> dpp build-pack / check
  -> GameTest --packs <built packs> --report junit.xml
  -> 发布到 Modrinth / Smithed / 私有静态源
  -> dpp install --locked + dpp apply
```

约束：

- dpp 保持单一 Rust 二进制；不内置 Node/Python。
- `dpp build-pack` 输出确定性 zip + sha512 + 静态源 index 条目。
- GameTest 使用 dpp 导出的 pack 目录/zip；测试世界可丢弃，不进入 snapshot。
- Smithed source adapter 是二期；没有 checksum 时必须本地计算并标记未验证。
- Weld 不默认使用：dpp 保留每个 pack 的独立身份和回滚能力；只把 Weld 的合并规则作为冲突分析参考。
- `pack.mcmeta.id` 是 hint；lock 的源身份是权威。

## 6. 风险与缺口

1. **Smithed 无 checksum**（在 v2 响应中未观察到）；依赖本地计算，首次信任弱。
2. **Modrinth 类型陷阱**：project_type 不是 datapack；必须按 version loader `datapack` + `.zip` + 根 `pack.mcmeta` 过滤。
3. **pack format 频繁变化**：1.21.9 引入 minor format；26.x calver；需要版本化映射表 + 本地 jar `version.json`。
4. **reload 限制**：实验性注册项、worldgen、feature flag、坏 JSON；`apply` 必须分级。
5. **加载顺序**：权威在 `level.dat`，前缀只是 hint；rename 改身份。
6. **作者工具不解决分发/激活**：beet/Spyglass/GameTest 是互补，不是竞品。
7. **运行时残留**：卸载 hook 必须两阶段；snapshot 不是世界备份。
8. **CI 复杂度**：GameTest 需要 Java/server.jar 和一次性世界；适合 CI，不适合 `dpp` 每次安装。

## 7. 结论

- 现在最值得做的是：`dpp build-pack` + `dpp check` + GameTest CI 的衔接，以及 Modrinth/静态源；Smithed adapter 二期。
- 不要做：把作者工具嵌进 dpp、默认 Weld 合并、把 Smithed/Modrinth 当签名源、用目录名当稳定身份。

## 参考

- Data pack：https://minecraft.wiki/w/Data_pack
- pack.mcmeta：https://minecraft.wiki/w/Pack.mcmeta
- Pack format：https://minecraft.wiki/w/Pack_format
- Version.json：https://minecraft.wiki/w/Version.json
- Java Edition level format：https://minecraft.wiki/w/Java_Edition_level_format
- `/reload`：https://minecraft.wiki/w/Commands/reload
- 中文 `/reload`：https://zh.minecraft.wiki/w/命令/reload
- `/datapack`：https://minecraft.wiki/w/Commands/datapack
- GameTest：https://minecraft.wiki/w/GameTest
- Smithed Wiki：https://docs.smithed.dev/
- Smithed Weld：https://docs.smithed.dev/weld/
- Smithed Weld merging：https://docs.smithed.dev/weld/merging/
- Smithed pack IDs：https://docs.smithed.dev/conventions/pack-ids/
- Smithed enabled tags：https://docs.smithed.dev/conventions/pack-enabled-tag/
- Smithed API v2（第三方客户端字段）：https://docs.rs/mcvm_net/0.3.0/src/mcvm_net/smithed.rs.html
- beet：https://mcbeet.dev/
- beet quick start：https://mcbeet.dev/quick-start/get-started/
- Spyglass：https://spyglassmc.com/
- Misode：https://misode.github.io/
- packwiz datapack 选项：https://packwiz.infra.link/reference/additional-options/
- DPM：https://www.npmjs.com/package/@bigstonedevelopment/dpm
- 26.3 严格加载：https://mojira.dev/MC-310325
- reload 失败：https://mojira.dev/MC-277148
