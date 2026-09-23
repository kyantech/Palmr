export function persist(value: string): void {
  localStorage.setItem("transfer", value);
  window.localStorage.setItem("transfer", value);
}
