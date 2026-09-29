# 注册码与生成器

LanPrint 必须注册后才能使用，采用**离线、本机绑定、永久有效**的授权。正式模式和 `--demo` 演示模式都需要注册。首次启动只提供注册页面，打印、扫描、文件上传下载、设备查询等接口均被锁定。注册成功后立即可用，无需重启。

## 给使用者

1. 双击 `LanPrint.exe`，在运行它的主机上打开 `http://127.0.0.1:17860`（自定义端口以实际配置为准）。
2. 点击“复制机器码”，发送给软件提供方。
3. 完整粘贴收到的注册码，点击“注册并进入打印站”。
4. 此后重启自动验证；同事通过内网浏览器使用此主机，无需各自注册。网页底部“注册信息”可查看授权单位与状态。

只允许从主机回环地址提交注册码；通过主机局域网 IP 访问时，即使浏览器就在主机上，也会提示在本机地址注册。默认授权文件为 `%LOCALAPPDATA%\LanPrint\license.json`，演示模式使用 `LanPrint-demo`；指定 `--data-dir` 时位于对应目录。相同主机可再次输入原注册码，也可将自己的 `license.json` 复制到新数据目录。

机器码取自 Windows MachineGuid、macOS IOPlatformUUID 或 Linux machine-id 的摘要，不包含原始标识。Windows 原有机器码保持不变；新平台使用独立前缀。修改 IP、网卡、程序位置、端口或重启不会改变机器码；重装系统、更换主机或改变系统标识后可能需要重新签发。各平台路径见[跨平台说明](CROSS-PLATFORM.md)。

删除、损坏或修改授权文件会重新锁定功能，可在注册页重新输入原注册码恢复。每次受保护的接口访问和设备工作进程启动都会校验。已交给 Windows 打印队列的作业或正在运行的设备操作不因授权文件随后改变而撤销。

## 给软件提供方：生成注册码

`admin-tools` 目录单独保管，不属于客户发行目录：

- `LicenseGenerator.exe`：双击打开交互式生成器。
- `signing-key.hex`：签发私钥，**务必备份，绝不能发送给客户或提交到公开仓库**。
- `README.md`：本说明。

双击生成器，粘贴客户机器码，再输入授权单位或姓名。工具显示注册码并在同目录保存一个 `license-LP1-…-….txt`，将该文本文件的正文发给客户即可。可连续签发多台主机的注册码，机器码提示处直接回车退出。客户只需要发行程序和为其签发的注册码，不需要生成器或私钥。

命令行方式（PowerShell）：

```powershell
./admin-tools/LicenseGenerator.exe issue --private-key ./admin-tools/signing-key.hex --machine LP1-此处替换为完整机器码 --customer '某某办公室' --out ./admin-tools/customer-license.txt
```

省略 `--out` 时只输出注册码；文件已存在时拒绝覆盖。授权名称必须为 1–80 字。注册码采用 Ed25519 数字签名，程序只内置公钥，修改授权信息或使用其他私钥签发均无法通过验证。不提供通用注册码、过期时间或在线撤销功能。

## 构建和私钥维护

本项目已经初始化专用密钥：公钥为 `assets/license-public-key.hex`，对应私钥仅在忽略提交的 `admin-tools/signing-key.hex`。构建**不会重新生成或替换密钥**，也不会向 `dist` 复制生成器或私钥。

```powershell
./scripts/build.ps1
```

公开构建脚本只构建客户程序，不构建、复制或发布生成器。源码、私钥和既有生成器只在维护者本机保留，公开仓库不能重建生成器。维护者本地可运行 `./admin-tools/build-private.ps1` 单独构建；须备份整个私有目录及被忽略的 `src/bin/license-generator.rs`。发行输出目录必须为新目录，例如 `./scripts/build.ps1 -OutputDirectory dist/windows-new`。

如果是在全新项目中初始化新的签发身份（仅在没有密钥时执行）：

```powershell
New-Item -ItemType Directory -Force admin-tools
cargo run --manifest-path admin-tools/Cargo.toml --bin license-generator -- init --private-key admin-tools/signing-key.hex --public-key assets/license-public-key.hex
./scripts/build.ps1
```

初始化使用系统安全随机数，拒绝覆盖已有密钥。替换公钥并重新编译会使旧注册码失效；丢失私钥后无法为已发布的公钥继续签发，因此升级时保留同一套密钥。`config.json` 的会话 secret 与签发密钥无关，不能用于生成注册码。

## 验证

`cargo test --all-targets --locked` 验证签名、篡改、错误机器码与格式限制。`node scripts/license-smoke.cjs` 验证注册页面、接口限制、LAN 端不能注册、设备进程检查、重启持久化与损坏恢复。其后 `node scripts/smoke.cjs` 验证注册后的打印、扫描与原有流程。

内部浏览器测试依赖开发机 Playwright、Edge 与私有生成器，公开仓库 CI 不运行这些需要私钥的测试。先构建主程序，并设置 `LANPRINT_GENERATOR` 指向 `admin-tools/LicenseGenerator.exe`；测试使用真实签名激活隔离临时目录，不改变正常数据目录。可设置 `LANPRINT_EXE`、`LANPRINT_SIGNING_KEY` 和 `PLAYWRIGHT_MODULE` 指定位置。客户程序没有测试绕过开关。

兼容性检查脚本需要主机先注册；使用自定义数据目录时传入 `-LicenseFile '路径\license.json'`。离线授权控制本程序的正常使用；掌握源代码或本机管理权限的人仍可能修改程序或系统标识，不能将其视为不可破解的硬件防复制系统。

本次验收（2026-09-29）：格式检查、Clippy、24 项单元测试通过；注册流程在开发版和发布 EXE 上通过浏览器验收，包括正式模式及演示模式锁定、错误/篡改/跨机器注册码、远程注册拒绝、重启验证和损坏恢复。生成器交互输入、文件保存、独立 Ed25519 验签与拒绝覆盖密钥通过。原有上传、预览、演示打印、扫描下载、会话隔离与手机布局回归通过；未消耗真实纸张。
