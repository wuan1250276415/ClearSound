import { useEffect, useRef, useState, useSyncExternalStore } from "react";
import { Button } from "@/components/ui/button";
import * as api from "@/lib/api";
import type { Lyrics } from "@/lib/types";
import { cn } from "@/lib/utils";

const PAUSE_MS = 5_000;

export function LyricsPanel({ onClose }: { onClose: () => void }) {
  const snap = useSyncExternalStore(api.subscribe, api.getSnapshot, api.getSnapshot);
  const current = snap.playback.current;
  const positionMs = snap.playback.positionMs;
  const [lyrics, setLyrics] = useState<Lyrics | null>(null);
  const [loading, setLoading] = useState(false);
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
    void api.loadLyrics(id).then((result) => {
      if (cancelled) return;
      setLyrics(result);
      setLoading(false);
      pausedUntil.current = 0;
    });
    return () => {
      cancelled = true;
    };
  }, [current?.id]);

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

  const empty = !loading && (!lyrics || lyrics.lines.every((line) => line.text.trim() === ""));
  const sourceLabel = lyrics?.source === "file" ? "歌词文件" : lyrics?.source === "embedded" ? "内嵌歌词" : null;

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
        <Button variant="ghost" size="sm" className="lg:hidden" onClick={onClose}>
          关闭
        </Button>
      </div>
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
        {loading && <p className="py-16 text-center text-sm text-muted">正在读取歌词</p>}
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
    </aside>
  );
}
