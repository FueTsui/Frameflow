# Windows 0.2.3 发布验收

日期：2026-10-02。目标：Windows x64，Rust GNU release 构建。

## 本轮变更

- 单实例控制：重复启动恢复已有窗口，转交媒体文件；引擎检测期间保留导入请求。
- 程序保持普通用户权限。新安装默认 `%LOCALAPPDATA%\Programs\Frameflow`，已有安装沿用原目录；升级前检查安装目录可写性。
- 媒体任务退出先确认并等待清理；引擎更新和软件下载可等待完成后退出。
- FFprobe 解析限制时间及输出大小，媒体子进程增加异常清理，超长日志限制内存占用。

原有八种功能、独立导出配置、主题及更新能力保留。内置 FFmpeg / FFprobe 9.0.2。详细发现、实现与前一轮原生交互验证见 [运行审计](runtime-audit-2026-10-02.md)。

## 已完成的本地验证

- `cargo test --locked --release --target x86_64-pc-windows-gnu`：137 通过、0 失败、5 忽略。3 个联网测试默认忽略，另 2 个是由测试调用的子进程夹具。
- 测试 PATH 包含实际 FFmpeg / FFprobe；CPU、本机 NVIDIA、音频、字幕、滤镜、取消清理等媒体回归实际执行。
- `cargo fmt --all -- --check`、严格 Clippy、`git diff --check` 通过，release 构建完成。当前 MinGW 对中文路径的构建脚本链接有 `.drectve` 提示，命令成功，最终产物仍需检查资源及运行。
- release 应用在隔离截图模式成功启动并正常退出；转换页和设置页渲染正常，软件版本显示 0.2.3，引擎显示 9.0.2。
- 缩短单实例测试临时目录名，避免 macOS CI 临时目录叠加超过 Unix socket 路径长度上限。

## 分发与验收规则

本次分发 Windows x64 安装包、便携 ZIP、源码 ZIP，以及对应 SHA-256。源码包使用明确允许列表，不包含 Git 元数据、测试临时文件、用户偏好或内置引擎二进制。

构建脚本验证程序版本与 release 来源、包内 EXE/引擎哈希及引擎许可。包验收使用独立临时目录检查 ZIP CRC、PE 架构/GUI 子系统、嵌入 manifest 和真实转码；安装验收使用独立 QA AppId，验证安装、同目录覆盖、非目标文件保留和卸载，避免修改正式安装。验收机器记录保存在 `tools/qa-0.2.3`，不进入分发包。

发布前以 GitHub Actions 检查 Windows x64、macOS Intel 和 Apple Silicon。公开发布后，使用未经修改的 v0.2.2 更新后端检查并下载 0.2.3，与最终本地安装包比对大小、SHA-256 和 PE 格式；该验证不会执行正式安装器。发布页记录 CI 与最终下载验证结果。

## 验证边界

- 隔离 QA 安装不等于覆盖用户正式安装；不会自动升级本机已有软件。
- macOS 由 CI 验证编译与测试，未做 macOS 原生桌面交互测试；本次公开资产为 Windows 与源码包。
- Intel / AMD GPU 没有本机硬件实测。进程强杀或断电时的全部子孙进程及暂存目录回收不作保证。
- 应用未配置 Authenticode 签名。引擎许可及源码说明见 [内置引擎说明](ffmpeg-bundle.md)。
