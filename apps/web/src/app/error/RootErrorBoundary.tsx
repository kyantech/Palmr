import type { i18n as I18n } from "i18next";
import { Component, type ReactNode } from "react";
import { ErrorRecovery } from "./ErrorRecovery";
import {
  isStaleChunkError,
  requestIdOf,
  staleChunkRecovery,
  type StaleChunkRecovery,
} from "./staleChunk";

interface RootErrorBoundaryProps {
  children: ReactNode;
  i18n?: I18n;
  recovery?: StaleChunkRecovery;
}

interface RootErrorBoundaryState {
  failure: { error: unknown } | null;
}

export interface FallbackText {
  title: string;
  description: string;
  reload: string;
  requestId: (requestId: string) => string;
}

const FALLBACK_TEXT: Readonly<Record<"crash" | "staleChunk", FallbackText>> = {
  crash: {
    title: "Something went wrong",
    description: "An unexpected error stopped Palmr from loading.",
    reload: "Reload",
    requestId: (requestId) => `Request ID: ${requestId}`,
  },
  staleChunk: {
    title: "Palmr was updated",
    description: "A newer version of Palmr is available. Reload the page to continue.",
    reload: "Reload",
    requestId: (requestId) => `Request ID: ${requestId}`,
  },
};

export function fallbackText(error: unknown, i18n?: I18n): FallbackText {
  const kind = isStaleChunkError(error) ? "staleChunk" : "crash";
  const english = FALLBACK_TEXT[kind];
  try {
    if (!i18n?.isInitialized || !i18n.hasLoadedNamespace("errors")) {
      return english;
    }
    const t = i18n.getFixedT(null, "errors");
    const prefix = kind === "staleChunk" ? "staleChunk" : "boundary";
    return {
      title: t(`${prefix}.title`),
      description: t(`${prefix}.description`),
      reload: t("boundary.reload"),
      requestId: (requestId) => t("details.requestId", { requestId }),
    };
  } catch {
    return english;
  }
}

export class RootErrorBoundary extends Component<RootErrorBoundaryProps, RootErrorBoundaryState> {
  override state: RootErrorBoundaryState = { failure: null };

  static getDerivedStateFromError(error: unknown): RootErrorBoundaryState {
    return { failure: { error } };
  }

  override render() {
    const { failure } = this.state;
    if (!failure) {
      return this.props.children;
    }
    const recovery = this.props.recovery ?? staleChunkRecovery;
    return (
      <ErrorRecovery
        error={failure.error}
        recovery={recovery}
        fallback={
          <RootFallback
            text={fallbackText(failure.error, this.props.i18n)}
            requestId={requestIdOf(failure.error)}
            onReload={recovery.reload}
          />
        }
      />
    );
  }
}

interface RootFallbackProps {
  text: FallbackText;
  requestId: string | null;
  onReload: () => void;
}

function RootFallback({ text, requestId, onReload }: RootFallbackProps) {
  return (
    <main
      role="alert"
      aria-labelledby="palmr-root-error-title"
      style={{
        maxWidth: "32rem",
        margin: "15vh auto",
        padding: "0 1.5rem",
        fontFamily: "system-ui",
      }}
    >
      <h1 id="palmr-root-error-title">{text.title}</h1>
      <p>{text.description}</p>
      {requestId === null ? null : (
        <p>
          <code>{text.requestId(requestId)}</code>
        </p>
      )}
      <button type="button" onClick={onReload}>
        {text.reload}
      </button>
    </main>
  );
}
