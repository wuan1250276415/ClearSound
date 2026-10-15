# 任务

澄音 0.1.0：Windows 11 本地音乐播放器，界面为简体中文。

## 已完成

- Tauri 2 + React + TypeScript 前端，Rust 后端。
- FLAC、MP3、WAV、M4A/ALAC 用 Symphonia 解码，cpal 播放，rubato 重采样。
- DSF/DFF/APE 走独立 FFmpeg 进程。DSD 先转为 176.4 kHz PCM。
- CUE 分轨，支持 UTF-8、UTF-16、GBK、Shift-JIS 等编码，偏移按 75 帧/秒。
- 文件夹递归扫描，SQLite 持久化，按专辑、艺术家、文件夹浏览，搜索。
- 队列、播放列表、播放/暂停/跳转/上一首/下一首/音量/随机/循环。
- 内嵌或文件夹封面；标题、艺术家、专辑、曲序、时长、采样率、位深。
- 同一输出流上的无缝衔接（采样率相同且不跳转时最好）。
- Windows 媒体键 / SMTC（souvlaki，仅 Windows 编译）。
- GitHub Actions：`windows-latest` 打 NSIS 与 MSI，上传产物，`v*` 标签发布。
- `files.md`、`task.md`、`changelog.md` 与本文件同步。

## 未能在此环境完成

- 这个草稿仓库在 Origin 上，不能运行 GitHub Actions，也不能发布 v0.1.0 安装包。工作流和打包配置已经写好，需要在 GitHub 的 `windows-latest` 上跑。
- 本机没有 Windows，也没有声音设备。播放进度在无设备时仍会走动，但听不到声音。
- 没有安装包下载地址。

## 已知限制

- DSD 转为 PCM，不是原生 DSD、DoP 或 ASIO。
- 安装包未签名，SmartScreen 会警告。
- 单曲循环会清空缓冲再从头播放，边界上有一小段空隙。
- 采样率变化时会刷新重采样器，曲目边界可能有极短间隙。
- WebView2 使用在线引导安装。
- FFmpeg 为 GPL 构建，只在 Windows 安装包中附带，不提交到仓库。
- WiX 语言设为 zh-CN。若 CI 因缺少语言包失败，把 `tauri.windows.conf.json` 里的 `wix.language` 改成 `en-US`。NSIS 仍保留简体中文。
