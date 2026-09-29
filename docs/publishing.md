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

## 4. Microsoft Store 上架（免费代签，最后做）

- 开发者账号**免费**（个人账号需政府 ID + 自拍实名）：<https://storedeveloper.microsoft.com>。
- 走**传统 Win32 应用（MSI/EXE）**通道，微软用自己的证书代签，SmartScreen 直接放行，不必用 MSIX。
- 提交前需备齐：安装包、应用描述、截图、图标（已有 `src-tauri/icons/`）、**隐私政策 URL**（可先放官网一个简单页面）、年龄分级问卷。
- 上架有政策审核，不是"上传即签名"，建议等官网和 README 成型后再提交。

---

## 交付物清单（本次已完成）

| 文件 | 作用 |
|---|---|
| `LICENSE` | MIT 许可证（开源硬门槛） |
| `README.md` | 定位、特性、下载、构建、安全模型 |
| `dist/.taurignore` | 打包时排除 `_test_*.mjs` 测试 harness，不破坏前端测试 |
| `website/index.html` + `styles.css` | Cloudflare Pages 可部署的官网骨架 |
| 本文件 | 从开源到上架的完整操作手册 |
