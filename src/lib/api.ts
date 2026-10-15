import type {
  Album,
  Artist,
  Folder,
  PlaybackState,
  Playlist,
  PlaylistEntry,
  RepeatMode,
  Root,
  Lyrics,
  ScanProgress,
  ScanReport,
  Track,
} from "@/lib/types";

export function inTauri(): boolean {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}

const emptyPlayback = (): PlaybackState => ({
  playing: false,
  positionMs: 0,
  durationMs: 0,
  volume: 0.85,
  shuffle: false,
  repeatMode: "off",
  queueIndex: null,
  current: null,
  queue: [],
  error: null,
  warning: inTauri() ? null : null,
});

export interface Snapshot {
  ready: boolean;
  error: string | null;
  scanning: boolean;
  scan: ScanProgress | null;
  scanReport: ScanReport | null;
  roots: Root[];
  tracks: Track[];
  albums: Album[];
  artists: Artist[];
  folders: Folder[];
  playlists: Playlist[];
  playlistTracks: PlaylistEntry[];
  activePlaylistId: number | null;
  search: Track[] | null;
  playback: PlaybackState;
  art: Record<number, string | null>;
}

let snapshot: Snapshot = {
  ready: false,
  error: null,
  scanning: false,
  scan: null,
  scanReport: null,
  roots: [],
  tracks: [],
  albums: [],
  artists: [],
  folders: [],
  playlists: [],
  playlistTracks: [],
  activePlaylistId: null,
  search: null,
  playback: emptyPlayback(),
  art: {},
};

const listeners = new Set<() => void>();
let started = false;
let mockSeq = 1;

function emit() {
  snapshot = { ...snapshot };
  listeners.forEach((listener) => listener());
}

export function subscribe(listener: () => void) {
  listeners.add(listener);
  return () => listeners.delete(listener);
}

export function getSnapshot() {
  return snapshot;
}

function fail(err: unknown): string {
  if (typeof err === "string") return err;
  if (err instanceof Error) return err.message;
  return "操作失败";
}

async function call<T>(command: string, args?: Record<string, unknown>): Promise<T> {
  const { invoke } = await import("@tauri-apps/api/core");
  return invoke<T>(command, args);
}

export async function init() {
  if (started) return;
  started = true;
  if (inTauri()) {
    const { listen } = await import("@tauri-apps/api/event");
    await listen<ScanProgress>("scan-progress", (event) => {
      snapshot.scanning = true;
      snapshot.scan = event.payload;
      emit();
    });
    try {
      await reloadLibrary();
      snapshot.playback = await call<PlaybackState>("playback_state");
      snapshot.ready = true;
      emit();
    } catch (err) {
      snapshot.ready = true;
      snapshot.error = fail(err);
      emit();
    }
    window.setInterval(() => {
      void call<PlaybackState>("playback_state")
        .then((playback) => {
          snapshot.playback = playback;
          emit();
        })
        .catch(() => undefined);
    }, 250);
    return;
  }
  snapshot.ready = true;
  emit();
  window.setInterval(tickMock, 250);
}

async function reloadLibrary() {
  const [roots, tracks, albums, artists, folders, playlists] = await Promise.all([
    call<Root[]>("library_roots"),
    call<Track[]>("library_tracks"),
    call<Album[]>("library_albums"),
    call<Artist[]>("library_artists"),
    call<Folder[]>("library_folders"),
    call<Playlist[]>("playlists_list"),
  ]);
  snapshot.roots = roots;
  snapshot.tracks = tracks;
  snapshot.albums = albums;
  snapshot.artists = artists;
  snapshot.folders = folders;
  snapshot.playlists = playlists;
  if (snapshot.activePlaylistId != null) {
    snapshot.playlistTracks = await call<PlaylistEntry[]>("playlist_tracks", {
      playlistId: snapshot.activePlaylistId,
    });
  }
}

export async function refresh() {
  if (!inTauri()) {
    deriveMock();
    emit();
    return;
  }
  try {
    await reloadLibrary();
    snapshot.error = null;
    emit();
  } catch (err) {
    snapshot.error = fail(err);
    emit();
  }
}

export async function pickFolder(): Promise<string | null> {
  if (!inTauri()) return null;
  const { open } = await import("@tauri-apps/plugin-dialog");
  const selected = await open({ directory: true, multiple: false, title: "选择音乐文件夹" });
  return typeof selected === "string" ? selected : null;
}

