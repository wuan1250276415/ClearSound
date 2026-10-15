# 任务

澄音 0.3.0：Windows 11 本地音乐播放器。界面为简体中文，带滚动歌词和在线封面。

## 已完成

- Tauri 2 + React + TypeScript 前端，Rust 后端。
- FLAC、MP3、WAV、M4A/ALAC 用 Symphonia 解码，cpal 播放，rubato 重采样。
- DSF/DFF/APE 走独立 FFmpeg 进程。DSD 先转为 176.4 kHz PCM。
- CUE 分轨，支持 UTF-8、UTF-16、GBK、Shift-JIS 等编码，偏移按 75 帧/秒。
- 文件夹递归扫描，SQLite 持久化，按专辑、艺术家、文件夹浏览，搜索。
- 队列、播放列表、播放/暂停/跳转/上一首/下一首/音量/随机/循环。
- 封面来源依次是内嵌图（ID3 APIC、FLAC PICTURE、M4A covr、DSF ID3）、文件夹里的 cover/folder/front 图片，然后才是在线封面。
- 在线封面按艺术家和专辑查询：iTunes Search，其次 MusicBrainz 与 Cover Art Archive，再其次网易云专辑图。结果缩略后缓存在应用数据目录，只写入还没有封面的曲目，并显示在播放条、歌词视图、专辑网格和曲目列表。Windows SMTC 使用同一封面文件。
- 歌词：同名 `.lrc`；CUE 曲目优先曲名或分轨 `.lrc`，否则用专辑 `.lrc` 并按 CUE 起点平移。没有旁路文件时读取 ID3 USLT/SYLT、FLAC/Vorbis `LYRICS` / `UNSYNCEDLYRICS`、M4A `©lyr`。
- 仍没有歌词时，可自动或手动在线匹配。优先 LRCLIB 的同步歌词，其次网易云音乐搜索和歌词接口。请求从 Rust 发出，带超时和 User-Agent，并缓存在应用数据目录。
- 歌词面板可「搜索歌词」或「重新匹配」，并从候选里挑选。设置里可以打开「自动获取在线歌词」「自动获取在线封面」，以及把匹配结果存成歌曲旁边的 `.lrc`。
- LRC 支持一行多个时间标签、`[offset:]`、元数据标签，以及 UTF-8、UTF-8 BOM、GBK/GB18030。
- 歌词面板高亮当前行并平滑滚到中间。点击一行会跳转。手动滚动暂停自动滚动约 5 秒。无时间标签时按静态文本显示。没有歌词时显示「暂无歌词」。
- 没有网络时沿用本地歌词、已缓存结果和占位封面，不打断播放。
- GitHub Actions：`windows-latest` 打 NSIS 与 MSI，`v*` 标签发布。
- `files.md`、`task.md`、`changelog.md` 与本文件同步。

## 已知限制

- DSD 转为 PCM，不是原生 DSD、DoP 或 ASIO。
- 安装包未签名，SmartScreen 会警告。
- 单曲循环会清空缓冲再从头播放，边界上有一小段空隙。
- 采样率变化时会刷新重采样器，曲目边界可能有极短间隙。
- WebView2 使用在线引导安装。
- FFmpeg 为 GPL 构建，只在 Windows 安装包中附带，不提交到仓库。
- WiX 语言为 `en-US`。NSIS 保留简体中文和 English。
- 在线歌词和封面依赖第三方公开接口，曲目对不上或服务不可用时保持「暂无歌词」或字母占位。
- 本机没有 Windows，也没有声音设备。播放进度在无设备时仍会走动，但听不到声音。Windows 安装包由 GitHub Actions 构建。
