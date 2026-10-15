import { useEffect, useState, useSyncExternalStore } from "react";
import {
  Disc3,
  FolderPlus,
  ListMusic,
  ListPlus,
  MoreHorizontal,
  Pause,
  Play,
  Repeat,
  Repeat1,
  Search,
  Shuffle,
  SkipBack,
  SkipForward,
  Trash2,
  Users,
  Volume2,
  VolumeX,
} from "lucide-react";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { Input } from "@/components/ui/input";
import { ScrollArea } from "@/components/ui/scroll-area";
import { Separator } from "@/components/ui/separator";
import { Slider } from "@/components/ui/slider";
import { Tooltip, TooltipContent, TooltipProvider, TooltipTrigger } from "@/components/ui/tooltip";
import * as api from "@/lib/api";
import type { RepeatMode, ScanReport, Track } from "@/lib/types";
import { cn, formatTech, formatTime } from "@/lib/utils";

type View =
  | { kind: "tracks" }
  | { kind: "albums" }
  | { kind: "album"; name: string; artist: string }
  | { kind: "artists" }
  | { kind: "artist"; name: string }
  | { kind: "folders" }
  | { kind: "folder"; path: string }
  | { kind: "playlist"; id: number };

const NAV: Array<{ kind: View["kind"]; label: string; icon: typeof ListMusic }> = [
  { kind: "tracks", label: "全部歌曲", icon: ListMusic },
  { kind: "albums", label: "专辑", icon: Disc3 },
  { kind: "artists", label: "艺术家", icon: Users },
  { kind: "folders", label: "文件夹", icon: FolderPlus },
];