export async function addRoot(path: string) {
  if (!path.trim()) return;
  try {
    if (!inTauri()) {
      if (!snapshot.roots.some((root) => root.path === path)) {
        snapshot.roots = [...snapshot.roots, { id: mockSeq++, path }];
        snapshot.tracks = [...snapshot.tracks, ...sampleTracks(path)];
        deriveMock();
      }
      emit();
      await scan();
      return;
    }
    await call("library_add_root", { path });
    await scan();
  } catch (err) {
    snapshot.error = fail(err);
    emit();
  }
}

export async function removeRoot(id: number) {
  try {
    if (!inTauri()) {
      const root = snapshot.roots.find((item) => item.id === id);
      snapshot.roots = snapshot.roots.filter((item) => item.id !== id);
      if (root) snapshot.tracks = snapshot.tracks.filter((track) => track.folder !== root.path);
      deriveMock();
      emit();
      return;
    }
    await call("library_remove_root", { id });
    await refresh();
  } catch (err) {
    snapshot.error = fail(err);
    emit();
  }
}

export async function scan() {
  snapshot.scanning = true;
  snapshot.scan = { scanned: 0, total: 0, current: "准备扫描" };
  snapshot.error = null;
  emit();
  try {
    if (!inTauri()) {
      await new Promise((resolve) => window.setTimeout(resolve, 600));
      snapshot.scanReport = {
        added: snapshot.tracks.length,
        updated: 0,
        removed: 0,
        skipped: 0,
        errors: [],
      };
      snapshot.scanning = false;
      snapshot.scan = null;
      deriveMock();
      emit();
      return;
    }
    const report = await call<ScanReport>("library_scan");
    snapshot.scanReport = report;
    snapshot.scanning = false;
    snapshot.scan = null;
    await reloadLibrary();
    emit();
  } catch (err) {
    snapshot.scanning = false;
    snapshot.error = fail(err);
    emit();
  }
}

export async function search(query: string) {
  const text = query.trim();
  if (!text) {
    snapshot.search = null;
    emit();
    return;
  }
  try {
    if (!inTauri()) {
      const needle = text.toLowerCase();
      snapshot.search = snapshot.tracks
        .filter((track) =>
          [track.title, track.artist, track.album, track.albumArtist].some((field) =>
            field.toLowerCase().includes(needle),
          ),
        )
        .slice(0, 300);
      emit();
      return;
    }
    snapshot.search = await call<Track[]>("library_search", { query: text });
    emit();
  } catch (err) {
    snapshot.error = fail(err);
    emit();
  }
}

export async function openPlaylist(id: number) {
  snapshot.activePlaylistId = id;
  try {
    if (!inTauri()) {
      snapshot.playlistTracks = mockPlaylistItems(id);
      emit();
      return;
    }
    snapshot.playlistTracks = await call<PlaylistEntry[]>("playlist_tracks", { playlistId: id });
    emit();
  } catch (err) {
    snapshot.error = fail(err);
    emit();
  }
}

export async function createPlaylist(name: string) {
  try {
    if (!inTauri()) {
      const playlist: Playlist = {
        id: mockSeq++,
        name: name.trim(),
        trackCount: 0,
        createdAt: Date.now(),
      };
      mockItems.set(playlist.id, []);
      snapshot.playlists = [...snapshot.playlists, playlist];
      emit();
      return playlist;
    }
    const playlist = await call<Playlist>("playlist_create", { name });
    await refresh();
    return playlist;
  } catch (err) {
    snapshot.error = fail(err);
    emit();
    return null;
  }
}

export async function deletePlaylist(id: number) {
  try {
    if (!inTauri()) {
      snapshot.playlists = snapshot.playlists.filter((playlist) => playlist.id !== id);
      mockItems.delete(id);
      if (snapshot.activePlaylistId === id) {
        snapshot.activePlaylistId = null;
        snapshot.playlistTracks = [];
      }
      emit();
      return;
    }
    await call("playlist_delete", { id });
    if (snapshot.activePlaylistId === id) snapshot.activePlaylistId = null;
    await refresh();
  } catch (err) {
    snapshot.error = fail(err);
    emit();
  }
}

export async function addToPlaylist(playlistId: number, trackIds: number[]) {
  try {
    if (!inTauri()) {
      const items = mockItems.get(playlistId) ?? [];
      for (const id of trackIds) {
        const track = snapshot.tracks.find((item) => item.id === id);
        if (!track) continue;
        items.push({ itemId: mockSeq++, position: items.length + 1, track });
      }
      mockItems.set(playlistId, items);
      snapshot.playlists = snapshot.playlists.map((playlist) =>
        playlist.id === playlistId ? { ...playlist, trackCount: items.length } : playlist,
      );
      if (snapshot.activePlaylistId === playlistId) snapshot.playlistTracks = [...items];
      emit();
      return;
    }
    await call("playlist_add", { playlistId, trackIds });
    await refresh();
    if (snapshot.activePlaylistId === playlistId) await openPlaylist(playlistId);
  } catch (err) {
    snapshot.error = fail(err);
    emit();
  }
}

