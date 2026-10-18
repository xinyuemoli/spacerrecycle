# Publishing SpaceRecycle

从"本地完成的开源项目"到"可下载、可上架、可被发现"的完整操作手册。
许可证已选定为 **MIT**（见根目录 `LICENSE`）。

## 0. 一次性前置

1. 在 GitHub 新建**公开**仓库：`https://github.com/xinyuemoli/spacerrecycle`。
2. 如需要，把 `LICENSE` 里的 `Copyright (c) 2026 SpaceRecycle contributors` 改成你的真名 / 组织名（一行文本，随时可改）。
3. ✅ 已清理测试 harness 里的硬编码机器路径：3 处本机 Chromium 绝对路径已改为 `process.env.CHROME_PATH` 环境变量覆盖，playwright 现在自动发现 Chromium，不再泄露本机用户名。

---

## 1. 开源：推送到 GitHub

```powershell
git remote add origin https://github.com/xinyuemoli/spacerrecycle.git
git add -A
git commit -m "chore: open source under MIT (LICENSE, README, website, .taurignore)"
git branch -M main
git push -u origin main
```

> 提示：当前分支是 `master`。上面的 `-M main` 会重命名为 `main`（GitHub 惯例），如果你想保留 `master` 就跳过这行。

---

## 2. 打 tag 并发布 v0.1.0

先确认 `.taurignore` 生效（可选但建议）：

```powershell
cargo tauri build
# 然后检查安装包体积，确认 _test_*.mjs 未被打进 exe
```

打 tag + 发布：

```powershell
git tag -a v0.1.0 -m "v0.1.0 — first open-source release"
git push origin v0.1.0
```

然后在 GitHub 的 **Releases** 页面基于该 tag 创建 release，把 `cargo tauri build` 产出的安装包（`src-tauri\target\release\bundle\` 下的 `.msi` / `.exe`）作为 asset 上传，正文用下面的 release notes。

### v0.1.0 release notes（草稿，可直接粘贴）

```markdown
SpaceRecycle's first open-source release — a Windows disk cleaner that never
deletes anything on its own.

**What it does**

- Five scanners: temp junk, system caches, dev artifacts (`node_modules`,
  `target`, `.venv`, package caches), large files, and app leftovers.
- Quarantine with NTFS compression: unsure items move to a restorable store,
  and are only destroyed when you explicitly purge them.
- Safety by design: nothing is preselected, every cleanup shows a preview,
  and a path denylist protects Windows / Program Files / personal folders.
- Audit log: every cleanup, restore, and purge is traceable.

**Requirements**

- Windows 10 1809+ or Windows 11 (WebView2 runtime, preinstalled on most systems).

**Install**

Download the `.msi` / `.exe` below. No bundled software, no background services.

MIT License — read the source and verify there is no silent cleanup.
```

---

## 3. 官网部署（Cloudflare Pages，免费）

`website/` 目录已经是可直接部署的静态站，下载链接已指向 `github.com/xinyuemoli`。

1. 注册 Cloudflare，进入 **Workers & Pages → Create → Pages**。
2. 连接 GitHub 仓库，构建配置：框架 **None**，构建命令留空，输出目录填 `website`。
3. 部署后，在 **Custom domains** 绑定你的域名（可选，`.com` 约 ¥60–80/年）。
4. 免费子域名 `*.pages.dev` 会立即可用。

---

## 4. Microsoft Store 上架（MSIX，免费代签）

### ⚠️ 关键前提：免费签名只属于 MSIX

| | **MSIX 打包** | **EXE/MSI 直链（未打包）** |
|---|---|---|
| 代码签名 | ✅ **微软免费签** | ⚠️ **必须自购 CA 证书**（¥700–1400/年） |
| 托管 | ✅ 微软免费托管 | 你自己托管 |
| 自动更新 | ✅ 系统每 24h 检查 | 自己实现 |
| S 模式 | ✅ 支持 | ❌ 不支持 |

Tauri 官方文档确认：Tauri 只产出 EXE/MSI，那条路 **"must be code signed"**。所以**想免签名费，只能走 MSIX**。

### 4.1 打包 MSIX（已实测可复现）

前置（一次性）：

```powershell
winget install microsoft.winappcli --source winget
```

打包（流程已固化在 `scripts/build-msix.ps1`）：

```powershell
.\scripts\build-msix.ps1 -Version 1.0.0.0 `
    -PackageName     "<Package/Identity/Name>" `
    -Publisher       "<Package/Identity/Publisher>" `
    -PublisherDisplay "<Package/Properties/PublisherDisplayName>"
```

