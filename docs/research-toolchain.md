# 现代开发工具链调研

> 研究记录，不是接口契约。目的是为 dpp 的 lockfile、缓存、严格模式、求解器、供应链和测试/发布找可借鉴的模式。规范以 [design.md](design.md)、[apply.md](apply.md) 为准。

## 结论

现代工具链里值得 dpp 借用的不是某个具体实现，而是五个模式：

1. **manifest / lock 分离**，lock 可提交、可复现，更新必须显式。
2. **严格模式**：`--locked`、`--offline`、CI 下禁止隐式改 lock。
3. **内容寻址缓存 + 硬链接/CoW**：同一 artifact 跨项目复用，缓存只读、不信任。
4. **求解器把元数据加载当瓶颈**：先过滤候选、优先精确约束、预取元数据、错误可读。
5. **校验和与签名分离**：sha512 只证明内容一致；真实性需要 minisign/TUF 等。

不应照搬：Nix 语言与 store daemon、pnpm 的 symlink farm、uv 的 Python toolchain 管理、mise 的全量工具管理、包内安装脚本/hook。

## 1. Lockfile 与严格模式

### Cargo

- `Cargo.toml` 描述依赖范围；`Cargo.lock` 记录精确版本/来源，由工具维护，建议提交。
- `--locked` 断言使用现有 lock；`--offline` 禁止网络；`--frozen` = `--locked` + `--offline`。
- Cargo 的锁以 package/version/source 为核心；dpp 的 lock 需要同样的“精确 artifact + 来源”信息。

对 dpp 的启示：

- `install --locked` 必须在 manifest 与 lock 不一致时失败，不能悄悄改 lock。
- `install --offline` 只用缓存；缺 artifact 就失败。
- 不定义 `--frozen`：Cargo 是 “locked + offline”，uv 是 “用 lock 但不检查新鲜度”，语义冲突。dpp 用 `--locked` + `--offline` 显式组合。

### uv

- `uv.lock` 是跨环境 lock；`uv sync` 默认 exact sync（删除多余包）。
- `--locked`：lock 过期就报错；`--frozen`：直接用 lock，不检查是否过期；`uv lock --check` 只检查。
- 已有 lock 时优先使用锁定版本；`uv lock --upgrade` / `--upgrade-package` 才升级。
- 解析器使用 pubgrub-rs；元数据加载是最慢部分；优先精确约束、预取元数据、保留 lock 偏好；错误需要可读的冲突链。
- 支持导出 requirements/pylock/CycloneDX SBOM。
- 预览中的 malware check 可查 OSV。

对 dpp 的启示：

- `dpp update` 明确重解；`dpp install` 默认优先 lock，不主动升级。
- 求解前先按目标 pack format / MC 版本过滤候选，避免在不可能版本上加载元数据。
- 错误信息要说明“哪个包/版本/format 冲突”，不要只给抽象无解。

### mise

- `mise.lock` 记录精确版本、下载 URL、checksum；`mise install --locked` 严格按 lock 安装。
- `mise lock --check` 检查 lock 是否最新；`--dry-run` 预览；`--bump` 只更新允许范围内的版本。
- 严格模式下缺 URL/checksum 会失败；lockfile 本身不等于离线安装。
- lockfile 按配置根/环境拆分；本地/secret 配置有独立 lock，通常不提交。

对 dpp 的启示：

- lock 条目必须记录 URL + sha512 + 来源 ID；CI 严格模式要求 checksum。
- `update --dry-run`、`--json` 对 CI 很有用。
- lock 不是离线缓存；`--offline` 需要单独定义。

### pnpm

- 内容寻址 store；文件 hardlink 或 copy-on-write 到 `node_modules`，跨项目复用。
- 安装分三阶段：解析、目录结构计算、链接；尽早并行推进。
- CI 自动进入 frozen-lockfile 模式；`pnpm ci` 先清理再 frozen install。
- 文档明确警告：只让可信 job 写 store/cache，不可信 job 可能污染后续可信安装。

对 dpp 的启示：

- 缓存按 sha512 内容寻址；`archives/<sha512>.zip` 与 `meta/<sha512>.json`。
- snapshot 可用 hardlink/reflink，但必须只读/不可变；dpp 更新用目录替换，不原地改文件。
- 缓存是“受信但需校验”的；每次使用重新校验 sha512，不能只信文件名。

### Nix

- `flake.lock` 固定递归 inputs；`follows` 用于复用同一依赖版本；纯模式提升可复现性。
- Nix store 以内容/输入地址组织，二进制缓存复用构建结果。
- flakes 仍是实验特性，有可组合性/版本管理问题；dpp 不需要 Nix 语言或 daemon。

对 dpp 的启示：

