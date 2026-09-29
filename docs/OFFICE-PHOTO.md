# Office/WPS 与照片驱动预设验证

2026-09-28 更新。

- Microsoft Office：中文文件名 DOCX（2 页）、XLSX（1 页）、PPTX（2 页）已完成真实转 PDF、页数检查、预览、下载和演示打印；损坏 DOCX、宏专用扩展名拒绝测试通过。转换成功删除源文件，失败删除源文件和未完成 PDF。
- WPS：实现 KWPS/KET/KWPP 和兼容 ProgID 自动化适配；本机未安装 WPS，WPS/ET/DPS 和不同 WPS 版本需安装后实测，依赖独立进程、AutomationSecurity 和 PDF 导出接口。
- EPSON L300：读取了本机 `\\单证\EPSON L300 Series` 驱动，预设总长 2640 字节，其中厂商私有数据 2420 字节；完整数据序列化/恢复、错误驱动版本拦截已验证。未实际消耗照片纸，照片质量和取消高速的最终效果需在原厂窗口保存预设后实打验收。
- Microsoft Print to PDF：真实 GDI 输出验证预设彩色覆盖网页灰度、600 DPI 路径、驱动份数不重复放大（请求 2 份输出 2 页）。没有向物理打印机提交测试任务。
- 浏览器：原有上传、PDF/图片预览、演示打印、扫描、隔离和移动端流程通过。

开发验证：

```powershell
cargo test --locked
# 读取 EPSON 驱动配置，启用 Microsoft Print to PDF 测试；不消耗纸张
$env:LANPRINT_TEST_DRIVER = '\\单证\EPSON L300 Series'
$env:LANPRINT_TEST_VIRTUAL_PRINT = '1'
cargo test --locked -- --nocapture
# Python 需要 python-docx、openpyxl、python-pptx，仅用于制作测试样张
python scripts/office-fixtures.py
node scripts/office-smoke.cjs
node scripts/smoke.cjs
```

主机 API：GET `/api/profiles` 只返回名称、编号和打印机。POST `/api/profiles` 接收 `{printer,name}`，弹出主机驱动窗口，5 分钟超时；DELETE `/api/profiles/{id}` 删除闲置预设。写操作限制为回环客户端且仍需 CSRF。预设二进制不接受网页上传，也不通过列表 API 返回。

`PrintOptions` 新增 `profile_id`（默认空）、`render_dpi`（300 或 600，默认 300）；旧数据按默认值读取。新增任务写入这些字段后，回退旧程序需恢复旧版数据备份，以免旧版拒绝未知字段。办公文档转换结果以原文件名加 `.pdf` 入库，沿用现有配额和清理规则。预设文件位于数据目录的 `profiles`，长期保留，备份时一并保存。