export default function App() {
  const snap = useSyncExternalStore(api.subscribe, api.getSnapshot, api.getSnapshot);
  const [view, setView] = useState<View>({ kind: "tracks" });
  const [query, setQuery] = useState("");
  const [queueOpen, setQueueOpen] = useState(false);
  const [folderOpen, setFolderOpen] = useState(false);
  const [folderPath, setFolderPath] = useState("示例音乐");
  const [playlistOpen, setPlaylistOpen] = useState(false);
  const [playlistName, setPlaylistName] = useState("");
  const [playlistTarget, setPlaylistTarget] = useState<Track[] | null>(null);
  const [newPlaylistName, setNewPlaylistName] = useState("");
  const [dismissedError, setDismissedError] = useState<string | null>(null);
  const [dismissedReport, setDismissedReport] = useState<ScanReport | null>(null);
  const [scrub, setScrub] = useState<number | null>(null);

  useEffect(() => {
    void api.init();
  }, []);

  useEffect(() => {
    const timer = window.setTimeout(() => {
      void api.search(query);
    }, 200);
    return () => window.clearTimeout(timer);
  }, [query]);

  const playback = snap.playback;
  const position = playback.positionMs;

  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      const target = event.target;
      if (target instanceof HTMLElement) {
        const tag = target.tagName;
        if (tag === "INPUT" || tag === "TEXTAREA" || target.isContentEditable) return;
      }
      if (event.code === "Space") {
        event.preventDefault();
        if (playback.current) api.toggle();
      } else if (event.code === "ArrowRight") {
        event.preventDefault();
        api.seek(position + 5000);
      } else if (event.code === "ArrowLeft") {
        event.preventDefault();
        api.seek(Math.max(0, position - 5000));
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [playback.current, position]);

  const searching = query.trim().length > 0;
  const visibleError = snap.error && snap.error !== dismissedError ? snap.error : null;
  const report = snap.scanReport && snap.scanReport !== dismissedReport ? snap.scanReport : null;
  const activePlaylist = snap.playlists.find((item) => item.id === viewId(view));

  async function chooseFolder() {
    if (api.inTauri()) {
      const path = await api.pickFolder();
      if (path) await api.addRoot(path);
      return;
    }
    setFolderPath("示例音乐");
    setFolderOpen(true);
  }

  async function confirmFolder() {
    const path = folderPath.trim();
    if (!path) return;
    setFolderOpen(false);
    await api.addRoot(path);
  }

  async function confirmPlaylist() {
    const name = playlistName.trim();
    if (!name) return;
    const created = await api.createPlaylist(name);
    setPlaylistName("");
    setPlaylistOpen(false);
    if (created) {
      await api.openPlaylist(created.id);
      setView({ kind: "playlist", id: created.id });
    }
  }

  function openPlaylist(id: number) {
    setView({ kind: "playlist", id });
    void api.openPlaylist(id);
  }

  const listed = tracksFor(view, snap, searching);
  const title = searching
    ? "搜索结果"
    : view.kind === "playlist"
      ? (activePlaylist?.name ?? "播放列表")
      : view.kind === "album"
        ? view.name
        : view.kind === "artist"
          ? view.name
          : view.kind === "folder"
            ? folderName(view.path)
            : NAV.find((item) => item.kind === view.kind)?.label ?? "澄音";
  const subtitle = searching
    ? `${listed.length} 首符合「${query.trim()}」`
    : view.kind === "album"
      ? view.artist
      : view.kind === "artist"
        ? `${listed.length} 首`
        : snap.roots.length === 0
          ? "还没有音乐文件夹"
          : `${snap.tracks.length} 首已入库`;

  return (
    <TooltipProvider delayDuration={400}>
      <div className="flex h-full min-h-0 flex-col">
        <header className="flex items-center gap-3 border-b border-line px-4 py-3">
          <div className="wordmark text-xl tracking-wide text-jade">澄音</div>
          <div className="relative min-w-0 flex-1">
            <Search className="pointer-events-none absolute top-1/2 left-3 size-4 -translate-y-1/2 text-muted" />
            <Input
              value={query}
              onChange={(event) => setQuery(event.target.value)}
              placeholder="搜索曲目、艺术家或专辑"
              aria-label="搜索"
              className="pl-9"
            />
          </div>
          <Button onClick={() => void chooseFolder()}>
            <FolderPlus className="size-4" />
            添加文件夹
          </Button>
        </header>

        {snap.scanning && (
          <div className="border-b border-line bg-jade-dim px-4 py-2 text-sm text-jade">
            正在扫描{snap.scan?.current ? `：${snap.scan.current}` : "…"}
            {snap.scan && snap.scan.total > 0 ? `（${snap.scan.scanned}/${snap.scan.total}）` : ""}
          </div>
        )}
        {visibleError && (
          <div className="flex items-start justify-between gap-3 border-b border-line bg-panel-2 px-4 py-2 text-sm text-brass">
            <p>{visibleError}</p>
            <Button variant="ghost" size="sm" onClick={() => setDismissedError(visibleError)}>
              关闭
            </Button>
          </div>
        )}
        {playback.warning && (
          <div className="border-b border-line px-4 py-2 text-sm text-muted">{playback.warning}</div>
        )}
        {report && (
          <div className="flex items-start justify-between gap-3 border-b border-line px-4 py-2 text-sm">
            <div>
              <p>
                扫描完成：新增 {report.added}，更新 {report.updated}，移除 {report.removed}，跳过{" "}
                {report.skipped}
              </p>
              {report.errors.length > 0 && (
                <ul className="mt-1 list-disc pl-4 text-brass">
                  {report.errors.slice(0, 4).map((item) => (
                    <li key={item}>{item}</li>
                  ))}
                </ul>
              )}
            </div>
            <Button variant="ghost" size="sm" onClick={() => setDismissedReport(report)}>
              知道了
            </Button>
          </div>
        )}

        <div className="flex min-h-0 flex-1">
          <aside className="hidden w-56 shrink-0 flex-col border-r border-line lg:flex">
            <ScrollArea className="min-h-0 flex-1">
              <nav className="flex flex-col gap-1 p-3">
                {NAV.map((item) => (
                  <Button
                    key={item.kind}
                    variant={view.kind === item.kind && !searching ? "secondary" : "ghost"}
                    className="justify-start"
                    onClick={() => {
                      setQuery("");
                      setView({ kind: item.kind } as View);
                    }}
                  >
                    <item.icon className="size-4" />
                    {item.label}
                  </Button>
                ))}
              </nav>
              <Separator />
              <div className="p-3">
                <div className="mb-2 flex items-center justify-between px-1 text-xs text-muted">
                  <span>播放列表</span>
                  <Button variant="ghost" size="icon" aria-label="新建播放列表" onClick={() => setPlaylistOpen(true)}>
                    <ListPlus className="size-4" />
                  </Button>
                </div>
                {snap.playlists.length === 0 && <p className="px-1 text-sm text-muted">还没有播放列表</p>}
                <div className="flex flex-col gap-1">
                  {snap.playlists.map((playlist) => (
                    <Button
                      key={playlist.id}
                      variant={view.kind === "playlist" && view.id === playlist.id ? "secondary" : "ghost"}
                      className="justify-start"
                      onClick={() => openPlaylist(playlist.id)}
                    >
                      <span className="truncate">{playlist.name}</span>
                      <span className="ml-auto text-xs text-muted">{playlist.trackCount}</span>
                    </Button>
                  ))}
                </div>
              </div>
              <Separator />
              <div className="p-3">
                <p className="mb-2 px-1 text-xs text-muted">资料库</p>
                {snap.roots.length === 0 && <p className="px-1 text-sm text-muted">尚未添加文件夹</p>}
                {snap.roots.map((root) => (
                  <div key={root.id} className="mb-1 flex items-center gap-1">
                    <p className="min-w-0 flex-1 truncate px-1 text-sm" title={root.path}>
                      {folderName(root.path)}
                    </p>
                    <Button variant="ghost" size="icon" aria-label={`移除 ${folderName(root.path)}`} onClick={() => void api.removeRoot(root.id)}>
                      <Trash2 className="size-3.5" />
                    </Button>
                  </div>
                ))}
                <Button
                  variant="outline"
                  size="sm"
                  className="mt-2 w-full"
                  disabled={snap.roots.length === 0 || snap.scanning}
                  onClick={() => void api.scan()}
                >
                  重新扫描
                </Button>
              </div>
            </ScrollArea>
          </aside>

          <main className="flex min-w-0 flex-1 flex-col">
            <div className="flex gap-2 overflow-x-auto border-b border-line p-2 lg:hidden">
              {NAV.map((item) => (
                <Button
                  key={item.kind}
                  size="sm"
                  variant={view.kind === item.kind ? "secondary" : "ghost"}
                  onClick={() => {
                    setQuery("");
                    setView({ kind: item.kind } as View);
                  }}
                >
                  {item.label}
                </Button>
              ))}
              <Button size="sm" variant="outline" onClick={() => setPlaylistOpen(true)}>
                新建播放列表
              </Button>
            </div>
            <div className="flex items-end justify-between gap-3 px-4 pt-4 pb-2">
              <div className="min-w-0">
                {(view.kind === "album" || view.kind === "artist" || view.kind === "folder" || view.kind === "playlist") &&
                  !searching && (
                    <button className="mb-1 text-xs text-jade" onClick={() => setView(parentView(view))}>
                      返回
                    </button>
                  )}
                <h1 className="truncate text-2xl font-medium">{title}</h1>
                <p className="text-sm text-muted">{subtitle}</p>
              </div>
              {view.kind === "playlist" && activePlaylist && !searching && (
                <Button
                  variant="outline"
                  size="sm"
                  onClick={() => {
                    void api.deletePlaylist(activePlaylist.id);
                    setView({ kind: "tracks" });
                  }}
                >
                  删除播放列表
                </Button>
              )}
            </div>
            <ScrollArea className="min-h-0 flex-1">
              <div className="px-4 pb-6">
                {!snap.ready && <p className="py-16 text-center text-muted">正在打开资料库…</p>}
                {snap.ready && snap.roots.length === 0 && !searching && (
                  <EmptyLibrary onAdd={() => void chooseFolder()} />
                )}
                {snap.ready && snap.roots.length > 0 && searching && listed.length === 0 && (
                  <p className="py-16 text-center text-muted">没有符合「{query.trim()}」的曲目</p>
                )}
                {snap.ready && (snap.roots.length > 0 || searching) && listed.length > 0 && view.kind === "albums" && !searching && (
                  <AlbumGrid
                    albums={snap.albums}
                    onOpen={(name, artist) => setView({ kind: "album", name, artist })}
                  />
                )}
                {snap.ready && snap.roots.length > 0 && view.kind === "artists" && !searching && (
                  <div className="grid gap-2 sm:grid-cols-2 xl:grid-cols-3">
                    {snap.artists.map((artist) => (
                      <button
                        key={artist.name}
                        className="rounded-lg border border-line bg-panel px-4 py-3 text-left hover:border-jade"
                        onClick={() => setView({ kind: "artist", name: artist.name })}
                      >
                        <p className="font-medium">{artist.name}</p>
                        <p className="text-sm text-muted">
                          {artist.albumCount} 张专辑 · {artist.trackCount} 首
                        </p>
                      </button>
                    ))}
                  </div>
                )}
                {snap.ready && snap.roots.length > 0 && view.kind === "folders" && !searching && (
                  <div className="flex flex-col gap-2">
                    {snap.folders.map((folder) => (
                      <button
                        key={folder.path}
                        className="rounded-lg border border-line bg-panel px-4 py-3 text-left hover:border-jade"
                        onClick={() => setView({ kind: "folder", path: folder.path })}
                      >
                        <p className="font-medium">{folderName(folder.path)}</p>
                        <p className="truncate text-sm text-muted">
                          {folder.path} · {folder.trackCount} 首
                        </p>
                      </button>
                    ))}
                  </div>
                )}
                {snap.ready &&
                  (searching ||
                    view.kind === "tracks" ||
                    view.kind === "album" ||
                    view.kind === "artist" ||
                    view.kind === "folder" ||
                    view.kind === "playlist") &&
                  (snap.roots.length > 0 || searching) &&
                  !(view.kind === "albums" && !searching) &&
                  !(view.kind === "artists" && !searching) &&
                  !(view.kind === "folders" && !searching) && (
                    <TrackTable
                      tracks={listed}
                      playlist={view.kind === "playlist" && !searching}
                      entries={view.kind === "playlist" ? snap.playlistTracks : []}
                      currentId={playback.current?.id ?? null}
                      playing={playback.playing}
                      capped={view.kind === "tracks" && !searching && snap.tracks.length >= 4000}
                      onPlay={(index) => api.playTracks(listed, index)}
                      onQueue={(track) => api.enqueue([track])}
                      onPlayNext={(track) => api.playNext(track)}
                      onAddPlaylist={(track) => {
                        setNewPlaylistName("");
                        setPlaylistTarget([track]);
                      }}
                      onRemoveItem={(itemId) => void api.removePlaylistItem(itemId)}
                    />
                  )}
                {snap.ready &&
                  snap.roots.length > 0 &&
                  view.kind === "playlist" &&
                  !searching &&
                  snap.playlistTracks.length === 0 && (
                    <p className="py-16 text-center text-muted">这个播放列表还没有曲目。在曲目菜单里选择「加入播放列表」。</p>
                  )}
              </div>
            </ScrollArea>
          </main>

          {queueOpen && (
            <aside className="fixed inset-x-3 top-16 bottom-28 z-30 flex flex-col rounded-xl border border-line bg-panel shadow-2xl lg:static lg:inset-auto lg:z-auto lg:w-80 lg:rounded-none lg:border-y-0 lg:border-r-0 lg:shadow-none">
              <div className="flex items-center justify-between px-4 py-3">
                <h2 className="text-sm font-medium">播放队列</h2>
                <div className="flex gap-1">
                  <Button variant="ghost" size="sm" onClick={() => api.clearQueue()} disabled={playback.queue.length === 0}>
                    清空
                  </Button>
                  <Button variant="ghost" size="sm" className="lg:hidden" onClick={() => setQueueOpen(false)}>
                    关闭
                  </Button>
                </div>
              </div>
              <ScrollArea className="min-h-0 flex-1">
                {playback.queue.length === 0 && <p className="px-4 text-sm text-muted">队列是空的</p>}
                <ol>
                  {playback.queue.map((track, index) => (
                    <li
                      key={`${track.id}-${index}`}
                      className={cn(
                        "track-row flex items-center gap-2 px-2 py-1 hover:bg-panel-2",
                        index === playback.queueIndex && "bg-jade-dim",
                      )}
                    >
                      <button className="flex min-w-0 flex-1 items-center gap-2 px-2 py-1 text-left" onClick={() => api.jump(index)}>
                        <span className="w-5 text-xs text-muted">{index + 1}</span>
                        <span className="min-w-0 flex-1">
                          <span className="block truncate">{track.title}</span>
                          <span className="block truncate text-xs text-muted">{track.artist}</span>
                        </span>
                      </button>
                      <Button variant="ghost" size="icon" aria-label={`从队列移除 ${track.title}`} onClick={() => api.removeQueued(index)}>
                        <Trash2 className="size-3.5" />
                      </Button>
                    </li>
                  ))}
                </ol>
              </ScrollArea>
            </aside>
          )}
        </div>

        <PlayerBar
          scrub={scrub}
          setScrub={setScrub}
          queueOpen={queueOpen}
          onToggleQueue={() => setQueueOpen((open) => !open)}
        />
      </div>

      <Dialog open={folderOpen} onOpenChange={setFolderOpen}>
        <DialogContent>
          <DialogHeader>
            <DialogTitle>添加音乐文件夹</DialogTitle>
            <DialogDescription>
              填写要扫描的路径。桌面版会打开系统文件夹选择器；这里用示例专辑演示资料库。
            </DialogDescription>
          </DialogHeader>
          <form
            className="space-y-3"
            onSubmit={(event) => {
              event.preventDefault();
              void confirmFolder();
            }}
          >
            <Input
              aria-label="文件夹路径"
              value={folderPath}
              onChange={(event) => setFolderPath(event.target.value)}
            />
            <div className="flex justify-end gap-2">
              <Button type="button" variant="ghost" onClick={() => setFolderOpen(false)}>
                取消
              </Button>
              <Button type="submit">开始扫描</Button>
            </div>
          </form>
        </DialogContent>
      </Dialog>

      <Dialog open={playlistOpen} onOpenChange={setPlaylistOpen}>
        <DialogContent>
          <DialogHeader>
            <DialogTitle>新建播放列表</DialogTitle>
            <DialogDescription>给这个列表起一个名字，之后可以把曲目加进来。</DialogDescription>
          </DialogHeader>
          <form
            className="space-y-3"
            onSubmit={(event) => {
              event.preventDefault();
              void confirmPlaylist();
            }}
          >
            <Input
              aria-label="播放列表名称"
              placeholder="例如：夜航"
              value={playlistName}
              onChange={(event) => setPlaylistName(event.target.value)}
            />
            <div className="flex justify-end gap-2">
              <Button type="button" variant="ghost" onClick={() => setPlaylistOpen(false)}>
                取消
              </Button>
              <Button type="submit" disabled={!playlistName.trim()}>
                创建
              </Button>
            </div>
          </form>
        </DialogContent>
      </Dialog>

      <Dialog open={playlistTarget != null} onOpenChange={(open) => !open && setPlaylistTarget(null)}>
        <DialogContent>
          <DialogHeader>
            <DialogTitle>加入播放列表</DialogTitle>
            <DialogDescription>
              {playlistTarget?.[0] ? `把「${playlistTarget[0].title}」放进一个列表。` : "选择播放列表。"}
            </DialogDescription>
          </DialogHeader>
          <div className="flex max-h-48 flex-col gap-1 overflow-auto">
            {snap.playlists.length === 0 && <p className="text-sm text-muted">还没有播放列表，先在下面新建。</p>}
            {snap.playlists.map((playlist) => (
              <Button
                key={playlist.id}
                variant="ghost"
                className="justify-start"
                onClick={() => {
                  if (!playlistTarget) return;
                  void api.addToPlaylist(
                    playlist.id,
                    playlistTarget.map((track) => track.id),
                  );
                  setPlaylistTarget(null);
                }}
              >
                {playlist.name}
              </Button>
            ))}
          </div>
          <form
            className="mt-3 flex gap-2"
            onSubmit={(event) => {
              event.preventDefault();
              void (async () => {
                const name = newPlaylistName.trim();
                if (!name || !playlistTarget) return;
                const created = await api.createPlaylist(name);
                if (!created) return;
                await api.addToPlaylist(
                  created.id,
                  playlistTarget.map((track) => track.id),
                );
                setPlaylistTarget(null);
              })();
            }}
          >
            <Input
              aria-label="新播放列表名称"
              placeholder="新建并加入"
              value={newPlaylistName}
              onChange={(event) => setNewPlaylistName(event.target.value)}
            />
            <Button type="submit" disabled={!newPlaylistName.trim()}>
              新建
            </Button>
          </form>
        </DialogContent>
      </Dialog>
    </TooltipProvider>
  );
}

