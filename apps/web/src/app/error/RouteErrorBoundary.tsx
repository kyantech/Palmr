import { Button, Result } from "antd";
import { Component, type ReactNode } from "react";
import { useTranslation } from "react-i18next";
import { RequestId } from "../../shared/errors";
import { ErrorRecovery } from "./ErrorRecovery";
import {
  isStaleChunkError,
  requestIdOf,
  staleChunkRecovery,
  type StaleChunkRecovery,
} from "./staleChunk";

interface RouteErrorViewProps {
  error: unknown;
  recovery?: StaleChunkRecovery | undefined;
}

export function RouteErrorView({ error, recovery = staleChunkRecovery }: RouteErrorViewProps) {
  return (
    <ErrorRecovery
      error={error}
      recovery={recovery}
      fallback={<RouteErrorPanel error={error} onReload={recovery.reload} />}
    />
  );
}

interface RouteErrorBoundaryProps {
  children: ReactNode;
  recovery?: StaleChunkRecovery;
}

interface RouteErrorBoundaryState {
  failure: { error: unknown } | null;
}

export class RouteErrorBoundary extends Component<
  RouteErrorBoundaryProps,
  RouteErrorBoundaryState
> {
  override state: RouteErrorBoundaryState = { failure: null };

  static getDerivedStateFromError(error: unknown): RouteErrorBoundaryState {
    return { failure: { error } };
  }

  override render() {
    const { failure } = this.state;
    if (!failure) {
      return this.props.children;
    }
    return <RouteErrorView error={failure.error} recovery={this.props.recovery} />;
  }
}

interface RouteErrorPanelProps {
  error: unknown;
  onReload: () => void;
}

function RouteErrorPanel({ error, onReload }: RouteErrorPanelProps) {
  const { t } = useTranslation("errors");
  const prefix = isStaleChunkError(error) ? "staleChunk" : "boundary";
  const requestId = requestIdOf(error);
  return (
    <Result
      status="error"
      title={t(`${prefix}.title`)}
      subTitle={t(`${prefix}.description`)}
      extra={
        <Button type="primary" onClick={onReload}>
          {t("boundary.reload")}
        </Button>
      }
    >
      {requestId === null ? null : <RequestId requestId={requestId} />}
    </Result>
  );
}
