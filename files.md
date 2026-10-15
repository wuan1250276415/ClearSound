# 文件

| 路径 | 作用 |
| --- | --- |
| `src/App.tsx` | 简体中文界面：资料库、搜索、队列、播放列表和播放条 |
| `src/lib/api.ts` | Tauri 命令封装；浏览器里使用内存示例资料库 |
| `src/lib/types.ts` | 与 Rust 模型对应的前端类型 |
| `src/components/ui/` | 按钮、输入、对话框、菜单、滑块等基础组件 |
| `src-tauri/src/lib.rs` | 应用启动、命令注册、系统媒体控件 |
| `src-tauri/src/commands.rs` | 资料库、播放列表和播放命令 |
| `src-tauri/src/library.rs` | SQLite 资料库、递归扫描、封面文件 |
| `src-tauri/src/cue.rs` | CUE 解析与编码识别 |
| `src-tauri/src/metadata.rs` | 标签、封面、DSF/DFF 头 |
| `src-tauri/src/audio/` | Symphonia / FFmpeg 解码、重采样、cpal 播放 |
| `src-tauri/src/ffmpeg_bin.rs` | 查找 FFmpeg 可执行文件 |
| `src-tauri/src/smtc.rs` | Windows SMTC（souvlaki），仅 Windows 编译 |
| `src-tauri/tauri.conf.json` | 产品名 Chengyin，窗口标题「澄音」 |
| `src-tauri/tauri.windows.conf.json` | NSIS、MSI，以及只在 Windows 打包的 FFmpeg |
| `scripts/fetch-ffmpeg.ps1` | 下载 win64 FFmpeg 并放到 sidecar 路径 |
| `.github/workflows/release.yml` | windows-latest 构建安装包并在标签上发布 |
| `task.md` | 需求与完成情况 |
| `changelog.md` | 版本记录 |