function PlayerBar({
  scrub,
  setScrub,
  queueOpen,
  onToggleQueue,
}: {
  scrub: number | null;
  setScrub: (value: number | null) => void;
  queueOpen: boolean;
  onToggleQueue: () => void;
}) {
  const snap = useSyncExternalStore(api.subscribe, api.getSnapshot, api.getSnapshot);
  const playback = snap.playback;
  const current = playback.current;
  const duration = playback.durationMs;
  const shown = scrub ?? playback.positionMs;

  return (
    <footer className="border-t border-line bg-panel px-3 py-3">
      <div className="flex flex-col gap-3 lg:flex-row lg:items-center">
        <div className="flex min-w-0 items-center gap-3 lg:w-72">
          <Cover trackId={current?.id} label={current?.album || current?.title || "澄"} className="size-12 rounded-md" />
          <div className="min-w-0">
            <p className="truncate font-medium">{current?.title ?? "未在播放"}</p>
            <p className="truncate text-xs text-muted">
              {current ? `${current.artist} · ${current.album}` : "从资料库选一首开始"}
            </p>
            {current && <p className="truncate text-xs text-jade">{formatTech(current)}</p>}
          </div>
        </div>
        <div className="flex min-w-0 flex-1 flex-col gap-2">
          <div className="flex items-center justify-center gap-1">
            <Tooltip>
              <TooltipTrigger asChild>
                <Button
                  variant={playback.shuffle ? "secondary" : "ghost"}
                  size="icon"
                  aria-label="随机播放"
                  aria-pressed={playback.shuffle}
                  onClick={() => api.setShuffle(!playback.shuffle)}
                >
                  <Shuffle className="size-4" />
                </Button>
              </TooltipTrigger>
              <TooltipContent>随机播放</TooltipContent>
            </Tooltip>
            <Button variant="ghost" size="icon" aria-label="上一首" disabled={!current} onClick={() => api.prev()}>
              <SkipBack className="size-4" />
            </Button>
            <Button
              size="icon"
              aria-label={playback.playing ? "暂停" : "播放"}
              disabled={!current}
              onClick={() => api.toggle()}
            >
              {playback.playing ? <Pause className="size-4" /> : <Play className="size-4" />}
            </Button>
            <Button variant="ghost" size="icon" aria-label="下一首" disabled={!current} onClick={() => api.next()}>
              <SkipForward className="size-4" />
            </Button>
            <Tooltip>
              <TooltipTrigger asChild>
                <Button
                  variant={playback.repeatMode === "off" ? "ghost" : "secondary"}
                  size="icon"
                  aria-label={repeatLabel(playback.repeatMode)}
                  onClick={() => api.setRepeat(cycleRepeat(playback.repeatMode))}
                >
                  {playback.repeatMode === "one" ? <Repeat1 className="size-4" /> : <Repeat className="size-4" />}
                </Button>
              </TooltipTrigger>
              <TooltipContent>{repeatLabel(playback.repeatMode)}</TooltipContent>
            </Tooltip>
          </div>
          <div className="flex items-center gap-2">
            <span className="w-10 text-right text-xs text-muted tabular-nums">{formatTime(shown)}</span>
            <Slider
              aria-label="播放进度"
              max={Math.max(duration, 1)}
              step={500}
              value={[Math.min(shown, Math.max(duration, 1))]}
              disabled={!current}
              onValueChange={(values) => setScrub(values[0] ?? 0)}
              onValueCommit={(values) => {
                api.seek(values[0] ?? 0);
                setScrub(null);
              }}
            />
            <span className="w-10 text-xs text-muted tabular-nums">{formatTime(duration)}</span>
          </div>
        </div>
        <div className="flex items-center gap-2 lg:w-64">
          <Button
            variant="ghost"
            size="icon"
            aria-label={playback.volume === 0 ? "取消静音" : "静音"}
            onClick={() => api.setVolume(playback.volume === 0 ? 0.85 : 0)}
          >
            {playback.volume === 0 ? <VolumeX className="size-4" /> : <Volume2 className="size-4" />}
          </Button>
          <Slider
            aria-label="音量"
            max={1}
            step={0.01}
            value={[playback.volume]}
            onValueChange={(values) => api.setVolume(values[0] ?? 0)}
          />
          <Button variant={queueOpen ? "secondary" : "ghost"} size="sm" onClick={onToggleQueue}>
            队列
          </Button>
        </div>
      </div>
    </footer>
  );
}

