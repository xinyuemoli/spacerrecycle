# SpaceRecycle 扩展设计：OneDrive 接入 · 订阅门控 · 文件重要级别

Date: 2026-10-12
Status: 阶段 A/B 已交付并测试；阶段 C/D 待前置条件
关联: `2026-09-28-spacerecycle-design.md`（基础设计，原则不变：绝不自动删除）

## 交付状态

| 阶段 | 状态 | 说明 |
|---|---|---|
| A · OneDrive 本地空间回收 | ✅ 已交付 | 检测同步根、扫描可释放文件夹、一键释放本地副本；不联网、不注册、零成本 |
| B · 用户手动重要级别 | ✅ 已交付 | 右键菜单四级标记、按路径持久化、清理管线服务端硬门控 |
| C · 商店订阅门控 | ✅ 已交付（待 Store ID 生效） | 权益策略、缓存与离线宽限期、设置页状态卡、后端门控函数均已就绪；填入 `subscriptionAddonStoreId` 即生效 |
| D · Graph 云端配额与云端备份 | ✅ 已交付 | 账号连接、令牌刷新、配额展示、分片上传、批次清单清单与**应用内云端恢复/删除**均已实现并测试 |

前置条件齐备前 A/B/C 已可完整使用；D 的联网部分需要 client ID 才能在真机跑通（本地逻辑全部可测）。

## 11. 关于订阅门控的实际强度（重要）

需要明确一点，因为它影响商业预期：

- 云备份上传到的是**用户自己的 OneDrive**，我们不承担任何存储成本，这个功能的边际成本是 0；
- 客户端以 **MIT 开源**发布，任何能编译项目的人都可以注释掉门控；
- 商店内购本身在**微软侧**校验，卸载重装、换账号都会重新校验，但本地二进制无法防止被改。

因此这道门控能拦住的是"无意使用"，不是"蓄意绕过"。如果订阅是主要收入来源，需要考虑把真正的价值放在服务端（例如云端索引、跨设备同步、团队共享），而不是放在这个纯客户端开关上。

## 1. 需求确认结果

来自产品确认的四个决定：

| 议题 | 决定 |
|---|---|
| OneDrive 接入程度 | **方案 C**：本地空间回收 + 云端配额展示，两者都要 |
| 订阅方式 | **微软商店加载项订阅**（Partner Center 配置，微软收款，无需自建服务器） |
| 上传备份的内容 | **清理时上传到 OneDrive**：移入隔离区的文件同步一份到云端，磁盘立即释放，云端成为可恢复备份 |
| 文件重要级别 | **先预留字段，由用户手动分类**：右键菜单标记等级，多个等级可选 |

## 2. 成本结论（回答"不需要钱可以吗"）

- Microsoft Graph API 调用**免费**；Azure 应用注册（App registration）不需要付费订阅，个人 Microsoft 账号即可。
- 微软商店加载项订阅由微软负责收款与授权校验，**无预付费用**，微软按比例抽成。
- OneDrive 存储空间由用户自己的账号承担（免费 5GB，Microsoft 365 为 1TB），产品方不承担。
- 本地空间回收（方案 A）完全不联网、不注册、**零成本**。

## 3. 数据模型变更

### 3.1 重要级别（新增）

```rust
pub enum Importance { Critical, High, Normal, Low }
```

- `Finding.importance: Option<Importance>`，`None` = 未标记，serde 默认。
- 持久化在 `%APPDATA%\SpaceRecycle\importance.json`，键为规范化后的绝对路径（小写、反斜杠、去尾部分隔符）。
- 与扫描解耦：扫描结束后统一"打标"（enrich），扫描器本身不读这个存储。

### 3.2 处置方式（新增两种）

```rust
pub enum Disposition {
    Purge,        // 永久删除
    Quarantine,   // 移入本地 NTFS 压缩隔离区
    Dehydrate,    // 仅 OneDrive：释放本地副本，云端保留（可随时重新下载）
    CloudBackup,  // 仅订阅用户：上传到 OneDrive 后删除本地副本
}
```

