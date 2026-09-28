import { act, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, test, vi } from "vitest";
import { ApiError } from "../../shared/errors";
import { BOOT_SPINNER_DELAY_MS, BootFailure, BootLoading } from "./BootScreens";

afterEach(() => {
  vi.useRealTimers();
});

describe("boot screens", () => {
  test("loading is an accessible status whose spinner appears only after a short delay", () => {
    vi.useFakeTimers();
    render(<BootLoading />);

    const status = screen.getByRole("status");
    expect(status.getAttribute("aria-busy")).toBe("true");
    expect(status.textContent).toBe("Loading Palmr");
    expect(screen.getByTestId("boot-spinner").style.visibility).toBe("hidden");

    act(() => {
      vi.advanceTimersByTime(BOOT_SPINNER_DELAY_MS);
    });

    expect(screen.getByTestId("boot-spinner").style.visibility).toBe("visible");
  });

  test("failure shows a generic message, the request id and a working Retry, never the server message", async () => {
    const onRetry = vi.fn();
    const failure = new ApiError({
      code: "INTERNAL_ERROR",
      status: 500,
      requestId: "req-7",
      details: {},
      request: { method: "GET", path: "/bootstrap" },
      serverMessage: "sqlite: database disk image is malformed",
    });

    render(<BootFailure error={failure} onRetry={onRetry} />);

    const alert = screen.getByRole("alert");
    expect(screen.getByRole("heading", { level: 1 }).textContent).toBe("Palmr could not start");
    expect(alert.textContent).toContain("Request ID: req-7");
    expect(alert.textContent).not.toContain("sqlite");
    expect(alert.textContent).not.toContain("INTERNAL_ERROR");

    await userEvent.click(screen.getByRole("button", { name: "Retry" }));
    expect(onRetry).toHaveBeenCalledTimes(1);
  });
});