export async function removePlaylistItem(itemId: number) {
  try {
    if (!inTauri()) {
      for (const [id, items] of mockItems) {
        const next = items.filter((item) => item.itemId !== itemId);
        if (next.length !== items.length) {
          mockItems.set(id, next);
          snapshot.playlists = snapshot.playlists.map((playlist) =>
            playlist.id === id ? { ...playlist, trackCount: next.length } : playlist,
          );
          if (snapshot.activePlaylistId === id) snapshot.playlistTracks = [...next];
        }
      }
      emit();
      return;
    }
    await call("playlist_remove_item", { itemId });
    if (snapshot.activePlaylistId != null) await openPlaylist(snapshot.activePlaylistId);
    await refresh();
  } catch (err) {
    snapshot.error = fail(err);
    emit();
  }
}

async function applyPlayback(next: Promise<PlaybackState>) {
  try {
    snapshot.playback = await next;
    snapshot.error = snapshot.playback.error ?? snapshot.error;
    emit();
  } catch (err) {
    snapshot.error = fail(err);
    emit();
  }
}

export function playTracks(tracks: Track[], index = 0) {
  if (tracks.length === 0) return;
  if (!inTauri()) {
    const start = Math.min(index, tracks.length - 1);
    snapshot.playback = {
      ...snapshot.playback,
      queue: tracks,
      queueIndex: start,
      current: tracks[start] ?? null,
      playing: true,
      positionMs: 0,
      durationMs: tracks[start]?.durationMs ?? 0,
      error: null,
    };
    emit();
    return;
  }
  void applyPlayback(
    call("playback_set_queue", { ids: tracks.map((track) => track.id), index }),
  );
}

export function toggle() {
  if (!inTauri()) {
    snapshot.playback = { ...snapshot.playback, playing: !snapshot.playback.playing };
    emit();
    return;
  }
  void applyPlayback(call("playback_toggle"));
}

export function pause() {
  if (!inTauri()) {
    snapshot.playback = { ...snapshot.playback, playing: false };
    emit();
    return;
  }
  void applyPlayback(call("playback_pause"));
}

export function next() {
  if (!inTauri()) {
    stepMock(1);
    return;
  }
  void applyPlayback(call("playback_next"));
}

export function prev() {
  if (!inTauri()) {
    if (snapshot.playback.positionMs > 3000) {
      seek(0);
      return;
    }
    stepMock(-1);
    return;
  }
  void applyPlayback(call("playback_prev"));
}

export function seek(positionMs: number) {
  if (!inTauri()) {
    const duration = snapshot.playback.durationMs;
    const position = Math.max(0, duration > 0 ? Math.min(positionMs, duration) : positionMs);
    snapshot.playback = { ...snapshot.playback, positionMs: position };
    emit();
    return;
  }
  void applyPlayback(call("playback_seek", { positionMs }));
}

export function setVolume(volume: number) {
  const value = Math.max(0, Math.min(1, volume));
  if (!inTauri()) {
    snapshot.playback = { ...snapshot.playback, volume: value };
    emit();
    return;
  }
  void applyPlayback(call("playback_volume", { volume: value }));
}

export function setShuffle(shuffle: boolean) {
  if (!inTauri()) {
    snapshot.playback = { ...snapshot.playback, shuffle };
    emit();
    return;
  }
  void applyPlayback(call("playback_shuffle", { shuffle }));
}

export function setRepeat(repeatMode: RepeatMode) {
  if (!inTauri()) {
    snapshot.playback = { ...snapshot.playback, repeatMode };
    emit();
    return;
  }
  void applyPlayback(call("playback_repeat", { repeatMode }));
}

export function enqueue(tracks: Track[]) {
  if (tracks.length === 0) return;
  if (!inTauri()) {
    snapshot.playback = { ...snapshot.playback, queue: [...snapshot.playback.queue, ...tracks] };
    emit();
    return;
  }
  void applyPlayback(call("playback_enqueue", { ids: tracks.map((track) => track.id) }));
}