产物：`src-tauri\target\msix\SpaceRecycle_1.0.0.0_x64.msix`（约 2.7 MB，已用开发证书签名）。

三个身份值取自 Partner Center 的 **Product identity**（产品标识）面板，**必须逐字符一致**（官方明确：值区分大小写，空格和标点都要对上），否则上传会被拒。

> ⚠️ **版本号规则（容易踩坑）**：四段版本号中，**第一段不能为 0**，**第四段必须留 0**（第四段由 Store 保留）。
> 所以 `0.1.0.0` 会被 Store 直接拒绝，必须写成 `1.0.0.0` 这类。脚本已内置校验，填错会立即报错而不会产出一个用不了的包。

脚本四步：`cargo build --release`（`-SkipBuild` 可跳过）→ `winapp manifest generate` 生成全部图标资产 → 套用 `src-tauri/msix/Package.appxmanifest.template` → `winapp package` 出包签名。

模板里有两处**按实测修正过**的关键点：

- `MinVersion="10.0.17763.0"`（Windows 10 1809，与 README/官网口径一致）。winapp 默认给的是 1903，会平白排除一批实际支持的用户。
- `win32dependencies:ExternalDependency` 声明 **WebView2**。MSIX 应用无法像 NSIS/MSI 那样运行 WebView2 引导安装器，靠 App Installer 链式安装来补齐（Win11 预装、绝大多数 Win10 已有）。

### 4.2 本地测试（已实测通过）

开发者模式开启时，**无需管理员权限**即可实测：

```powershell
winapp run src-tauri\target\msix\stage --detach
```

实测结论（2026-09-30）：应用以包身份正常启动，`msedgewebview2.exe` 子进程生成，UI 完整渲染，Rust 后端读到真实磁盘数据（`C:\ 63.8 GB 可用 / 476 GB`）。

清理测试注册：`Get-AppxPackage -Name "SpaceRecycle*" | Remove-AppxPackage`

> 若要手动安装 MSIX 文件，需先信任开发证书（需管理员，每张证书只需一次）：
> `winapp cert install src-tauri\target\msix\SpaceRecycle_cert.pfx`

### 4.3 提交上架

1. 免费开发者账号（个人需政府 ID + 自拍实名）：<https://storedeveloper.microsoft.com>
2. Partner Center → **New Product** → 预留名称 `SpaceRecycle`
3. **把 `-Publisher` 换成 Partner Center 分配给你的 Publisher DN**（关键，填错会被拒）
4. 上传 MSIX + 填写商店信息（文案直接取 `docs/store-listing.md`）
5. 提交 → 政策审核 → 通过后**微软自动签名**

### 4.4 仍需人工实测的一项

**UAC 提权路径**：应用有「清理失败时重试提权」（`retry_cleanup_elevated`）。MSIX 应用运行在容器中，自行提权可能受限。需要在打包版上实际触发一次权限失败、点「提权重试」来验证。这是目前唯一无法自动验证的点。

### 4.5 订阅加载项（收费层）

免费层 = 扫描 + 清理 + 本地隔离。订阅层 = OneDrive 隔离 / 云端备份 / 配额 + 重要性标记 + 驱动防护。**一次订阅开全部**，不是按功能收费。

**两个 ID 是编译进二进制的**，用户不需要改任何配置文件：

| 环境变量 | 作用 | 不填的后果 |
|---|---|---|
| `SPACERECYCLE_ADDON_ID` | 订阅加载项的 Product ID | ⚠️ 见下方警告 |
| `SPACERECYCLE_GRAPH_CLIENT_ID` | OneDrive 设备码授权的 Azure 应用 ID | 登录按钮失效并提示"未配置" |

```powershell
.\scripts\build-msix.ps1 -Version 1.0.1.0 `
  -AddonStoreId SpaceRecyclePro `
  -GraphClientId <azure-app-client-id>
```

#### ⚠️ 不填 AddonStoreId 不会"关掉付费层"，而是"送掉付费层"

没编译进加载项 ID 时，权益判定落到 `NotConfigured`，语义是**"这里没有东西可卖"**，于是应用正确地认为**没有东西需要拦**——`paid_features_open()` 返回 `true`，**所有付费功能对所有人免费开放**。

