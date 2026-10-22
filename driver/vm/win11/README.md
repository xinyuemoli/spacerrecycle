# 验证虚拟机工具

这些脚本通过 `vmrun` 操作 Windows 11 验证虚拟机。它们**不包含密码，也不包含本机绝对路径** ——
这个仓库是公开的，那些东西从环境变量读取。

## 运行前必须设置

```powershell
$env:SR_VM_PASSWORD = '<虚拟机本地账号的密码>'
$env:SR_VMX         = '<测试虚拟机 .vmx 的绝对路径>'
```

重建安装 ISO 时另外需要：

```powershell
$env:SR_VMX                     = '<同上>'
$env:SR_SIGN_PFX_PASSWORD       = '<测试签名证书的口令>'
$env:SR_ISO                     = '<原版 Windows 安装 ISO>'
$env:SR_ISO_STAGE               = '<重建用的工作目录>'
$env:SR_ISO_OUT                 = '<重建后 ISO 的输出路径>'
```

用到哪个变量：

| 脚本 | 需要 |
|---|---|
| `guest.ps1` | `SR_VM_PASSWORD`, `SR_VMX` |
| `launch-in-session.ps1` | `SR_VM_PASSWORD` |
| `verify-ui.ps1` | `SR_VM_PASSWORD`, `SR_VMX` |
| `wait-for-desktop.ps1` | `SR_VM_PASSWORD`, `SR_VMX` |
| `rebuild.ps1` | `SR_VMX` |
| `build-noprompt-iso.ps1` | `SR_VM_PASSWORD`, `SR_ISO`, `SR_ISO_STAGE`, `SR_ISO_OUT` |
| `driver/sign-test.ps1` | `SR_SIGN_PFX_PASSWORD` |

未设置时脚本会立刻抛错并说明缺哪个，而不是拿空值继续跑。

## 无人值守安装

`driver/vm/unattend/autounattend.xml` 和 `srarm.cmd` 里，账号密码的位置写的是占位符
`__VM_PASSWORD__`。`build-noprompt-iso.ps1` 在把这两个文件放进安装介质时用
`$env:SR_VM_PASSWORD` 替换，并在替换后检查占位符确实消失了 —— 否则装出来的系统会用
字面量 `__VM_PASSWORD__` 当密码。

## 已知缺口

- 这份工具是**测试用**的，不是产品的一部分。
- VM 定义文件（`.vmx`）不进仓库：它含绝对主机路径（ISO 位置、VMware 安装目录、
  共享目录）。虚拟机实际运行的 `.vmx` 在仓库外，位置由 `SR_VMX` 给出。
- 从 VM 抓回的截图和文本证据不进仓库（`driver/vm/evidence/`、`shell-ext/vm/evidence/`）。
- 驱动签名证书的私钥（`.pfx`）由 `sign-test.ps1` 现场生成到 `driver/bin/`，不进仓库。
