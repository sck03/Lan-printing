# macOS、Linux 与 GitHub 构建

## 下载与架构

仓库提供三份独立 GitHub Actions 工作流，所有任务运行格式检查、严格 Clippy、Rust 单元测试、发布构建及 HTTP 启动验证，然后上传压缩包。

| 工作流 | 原生运行环境 | 产物 |
| --- | --- | --- |
| `.github/workflows/build-windows.yml` | Windows 2022 / MSVC | `LanPrint-windows-x64.zip` |
| `.github/workflows/build-macos.yml` | macOS 15 Intel / Apple Silicon | `LanPrint-macos-x64.tar.gz`、`LanPrint-macos-arm64.tar.gz` |
| `.github/workflows/build-linux.yml` | Ubuntu 22.04 x64 / Ubuntu 24.04 ARM64 | `LanPrint-linux-x64.tar.gz`、`LanPrint-linux-arm64.tar.gz` |

Linux x64 包要求 glibc 2.35 或更新；ARM64 包要求 glibc 2.39 或更新。Alpine/musl 不在此构建范围。macOS 可执行文件设置最低部署版本 11.0，但安装的 Poppler、SANE、LibreOffice 可能要求更新系统；CI 验证环境为 macOS 15。其他系统版本及真实设备仍需现场验收。macOS 包为命令行程序，尚未做 Apple Developer 签名、公证或 DMG 安装器。

下载路径：仓库 **Actions → 对应工作流 → 成功运行 → Artifacts**。触发方式为 main 推送、PR、v* 标签、手动 Run workflow。Windows 包内直接提供 EXE；Unix tar.gz 保留执行权限。解压后用包内 `SHA256SUMS.txt` 检查文件。

## macOS

在“系统设置 → 打印机与扫描仪”添加并测试打印机。安装 Homebrew 后：

```sh
brew install poppler sane-backends
# 需要 Word/Excel/PowerPoint 转换时安装：
brew install --cask libreoffice
./LanPrint
```

打印使用系统 CUPS。扫描要求 `scanimage -L` 能列出设备；仅支持 Apple Image Capture 的扫描仪需要另用支持 SANE 的驱动/设备。正常启动尝试打开默认浏览器，终端显示本机与局域网地址；关闭终端可能结束程序，按 Ctrl+C 可退出。

## Linux

Ubuntu / Debian 示例（其他发行版安装对应 CUPS、Poppler、SANE 包）：

```sh
sudo apt-get install cups cups-client poppler-utils sane-utils xdg-utils
# 可选办公文档转换：
sudo apt-get install libreoffice-writer libreoffice-calc libreoffice-impress
sudo systemctl enable --now cups
lpstat -p
scanimage -L
./LanPrint
```

先在系统打印设置或 CUPS 配好队列，确认当前用户可以打印/扫描。程序不自动安装驱动或修改 USB/扫描权限。无桌面运行：`./LanPrint --no-tray --background`，在主机回环地址完成注册。长期运行可以由管理员用 systemd 用户服务启动，`ExecStart` 使用可执行文件的绝对路径并追加 `--no-tray --background`；macOS 可用 LaunchAgent。SIGTERM 和 Ctrl+C 都会正常结束网页服务，工作进程退出时会清理其工具子进程。

## 依赖与行为

| 功能 | Windows | macOS / Linux |
| --- | --- | --- |
| PDF 页数 / 预览 | 系统 WinRT PDF | `pdfinfo`、`pdftoppm`（Poppler） |
| 打印 | GDI | `lpstat`、`lpoptions`、`lp`（CUPS） |
| 图片 | Rust 解码、白底合成 | Rust 解码、白底合成；打印时转 PDF |
| 单页扫描 | WIA | `scanimage`（SANE），100/200/300 DPI、PNG/JPG/PDF |
| DOC/DOCX/XLS/XLSX/PPT/PPTX | Office/WPS | 可选 LibreOffice，独立临时配置、禁用宏 |
| WPS/ET/DPS | 兼容 WPS | 不提供，请先导出 PDF |
| 原厂驱动预设 / 托盘 / 登录启动开关 | 提供 | 不提供托盘；使用 CUPS 默认设置与系统服务管理器 |
| 防火墙 | 检查与管理员修复 | 只检查端口监听，防火墙由管理员手动配置 |

CUPS 按驱动报告的纸张、颜色、双面能力提供选项；不识别的驱动能力会显示警告。网页设置转换为 CUPS 标准参数，实际支持以驱动为准。扫描每次仅取一页，SANE 驱动须支持 PNG 输出、Color/Gray 模式及所选分辨率；无法识别进纸器来源会报错，避免误用平板。办公文档转换需要足够字体，否则排版可能变化。上传后应检查预览。

所有平台的“已提交”均表示进入系统队列，不能保证已经出纸。超时后先查主机队列再重试，避免重复打印。程序不自动提升权限或修改 macOS/Linux 防火墙；只开放所配置的 TCP 端口到可信内网。

## 数据与注册

| 系统 | 默认数据目录 | 机器标识 |
| --- | --- | --- |
| Windows | `%LOCALAPPDATA%\LanPrint` | MachineGuid（保留现有机器码） |
| macOS | `~/Library/Application Support/LanPrint` | IOPlatformUUID |
| Linux | `$XDG_DATA_HOME/LanPrint`，未设置时 `~/.local/share/LanPrint` | `/etc/machine-id`，备用 `/var/lib/dbus/machine-id` |

`--data-dir PATH` 覆盖目录，`--demo` 使用 `LanPrint-demo`。授权码格式与验证公钥不变，生成器可为三个系统网页显示的 LP1 机器码签发；macOS/Linux 机器标识加平台前缀后摘要，不会泄露原始标识。系统重装、系统标识变化或换机后可能需重新申请。Linux 镜像克隆应先生成独立 machine-id。

公开仓库只包含主程序及验证公钥。`admin-tools/`、`src/bin/license-generator.rs`、生成器二进制、签发私钥、授权文件均不跟踪；Cargo 禁止自动发现 bin，默认构建不再依赖生成器。每份工作流还会检查跟踪文件边界，包内容按明确路径复制。

## 本地构建与验证

使用 Rust stable 与对应平台本机工具链；先安装上述运行依赖。示例：

```sh
# Apple Silicon；Intel 改用 x86_64-apple-darwin
bash scripts/build-unix.sh aarch64-apple-darwin dist/macos-new
# Linux x64；ARM64 改用 aarch64-unknown-linux-gnu
bash scripts/build-unix.sh x86_64-unknown-linux-gnu dist/linux-new
python3 scripts/host-smoke.py dist/linux-new/LanPrint
```

Windows：`./scripts/build.ps1 -Target x86_64-pc-windows-msvc -OutputDirectory dist/windows-new`。输出目录必须尚不存在。Unix Rust 测试使用真实 Poppler 打开和渲染生成的扫描 PDF；CUPS 参数与 SANE 来源解析使用固定输入，不会消耗纸张。Linux CI 还运行真实 LibreOffice 转换。HTTP 测试验证启动、机器码、未授权保护、实例锁和重启持久化，不使用私钥。跨目标 `cargo check` 只能验证编译，不能代替原生运行或设备验收。
