# 发布记录

每个版本对应的**提交**和**二进制哈希**。有了这张表，"测试机上跑的是哪个版本"和
"这个二进制是不是从那个提交构建的"都有答案，而不是靠记忆。

## 为什么需要它

1.0.0.0 ~ 1.0.5.0 六个版本发布时，版本号只是 `build-msix.ps1 -Version` 的一个参数，
仓库里没有任何记录。结果是：

- 无法从仓库回答"1.0.5.0 是哪个提交构建的"
- 测试机上装的二进制无法追溯到源码
- 两份"同版本"的二进制哈希不同时，无法判断是代码差异还是编译时间戳

从 1.0.6 起：版本在 `tauri.conf.json` 里，构建用 `/Brepro`（同源码 = 同字节），
每个版本的哈希记在下面。

## 记录

| 版本 | 提交 | App exe SHA256 | Shell DLL SHA256 | 说明 |
|---|---|---|---|---|
| v1.0.6 | `77fe92f`¹ | `41BE41CD33ECE58C09777362CFDCC8F77F2DEFFB5318815B42EA3DCE5DD92A4E` | `C675918338B03D922D218E690F3AED782C99D61008389958D69C462C250433D5` | 外壳扩展改为可复现构建；右键菜单崩溃修复 |
| v1.0.0 ~ v1.0.5 | **不可知** | — | — | 版本号未入库，无标签，无法追溯。**不要猜测** |

¹ 提交号和 `tauri.conf.json` 里的版本一致性的最终提交见 `git log`；标签 `v1.0.6` 指向记录版本控制的那个提交。

## 二进制之间的关系

- **App exe** 在 MSIX 包里，由 `scripts\build-msix.ps1` 从 `src-tauri` 构建。
- **Shell DLL** 是**独立部署**的（不在 MSIX 里），由 `shell-ext\build.cmd` 构建，
  用 `scripts\install-shell-ext.ps1` 安装。
  这个"安装器"目前只服务测试机 —— 正式安装流程尚未实现，是已知缺口。

## 怎么用这张表

```powershell
# 确认测试机上的 DLL 是不是这个版本
(Get-FileHash 'C:\sr-test\shell-ext\bin\SpaceRecycleShell.dll' -Algorithm SHA256).Hash
# 应与上表一致；不一致就说明部署的不是这一版

# 确认本地构建可复现（两次构建应得到同一哈希）
shell-ext\build.cmd ; (Get-FileHash shell-ext\bin\SpaceRecycleShell.dll).Hash
shell-ext\build.cmd ; (Get-FileHash shell-ext\bin\SpaceRecycleShell.dll).Hash
```

## 已知缺口

- `v1.0.0` ~ `v1.0.5.0` 无法补标签：没有记录，补了就是编造。
- Shell 扩展的正式安装器还没有：目前只有测试用的 `install-shell-ext.ps1`。
- MSIX 包本身没有记录哈希（每次重新签名都会变），记录的是**包内的 exe**。
