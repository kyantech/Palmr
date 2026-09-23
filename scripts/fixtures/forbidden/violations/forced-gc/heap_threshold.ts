interface MemoryInfo {
  usedJSHeapSize: number;
  jsHeapSizeLimit: number;
}

export function shouldThrottle(): boolean {
  const memory = (performance as Performance & { memory: MemoryInfo }).memory;
  return memory.usedJSHeapSize > memory.jsHeapSizeLimit * 0.8;
}
