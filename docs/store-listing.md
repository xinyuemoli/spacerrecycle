# Microsoft Store 上架文案（SpaceRecycle）

可直接复制粘贴到 Partner Center。中英各一份，建议两种语言都填（覆盖更多市场）。

> 字符上限参考（以 Partner Center 实际提示为准）：应用名称 256 / 简短描述 1000 / 完整描述 10000 / 搜索词最多 7 个、每个 30 字符 / 功能列表最多 20 条。

---

## 1. 应用名称

| 语言 | 值 |
|---|---|
| en-US | `SpaceRecycle` |
| zh-CN | `SpaceRecycle` |

预留名称时若被占用，可尝试 `SpaceRecycle — Disk Cleaner` / `SpaceRecycle 磁盘清理`。

---

## 2. 简短描述（Short description）

**en-US**

```
A Windows disk-space tool that never deletes anything on its own. See what is eating your disk, review it item by item, and decide — every cleanup is previewed and reversible.
```

**zh-CN**

```
一款从不擅自删除任何文件的 Windows 磁盘空间工具。看清空间去哪了、逐项确认、再决定——每次清理都可预览、可恢复。
```

---

## 3. 完整描述（Description）

### en-US

```
SpaceRecycle tells you where your disk space went — and then gets out of the way.

Most disk cleaners have a trust problem: they bundle junk, run scheduled background tasks, and delete things you never asked them to. SpaceRecycle takes the opposite stance. Every action starts with you, from the UI, with a preview and a confirm step before anything happens.

IT DOES EXACTLY THREE THINGS
1. Tells you where the space went.
2. Lets you pick what to clean.
3. Asks you to confirm.

WHAT IT FINDS
• Temp junk and caches — temp folders, browser caches, crash dumps, and Windows Update leftovers (typically 1–20 GB).
• Dev artifacts — node_modules, Cargo target, .venv, pip/npm/pnpm caches, __pycache__, Gradle/Maven, .nuget (usually the biggest win: 10–100+ GB).
• Large files — files over 500 MB, aggregated by directory so you can see exactly who is eating the space.
• App leftovers — directories left behind by uninstalled apps and stale installers, cross-checked against what is actually installed.

QUARANTINE, NOT DELETION
Not sure about an item? Move it to quarantine instead. Items go into an NTFS-compressed, restorable store and are only destroyed when you explicitly purge them. Deletion is never final until you say so.

SAFETY BY DESIGN
• Nothing is preselected. Every action starts with you.
• Every cleanup shows a preview, grouped by category and disposition.
• A path denylist protects Windows, Program Files, and your personal folders.
• Items that may contain personal data require an extra confirmation before permanent purge.
• No background auto-cleanup, no scheduled tasks, no boot-time scans.

AUDIT LOG
Every cleanup, restore, and purge is written to a traceable log, so you can always see what happened and when.

OPEN SOURCE
The source code is public under the MIT license. Read it, build it yourself, and verify for yourself that there is no silent cleanup and no telemetry.

REQUIREMENTS
Windows 10 version 1809 or later, or Windows 11. Requires the Microsoft Edge WebView2 runtime (preinstalled on Windows 11 and most Windows 10 systems).
```

### zh-CN

```
SpaceRecycle 告诉你磁盘空间去了哪里，然后就交还控制权。

大多数磁盘清理工具都有信任问题：捆绑垃圾、后台跑定时任务、删除你从未同意删除的东西。SpaceRecycle 反其道而行：每个动作都由你在界面上发起，任何操作前都有预览和确认。

它只做三件事
1. 告诉你空间去哪了。
2. 让你选择要清理什么。
3. 请你确认。

它能发现什么
• 临时垃圾与缓存——临时文件夹、浏览器缓存、崩溃转储、Windows 更新残留（通常 1–20 GB）。
• 开发产物——node_modules、Cargo target、.venv、pip/npm/pnpm 缓存、__pycache__、Gradle/Maven、.nuget（通常是最大头：10–100+ GB）。
• 大文件——超过 500 MB 的文件，按目录聚合，让你看清到底是谁在吃空间。
• 应用残留——卸载应用后遗留的目录和陈旧安装包，并与实际已安装程序交叉比对。

隔离，而非删除
不确定的项可以直接移入隔离区。它们会存进 NTFS 压缩、可恢复的隔离区，只有你明确下令才会被销毁。在你点头之前，删除永远不是最终结果。

安全至上
• 绝不预选任何项。每个动作都由你发起。
• 每次清理都先给出预览，按类别和处理方式分组。
• 路径黑名单保护 Windows、Program Files 和你的个人文件夹。
• 可能包含个人数据的项，需要额外确认才能永久删除。
• 无后台自动清理、无定时任务、无开机扫描。

审计日志
每次清理、恢复和永久删除都会写入可追溯的日志，你随时能看到发生了什么、什么时候发生的。

开源
源码以 MIT 协议公开。你可以阅读、自行构建，亲自验证不存在任何静默清理和遥测。

系统要求
Windows 10 1809 或更高版本，或 Windows 11。需要 Microsoft Edge WebView2 运行时（Windows 11 和大多数 Windows 10 系统已预装）。
```