export function playNext(track: Track) {
  if (!inTauri()) {
    const queue = [...snapshot.playback.queue];
    const index = snapshot.playback.queueIndex;
    const at = index == null ? 0 : index + 1;
    queue.splice(at, 0, track);
    snapshot.playback = {
      ...snapshot.playback,
      queue,
      queueIndex: index ?? 0,
      current: snapshot.playback.current ?? track,
      durationMs: snapshot.playback.current?.durationMs ?? track.durationMs,
    };
    emit();
    return;
  }
  void applyPlayback(call("playback_play_next", { id: track.id }));
}

export function jump(index: number) {
  if (!inTauri()) {
    const track = snapshot.playback.queue[index];
    if (!track) return;
    snapshot.playback = {
      ...snapshot.playback,
      queueIndex: index,
      current: track,
      positionMs: 0,
      durationMs: track.durationMs,
      playing: true,
    };
    emit();
    return;
  }
  void applyPlayback(call("playback_jump", { index }));
}

export function removeQueued(index: number) {
  if (!inTauri()) {
    const queue = snapshot.playback.queue.filter((_, item) => item !== index);
    let queueIndex = snapshot.playback.queueIndex;
    if (queueIndex != null) {
      if (index < queueIndex) queueIndex -= 1;
      else if (index === queueIndex) queueIndex = queue.length === 0 ? null : Math.min(index, queue.length - 1);
    }
    const current = queueIndex == null ? null : queue[queueIndex] ?? null;
    snapshot.playback = {
      ...snapshot.playback,
      queue,
      queueIndex,
      current,
      durationMs: current?.durationMs ?? 0,
      playing: current ? snapshot.playback.playing : false,
    };
    emit();
    return;
  }
  void applyPlayback(call("playback_remove", { index }));
}

export function clearQueue() {
  if (!inTauri()) {
    snapshot.playback = { ...emptyPlayback(), volume: snapshot.playback.volume };
    emit();
    return;
  }
  void applyPlayback(call("playback_clear"));
}

export async function loadLyrics(id: number): Promise<Lyrics> {
  if (!inTauri()) return mockLyrics(id);
  try {
    return await call<Lyrics>("track_lyrics", { id });
  } catch {
    return { synced: false, source: null, lines: [] };
  }
}

export async function loadArt(id: number) {
  if (id in snapshot.art) return snapshot.art[id];
  try {
    const url = inTauri() ? await call<string | null>("track_art", { id }) : placeholderArt(id);
    snapshot.art = { ...snapshot.art, [id]: url };
    emit();
    return url;
  } catch {
    snapshot.art = { ...snapshot.art, [id]: null };
    emit();
    return null;
  }
}

function tickMock() {
  const playback = snapshot.playback;
  if (!playback.playing || !playback.current) return;
  const position = playback.positionMs + 250;
  if (playback.durationMs > 0 && position >= playback.durationMs) {
    if (playback.repeatMode === "one" && playback.queueIndex != null) {
      snapshot.playback = { ...playback, positionMs: 0, playing: true };
      emit();
      return;
    }
    stepMock(1);
    return;
  }
  snapshot.playback = { ...playback, positionMs: position };
  emit();
}

function stepMock(direction: 1 | -1) {
  const playback = snapshot.playback;
  const queue = playback.queue;
  if (queue.length === 0 || playback.queueIndex == null) return;
  let index = playback.queueIndex + direction;
  if (index < 0) index = playback.repeatMode === "all" ? queue.length - 1 : 0;
  if (index >= queue.length) {
    if (playback.repeatMode === "all") index = 0;
    else {
      snapshot.playback = { ...playback, playing: false, positionMs: playback.durationMs };
      emit();
      return;
    }
  }
  const track = queue[index];
  if (!track) return;
  snapshot.playback = {
    ...playback,
    queueIndex: index,
    current: track,
    positionMs: 0,
    durationMs: track.durationMs,
    playing: true,
  };
  emit();
}

const mockItems = new Map<number, PlaylistEntry[]>();

function deriveMock() {
  const albums = new Map<string, Album>();
  const artists = new Map<string, Artist>();
  const folders = new Map<string, Folder>();
  for (const track of snapshot.tracks) {
    const artist = track.albumArtist || track.artist;
    const key = `${track.album}\u0000${artist}`;
    const album = albums.get(key) ?? {
      name: track.album,
      artist,
      trackCount: 0,
      durationMs: 0,
      artTrackId: track.id,
      year: track.year,
    };
    album.trackCount += 1;
    album.durationMs += track.durationMs;
    albums.set(key, album);
    const person = artists.get(track.artist) ?? {
      name: track.artist,
      albumCount: 0,
      trackCount: 0,
    };
    person.trackCount += 1;
    artists.set(track.artist, person);
    const folder = folders.get(track.folder) ?? { path: track.folder, trackCount: 0 };
    folder.trackCount += 1;
    folders.set(track.folder, folder);
  }
  for (const person of artists.values()) {
    person.albumCount = new Set(
      snapshot.tracks.filter((track) => track.artist === person.name).map((track) => track.album),
    ).size;
  }
  snapshot.albums = [...albums.values()];
  snapshot.artists = [...artists.values()];
  snapshot.folders = [...folders.values()];
}

