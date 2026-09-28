import type { ReactNode } from "react";
import { BootFailure, BootLoading } from "./BootScreens";
import { BootStateContext, useBootStatus } from "./bootState";

export function BootGate({ children }: { children: ReactNode }) {
  const status = useBootStatus();
  switch (status.phase) {
    case "loading":
      return <BootLoading />;
    case "failed":
      return <BootFailure error={status.error} onRetry={status.retry} />;
    case "ready":
      return <BootStateContext value={status.state}>{children}</BootStateContext>;
  }
}