---

## 4. 功能列表（Feature list，最多 20 条）

**en-US**

```
Five scanners: temp junk, system cache, dev artifacts, large files, app leftovers
NTFS-compressed quarantine with one-click restore
Preview before every cleanup — nothing is ever preselected
Path denylist protects Windows, Program Files, and personal folders
Risk tags: safe / rebuildable / personal data / app leftovers
Full audit log of every cleanup, restore, and purge
Cancelable scans with live progress
No background services, no scheduled tasks, no telemetry
Open source under the MIT license
Windows 10 1809+ / Windows 11
```

**zh-CN**

```
五大扫描器：临时垃圾、系统缓存、开发产物、大文件、应用残留
NTFS 压缩隔离区，一键恢复
每次清理都有预览——绝不预选任何项
路径黑名单保护 Windows、Program Files 和个人文件夹
风险标签：安全 / 可重建 / 个人数据 / 应用残留
完整的清理、恢复、删除审计日志
扫描可随时取消，进度实时显示
无后台服务、无定时任务、无遥测
MIT 协议开源
Windows 10 1809+ / Windows 11
```

---

## 5. 搜索词（Search terms，最多 7 个 / 每个 30 字符）

```
disk cleanup
free up space
node_modules
temp files
cache cleaner
storage
disk space
```

（中文版可填：磁盘清理 / 释放空间 / 临时文件 / 缓存清理 / 磁盘空间）

---

## 6. 分类与其它字段

| 字段 | 建议值 |
|---|---|
| Category | **Utilities & tools** → System tuning / 系统调优 |
| Privacy policy URL | `https://spacerrecycle.pages.dev/privacy.html` |
| Support contact | `https://github.com/xinyuemoli/spacerrecycle/issues` |
| Website | `https://spacerrecycle.pages.dev` |
| Pricing | Free |
| 系统要求 | Windows 10 1809+ / Windows 11 |

> ⚠️ 提交前确认隐私政策页面已上线（push `website/` 后自动部署）。

---

## 7. 提交前检查清单

- [ ] 开发者账号已实名（storedeveloper.microsoft.com，个人账号免费）
- [ ] 在 Partner Center **New Product** 预留名称 `SpaceRecycle`
- [ ] MSIX 包已打出来（`winapp CLI`，见 `docs/publishing.md`）
- [ ] **截图**至少 1 张（建议 4 张，1366×768 或更高，16:9）——需你截应用界面
- [ ] 商店图标 300×300（可从 `src-tauri/icons/icon.png` 放大导出）
- [ ] 描述 / 功能列表 / 搜索词（本文件，直接粘贴）
- [ ] 隐私政策 URL 可访问
- [ ] 年龄分级问卷（IARC，在线填写）
- [ ] 支持联系方式已填

## 8. 上架前必须实测的两个技术点

1. **UAC 提权路径**：应用有「清理失败时提权重试」功能（`retry_cleanup_elevated`）。MSIX 应用运行在容器中，自行提权可能受限——打包成 MSIX 后必须实测这条路径。
2. **WebView2 依赖**：MSIX 下需在 `Package.appxmanifest` 中声明 WebView2 依赖，与裸 exe 不同（裸 exe 用 `downloadBootstrapper` 自动装）。
