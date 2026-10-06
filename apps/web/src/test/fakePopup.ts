import { vi } from "vitest";

export interface FakePopup {
  closed: boolean;
  location: { href: string };
  close: ReturnType<typeof vi.fn>;
  postMessage: ReturnType<typeof vi.fn>;
}

export interface PopupHarness {
  open: ReturnType<typeof vi.fn>;
  popups: FakePopup[];
  locationAtOpen: string[];
  restore: () => void;
}

export function stubWindowOpen({ blocked = false }: { blocked?: boolean } = {}): PopupHarness {
  const popups: FakePopup[] = [];
  const locationAtOpen: string[] = [];
  const open = vi.fn((url?: string | URL) => {
    locationAtOpen.push(String(url));
    if (blocked) {
      return null;
    }
    const popup: FakePopup = {
      closed: false,
      location: { href: String(url) },
      close: vi.fn(() => {
        popup.closed = true;
      }),
      postMessage: vi.fn(),
    };
    popups.push(popup);
    return popup;
  });
  const original = window.open.bind(window);
  window.open = open as unknown as typeof window.open;
  return {
    open,
    popups,
    locationAtOpen,
    restore: () => {
      window.open = original;
    },
  };
}

export function dispatchWindowMessage(
  data: unknown,
  {
    source,
    origin = window.location.origin,
  }: { source: FakePopup | Window | null; origin?: string },
) {
  const event = new MessageEvent("message", { data, origin });
  Object.defineProperty(event, "source", { value: source });
  window.dispatchEvent(event);
}