这是刻意的设计：非商店版（GitHub 直接下载）本来就不该收费，所以"没有可卖的东西"必须等于"全开"，否则 GitHub 版会把用户看得见的功能锁死。但它同时意味着**商店版忘记填 ID = 静默白送**。

`scripts/build-msix.ps1` 在这种情况下会打印醒目警告。发布前确认没有这条警告。

#### 加载项创建流程

1. Partner Center → 你的应用 → **加载项** → **创建新加载项**
2. **产品类型**：Subscription
3. **产品 ID**：自己取（如 `SpaceRecyclePro`）——**发布后不可更改**
4. **内容类型**：电子软件下载
5. **订阅周期**（月 / 3月 / 6月 / 年 / 24月）与**免费试用**（1周 / 1月）——**发布后均不可更改**
6. 提交并发布，然后把这个 Product ID 填进 `-AddonStoreId`

#### 权益判定的三态

| 情形 | `source` | 付费功能 | 设置页显示 |
|---|---|---|---|
| 从商店安装且已订阅 | `store` / `cache` | 开放 | "已订阅" |
| 从商店安装但未订阅 | `store` | 关闭 | 引导订阅 |
| 直接跑 exe（GitHub 版） | `unpackaged` | **开放** | "此副本非商店版" |
| 商店版但没编进加载项 ID | `not_configured` | **开放** ⚠️ | "订阅尚未开卖" |

`is_subscriber()` 与"功能是否开放"是**两件事**：前者只在前两行及 `dev_override` 时为真，所以"没有东西可卖"的构建不会在设置页谎称用户已订阅。

#### 自己测试付费路径：不需要买订阅

`dev_override` 让构建在不问商店的情况下解锁付费功能，用来在**没有订阅的账号**上测订阅路径。

**它现在编译在 `dev-entitlement` feature 后面，默认关闭。** 以前它是个普通配置项，任何构建都生效——那意味着任何用户拿记事本往 `%APPDATA%\SpaceRecycle\config.json` 里加一行 `"devSubscriberOverride": true`，商店版就会**白送全部付费功能**，没有任何提示。

```powershell
# 测试包（自己用）
.\scripts\build-msix.ps1 -Version 1.0.5.0 -DevEntitlement

# 上架包（绝不要带这个参数）
.\scripts\build-msix.ps1 -Version 1.0.5.0
```

| | feature 关闭（商店版） | feature 打开（测试版） |
|---|---|---|
| 配置里写 `devSubscriberOverride` | **被忽略**（字段不存在） | 解锁全部付费功能 |
| `source` | 按商店结果 | `dev_override` |
| 设置页 | "未订阅" | 明写是开发覆盖 |

关闭时 `Config` 结构体里**根本没有这个字段**，所以配置文件里的那一行会被 serde 跳过，保存配置时顺手丢掉——发布版不该带着一个自己永远不会执行的开关。

`build-msix.ps1` 会在构建后**检查二进制里有没有那段字符串**，不匹配就报错：带了 `-DevEntitlement` 却没编进去、或者没带参数却编进去了（通常是残留的旧二进制），都会被拦下。

`scripts/check-all.ps1` 会分别用两种配置各跑一遍测试，并检查 `target\release` 里的二进制是否残留后门。

---

## 交付物清单（本次已完成）

| 文件 | 作用 |
|---|---|
| `LICENSE` | MIT 许可证（开源硬门槛） |
| `README.md` | 定位、特性、下载、构建、安全模型 |
| `dist/.taurignore` | 打包时排除 `_test_*.mjs` 测试 harness，不破坏前端测试 |
| `website/`（`index.html`+`styles.css`+`i18n.js`+`privacy.html`） | 中英双语官网，Cloudflare Pages 自动部署 |
| `docs/store-listing.md` | Store 上架文案（中英双语，可直接粘贴） |
| `src-tauri/icons/icon-1024.png` | MSIX 资产生成用的图标源（≥400×400） |
| `src-tauri/msix/Package.appxmanifest.template` | MSIX manifest 模板（含 WebView2 依赖、1809 最低版本） |
| `scripts/build-msix.ps1` | 一键打包 MSIX 的脚本 |
| 本文件 | 从开源到上架的完整操作手册 |