function EmptyLibrary({ onAdd }: { onAdd: () => void }) {
  return (
    <div className="flex flex-col items-center px-6 py-20 text-center">
      <div className="mb-5 grid size-24 place-items-center rounded-full bg-jade-dim text-3xl text-jade">澄</div>
      <h2 className="text-xl">资料库还是空的</h2>
      <p className="mt-2 max-w-md text-sm text-muted">
        添加一个文件夹后，澄音会递归读取 FLAC、MP3、WAV、M4A、DSF 和 APE，并按 CUE 把整轨专辑拆成曲目。
      </p>
      <Button className="mt-6" onClick={onAdd}>
        <FolderPlus className="size-4" />
        添加文件夹
      </Button>
    </div>
  );
}

function AlbumGrid({
  albums,
  onOpen,
}: {
  albums: api.Snapshot["albums"];
  onOpen: (name: string, artist: string) => void;
}) {
  return (
    <div className="grid grid-cols-2 gap-3 sm:grid-cols-3 xl:grid-cols-4">
      {albums.map((album) => (
        <button
          key={`${album.name}\u0000${album.artist}`}
          className="rounded-xl border border-line bg-panel p-3 text-left hover:border-jade"
          onClick={() => onOpen(album.name, album.artist)}
        >
          <Cover trackId={album.artTrackId} label={album.name} className="mb-3 aspect-square w-full rounded-lg" />
          <p className="truncate font-medium">{album.name}</p>
          <p className="truncate text-sm text-muted">
            {album.artist}
            {album.year ? ` · ${album.year}` : ""}
          </p>
          <p className="text-xs text-muted">
            {album.trackCount} 首 · {formatTime(album.durationMs)}
          </p>
        </button>
      ))}
    </div>
  );
}