### 3.3 分类（新增一种）

```rust
pub enum Category { ..., CloudPlaceholder }
```

- 风险等级复用 `Safe`（云端有原件，本地副本不唯一）。
- 默认处置 `Dehydrate`，**不允许 Purge**（云端文件不能由本工具删除）。
- 该扫描器**默认关闭、需用户勾选**，与 LargeFile 同样是 opt-in：遍历 OneDrive 树可能很大。

### 3.4 重要级别驱动的差异化清理（预留）

重要级别不改变扫描结果，只在三处生效：

1. **默认处置建议**：`Critical` → 不提供任何清理动作；`High` → 只允许 `Quarantine`/`CloudBackup`；`Normal` → 按分类默认；`Low` → 允许 `Purge`。
2. **预览分组**：按级别排序，`Critical` 项目置顶提示。
3. **门控**：`Critical` 级项目在 `run_cleanup` 中被硬性阻止（服务端判定，不信任前端）。

本轮只落地第 3 条的后端骨架与 UI 展示，第 1、2 条在后续版本。

## 4. 阶段 A：OneDrive 本地空间回收

### 4.1 检测

- 根目录来源：环境变量 `OneDrive`、`OneDriveCommercial`、`OneDriveConsumer`，回退 `%USERPROFILE%\OneDrive`。去重。
- 逐个根目录并行遍历，按属性判定每个文件/目录状态：

| 状态 | 属性特征 | 本地占用 |
|---|---|---|
| `OnlineOnly` | 含 `RECALL_ON_DATA_ACCESS` 或 `RECALL_ON_OPEN`，且不含 `PINNED` | ≈0，**已经释放** |
| `Pinned` | 含 `PINNED` | 完整占用，用户明确要求保留 |
| `LocallyAvailable` | 其余 | 完整占用，**可释放** |

### 4.2 释放动作（Dehydrate）

- 实现：`SetFileAttributesW` 设置 `FILE_ATTRIBUTE_UNPINNED`、清除 `FILE_ATTRIBUTE_PINNED`，与资源管理器"释放空间"同机制。
- **异步性**：提交后由 OneDrive 同步引擎在后台执行，本工具只负责"请求 + 校验"。
- **校验**：返回后重新读取属性与 `GetCompressedFileSizeW`，只有实际变为 `OnlineOnly` 才计入 `reclaimed`；否则标为"已请求，尚未生效"。
- **可逆**：文件仍在 OneDrive，用户再次访问时自动下载。因此**不需要二次确认复选框**，但预览仍然必过（遵守基础设计的"preview first"）。
- 失败的常见原因：文件被占用、根目录不在 OneDrive 同步范围内、企业策略禁止。逐个报告，不中断批次。

### 4.3 安全

- 复用 `safety::is_allowed`：Denylist 同样生效。
- 额外约束：**只有路径位于已识别的 OneDrive 根之下才允许 Dehydrate**，防止误伤普通文件。
- 绝不删除云端文件；本阶段没有任何云端写操作。

## 5. 阶段 B：用户手动重要级别

- 右键菜单：`关键 / 重要 / 普通 / 可清理 / 清除标记`。
- 表格行显示等级徽标；筛选器可按等级过滤。
- 持久化按路径记录，重新扫描后依然生效。
- 扫描出的 `Critical` 项目在清理管线中被阻止（后端硬门控）。

## 6. 阶段 C：微软商店订阅门控

- 校验方式：`Windows.Services.Store` 的 `StoreContext.GetAppLicenseAsync()`，在 `AddOnLicenses` 中查找订阅加载项 Store ID。
- **前置条件**：需要 MSIX 包标识。开发态（未打包）无法校验，回退策略：
  - 配置项 `devSubscriberOverride`（默认 false）用于本地开发调试；
  - 校验不可用时按"未订阅"处理，不阻断非订阅功能。
