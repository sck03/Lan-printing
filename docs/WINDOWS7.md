# Windows 7 32 位独立版

旧电脑连接打印机/扫描仪时，使用 **LanPrint Windows 7 x86** 独立安装包。它与 Windows 10/11 的 Rust x64 版分别构建，不会替换正常使用的主程序。

## 安装与使用

1. 主机需要 **Windows 7 SP1 32 位**、支持 SSE2 的 CPU、至少 2 GB 内存，以及 **.NET Framework 4.0 完整版或 4.0–4.8 系列完整运行库**。仅安装 4.0 Client Profile 不够；无需安装 Rust、Node、Python、VC++ 运行库、PDF 阅读器或 WebView2。可从 [Microsoft .NET Framework 下载页](https://dotnet.microsoft.com/download/dotnet-framework)取得适用于 Windows 7 的安装程序；4.8.1 不适用于 Windows 7。
2. 安装设备厂商提供的 **Windows 7 32 位打印/WIA 扫描驱动**，先确认系统可以打印和扫描。只有 TWAIN 驱动的扫描仪不支持。
3. 完整解压 `LanPrint-windows7-x86.zip`，保留 `LanPrintLegacy.exe`、`LanPrintLegacy.exe.config`、`pdfium.dll` 和文档在同一目录，双击 EXE。程序在当前用户会话运行，进入系统托盘并打开浏览器。
4. 在主机打开 `http://127.0.0.1:17860`，复制机器码并申请注册码。授权格式、公钥和 Windows 机器码与正式版一致；同一 Windows 安装可使用原注册码，或复制自己的 `license.json`。客户端无需注册。
5. Windows 防火墙中允许 `LanPrintLegacy.exe` 的局域网入站连接。旧系统版托盘可以打开防火墙设置；网页显示监听地址，但不自动修改防火墙规则。最后从另一台电脑打开共享地址验证。

主机浏览器需支持现代 JavaScript，Windows 7 可使用 Chrome/Edge 109 或相当版本的浏览器；IE 不支持。其他电脑、手机可用较新浏览器访问。主机保持开机且用户已登录；托盘可开启当前用户登录自动启动、防自动睡眠、创建桌面快捷方式。

## 功能与差异

| 项目 | 独立旧系统版 |
| --- | --- |
| PDF / 图片 | PDF、PNG、JPEG、BMP 上传，页数检查、翻页预览、下载、打印；PDF 由随包的 PDFium 在 CPU 上处理 |
| 上传限制 | **默认单文件 100 MB**，可设置 1–200 MB；上传/下载流式读写，PDF 按需读取磁盘，不整文件复制到托管内存 |
| 打印 | 1–99 份、页码选择、A4/A5/Letter、横纵向、彩色/黑白、设备支持时自动双面；按实际打印区域适配 |
| 图像 | 打印 300 DPI、渲染长边最多 4096；输入图片最多 2400 万像素、单边最多 16000，避免耗尽 32 位地址空间 |
| 扫描 | WIA 平板/单页 ADF，100/200/300 DPI、彩色/灰度、PDF/JPG/PNG，每个任务一页 |
| 任务 | 串行持久化队列、排队取消、重试幂等、重启标记中断且绝不自动重印；驱动运行在独立受限进程，具备超时 |
| 授权与隔离 | 同一离线 Ed25519 注册规则，接口及设备进程校验；签名 Cookie、CSRF/Origin、内网 Host 检查、浏览器文件/任务隔离 |
| 办公文档 / 照片预设 | 旧系统版请先导出 PDF；网页隐藏 Office/WPS 转换、原厂驱动预设与 600 DPI 选项 |
| 分享与网络 | 显示/复制共享地址；旧系统版不生成分享二维码，不提供一键防火墙修复 |

扫描结果保留原始协商 DPI。上传大小和图片像素是独立限制；很大的压缩图片不能只按文件字节数判断内存需求。单个设备进程限制 384 MB 提交内存；过于复杂的文档可能失败，应按提示分拆文件，打印超时后先检查系统队列和出纸情况。

Windows 7、其可用浏览器与本包冻结的 PDFium 分支已停止安全更新；此版本面向可信办公室内网和可信文档，建议可升级的主机继续使用正常版。当前未提供无人登录的 Windows 系统服务或多页批量扫描。

## 独立配置与升级

默认数据在 `%LOCALAPPDATA%\LanPrint-legacy`，演示在 `LanPrint-legacy-demo`；与正常版的 `LanPrint` 分开。两种版本不要共用数据目录，`state.json` 格式不同；仅机器码及注册码兼容。退出后备份整个数据目录，升级时保留自己的配置和授权文件。

```text
LanPrintLegacy.exe [--port 17860] [--data-dir PATH] [--no-tray] [--background] [--demo]
```

配置 `config.json`：`port=17860`、`retention_minutes=30`、`max_upload_mb=100`、`max_storage_mb=512`、`max_pending=20`、`show_virtual_printers=false`。退出程序后编辑；已有配置不会被升级覆盖，若之前手动设过较小上传限制，需要自行改为 100。`secret` 是随机会话密钥，不要公开。暂存最多 1000 个文件，任务记录保留 24 小时；预览/下载续期，活动任务和正在读取的文件不会过期清理。

PDFium 从固定版本 `chromium/5418` 的官方构建仓库取得（关闭 V8/XFA，静态 C++ 运行库），下载校验 SHA256，随包附上完整第三方许可证 `PDFIUM-LICENSE.txt`。源码仓库不保存 DLL、参考程序集、私钥、生成器或客户授权。

## 构建与验收

在 Windows 开发机安装 .NET Framework 编译器及 Python 3.11+，运行：

```powershell
./scripts/build-legacy.ps1 -OutputDirectory dist/windows7-new
```

脚本锁定 .NET 4.0 参考程序集和 PDFium，使用 C# 4.0、`/platform:x86` 和严格编译检查。复用 `web/` 并内嵌到 EXE，不复制另一套网页。依赖缓存位于被忽略的 `artifacts/legacy-deps`，不进入客户安装包。构建测试实际运行 32 位进程，检查严格验签、状态/配额/失败回滚、透明图片、原生 PDF 与 100 MB PDF 流式读取，并验证最终包的 PE32、CLR 4.0、32 位必需标志、原生导入、SHA256、启动与授权锁定。

GitHub Actions 的 **Build Windows 7 x86** 工作流会生成独立 ZIP。CI 在新 Windows 上执行 32 位程序，并不能替代 Windows 7 真机验收。

在已注册的 Windows 7 主机上运行（支持系统自带 Windows PowerShell）：

```powershell
powershell -ExecutionPolicy Bypass -File .\check-legacy.ps1
```

可传 `-LicenseFile "路径\license.json"` 检查自定义数据目录。脚本验证 32 位设备进程、真实 PDF 页数/预览和设备枚举，不消耗纸张；随后还需连接实际打印机/扫描仪，验证单面/双面、彩色/灰度、纸张、份数、单页扫描下载、跨电脑访问、重新登录启动和重启不重印。**当前没有 Windows 7 32 位真机及实际设备的验收证据。**