function TrackTable({
  tracks,
  playlist,
  entries,
  currentId,
  playing,
  capped,
  onPlay,
  onQueue,
  onPlayNext,
  onAddPlaylist,
  onRemoveItem,
}: {
  tracks: Track[];
  playlist: boolean;
  entries: api.Snapshot["playlistTracks"];
  currentId: number | null;
  playing: boolean;
  capped: boolean;
  onPlay: (index: number) => void;
  onQueue: (track: Track) => void;
  onPlayNext: (track: Track) => void;
  onAddPlaylist: (track: Track) => void;
  onRemoveItem: (itemId: number) => void;
}) {
  if (tracks.length === 0) return null;
  return (
    <div>
      {capped && (
        <p className="mb-2 text-sm text-muted">全部歌曲只列出前 4000 首。用搜索或按专辑、艺术家、文件夹继续浏览。</p>
      )}
      <div className="overflow-hidden rounded-lg border border-line">
        {tracks.map((track, index) => {
          const entry = playlist ? entries[index] : undefined;
          const active = track.id === currentId;
          return (
            <div
              key={entry ? entry.itemId : `${track.id}-${index}`}
              className={cn(
                "track-row flex items-center gap-3 border-b border-line px-3 py-2 last:border-b-0 hover:bg-panel-2",
                active && "bg-jade-dim",
              )}
            >
              <button className="w-8 text-left text-xs text-muted tabular-nums" onClick={() => onPlay(index)} aria-label={`播放 ${track.title}`}>
                {active && playing ? "▶" : (track.trackNumber ?? index + 1)}
              </button>
              <button className="min-w-0 flex-1 text-left" onClick={() => onPlay(index)}>
                <span className="flex items-center gap-2">
                  <span className="truncate">{track.title}</span>
                  {track.cuePath && <span className="shrink-0 text-[10px] text-brass">CUE</span>}
                </span>
                <span className="block truncate text-xs text-muted lg:hidden">
                  {track.artist} · {formatTime(track.durationMs)}
                </span>
              </button>
              <button className="hidden w-36 truncate text-left text-sm text-muted lg:block" onClick={() => onPlay(index)}>
                {track.artist}
              </button>
              <span className="hidden w-40 truncate text-sm text-muted xl:block">{track.album}</span>
              <span className="hidden w-36 truncate text-xs text-muted xl:block">{formatTech(track)}</span>
              <span className="hidden w-12 text-right text-xs text-muted tabular-nums lg:block">{formatTime(track.durationMs)}</span>
              <DropdownMenu>
                <DropdownMenuTrigger asChild>
                  <Button variant="ghost" size="icon" aria-label={`${track.title} 的更多操作`}>
                    <MoreHorizontal className="size-4" />
                  </Button>
                </DropdownMenuTrigger>
                <DropdownMenuContent align="end">
                  <DropdownMenuItem onSelect={() => onPlay(index)}>播放</DropdownMenuItem>
                  <DropdownMenuItem onSelect={() => onPlayNext(track)}>下一首播放</DropdownMenuItem>
                  <DropdownMenuItem onSelect={() => onQueue(track)}>加入队列</DropdownMenuItem>
                  <DropdownMenuItem onSelect={() => onAddPlaylist(track)}>加入播放列表</DropdownMenuItem>
                  {entry && (
                    <>
                      <DropdownMenuSeparator />
                      <DropdownMenuItem onSelect={() => onRemoveItem(entry.itemId)}>从播放列表移除</DropdownMenuItem>
                    </>
                  )}
                </DropdownMenuContent>
              </DropdownMenu>
            </div>
          );
        })}
      </div>
    </div>
  );
}