- 缓存：`%APPDATA%\SpaceRecycle\entitlement.json`，含 `checkedUnix`、`expiresUnix`。
- 宽限期：校验暂时不可用（离线）时，保留最近一次成功结果若干天，避免订阅用户离线被误判。
- 门控点：`Disposition::CloudBackup` 在 `run_cleanup` 中要求 `subscriber == true`，否则该项被 blocked 并给出明确原因。

## 7. 阶段 D：Graph 云端配额与上传备份

- 授权：OAuth 2.0 **设备代码流**（公共客户端，无需客户端密钥），scope `Files.ReadWrite.All offline_access`。
- 配额：`GET /me/drive` → `quota.total / used / remaining`。
- 备份目录：`/me/drive/special/approot:/SpaceRecycle Backup/<batch-id>/`。
- 上传：小于 4MB 用单次 PUT；大于 4MB 用 `createUploadSession` 分片上传（可断点续传、可报告进度）。
- **提交顺序安全**：先上传并校验（大小 + 哈希），**校验通过后才删除本地副本**；上传失败则回退为本地隔离区，绝不因网络问题丢数据。
- 令牌存储：`%APPDATA%\SpaceRecycle\auth.json`，刷新令牌用 DPAPI 加密（`CryptProtectData`）。

## 8. 失败与降级

| 场景 | 行为 |
|---|---|
| OneDrive 未安装 / 未同步 | 隐藏 OneDrive 卡片，扫描器跳过，无报错弹窗 |
| Dehydrate 未立即生效 | 标为"已请求"，不谎报已释放 |
| 订阅校验离线 | 宽限期内保留上次结果；超期按未订阅，仅禁用云端备份 |
| 上传中断 | 回退本地隔离区，记录失败原因，可重试 |
| 未登录 OneDrive 账号 | 配额区显示"未连接"，上传备份不可用 |

## 9. 需要产品方（你）提供的前置条件

1. **Partner Center**：创建订阅型加载项，拿到 **Store ID**（阶段 C 用）→ 填入配置项 `subscriptionAddonStoreId`
2. **Azure 门户**：注册一个公共客户端应用，拿到 **Application (client) ID**，启用"允许公共客户端流"（阶段 D 用）→ 填入配置项 `graphClientId`
   - 该应用只需 Graph 委派权限 `Files.ReadWrite.AppFolder`（**不需要** `Files.ReadWrite.All`）
3. 决定订阅价格与试用天数。

在拿到这两项之前，阶段 A/B 可以完整交付并测试；阶段 D 的代码路径可用假传输层测试，联网部分有 `--ignored` 的实测用例。

## 9.1 阶段 D 的实现取舍（已定）

**用 WinHTTP，不用 rustls。** 理由：本产品卖点是"单文件小体积 exe"，引入 rustls + 加密提供商会增加约 1MB；而 WinHTTP 是系统组件，零体积，且自动继承系统证书库与代理配置——这在企业环境下是"能用"和"莫名超时"的区别。基础设计明确"不做跨平台抽象、直接调 Win32"，因此这是更忠实的选择。

**权限范围取 `Files.ReadWrite.AppFolder`。** 应用只能看到自己在 OneDrive 里的 `Apps/` 目录，读不到、列不出、动不了用户的其他任何文件。一个磁盘清理工具索要全盘云文件权限，是更糟的产品而不是更强的产品。

**刷新令牌用 DPAPI 加密存储。** 明文放在 `%APPDATA%` 下等于任何以该用户身份运行的进程都能拿走——那是一个持有用户全部云文件访问权的 bearer 凭据。DPAPI 把它绑定到用户账户，使"拷走文件"这一现实威胁失效（但不足以对抗已在该用户下运行的恶意进程，文档中已如实说明）。

## 10. 非目标

- 不做云端文件的删除或大范围同步；OneDrive 只作为"备份目的地"和"配额展示"。
- 不做跨云盘（Google Drive / Dropbox）抽象。
- 本次不改变"绝不自动清理"的核心原则。