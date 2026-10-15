import { clsx, type ClassValue } from "clsx";
import { twMerge } from "tailwind-merge";

export function cn(...inputs: ClassValue[]) {
  return twMerge(clsx(inputs));
}

export function formatTime(ms: number): string {
  if (!Number.isFinite(ms) || ms < 0) ms = 0;
  const total = Math.floor(ms / 1000);
  const minutes = Math.floor(total / 60);
  const seconds = total % 60;
  return `${minutes}:${seconds.toString().padStart(2, "0")}`;
}

export function formatTech(track: {
  format: string;
  sampleRate?: number | null;
  bitDepth?: number | null;
}): string {
  const parts = [track.format];
  if (track.sampleRate) {
    const khz = track.sampleRate / 1000;
    parts.push(`${Number.isInteger(khz) ? khz.toFixed(0) : khz.toFixed(1)} kHz`);
  }
  if (track.bitDepth) parts.push(`${track.bitDepth} bit`);
  return parts.join(" · ");
}
