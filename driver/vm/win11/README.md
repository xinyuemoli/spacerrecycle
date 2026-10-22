# 验证虚拟机工具

这些脚本通过 `vmrun` 操作 Windows 11 验证虚拟机。它们**不包含密码** ——
密码从环境变量读取，因为这是一个公开仓库。

## 运行前必须设置

```powershell
$env:SR_VM_PASSWORD = '<虚拟机本地账号的密码>'
```

用到它的脚本：

| 脚本 | 用途 |
|---|---|
| `guest.ps1` | 在客户机里执行命令 |
| `launch-in-session.ps1` | 在交互会话里启动程序 |
| `verify-ui.ps1` | 检查界面状态 |
| `wait-for-desktop.ps1` | 等待桌面就绪 |
| `build-noprompt-iso.ps1` | 重建安装 ISO（会把它写进无人值守应答文件） |

未设置时脚本会立刻报错并说明原因，而不是用一个空密码去连。

驱动签名另外需要：

```powershell
$env:SR_SIGN_PFX_PASSWORD = '<测试签名证书的口令>'
```

那个证书只用于一次性测试 VM，`driver/bin/` 里的产物是生成的、不进仓库。

## 无人值守安装

`driver/vm/unattend/autounattend.xml` 和 `srarm.cmd` 里，账号密码的位置写的是
占位符 `__VM_PASSWORD__`。`build-noprompt-iso.ps1` 在把这两个文件放进安装介质时
用 `$env:SR_VM_PASSWORD` 替换它，并在替换后检查占位符确实消失了 —— 否则装出来的
系统会用一个字面量 `__VM_PASSWORD__` 当密码。

## 已知缺口

- 这份工具是**测试用**的，不是产品的一部分。
- VM 定义文件（`.vmx`）不进仓库：它含绝对主机路径（ISO 位置、VMware 安装目录、
  共享目录）。虚拟机实际运行的 `.vmx` 在仓库外。
- 从 VM 抓回的截图和文本证据不进仓库（`driver/vm/evidence/`、`shell-ext/vm/evidence/`）。
