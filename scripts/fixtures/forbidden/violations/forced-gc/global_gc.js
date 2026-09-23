export function releaseMemory() {
  if (typeof global !== "undefined" && global.gc) {
    setImmediate(() => global.gc());
  }
}
