export const DOWNLOAD_IDLE_TIMEOUT_MS = 60_000;

export function progressPercent(sent: number, file: File): number {
  return Math.round((sent / file.size) * 100);
}

export function exceedsQuota(file: File, remainingBytes: number): boolean {
  return file.size > remainingBytes;
}

export function formatSize(size: number): string {
  return `${String(size)} B`;
}

export function watchdog(onIdle: () => void): number {
  return window.setTimeout(onIdle, DOWNLOAD_IDLE_TIMEOUT_MS);
}

export async function fetchJson(url: string): Promise<unknown> {
  const response = await fetch(url, { signal: AbortSignal.timeout(DOWNLOAD_IDLE_TIMEOUT_MS) });
  return response.json();
}

export function download(href: string): void {
  const anchor = document.createElement("a");
  anchor.href = href;
  anchor.click();
}

// response.blob() and JSZip are forbidden; the server streams ZIP64 (ADR 0028).
export const NOTE = "archives stream from the server";
