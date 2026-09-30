# Frameflow 0.2.1 功能覆盖检查

更新日期：2026-09-30。本报告依据当前源码与 [FFmpeg 官方命令文档](https://ffmpeg.org/ffmpeg.html) 检查桌面工作流。上一轮列出的 **3 项「优先」和 4 项「次优先」均已实现**，版本保持 **0.2.1**；这不代表已封装 FFmpeg 的全部能力。

## 优先与次优先功能

八种功能各有对应的设置页签和独立保存的导出配置。先在「文件与任务」选择素材，再打开转换、压缩、裁剪、音频、GIF、截图、封装或字幕设置。轨道列表对应当前选中的素材，设置应用于本批任务；不兼容的参数或不存在的轨道会报告错误。

| 原顺序 | 功能 | 当前入口与实现 | 限制与依据 |
| --- | --- | --- | --- |
| 优先 · 已实现 | 源音轨 / 字幕选择、语言与默认轨 | 视频的「音频」「字幕」分别管理对应轨道；音频处理的源音轨在「音轨与混音」。可编辑语言、标题和默认轨，视频输出可选多音轨 | 独立音频输出和混音原声只选一条；默认仍为首条音轨、不选源字幕。使用明确流编号，逐文件校验。[流选择](https://ffmpeg.org/ffmpeg.html#Stream-selection-1) |
| 优先 · 已实现 | 音频采样率、声道、位深与质量 | 「音频设置 → 输出音频」集中选择格式、码率 / VBR、采样率、声道和无损位深；视频音轨在「音频」中调整 | 参数按格式限制；AC3 无质量模式，ALAC / TTA 不接受 32 位，WAV 自动为 16 位，自动位深不保证原样保留。[音频选项](https://ffmpeg.org/ffmpeg.html#Audio-Options) |
| 优先 · 已实现 | 视频目标码率、体积与两遍编码 | 压缩设置中的「压缩目标」是唯一控制入口；转换和精确裁剪集中在「画质与尺寸」。CPU 在码率或体积模式下可启用两遍编码 | 体积按时长、音轨码率及 2% 容器开销估算，非精确承诺；目标体积自动采用音频码率模式；GPU 不使用两遍流程。[视频选项](https://ffmpeg.org/ffmpeg.html#Video-Options) |
| 次优先 · 已实现 | 硬件解码、GPU 滤镜与显卡选择 | 「输出视频 → 硬件选项」选择 CUDA / D3D11VA / QSV 解码、显卡编号及 NVIDIA GPU 缩放 | GPU 滤镜当前提供缩放，需要 NVIDIA 编码和指定高度；其他编辑滤镜使用软件处理。驱动与素材编码决定实际可用性。[高级视频选项](https://ffmpeg.org/ffmpeg.html#Advanced-Video-options) |
| 次优先 · 已实现 | 旋转、翻转、水印、画面合成与混音 | 「画面编辑」提供旋转、翻转、水印和并排 / 画中画合成；混音在「音频」或「音轨与混音」中，与音轨选择相邻 | 最多 4 个画面、8 个附加音频；短视频定格、音频补静音，总时长按主片段；附加视频音轨不自动混入。不提供任意滤镜图编辑。[滤镜](https://ffmpeg.org/ffmpeg.html#Filtering) |
| 次优先 · 已实现 | 无损片段裁剪 | 「裁剪设置 → 裁剪方式 → 无损裁剪」按时间范围复制媒体流，仅显示该模式可用选项 | 受关键帧边界限制，可能包含邻近帧或有时长偏差；不应用画质、滤镜、混音和字幕烧录；容器必须兼容原编码。[流复制](https://ffmpeg.org/ffmpeg.html#Streamcopy) |
| 次优先 · 已实现 | 字幕提取 / 保留、烧录、字体附件 | 「字幕设置」提供选择字幕及输出格式；视频的「字幕」将源字幕、外部字幕、烧录、字体目录和 MKV 附件合并管理 | 文本字幕可导出 SRT / ASS / VTT / MKS；图形字幕仅可复制到兼容 MKV / MKS，不做 OCR 或烧录；字体附件支持 TTF / TTC / OTF，并可保留源附件。[主选项](https://ffmpeg.org/ffmpeg.html#Main-options)、[高级选项](https://ffmpeg.org/ffmpeg.html#Advanced-options) |

## 功能布局检查

- **按功能组织**：八个对应设置页签分别保存配置，切换后恢复各自的格式、时间和处理参数。
- **单一参数入口**：压缩目标只调整一次；转换 / 精确裁剪的画质与尺寸同组；音频格式与参数、源字幕与外部字幕分别集中管理。
- **只显示适用选项**：GIF 不提供固定格式选择框；无损裁剪和封装不显示重新编码参数；容器和处理方式决定字幕烧录、字体附件与硬件选项。
- **无悬浮提示**：图标保留无障碍名称，说明和错误直接显示；字幕、字体及效果素材可显式展开和复制完整路径。
- **实现边界**：采用 Fluent 2 视觉布局，保留 Rust / egui 内容绘制、Windows 系统标题栏和原生文件选择器，并未迁移到 WinUI 3。

预览：[转换](docs/screenshots/workflow-convert.png) · [压缩](docs/screenshots/workflow-compress.png) · [裁剪](docs/screenshots/workflow-trim.png) · [音频](docs/screenshots/workflow-audio.png) · [GIF](docs/screenshots/workflow-gif.png) · [截图](docs/screenshots/workflow-snapshot.png) · [封装](docs/screenshots/workflow-remux.png) · [字幕](docs/screenshots/workflow-subtitle.png)。

## 使用与验证边界

- **音频**：默认保留源采样率和声道，不再固定为 48 kHz 双声道。MP3 最多双声道 / 48 kHz；AC3 最多 6 声道以及 32 / 44.1 / 48 kHz；AAC 最高 96 kHz。有损码率为 32–320 kbps，OGG 最低 64 kbps。视频音轨也使用所选音频参数。
- **字幕**：MKV 可保留 ASS / SSA 样式，字体需按需要附加；MP4 / MOV 转为 mov_text，WebM 转为 WebVTT，转换可能丢失样式。烧录支持内嵌或外部文本字幕，外部选择优先；裁剪时按原时间轴偏移。无字幕 / 烧录模式不合并「外部字幕」列表中的软字幕。
- **硬件**：本机已实测 RTX 3060 的 H.264 / H.265 编码、设备 `0` 的 CUDA 解码和 GPU 缩放。Intel / AMD 路径已实现，尚未在对应硬件上实测；编码器短探测通过不代表所有素材和分辨率都可处理。实际失败会显示原因。
- **两遍与体积**：CPU H.264 / H.265 两遍流程已实测。目标体积仍是估算，字幕、附件、编码器误差等都会影响实际大小。两遍任务分阶段显示进度，取消或失败不发布未完成输出。
- **时间与输出**：毫秒是输入精度，视频仍受帧边界限制，无损裁剪另受关键帧限制。输出保留源文件，遇到重名使用编号。FFmpeg 更新、内置引擎、折叠菜单与单一输出目录入口保留。

媒体功能回归范围包括实际编码、源轨道元数据、字幕提取与裁剪时间轴、烧录画面、字体附件、画面滤镜、音频混音及两遍流程。当前界面、完整测试与分发包核验结果以 [发布验收记录](docs/windows-release-verification.md) 为准。安装器是否实际执行安装、不同硬件和 macOS 是否验证，分别以该记录为准，不由源码实现推定。

## 后续仍未提供的功能

| 功能 | 当前边界与官方依据 |
| --- | --- |
| 更多视频编码器、10 位像素格式、HDR / 色彩控制、编码预设 | 界面提供 H.264 / H.265，以及 WebM 的 VP9；未提供用户可选 10 位 / HDR 管线或完整编码预设，不能承诺 HDR 保真。[高级视频选项](https://ffmpeg.org/ffmpeg.html#Advanced-Video-options)、[预设](https://ffmpeg.org/ffmpeg.html#Preset-files) |
| 完整元数据、章节和任意附件管理 | 已有轨道语言 / 标题 / 默认轨和字体附件；容器全局元数据、章节编辑、任意附件提取与编辑未提供。[高级选项](https://ffmpeg.org/ffmpeg.html#Advanced-options) |
| 图像序列导入、多帧导出、单输入多种输出 | 截图一次一帧，任务一次输出一种格式。[文件格式转换示例](https://ffmpeg.org/ffmpeg.html#Video-and-Audio-file-format-conversion) |
| 摄像头 / 屏幕采集、网络输入、推流 | 工作流限本地文件。[采集示例](https://ffmpeg.org/ffmpeg.html#Video-and-Audio-grabbing) |
| 通用高级参数、任意滤镜图、码流过滤器与逐帧诊断 | 命令支持查看和复制，未提供任意 AVOption / filtergraph / bitstream filter 编辑。[AVOptions](https://ffmpeg.org/ffmpeg.html#AVOptions)、[高级选项](https://ffmpeg.org/ffmpeg.html#Advanced-options) |

## 使用说明检查

README 已同步八种设置页签、新分组入口、独立配置、无悬浮提示交互及显式路径查看方式，并保留参数限制与硬件验证范围。界面设计依据及预览见 [Windows 11 Fluent 2 设计说明](docs/windows-11-design.md)。版本和默认安装路径仍为 `0.2.1`、`C:\Programs\Frameflow`。

主要实现位于 `src/encoding.rs`、`src/tracks.rs`、`src/effects.rs`；`src/media.rs` 负责计划、两遍执行与输出发布，`src/app.rs` 负责界面、文件选择与任务队列。

## GitHub 发布与软件更新

公开项目：[FueTsui/Frameflow](https://github.com/FueTsui/Frameflow)。设置页新增独立的软件自动检查与校验后安装流程，沿用当前目录；安装器支持已有路径覆盖。软件更新不替代 FFmpeg 引擎更新。底部状态栏和菜单描述已移除，帮助链接位于关于信息右侧。
