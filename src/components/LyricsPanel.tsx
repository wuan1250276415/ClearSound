import { useEffect, useRef, useState, useSyncExternalStore } from "react";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import * as api from "@/lib/api";
import type { LyricHit, Lyrics } from "@/lib/types";
import { cn, formatTime } from "@/lib/utils";

const PAUSE_MS = 5_000;

export function LyricsPanel({ onClose }: { onClose: () => void }) {
  const snap = useSyncExternalStore(api.subscribe, api.getSnapshot, api.getSnapshot);
  const current = snap.playback.current;
  const positionMs = snap.playback.positionMs;
  const [lyrics, setLyrics] = useState<Lyrics | null>(null);
  const [loading, setLoading] = useState(false);
  const [statusText, setStatusText] = useState("正在读取歌词");
  const [searchError, setSearchError] = useState<string | null>(null);
  const [hits, setHits] = useState<LyricHit[] | null>(null);
  const [searching, setSearching] = useState(false);
  const autoLyrics = snap.settings.autoOnlineLyrics;
  const scrollerRef = useRef<HTMLDivElement>(null);
  const lineRefs = useRef<Array<HTMLElement | null>>([]);
  const pausedUntil = useRef(0);
  const programmatic = useRef(false);
  const settleTimer = useRef<number | null>(null);
  const seenTrack = useRef<number | null>(null);
  const [resumeAt, setResumeAt] = useState(0);

  useEffect(() => {
    const id = current?.id;
    if (id == null) {
      setLyrics(null);
      setLoading(false);
      return;
    }
    let cancelled = false;
    setLoading(true);
    setStatusText("正在读取歌词");
    setSearchError(null);
    setHits(null);
    void (async () => {
      const local = await api.loadLyrics(id);
      if (cancelled) return;
      if (!isEmpty(local) || !api.getSnapshot().settings.autoOnlineLyrics) {
        setLyrics(local);
        setLoading(false);
        pausedUntil.current = 0;
        return;
      }
      setStatusText("正在搜索在线歌词");
      const matched = await api.matchLyrics(id);
      if (cancelled) return;
      setLyrics(matched.lines.length > 0 ? matched : local);
      setLoading(false);
      pausedUntil.current = 0;
    })();
    return () => {
      cancelled = true;
    };
  }, [current?.id, autoLyrics]);

  let activeIndex = -1;
  if (lyrics?.synced) {
    for (let index = 0; index < lyrics.lines.length; index += 1) {
      const time = lyrics.lines[index]?.timeMs;
      if (time != null && time <= positionMs) activeIndex = index;
    }
  }

  useEffect(() => {
    if (!lyrics?.synced || activeIndex < 0 || current == null) return;
    const wait = pausedUntil.current - Date.now();
    const timer = window.setTimeout(() => {
      const scroller = scrollerRef.current;
      const line = lineRefs.current[activeIndex];
      if (!scroller || !line) return;
      if (Date.now() < pausedUntil.current) return;
      const lineRect = line.getBoundingClientRect();
      const scrollerRect = scroller.getBoundingClientRect();
      const top =
        scroller.scrollTop +
        (lineRect.top - scrollerRect.top) -
        scroller.clientHeight / 2 +
        lineRect.height / 2;
      const instant = seenTrack.current !== current.id;
      seenTrack.current = current.id;
      programmatic.current = true;
      if (settleTimer.current != null) window.clearTimeout(settleTimer.current);
      settleTimer.current = window.setTimeout(() => {
        programmatic.current = false;
      }, 400);
      scroller.scrollTo({ top: Math.max(0, top), behavior: instant ? "auto" : "smooth" });
    }, Math.max(0, wait));
    return () => window.clearTimeout(timer);
  }, [activeIndex, lyrics?.synced, current, resumeAt]);

  function holdAutoScroll() {
    const until = Date.now() + PAUSE_MS;
    pausedUntil.current = until;
    setResumeAt(until);
  }

  function onScroll() {
    if (programmatic.current) {
      if (settleTimer.current != null) window.clearTimeout(settleTimer.current);
      settleTimer.current = window.setTimeout(() => {
        programmatic.current = false;
      }, 250);
      return;
    }
    holdAutoScroll();
  }

  const empty = !loading && isEmpty(lyrics);
  const sourceLabel = sourceText(lyrics);

  async function search() {
    if (!current) return;
    setSearching(true);
    setSearchError(null);
    try {
      const found = await api.searchLyrics(current.id);
      setHits(found);
    } catch (err) {
      setSearchError(err instanceof Error ? err.message : "网络不可用，无法搜索歌词");
      setHits([]);
    } finally {
      setSearching(false);
    }
  }

  async function pick(hit: LyricHit) {
    if (!current) return;
    setHits(null);
    setLoading(true);
    setStatusText("正在读取歌词");
    try {
      const chosen = await api.chooseLyrics(current.id, hit.key);
      setLyrics(chosen);
      pausedUntil.current = 0;
    } catch (err) {
      setSearchError(err instanceof Error ? err.message : "无法使用这条歌词");
    } finally {
      setLoading(false);
    }
  }

  return (
    <aside className="fixed inset-x-3 top-16 bottom-28 z-40 flex min-h-0 flex-col overflow-hidden rounded-xl border border-line bg-panel shadow-2xl lg:static lg:inset-auto lg:z-auto lg:w-[22rem] lg:rounded-none lg:border-y-0 lg:border-r-0 lg:shadow-none">
      <div className="flex items-center justify-between gap-2 px-4 py-3">
        <div className="min-w-0">
          <h2 className="text-sm font-medium">歌词</h2>
          <p className="truncate text-xs text-muted">
            {current ? current.title : "未在播放"}
            {sourceLabel ? ` · ${sourceLabel}` : ""}
          </p>
        </div>
        <div className="flex shrink-0 gap-1">
          <Button variant="ghost" size="sm" onClick={() => void search()} disabled={!current || searching}>
            {searching ? "正在搜索" : empty ? "搜索歌词" : "重新匹配"}
          </Button>
          <Button variant="ghost" size="sm" className="lg:hidden" onClick={onClose}>
            关闭
          </Button>
        </div>
      </div>
      {current && (
        <div className="px-6 pb-2">
          <CoverImage trackId={current.id} label={current.album || current.title} className="mx-auto aspect-square w-44 rounded-xl" />
        </div>
      )}
      <div
        ref={scrollerRef}
        className="min-h-0 flex-1 overflow-y-auto px-6 [container-type:size]"
        data-testid="lyrics-scroller"
        onScroll={onScroll}
        onWheel={holdAutoScroll}
        onTouchMove={holdAutoScroll}
        onPointerDown={(event) => {
          if (event.target === scrollerRef.current) holdAutoScroll();
        }}
      >
        {loading && <p className="py-16 text-center text-sm text-muted">{statusText}</p>}
        {searchError && <p className="px-2 pb-2 text-center text-xs text-brass">{searchError}</p>}
        {!loading && empty && (
          <p className="py-16 text-center text-sm text-muted" data-testid="lyrics-empty">
            暂无歌词
          </p>
        )}
        {!loading && lyrics?.synced && !empty && (
          <div className="flex flex-col pt-[calc(50cqh-1.25rem)] pb-[calc(50cqh-1.25rem)]">
            {lyrics.lines.map((line, index) => {
              const active = index === activeIndex;
              const label = line.text.trim() === "" ? " " : line.text;
              return (
                <button
                  key={`${index}-${line.timeMs ?? "x"}`}
                  type="button"
                  ref={(node) => {
                    lineRefs.current[index] = node;
                  }}
                  data-line={index}
                  data-active={active ? "true" : "false"}
                  aria-current={active ? "true" : undefined}
                  className={cn(
                    "rounded-md px-2 py-2 text-center transition-all duration-300",
                    active ? "text-lg font-medium text-cream" : "text-sm text-muted hover:text-cream",
                  )}
                  onClick={() => {
                    if (line.timeMs != null) api.seek(line.timeMs);
                  }}
                >
                  {label}
                </button>
              );
            })}
          </div>
        )}
        {!loading && lyrics && !lyrics.synced && !empty && (
          <div className="whitespace-pre-wrap py-8 text-center text-sm leading-7 text-cream" data-testid="lyrics-static">
            {lyrics.lines.map((line) => line.text).join("\n")}
          </div>
        )}
      </div>
      <Dialog open={hits != null} onOpenChange={(open) => !open && setHits(null)}>
        <DialogContent>
          <DialogHeader>
            <DialogTitle>搜索歌词</DialogTitle>
            <DialogDescription>
              {current ? `为「${current.title}」选择一条歌词。` : "选择一条歌词。"}
            </DialogDescription>
          </DialogHeader>
          <div className="flex max-h-72 flex-col gap-1 overflow-auto">
            {hits?.length === 0 && <p className="text-sm text-muted">没有匹配的歌词</p>}
            {hits?.map((hit) => (
              <button
                key={hit.key}
                type="button"
                className="rounded-md px-3 py-2 text-left hover:bg-panel-2"
                onClick={() => void pick(hit)}
              >
                <span className="block truncate">{hit.title}</span>
                <span className="block truncate text-xs text-muted">
                  {hit.artist}
                  {hit.album ? ` · ${hit.album}` : ""}
                  {hit.durationMs > 0 ? ` · ${formatTime(hit.durationMs)}` : ""}
                  {` · ${hit.provider}`}
                  {hit.synced ? " · 同步" : " · 文本"}
                </span>
              </button>
            ))}
          </div>
        </DialogContent>
      </Dialog>
    </aside>
  );
}

