# 帧流 Frameflow

[下载当前版本](https://github.com/FueTsui/Frameflow/releases/latest) · [GitHub 项目](https://github.com/FueTsui/Frameflow) · [反馈问题](https://github.com/FueTsui/Frameflow/issues)

一个用 Rust 编写的本地媒体处理桌面应用。按 Windows 11 Fluent 2 视觉语言组织侧栏导航、功能设置、系统字体与浅色 / 深色主题，让 FFmpeg 的常用操作更容易使用。

当前版本 **0.2.1**：八种功能使用各自的设置页签，合并重复的画质、音频和字幕入口，分别记住导出配置；移除鼠标悬浮提示。保留轨道编辑、音频参数、目标体积与两遍编码、硬件解码、GPU 缩放、画面合成、混音、无损裁剪及字幕处理。默认安装到 `C:\Programs\Frameflow`，内置 FFmpeg 与 FFprobe，支持引擎及软件更新。版本记录见 [更新日志](CHANGELOG.md)，功能边界见 [功能检查](FFmpeg功能检查-0.2.1.md)。

界面使用 `egui / eframe` 按 Fluent 2 设计规范绘制，使用系统标题栏和原生文件对话框，并非 WinUI 3 控件迁移。界面语言为简体中文，外观默认跟随系统；Windows 上优先使用 Segoe UI Variable、微软雅黑 UI 和 Segoe Fluent Icons，系统缺少对应字体时自动回退。媒体处理由本机的 `ffmpeg` 与 `ffprobe` 完成。文件在本地处理，无需上传。

功能界面预览：[转换](docs/screenshots/workflow-convert.png) · [压缩](docs/screenshots/workflow-compress.png) · [裁剪](docs/screenshots/workflow-trim.png) · [音频](docs/screenshots/workflow-audio.png) · [GIF](docs/screenshots/workflow-gif.png) · [截图](docs/screenshots/workflow-snapshot.png) · [封装](docs/screenshots/workflow-remux.png) · [字幕](docs/screenshots/workflow-subtitle.png)。每种功能都有「文件与任务」及对应设置页签；较长的设置可滚动，底部操作始终可见。应用设置位于左侧导航底部。

## 功能

| 左侧功能 | 设置页签 | 主要设置 |
| --- | --- | --- |
| 格式转换 | 转换设置 | 输出视频、画质与尺寸；音频、字幕及画面编辑 |
| 视频压缩 | 压缩设置 | 压缩目标、输出视频、尺寸与帧率；音频、字幕及画面编辑 |
| 片段裁剪 | 裁剪设置 | 时间范围、精确 / 无损裁剪及对应输出选项 |
| 音频处理 | 音频设置 | 时间范围、输出音频、音轨与混音 |
| 制作 GIF | GIF 设置 | 时间范围、动画尺寸与帧率、画面编辑；固定输出 GIF |
| 截取画面 | 截图设置 | 截取位置、输出图片及画面编辑 |
| 无损封装 | 封装设置 | 封装格式、音频、字幕；复制原始音视频编码 |
| 字幕提取 | 字幕设置 | 选择源字幕及 SRT / ASS / VTT / MKS 输出格式 |

支持文件选择与拖放、媒体信息检查、任务进度、取消任务，以及生成命令的查看与复制。界面不显示鼠标悬浮提示，图标按钮保留无障碍名称；字幕、字体和效果素材可点击「路径」展开完整路径，再点击「复制路径」。默认主题跟随系统，也可以在设置里选择浅色或深色。

时间输入示例：裁剪、提取音频和截取画面使用 `01:02:03:456`（1 小时 2 分 3 秒 456 毫秒）；GIF 使用 `65:250`（65 秒 250 毫秒）。毫秒显示三位，结束时间全零表示处理到文件末尾。输入格式有误时会提示，并阻止开始处理，避免误用旧时间。毫秒是输入精度；视频截图仍取现有帧，GIF 的帧边界受帧率与格式影响。

快捷键仍可使用：`Ctrl+O` 添加文件、`Ctrl+Enter` 开始处理（macOS 使用 `⌘`）。

导出默认保存到源文件所在文件夹，并附加操作名称；遇到重名会使用编号，保留已有文件。压缩结果受源文件、画质与编码器影响，不保证每次都比原文件更小。默认使用首个视频流和首条音轨；音轨可在导出设置中重选。无损封装要求目标容器兼容原编码，不应用时间裁剪或画面滤镜。

八种功能分别保存导出配置。切换功能时恢复该功能上次的格式、画质、时间等设置，避免 GIF 的帧率或压缩的目标体积带入其他功能。需要额外素材的文件选择仅保留在当前会话，重启后需重新添加。

先在「文件与任务」选择素材，再打开对应设置页签。常用参数直接显示，音频、字幕等可按需展开；轨道列表对应当前选中文件，设置应用于本批任务。每个文件会校验所选轨道是否存在，失败时显示原因。

「无损封装 → 封装设置 → 音频 → 外部音轨」可添加多个文件，每个文件的首条音轨成为独立可切换轨道，从各自起点开始并保留完整时长。需要合成为一条音轨时，在转换、压缩、精确裁剪的「音频」或音频处理的「音轨与混音」中启用混音。

本应用提供明确的桌面工作流，未封装 FFmpeg 的全部命令行能力。任意滤镜图、直播推流、字幕内容编辑等仍需其他工具；参数含义可查阅 [FFmpeg 命令文档](https://ffmpeg.org/ffmpeg.html) 与 [滤镜文档](https://ffmpeg.org/ffmpeg-filters.html)。

## 编码与压缩

左侧顶部的菜单图标可收起或展开导航，无文字描述或悬浮提示；状态会保存。

在转换、压缩、精确裁剪的对应设置页签中，通过「输出视频」选择容器、H.264 / H.265 与 CPU / GPU；「硬件选项」集中提供解码、显卡编号及 GPU 缩放。程序通过短编码检测 NVIDIA NVENC、Intel QSV、AMD AMF；更换驱动或引擎后可在设置中重新检测。GPU 能力还受输入编码、画面尺寸和参数限制，失败时显示错误，可手动改用 CPU。

MP4、MKV、MOV 支持 H.264 / H.265。WebM 使用 CPU / VP9，选择 WebM 时界面会显示这一限制。音频、GIF、截图和无损封装不使用此视频编码选项。

视频压缩的「压缩目标」是画质、目标码率、目标体积和两遍编码的唯一入口，画面尺寸和帧率在下方调整。转换与精确裁剪将这些参数集中在「画质与尺寸」中。切换格式或执行设备后，即使分组未展开，也会同步调整不适用的选项。

音频处理的「输出音频」集中提供格式、码率 / VBR、采样率、声道和无损位深，支持 MP3、AAC / M4A、AC3、FLAC、ALAC、TTA、WAV、OGG；源音轨与混音单独归入「音轨与混音」。视频中的音轨选择、编码参数和混音则集中在「音频」中。

主要参数规则：

- **视频控制**：质量、目标码率或目标体积（十进制 MB）。质量提供高清、均衡、小体积预设及数值调整。目标体积按选定时长估算，预留 2% 容器开销并扣除输出音轨码率；不是精确文件大小承诺。使用目标体积时，音频自动使用码率模式。CPU 在目标码率或体积模式下可启用两遍编码，分别显示分析和输出进度。
- **硬件解码**：关闭、CUDA、D3D11VA 或 QSV；显卡编号留空自动选择，`0` 表示第一张。NVIDIA 使用 CUDA/NVENC 编号，Intel/AMD 使用 Windows 适配器编号。解码与编码需兼容所选硬件。
- **GPU 缩放**：选择 NVIDIA 编码及明确输出高度后启用。旋转、水印、合成与字幕烧录仍使用软件滤镜；并非整个处理过程都在 GPU 上运行。
- **音频参数**：默认保留源采样率、声道；可选项随格式调整。MP3 最多双声道 / 48 kHz，AC3 最多 6 声道及 32 / 44.1 / 48 kHz，AAC 最高 96 kHz；WebM 的 Opus 音轨可指定 8 / 12 / 16 / 24 / 48 kHz。音频处理支持纯音频输入和从视频提取一条所选音轨。
- **音频质量**：有损格式可选码率或质量 / VBR，质量值越高通常越清晰。码率为 32–320 kbps，OGG 最低 64 kbps；AC3 仅有码率模式。FLAC、ALAC、TTA 为无损编码，WAV 为 PCM；位深可选自动、16、24、32 位，ALAC / TTA 仅接受 16 / 24 位。WAV 自动档使用 16 位，其余自动档遵循编码器默认，不能保证原位深不变。

本机已实测 RTX 3060 的 H.264 / H.265 编码，以及设备 `0` 的 CUDA 解码和 GPU 缩放。Intel / AMD 路径已实现，但尚无对应硬件实测。所有模式均不能恢复已经丢失的画质或音质。

## 轨道、字幕与无损裁剪

在对应设置页签的「音频」中选择源音轨，在「字幕」中选择源字幕，编辑语言代码、标题及默认轨。音频处理的源音轨位于「音轨与混音」。视频输出和无损封装可保留多音轨；独立音频输出和混音的原声音轨只选择一条。没有勾选的源字幕不会自动保留。

| 字幕方式 | 入口与规则 |
| --- | --- |
| 保留字幕 | 在同一「字幕」分组中勾选源字幕并添加外部 SRT / ASS / SSA / VTT。MKV 保留原字幕；MP4 / MOV 转为 mov_text，WebM 转为 WebVTT，转换可能丢失样式 |
| 烧录字幕 | 在「字幕」选择烧录及一条内嵌文本字幕，或点击「外部字幕…」；外部选择优先。支持字体文件夹，需重新编码。裁剪时按原字幕时间轴处理 |
| 字幕提取 | 在「字幕设置 → 选择字幕」选择一条源字幕，再在「字幕格式」选择 SRT / ASS / VTT / MKS；图形字幕仅可复制为兼容的 MKS，不做 OCR 或文本转换 |
| 字体附件 | MKV 输出时，在「字幕 → 字体附件」添加 TTF / TTC / OTF，或保留源附件；其他容器不显示此选项 |

图形字幕可保留到兼容的 MKV / MKS，不能直接烧录或转为文本。外部字幕、字体和效果素材路径仅保留在当前会话；恢复设置后需重新选择。

「片段裁剪 → 裁剪设置 → 裁剪方式 → 无损裁剪」使用流复制，速度快且不重新压缩。开始位置受关键帧限制，输出可能包含邻近帧，实际时长也可能有偏差；毫秒输入不代表无损模式能逐帧精确裁剪。无损模式仅显示可用的格式、音轨和字幕选项，不显示画质、缩放、混音或烧录字幕，目标容器仍须兼容原编码。

## 画面编辑与混音

转换、压缩、精确裁剪、GIF 和截图的「画面编辑」中提供画面效果；音频混音位于「音频」或「音轨与混音」，与音轨选择相邻：

- **旋转与翻转**：顺 / 逆时针 90°、180°，水平或垂直翻转。
- **图片水印**：选择图片，调整位置、宽度比例、不透明度与边距。
- **画面合成**：左右并排、上下排列或画中画，含主画面最多 4 个视频。附加视频从各自开头播放，短视频定格至主片段结束；附加视频的音轨不会自动混入。
- **音频混音**：选择是否保留原声，并加入最多 8 个音频素材，分别调整音量、开始时间与时长。开始时间相对导出片段，时长 `0` 表示至结束；最终输出一条混音轨，空缺部分补静音。

画面效果用于视频转码、GIF 和截图；混音用于视频转码及音频处理。合成和混音时长以主文件所选片段为准，均不用于无损封装或无损裁剪。

## 软件更新与覆盖安装

「设置」中，软件更新位于 FFmpeg 更新右侧；窄窗口自动改为上下排列。Windows x64 默认自动检查 [GitHub 稳定版本](https://github.com/FueTsui/Frameflow/releases/latest)，可分别开关软件与引擎的自动检查。

发现更高版本后点击「下载更新」，程序校验对应安装包的 SHA-256 及安装器格式，再显示「安装并退出」。安装器默认沿用当前程序目录；已有任务完成后才可更新。安装将退出 Frameflow，当前文件队列不保留，导出设置继续保存。检查和下载不会自动执行程序，不要求填写 GitHub 凭据。

安装包支持在已有目录覆盖同版本或旧版程序，记住上次安装位置；首次安装默认 `C:\Programs\Frameflow`。选择其他现有目录时直接使用该目录，不额外追加子文件夹。覆盖只更新随包文件，不清空目录或删除用户素材；占用的应用通过安装向导处理。软件更新下载缓存位于 `%LOCALAPPDATA%\Frameflow\updates`。

便携用户也可手动下载新便携包覆盖原目录；若使用应用内安装，会在当前目录进行常规安装。仅发布正式稳定版本时提供自动更新；相同版本号不会反复提示，尚无联网或 GitHub 暂时不可用时可稍后重试。

## 内置与自定义 FFmpeg

Windows 安装包和便携包已经包含 `ffmpeg.exe` 与 `ffprobe.exe`，默认自动识别，无需额外安装。来源、版本与许可见 [内置引擎说明](docs/ffmpeg-bundle.md)。左侧“设置”页可选择其他 FFmpeg 文件夹，也可恢复自动检测。

Windows x64 默认在启动后自动检查 FFmpeg 更新，可在设置中关闭。发现新版本后点击“下载并更新”，程序会从 Gyan 下载稳定版，校验 SHA-256、架构与两个工具的版本后启用。更新期间不启动媒体任务；正在处理时不能安装更新。检查和下载失败均可重试，原有引擎继续可用。帮助与使用文档也位于设置页。

更新引擎保存在 `%LOCALAPPDATA%\Frameflow\engines`，不覆盖安装目录或自定义引擎。手动选择的引擎优先；恢复自动检测后，优先使用已更新版本。更新只联网获取版本和程序，不上传媒体文件。

macOS 或需要自定义引擎时，可从 [FFmpeg 官方下载页](https://ffmpeg.org/download.html) 选择适合系统和 CPU 架构的版本。

### Windows

如要使用自己的 FFmpeg 版本：

1. 从官方下载页列出的 Windows 构建来源下载并解压。
2. 将包含 `ffmpeg.exe` 和 `ffprobe.exe` 的 `bin` 文件夹加入 `PATH`，或在应用设置里选择该文件夹。
3. 也可以将这两个文件放在 `frameflow.exe` 同目录，或其 `tools` / `bin` 子目录中。

打开新的 PowerShell，检查是否安装成功：

```powershell
ffmpeg -version
ffprobe -version
```

### macOS

已安装 Homebrew 时，使用包含 Vorbis 和字幕烧录支持的 [FFmpeg 完整软件包](https://formulae.brew.sh/formula/ffmpeg-full)：

```bash
brew install ffmpeg-full
export PATH="$(brew --prefix ffmpeg-full)/bin:$PATH"
ffmpeg -version
ffprobe -version
```

`ffmpeg-full` 不自动链接到默认命令目录。从 Finder 启动应用时，请在设置中选择 `brew --prefix ffmpeg-full` 输出路径下的 `bin` 文件夹。应用也会检查 `/opt/homebrew/bin` 和 `/usr/local/bin`；所用编码器和容器的可用性取决于实际安装的 FFmpeg 构建。

## 本地开发

安装 [Rust stable](https://rust-lang.org/tools/install/)。Windows 使用 MSVC 工具链，并安装 Visual Studio Build Tools 的「使用 C++ 的桌面开发」工作负载与 Windows SDK；macOS 安装 Xcode Command Line Tools：

```bash
xcode-select --install
```

在项目目录运行：

```bash
cargo run --release
```

首次构建会下载 Rust 依赖。启动界面不要求 FFmpeg 已安装；开始媒体处理前请完成上述 FFmpeg 配置。

常规质量检查：

```bash
cargo fmt --all -- --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
```

本次 Windows 版本的验证结果见 [发布验收记录](docs/windows-release-verification.md)。回归测试包含媒体处理、无损音视频哈希、字幕 / 音频列表操作、浅色 / 深色主题与最小窗口布局。macOS 构建脚本保留，本次未生成或验证 macOS 安装包。

## 打包

### Windows 安装包与便携包

在 Windows 项目目录执行：

```powershell
./scripts/build-windows.ps1
```

Windows x64 发布产物为 `dist/Frameflow-<版本>-windows-x64.zip` 和 `dist/Frameflow-<版本>-windows-x64-setup.exe`，并附各自的 SHA-256 校验文件。便携包解压后运行 `frameflow.exe`；安装包默认位置为 `C:\Programs\Frameflow`，可在安装向导中修改路径，在当前用户开始菜单创建入口，并提供正常卸载。两种包均内置 FFmpeg 与 FFprobe。

构建脚本默认读取本地 `tools/bin` 的引擎，也可使用 `-FFmpegDirectory 'C:\path\to\ffmpeg\bin'` 指定来源；源码 ZIP 不包含第三方引擎二进制。

安装包使用 Inno Setup 6 编译。GNU 工具链也可通过 `-Target x86_64-pc-windows-gnu` 构建。脚本还支持 `-Target aarch64-pc-windows-msvc`，前提是已安装该 Rust 目标与对应 ARM64 MSVC 编译工具；本次只验证 Windows x64。

### macOS 应用包

在 Mac 项目目录执行，默认使用本机 CPU 架构：

```bash
bash scripts/build-macos.sh
```

也可以指定已安装的目标：

```bash
rustup target add aarch64-apple-darwin
bash scripts/build-macos.sh aarch64-apple-darwin

rustup target add x86_64-apple-darwin
bash scripts/build-macos.sh x86_64-apple-darwin
```

输出为 `dist/Frame-<版本>-macos-arm64.zip` 或 `dist/Frame-<版本>-macos-x64.zip`。压缩包内包含 `Frame.app`，可移入「应用程序」。脚本设置最低部署版本为 macOS 11.0；旧版系统的实际兼容性仍需在对应系统验证。App 图标取自 `assets/Frameflow.ico`，打包需要 macOS 自带的 `sips` 与 `iconutil` 工具。

构建脚本产物未经过开发者证书签名或 Apple 公证。面向公开分发时，需要另行配置平台签名与公证。

安装时，Apple Silicon Mac 选择 `macos-arm64.zip`，Intel Mac 选择 `macos-x64.zip`；解压并将 `Frame.app` 拖到「应用程序」，然后配置 FFmpeg。首次打开可能遇到开发者验证提示。确认文件来自可信来源且校验值一致后，可按照 [Apple 官方说明](https://support.apple.com/zh-cn/102445)，在尝试打开后进入「系统设置 → 隐私与安全性」，选择「仍要打开」，仅为这个应用设置例外。

### 在 Windows 交叉构建 macOS

仓库也提供 Windows 交叉构建脚本，需要 Rust stable、两个 Apple 目标，以及包含系统库链接描述文件的 macOS SDK。SDK 不随项目分发；`SDKROOT` 的用途见 [Rust 官方 macOS 目标说明](https://doc.rust-lang.org/rustc/platform-support/apple-darwin.html)。

```powershell
rustup target add aarch64-apple-darwin x86_64-apple-darwin
./scripts/build-macos-cross.ps1 -SdkRoot 'C:\SDKs\MacOSX14.5.sdk'
```

脚本使用 Rust 工具链内的 LLD 生成真正的 Mach-O 可执行文件，最低部署版本设为 macOS 11.0。默认构建两个架构；可用 `-Target aarch64-apple-darwin` 或 `-Target x86_64-apple-darwin` 单独构建。交叉构建完成后，使用 Python 3.11 以上和 Pillow生成应用包：

```powershell
python -m venv tools/cross/package-env
./tools/cross/package-env/Scripts/python.exe -m pip install Pillow
./tools/cross/package-env/Scripts/python.exe scripts/package-macos.py --target aarch64-apple-darwin
./tools/cross/package-env/Scripts/python.exe scripts/package-macos.py --target x86_64-apple-darwin
```

打包脚本默认读取 `target/<目标>/release/frameflow`，也可通过 `--binary` 指定预先构建的文件。它检查 Mach-O 架构、部署版本、系统动态库依赖，并逐页验证已有的 ad-hoc 签名；Apple Silicon 文件必须具有有效签名。图标由 `assets/Frameflow.ico` 生成多尺寸 ICNS，ZIP 内保留主程序的 Unix 可执行权限。

输出名称与 Mac 本机构建一致，另附 `.zip.sha256` 校验文件；ZIP 内的 `BUILD_INFO.json` 记录架构、动态库和静态验证结果。链接器的 ad-hoc 签名不代表 Developer ID 签名或 Apple 公证。静态检查不能代替实际 Mac 上的启动、文件对话框与媒体处理验证。

Mac 交叉构建使用 macOS 14.5 SDK 的链接描述文件；可执行文件的 SDK 声明字段为 Rust 交叉链接时写入的 `11.0.0`，在 `BUILD_INFO.json` 中以 `declared_sdk` 单独记录。两个架构的最低部署版本均为 macOS 11.0。

### GitHub Actions

`.github/workflows/build.yml` 会在推送、拉取请求或手动运行时执行格式检查、Clippy、测试和 release 打包。Windows x64、Apple Silicon 与 Intel Mac 分别使用原生运行器，完成后可在该次 Actions 运行的 Artifacts 中下载 ZIP。工作流只生成构建产物，不创建公开 Release。运行器标签参照 [GitHub 官方支持列表](https://docs.github.com/en/actions/reference/runners/github-hosted-runners)。

### 源码包

执行 `python scripts/package-source.py` 可生成 `dist/Frameflow-<版本>-source.zip` 及 SHA256 校验文件，需要 Python 3.11 以上。脚本仅收录项目源码、依赖锁定文件、文档、图标和构建脚本，不收录本地构建目录或 FFmpeg / SDK 工具。

## 项目结构

```text
src/                        Rust 界面、主题、图标和媒体任务实现
src/encoding.rs             码率、音频参数、两遍编码与硬件管线
src/tracks.rs               源轨道、字幕提取 / 烧录与字体附件
src/effects.rs              旋转、水印、画面合成与混音
src/app_updater.rs          GitHub 软件更新下载与安装校验
src/software_update.rs      软件更新界面及异步状态
assets/Frameflow.ico        应用多尺寸图标
scripts/build-windows.ps1   Windows 便携 ZIP 打包
scripts/build-macos.sh      macOS .app 和 ZIP 打包
scripts/build-macos-cross.ps1 Windows 交叉构建 macOS Mach-O
scripts/package-macos.py    跨平台检查与打包预构建 macOS 程序
scripts/package-source.py   按项目文件清单生成源码 ZIP
scripts/macos-icon.swift    辅助图标脚本（当前打包未使用）
.github/workflows/build.yml 跨平台检查与构建
```

本应用源码以 [MIT License](LICENSE) 提供，Rust 依赖及内置引擎的许可说明见 [第三方声明](THIRD_PARTY_NOTICES.md)。Windows 包随附独立 FFmpeg / FFprobe 程序及其许可与来源说明；源码 ZIP 仅包含帧流源码和文档，不包含 FFmpeg 二进制或第三方对应源码。FFmpeg 的许可取决于其构建配置，详情参阅 [FFmpeg 许可说明](https://ffmpeg.org/legal.html)。本应用不代表 FFmpeg 官方项目。
