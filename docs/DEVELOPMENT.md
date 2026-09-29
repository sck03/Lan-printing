# 开发与接口约定

三平台目标与原生构建说明见 [CROSS-PLATFORM.md](CROSS-PLATFORM.md)。下列 Playwright、Edge、原厂预设与 WIA 测试为内部 Windows 测试，依赖私有生成器；公开 CI 运行 Rust 测试与 `scripts/host-smoke.py`，不需要私钥。发行脚本只构建主程序，输出目录必须尚未存在。

## 构建与检查

```powershell
cargo fmt --check
cargo clippy --all-targets --locked -- -D warnings
cargo test --locked
cargo build --locked
node scripts/smoke.cjs
node scripts/network-smoke.cjs
node scripts/qr-smoke.cjs
./scripts/build.ps1
```

Node/Playwright 只用于开发测试。可用 `PLAYWRIGHT_MODULE` 指定模块绝对路径、`EDGE_EXE` 指定浏览器、`LANPRINT_EXE` 指定待测 EXE。构建脚本执行格式检查、严格 Clippy、Rust 测试、release 构建并复制文档和检测脚本；可用 `-OutputDirectory PATH` 指定输出目录。交付前还须对发行 EXE 执行 smoke 测试及兼容检测。

集成测试共用 `scripts/test-server.cjs`，自动选取空闲端口，在系统临时目录创建独立数据，退出时结束测试进程树并删除数据。排错时设置 `KEEP_TEST_DATA=1` 才保留现场。截图和报告仍输出到 `artifacts`，Office 样张用 `scripts/office-fixtures.py` 生成。不要删除仍在使用的样张、最新验证报告或升级回滚备份。网络测试使用独立正常模式实例读取真实防火墙状态；修复/UAC 成功、取消和策略限制的网页交互使用 HTTP 测试响应，不实际修改系统防火墙。

原生测试在独立测试子进程生成/打开/渲染 PDF，无需硬件；设置 `LANPRINT_TEST_VIRTUAL_PRINT=1` 可额外测试 Microsoft Print to PDF 两份横向输出，只在安装该虚拟打印机的开发机执行。启动设置测试使用独立临时注册表键，不会登记真实登录启动项。

没有 Git 元数据的目录应先由维护者纳入自己的版本库，再进行分支/版本发布；不要把数据、测试输出、发行 EXE 或会话密钥加入源码。依赖版本锁定在 `Cargo.lock`；更新依赖时重点回归 WinRT/COM 生命周期与子进程退出码。

二维码测试额外使用开发依赖 `jsqr`（可用 `JSQR_MODULE` 指定模块绝对路径），独立解码网页实际尺寸的二维码，并检查局域网地址、复制、移动端布局和加载失败提示。

## HTTP API

所有端点都经过内网/Host/会话中间件。先 GET `/api/session`，保留 Cookie，并在写请求提供返回的 `csrf` 为 `x-csrf-token`。浏览器 Origin 必须与访问的 HTTP Host 一致。

| 方法与路径 | 用途 |
| --- | --- |
| GET `/api/session` | CSRF、演示状态、上传上限、保留时间、共享地址 |
| GET `/api/share-qr.svg` | 共享地址二维码，由主机本地生成，无局域网地址时返回 503 |
| GET `/api/devices` | printers/scanners/warnings，缓存 30 秒 |
| GET `/api/network` | 仅主机回环地址：端口监听、当前网络类型、防火墙检查、是否可修复 |
| POST `/api/network/repair` | 仅主机回环地址且需 CSRF：管理员授权修复后返回重新检查结果，无请求参数 |
| GET/POST `/api/files` | 当前会话文件列表 / multipart 单文件上传 |
| DELETE `/api/files/{id}` | 删除非活动任务引用的自有文件 |
| GET `/api/files/{id}/preview?page=0` | 从 0 开始的页码，JPEG 预览 |
| GET `/api/files/{id}/download` | 下载自有文件并延长保留时间 |
| GET `/api/jobs` | 当前会话最近最多 100 条任务 |
| POST `/api/print` | `{request_id, file_id, options}` |
| POST `/api/scan` | `{request_id, options}` |
| POST `/api/jobs/{id}/cancel` | 仅取消 queued |

`request_id` 使用 UUID，同一会话、同一操作类型的重复请求在记录保留期内返回已有任务，即使原文件已删除或设备已离线；不可在打印和扫描之间复用标识。打印 options 字段为 `printer,copies,duplex,paper,landscape,color,pages,profile_id,render_dpi`；扫描为 `scanner,dpi,color,format,source`，默认值和合法集合以 `src/model.rs` 为准。公开文件/任务 JSON 使用 `fileId/createdAt/expiresAt` 等前端字段；worker 协议直接序列化 Rust 字段。API 一般返回 JSON `{error: ...}`，安全拒绝为 403、业务校验一般为 400，忙时部分接口为 429。

## 修正问题时的顺序

网络检查和修复共用单个操作许可；许可由后台任务持有，浏览器断开后也不会并发启动第二次管理员提示。提权入口 `--repair-firewall PORT` 只接受 1024–65535 端口，目标 EXE 来自提权进程自身，不读取网页传入的路径、脚本或请求文件。放行规则按程序路径和端口生成稳定名称，重复修复更新同一规则；不自动覆盖企业策略或其他阻止规则。

先在独立 `--demo --data-dir` 环境复现；记录 EXE 哈希、失败操作、错误和最小脱敏文件。设备层问题先以 worker/兼容脚本缩小范围，队列问题检查持久化状态，页面问题检查浏览器错误。不要将“清空数据”“自动重试打印”作为默认修复。

新增配置保持缺省兼容；`Config` 拒绝未知字段，因此回退到旧版前需要恢复旧配置。引入持久化格式变化前增加明确 schema 版本、迁移备份和老版本夹具测试。增加失败恢复、磁盘满、队列取消、跨会话访问测试比只检查成功响应更有价值。

优先后续工作：多种真实打印/扫描设备与 Intel/NVIDIA 实测、结构化日志与脱敏导出、版本化数据迁移、多页扫描、可选软件 PDF 后端。自动更新和无人登录服务需要另外设计发布信任、回滚及 Windows 会话模型。
