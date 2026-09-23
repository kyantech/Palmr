import { AlphaTitle } from "../features/alpha";
import { formatName } from "../shared/format";
import { createEngine } from "../transfer-engine/engine";

export function ComposesFeatures() {
  return <AlphaTitle name={formatName(String(createEngine().queued))} />;
}
