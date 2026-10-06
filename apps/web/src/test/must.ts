export function must<Value>(value: Value | null | undefined, what = "value"): Value {
  if (value === null || value === undefined) {
    throw new Error(`expected ${what} to be present`);
  }
  return value;
}