function isEmpty(lyrics: Lyrics | null): boolean {
  return !lyrics || lyrics.lines.every((line) => line.text.trim() === "");
}

function sourceText(lyrics: Lyrics | null): string | null {
  if (!lyrics || isEmpty(lyrics)) return null;
  if (lyrics.source === "file") return "歌词文件";
  if (lyrics.source === "embedded") return "内嵌歌词";
  if (lyrics.source === "online") return lyrics.provider ? `在线 · ${lyrics.provider}` : "在线歌词";
  return null;
}

function CoverImage({ trackId, label, className }: { trackId: number; label: string; className?: string }) {
  const art = useSyncExternalStore(api.subscribe, api.getSnapshot, api.getSnapshot).art;
  const autoCover = useSyncExternalStore(api.subscribe, api.getSnapshot, api.getSnapshot).settings.autoOnlineCover;
  useEffect(() => {
    let cancelled = false;
    void (async () => {
      const url = await api.loadArt(trackId);
      if (cancelled || url) return;
      if (api.getSnapshot().settings.autoOnlineCover) await api.ensureCover(trackId);
    })();
    return () => {
      cancelled = true;
    };
  }, [trackId, autoCover]);
  const url = art[trackId];
  if (url) return <img src={url} alt="" data-testid="now-cover" className={cn("bg-panel-2 object-cover", className)} />;
  return (
    <div data-testid="now-cover-fallback" className={cn("grid place-items-center bg-jade-dim font-medium text-jade", className)}>
      {(label || "澄").slice(0, 1)}
    </div>
  );
}