- lock 要包含传递依赖，而不只是直接依赖。
- 依赖去重/共享是有价值的，但 dpp 用 lock + 内容寻址缓存即可。
- 不引入 daemon、store 路径语义或 Nix 表达式。

### aqua

- 声明式 CLI 版本管理；registry 提供包元数据。
- `aqua-checksums.json` 记录 checksum；下载后计算并与期望比较，不匹配则安装失败。
- checksum 来源：本地 lock、发布方 checksum 文件、必要时下载后计算；支持 `AQUA_ENFORCE_CHECKSUM` 在 CI 强制。
- registry 本身也可以校验 checksum。

对 dpp 的启示：

- 私有静态源应有 `sha512`；可选 `aqua-checksums.json` 类似的 sidecar。
- 如果发布方没有 checksum，dpp 可以本地计算并 pin，但要标记“首次信任/未验证来源”。
- CI 应强制 checksum，不允许跳过。

## 2. 缓存与原子性

| 工具 | 缓存模型 | 对 dpp |
|---|---|---|
| pnpm | 内容寻址 store + hardlink/CoW | 缓存按 sha512；快照 hardlink/reflink |
| uv | 全局 cache，lock 优先 | 缓存不是 lock；离线需显式 |
| Nix | store + binary cache | 只借内容寻址/闭包思想 |
| Cargo | `~/.cargo` registry/cache | 不照搬 registry index |
| aqua | 下载后校验再安装 | 先校验，后落地 |

dpp 的缓存设计：

```text
~/.cache/dpp/
├── archives/<sha512>.zip
├── meta/<sha512>.json
└── api/                  # ETag/TTL 缓存
```

规则：

- 缓存 key 是内容 hash，不是包名/文件名。
- 每次读取重新校验；校验失败直接删除/重建。
- 缓存目录只允许可信 job 写入；CI 缓存视为不可信输入。
- 缓存 GC 不能删 lock 仍引用的 artifact；默认保留最近版本，`--all` 才全清。
- snapshot 不是缓存：snapshot 记录世界状态，缓存记录 artifact。

## 3. 求解器

uv 的 pubgrub 实践值得直接借用：

- 最慢的是加载包/版本元数据，即使已缓存。
- 优先选择精确约束（`==`/URL）和已锁定的版本。
- 先选高优先级包，预取依赖元数据；冲突时回溯并生成可读的 incompatibility。
- 解析顺序尽量确定；lock 偏好让重复求解稳定。
- 失败要给用户可操作的冲突链，而不是“无解”。

dpp 的差异：

- 目标 pack format 是全局约束；候选先过滤，再求解。
- 不需要 uv 的 marker/fork 机制（没有 Python 平台标记）；但需要 MC 版本/format 维度。
- 版本排序用 `LooseVersion`（自然排序）→ `published_at` → `version_id`，不要直接用 SemVer 比较 `5.5.4+26.3`。
- 冲突声明（`[conflicts]`、Modrinth `incompatible`）在求解后校验；不要硬编码成 pubgrub 虚拟包。
- 元数据缓存按 artifact hash；lock 中的版本优先。

## 4. 供应链

| 机制 | 作用 | dpp 取舍 |
|---|---|---|
| sha512 | 内容完整性 | 必须；lock 记录 |
| minisign | Ed25519 文件签名，支持 trusted comment | 私有源二期 |
| TUF | 角色/密钥/新鲜度，防仓库或密钥被攻破 | 二期可做 TUF-lite |
| Sigstore/cosign | keyless 签名/透明日志 | 暂不引入 |
| cargo-audit | 扫 Cargo.lock 漏洞 | dpp 自身 CI |
| cargo-deny | 许可证/来源/重复版本/漏洞 | dpp 自身 CI |
| zizmor | GitHub Actions 静态安全分析 | dpp 自身 CI |

关键区分：

- **sha512 不是签名**。它只证明“下载到的内容和 lock 中记录的一致”；如果 lock/索引本身被篡改，hash 也会跟着变。
- Modrinth 的 `sha512` 来自 API；它提供完整性，不等于作者签名。
- 私有源应至少支持 minisign；更高要求再做 TUF 角色（root/timestamp/snapshot/targets）。
- 不执行包内脚本；`uninstall_hook` 只打印，不执行。

## 5. Rust 实现栈

