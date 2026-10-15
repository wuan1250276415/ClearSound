# 更新记录

## 0.1.0 — 2026-10-15

- 首个可运行版本：简体中文资料库、搜索、队列、播放列表和播放控制。
- 支持 FLAC、MP3、WAV、M4A、DSF、DFF、APE，以及 CUE 虚拟曲目。
- 资料库保存在应用数据目录的 SQLite 文件 `library.db`，封面缓存在 `art/`。
- Windows 打包配置为 NSIS 与 MSI，并在 CI 中下载 FFmpeg sidecar。
- 尚未发布安装包：当前 Origin 草稿无法执行 GitHub Actions。
- 专辑列表按专辑数量显示。页面标记为不翻译，避免浏览器把简体中文界面改成英文。
