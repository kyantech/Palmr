import { type CSSProperties, useEffect, useRef, useState } from "react";
import { requestIdOf } from "../error/staleChunk";

export const BOOT_TEXT = {
  loading: "Loading Palmr",
  failedTitle: "Palmr could not start",
  failedDescription:
    "The server could not be reached or did not respond as expected. Check your connection and try again.",
  retry: "Retry",
  requestId: (requestId: string) => `Request ID: ${requestId}`,
} as const;

export const BOOT_SPINNER_DELAY_MS = 250;

const screen: CSSProperties = {
  boxSizing: "border-box",
  minHeight: "100dvh",
  display: "flex",
  alignItems: "center",
  justifyContent: "center",
  padding: "24px",
  colorScheme: "light dark",
  background: "Canvas",
  color: "CanvasText",
  fontFamily: "system-ui, -apple-system, 'Segoe UI', Roboto, sans-serif",
};

const visuallyHidden: CSSProperties = {
  position: "absolute",
  width: 1,
  height: 1,
  margin: -1,
  padding: 0,
  overflow: "hidden",
  clip: "rect(0 0 0 0)",
  whiteSpace: "nowrap",
  border: 0,
};

const ring: CSSProperties = {
  boxSizing: "border-box",
  width: 28,
  height: 28,
  borderRadius: "50%",
  border: "2px solid color-mix(in srgb, CanvasText 14%, transparent)",
  borderTopColor: "color-mix(in srgb, CanvasText 60%, transparent)",
};

function prefersReducedMotion(): boolean {
  return window.matchMedia("(prefers-reduced-motion: reduce)").matches;
}

export function BootLoading() {
  const [visible, setVisible] = useState(false);
  const spinner = useRef<HTMLDivElement>(null);

  useEffect(() => {
    const timer = window.setTimeout(() => {
      setVisible(true);
    }, BOOT_SPINNER_DELAY_MS);
    return () => {
      window.clearTimeout(timer);
    };
  }, []);

  useEffect(() => {
    const element = spinner.current;
    if (!visible || !element || typeof element.animate !== "function" || prefersReducedMotion()) {
      return;
    }
    const animation = element.animate(
      [{ transform: "rotate(0deg)" }, { transform: "rotate(360deg)" }],
      { duration: 800, iterations: Infinity, easing: "linear" },
    );
    return () => {
      animation.cancel();
    };
  }, [visible]);

  return (
    <div role="status" aria-live="polite" aria-busy="true" style={screen}>
      <span style={visuallyHidden}>{BOOT_TEXT.loading}</span>
      <div
        ref={spinner}
        aria-hidden="true"
        data-testid="boot-spinner"
        style={{ ...ring, visibility: visible ? "visible" : "hidden" }}
      />
    </div>
  );
}

interface BootFailureProps {
  error: unknown;
  onRetry: () => void;
}

export function BootFailure({ error, onRetry }: BootFailureProps) {
  const requestId = requestIdOf(error);
  return (
    <div style={screen}>
      <main
        role="alert"
        aria-labelledby="palmr-boot-error-title"
        style={{ width: "100%", maxWidth: "28rem" }}
      >
        <h1
          id="palmr-boot-error-title"
          style={{ margin: "0 0 8px", fontSize: "1.25rem", fontWeight: 600, lineHeight: 1.4 }}
        >
          {BOOT_TEXT.failedTitle}
        </h1>
        <p style={{ margin: "0 0 16px", fontSize: "0.875rem", lineHeight: 1.6, opacity: 0.75 }}>
          {BOOT_TEXT.failedDescription}
        </p>
        {requestId === null ? null : (
          <p style={{ margin: "0 0 20px", fontSize: "0.8125rem", opacity: 0.75 }}>
            <code style={{ userSelect: "all" }}>{BOOT_TEXT.requestId(requestId)}</code>
          </p>
        )}
        <button
          type="button"
          onClick={onRetry}
          style={{
            font: "inherit",
            fontSize: "0.875rem",
            fontWeight: 500,
            padding: "6px 16px",
            borderRadius: 6,
            border: "1px solid CanvasText",
            background: "CanvasText",
            color: "Canvas",
            cursor: "pointer",
          }}
        >
          {BOOT_TEXT.retry}
        </button>
      </main>
    </div>
  );
}