function Cover({ trackId, label, className }: { trackId?: number | null; label: string; className?: string }) {
  const art = useSyncExternalStore(api.subscribe, api.getSnapshot, api.getSnapshot).art;
  useEffect(() => {
    if (trackId != null) void api.loadArt(trackId);
  }, [trackId]);
  const url = trackId != null ? art[trackId] : null;
  if (url) return <img src={url} alt="" className={cn("bg-panel-2 object-cover", className)} />;
  return (
    <div className={cn("grid place-items-center bg-jade-dim font-medium text-jade", className)}>
      {(label || "澄").slice(0, 1)}
    </div>
  );
}

function tracksFor(view: View, snap: api.Snapshot, searching: boolean): Track[] {
  if (searching) return snap.search ?? [];
  if (view.kind === "album") return byAlbum(snap.tracks, view.name, view.artist);
  if (view.kind === "artist") return snap.tracks.filter((track) => track.artist === view.name);
  if (view.kind === "folder") return snap.tracks.filter((track) => track.folder === view.path);
  if (view.kind === "playlist") return snap.playlistTracks.map((entry) => entry.track);
  if (view.kind === "tracks") return snap.tracks;
  return [];
}

function byAlbum(tracks: Track[], album: string, artist: string): Track[] {
  return tracks
    .filter((track) => track.album === album && (track.albumArtist || track.artist) === artist)
    .sort(
      (a, b) =>
        (a.discNumber ?? 0) - (b.discNumber ?? 0) ||
        (a.trackNumber ?? 0) - (b.trackNumber ?? 0) ||
        a.title.localeCompare(b.title, "zh"),
    );
}

function parentView(view: View): View {
  if (view.kind === "album") return { kind: "albums" };
  if (view.kind === "artist") return { kind: "artists" };
  if (view.kind === "folder") return { kind: "folders" };
  return { kind: "tracks" };
}

function viewId(view: View): number | null {
  return view.kind === "playlist" ? view.id : null;
}

function folderName(path: string): string {
  const parts = path.split(/[/\\]/).filter(Boolean);
  return parts[parts.length - 1] ?? path;
}

function repeatLabel(mode: RepeatMode): string {
  if (mode === "all") return "列表循环";
  if (mode === "one") return "单曲循环";
  return "顺序播放";
}

function cycleRepeat(mode: RepeatMode): RepeatMode {
  if (mode === "off") return "all";
  if (mode === "all") return "one";
  return "off";
}
