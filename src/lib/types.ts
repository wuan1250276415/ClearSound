export type RepeatMode = "off" | "all" | "one";

export interface Track {
  id: number;
  path: string;
  cuePath?: string | null;
  startMs: number;
  endMs?: number | null;
  title: string;
  artist: string;
  albumArtist: string;
  album: string;
  trackNumber?: number | null;
  discNumber?: number | null;
  year: string;
  durationMs: number;
  sampleRate?: number | null;
  bitDepth?: number | null;
  channels?: number | null;
  format: string;
  folder: string;
  hasArt: boolean;
  artPath?: string | null;
}

export interface Album {
  name: string;
  artist: string;
  trackCount: number;
  durationMs: number;
  artTrackId: number;
  year: string;
}

export interface Artist {
  name: string;
  albumCount: number;
  trackCount: number;
}

export interface Folder {
  path: string;
  trackCount: number;
}

export interface Playlist {
  id: number;
  name: string;
  trackCount: number;
  createdAt: number;
}

export interface PlaylistEntry {
  itemId: number;
  position: number;
  track: Track;
}

export interface Root {
  id: number;
  path: string;
}

export interface PlaybackState {
  playing: boolean;
  positionMs: number;
  durationMs: number;
  volume: number;
  shuffle: boolean;
  repeatMode: RepeatMode;
  queueIndex?: number | null;
  current?: Track | null;
  queue: Track[];
  error?: string | null;
  warning?: string | null;
}

export interface Settings {
  volume: number;
  shuffle: boolean;
  repeatMode: RepeatMode;
}

export interface ScanProgress {
  scanned: number;
  total: number;
  current: string;
}

export interface ScanReport {
  added: number;
  updated: number;
  removed: number;
  skipped: number;
  errors: string[];
}
