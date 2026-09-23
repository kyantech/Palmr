export function getTimeoutForFileSize(fileSizeBytes: number): number {
  return Math.max(60_000, fileSizeBytes / 1024);
}

export function upload(xhr: XMLHttpRequest, file: File): void {
  xhr.timeout = file.size / 100;
  void fetch("/upload", { signal: AbortSignal.timeout(file.size / 10) });
}
