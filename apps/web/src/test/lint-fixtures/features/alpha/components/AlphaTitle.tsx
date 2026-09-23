import { formatName } from "../../../shared/format";

export function AlphaTitle({ name }: { name: string }) {
  return <h2 title={formatName(name)}>Palmr {formatName(name)}</h2>;
}
