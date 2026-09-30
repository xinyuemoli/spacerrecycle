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

**UAC 提权路径**：应用有「清理失败时提权重试」（`retry_cleanup_elevated`）。MSIX 应用运行在容器中，自行提权可能受限。需要在打包版上实际触发一次权限失败、点「提权重试」来验证。这是目前唯一无法自动验证的点。

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