function mockPlaylistItems(id: number): PlaylistEntry[] {
  return [...(mockItems.get(id) ?? [])];
}

function sampleTracks(folder: string): Track[] {
  const rows: Array<[string, string, string, string, number, string, number, number]> = [
    ["启程", "林深", "夜航", "FLAC", 192_000, "24", 192_400, 1],
    ["靠岸", "林深", "夜航", "FLAC", 213_000, "24", 192_400, 2],
    ["灯塔", "林深", "夜航", "FLAC", 241_000, "24", 192_400, 3],
    ["潮汐", "南风", "海岸线", "MP3", 205_000, "16", 44_100, 1],
    ["盐", "南风", "海岸线", "MP3", 188_000, "16", 44_100, 2],
    ["试音", "澄音", "单曲", "WAV", 154_000, "16", 48_000, 1],
  ];
  return rows.map(([title, artist, album, format, durationMs, bits, rate, number]) => ({
    id: mockSeq++,
    path: `${folder}/${album}/${title}.${format.toLowerCase()}`,
    cuePath: album === "夜航" ? `${folder}/夜航.cue` : null,
    startMs: 0,
    endMs: null,
    title,
    artist,
    albumArtist: artist,
    album,
    trackNumber: number,
    discNumber: 1,
    year: album === "夜航" ? "2020" : "2018",
    durationMs,
    sampleRate: rate,
    bitDepth: Number(bits),
    channels: 2,
    format,
    folder,
    hasArt: true,
  }));
}

function findMockTrack(id: number): Track | null {
  return (
    snapshot.tracks.find((track) => track.id === id) ??
    snapshot.playback.queue.find((track) => track.id === id) ??
    (snapshot.playback.current?.id === id ? snapshot.playback.current : null)
  );
}

function mockLyrics(id: number): Lyrics {
  const track = findMockTrack(id);
  if (track?.title === "启程") {
    const lines = [
      [0, "雾还停在甲板"],
      [4_000, "缆绳松开了"],
      [8_000, "灯塔只剩一个点"],
      [12_000, "潮水把航线推开"],
      [16_000, "城市的灯往后退"],
      [20_000, "夜航从此开始"],
      [28_000, "风从左舷过来"],
      [36_000, "罗盘轻轻偏了一度"],
      [44_000, "有人在舱里哼歌"],
      [52_000, "歌词比海更慢"],
      [64_000, "靠岸还很远"],
      [76_000, "先把这一句唱完"],
      [88_000, "星光铺在水上"],
      [104_000, "船尾留下一道白"],
      [120_000, "启程，就是不再回头"],
      [140_000, "下一站还没有名字"],
      [164_000, "雾终于散了"],
    ];
    return {
      synced: true,
      source: "file",
      lines: lines.map(([timeMs, text]) => ({ timeMs: Number(timeMs), text: String(text) })),
    };
  }
  if (track?.title === "潮汐") {
    return {
      synced: false,
      source: "embedded",
      lines: [
        { timeMs: null, text: "海岸线慢慢后退" },
        { timeMs: null, text: "盐粒留在袖口" },
        { timeMs: null, text: "潮汐不问归期" },
        { timeMs: null, text: "我们只是站在原地" },
      ],
    };
  }
  return { synced: false, source: null, lines: [] };
}

function placeholderArt(id: number): string | null {
  const track = snapshot.tracks.find((item) => item.id === id) ?? snapshot.playback.current;
  const label = (track?.album || track?.title || "澄").slice(0, 1);
  const hue = [...(track?.album ?? "澄")].reduce((sum, char) => sum + char.charCodeAt(0), 0) % 360;
  const svg = `<svg xmlns="http://www.w3.org/2000/svg" width="320" height="320"><rect width="320" height="320" fill="hsl(${hue} 22% 14%)"/><text x="160" y="188" text-anchor="middle" font-size="150" fill="hsl(${hue} 42% 74%)" font-family="Noto Sans CJK SC, serif">${label}</text></svg>`;
  return `data:image/svg+xml;charset=UTF-8,${encodeURIComponent(svg)}`;
}