| 层面 | 建议 | 说明 |
|---|---|---|
| CLI | `clap` derive | 子命令、completion、严格参数 |
| 配置/清单 | `serde` + `toml` + `toml_edit` | 写 `dpp.toml` 用 `toml_edit` 保留注释 |
| HTTP | `reqwest` + rustls | 流式下载、超时、重试 |
| 异步 | `tokio` | 并发下载/API；限制并发 |
| zip | `zip` | `enclosed_name()` 防 zip-slip；拒绝 symlink |
| hash | `sha2` | sha512 为主；sha256 本地可选 |
| NBT | `fastnbt` / `quartz_nbt` + `flate2` | 只读 `level.dat` |
| 求解 | `pubgrub` | `OfflineDependencyProvider` + reporter |
| 文件锁 | `fs4` + `rustix`/`nix` | Unix 用 fcntl 与 Java `session.lock` 互斥；flock 不互通 |
| 原子写 | `tempfile` + rename + fsync | staging/journal |
| 路径 | `directories` | XDG cache/config |
| 错误 | `thiserror` + `miette` | 可操作诊断 |
| 日志 | `tracing` | `-v`、JSON 可选 |
| 进度 | `indicatif` | 非 TTY 自动关闭 |
| 测试 | `cargo-nextest` | 进程隔离、JUnit、CI |
| 快照 | `insta` | CLI 输出/错误信息 |
| 属性测试 | `proptest` | 版本排序、zip 路径、lock round-trip |
| 供应链 | `cargo-deny`、`cargo-audit` | CI 必需 |
| CI 安全 | `zizmor` | GitHub Actions |
| 发布 | `dist`（cargo-dist）、`release-plz`、`cargo-semver-checks` | 跨平台二进制/changelog/版本检查 |

`cargo-nextest` 不支持 doctest，doctest 需单独 `cargo test --doc`。

## 6. 测试与发布

- `cargo-nextest`：进程隔离、超时/重试、JUnit XML、CI 分区；适合 dpp 的 CLI/集成测试。
- `insta`：快照测试适合 `dpp status`、错误信息、lock/index 输出；评审用 `cargo insta review`。
- `proptest`：版本比较、zip 路径清洗、lock 读写往返、冲突判定；失败自动 shrink。
- `cargo-deny`：许可证、来源、重复版本、漏洞。
- `cargo-audit`：Cargo.lock 漏洞。
- `zizmor`：GitHub Actions 安全（权限、pin、注入面）。
- `dist`：生成跨平台 tarball/installer 和 release CI；`release-plz`：changelog + 版本 + PR + 发布；`cargo-semver-checks`：API breaking 检查。

建议 dpp 自身 CI：

```text
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo nextest run --workspace --all-targets
cargo test --doc
cargo deny check
cargo audit
zizmor .github/workflows
dist plan
```

## 7. 对 dpp 的具体结论

采用：

- `--locked`：manifest/lock 不一致即失败，不改 lock。
- `--offline`：只用缓存，缺 artifact 即失败。
- 不定义 `--frozen`：避免 Cargo/uv 语义冲突；需要时用 `--locked --offline`。
- lock 记录 URL、sha512、source/project/version ID；CI 强制 checksum。
- 内容寻址缓存 + hardlink/reflink snapshot；缓存视为不可信输入。
- 求解前按 pack format 过滤；pubgrub + 可读错误；lock 优先。
- 私有源二期支持 minisign；再高可选 TUF-lite。
- 测试/发布采用 nextest/insta/proptest/deny/audit/zizmor/dist/release-plz。

不采用：

- Nix store/daemon/语言。
- pnpm 的 symlink farm（不适用于 `datapacks/`）。
- 包内安装脚本、自动执行 hook、自动重启。
- 自动修改 lock 的 CI 安装。
- 把 sha512 当作者签名。

## 参考

- Cargo.lock：https://doc.rust-lang.org/cargo/guide/cargo-toml-vs-cargo-lock.html
- Cargo `--locked/--offline/--frozen`：https://doc.rust-lang.org/cargo/commands/cargo-build.html
- uv resolver internals：https://docs.astral.sh/uv/reference/internals/resolver/
- uv locking/syncing：https://docs.astral.sh/uv/concepts/projects/sync/
- mise.lock：https://mise.jdx.dev/dev-tools/mise-lock.html
- pnpm store/motivation：https://pnpm.io/motivation
- pnpm CI frozen-lockfile：https://pnpm.io/continuous-integration
- Nix flakes：https://nix.dev/concepts/flakes.html
- aqua checksum：https://aquaproj.github.io/docs/reference/security/checksum/
- TUF：https://theupdateframework.io/
- minisign：https://jedisct1.github.io/minisign/
- cargo-deny：https://embarkstudios.github.io/cargo-deny/
- RustSec/cargo-audit：https://rustsec.org/
- cargo-nextest：https://nexte.st/
- insta：https://insta.rs/
- proptest：https://proptest-rs.github.io/proptest/
- zizmor：https://docs.zizmor.sh/
- dist：https://github.com/axodotdev/cargo-dist
- release-plz：https://release-plz.ieni.dev/
- pubgrub：https://docs.rs/pubgrub/latest/pubgrub/
