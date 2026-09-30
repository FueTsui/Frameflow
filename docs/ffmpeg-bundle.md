# 内置 FFmpeg

Windows 安装包和便携包默认包含 `tools/ffmpeg.exe` 与 `tools/ffprobe.exe`。程序自动识别该目录，安装或解压后即可使用。左侧“设置”仍允许选择自己的 FFmpeg 文件夹。

## 当前默认引擎

- 版本：`9.0.2-essentials_build-www.gyan.dev`，Windows x64 静态构建。
- 构建提供者：Gyan Doshi，见 [Gyan FFmpeg builds](https://www.gyan.dev/ffmpeg/builds/)。
- 对应版本发布页：[FFmpeg 9.0.2 builds](https://github.com/GyanD/codexffmpeg/releases/tag/9.0.2)。
- 该发布页记录的 FFmpeg 源码提交：[946fcce07b6dcd0331c8cc609192aeff5e1924f8](https://github.com/FFmpeg/FFmpeg/commit/946fcce07b6dcd0331c8cc609192aeff5e1924f8)。[FFmpeg 9.0.2 上游源码归档](https://ffmpeg.org/releases/ffmpeg-9.0.2.tar.xz)也可单独取得。
- 两个程序的 `-L` 输出声明 GNU GPL 第 3 版或更新版本；完整许可文本保存在 [FFmpeg-GPL-3.0.txt](FFmpeg-GPL-3.0.txt)。该文件保留完整 GPLv3 许可文本。

当前引擎来自 Gyan 官方版本 ZIP，下载后按发布方 SHA-256 校验，再提取两个程序；没有修改或重新编译。构建脚本保存实际 `-version`、`-buildconf`、`-L` 输出和文件 SHA-256，便携包中的程序与已校验归档逐文件核对。

安装包和便携包包含以下记录：

| 文件 | 内容 |
| --- | --- |
| `BUILD_INFO.json` | Frameflow 与两个工具的版本、架构和 SHA-256；内置状态与默认安装路径 |
| `tools/ffmpeg-version.txt`、`tools/ffprobe-version.txt` | 实际程序版本、编译器与库版本 |
| `tools/ffmpeg-buildconf.txt`、`tools/ffprobe-buildconf.txt` | 实际编译选项，包括启用的外部库 |
| `tools/ffmpeg-license.txt`、`tools/ffprobe-license.txt` | 实际程序输出的许可声明 |
| `tools/SOURCE_INFO.json` | 默认构建的提供者、发布页、上游源码提交；明确标记没有捆绑对应源码 |
| `docs/FFmpeg-GPL-3.0.txt` | 完整 GPLv3 许可 |
| `tools/provider-notices/`（如有） | 工具目录或其父目录已有的提供者 README、许可和声明 |

## 构建与替换

### 应用内更新

Windows x64 在启动时自动检查一次稳定版，可在设置中关闭，也可随时手动检查。版本信息和 SHA-256 来自 [Gyan 官方构建页及其 API](https://www.gyan.dev/ffmpeg/builds/#api)。点击“下载并更新”后，程序下载与该版本对应的固定 ZIP，校验 SHA-256，并检查 FFmpeg / FFprobe 架构与版本。

更新保存在 `%LOCALAPPDATA%\Frameflow\engines`，通过指针切换已验证的引擎，不改写安装目录。失败不会切换或删除原引擎。自定义目录始终优先；使用新版本时可恢复自动检测。媒体任务运行时禁用安装更新，更新安装期间禁用开始处理。此功能不更新 Frameflow 本身，其他平台需自行管理 FFmpeg。

### 重新打包

默认从工作区 `tools/bin` 读取这两个程序。也可给 `scripts/build-windows.ps1` 传入 `-FFmpegDirectory 'D:\FFmpeg\bin'`，选择匹配目标架构的静态 GPLv3 构建。两个文件都必须存在、可运行且报告相同版本；缺少任一文件、架构不匹配、命令失败或超时都会阻止发布新包。替换为其他许可证或共享库构建前，应相应调整打包脚本与随附文档。

构建脚本先校验源程序，复制到共同的包目录，再由该目录生成便携 ZIP 和安装包。便携 ZIP 中两份工具的哈希会再次核对。生成包后，可查看 `BUILD_INFO.json` 中的 `ffmpeg`、`ffprobe` 对象确认实际使用的构建；如果覆盖了默认引擎，应以包内实际版本与提供者记录为准。

## 源码范围

可取得[固定提交的 FFmpeg 核心源码](https://github.com/FFmpeg/FFmpeg/archive/946fcce07b6dcd0331c8cc609192aeff5e1924f8.tar.gz)。包内提供者 README 记录外部库版本，实际 `ffmpeg-buildconf.txt` 记录启用选项。Gyan 发布仓库的 GitHub 自动生成 Source code 归档属于其支持仓库，不能当作 FFmpeg 构建源码。未找到与该版本固定对应的全部外部依赖及构建脚本快照，因此不把一般构建项目描述为已验证的本版对应脚本。


`Frameflow-*-source.zip` 是 Frameflow 应用源码包，不包含 FFmpeg 二进制或 FFmpeg 及所有外部依赖的对应源码。上面的源码链接指向 FFmpeg 上游；它们不表示已归档 Gyan 静态构建中的全部外部库源码和构建脚本。源码提供与分发要求参见随附 GPLv3 第 1、6 节及 [FFmpeg 官方许可说明](https://ffmpeg.org/legal.html)。这些版本、许可和来源记录用于识别随包引擎，不代表完成了面向公开分发的完整对应源码审计。
