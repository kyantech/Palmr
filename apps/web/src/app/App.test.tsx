import { render, screen } from "@testing-library/react";
import { expect, test } from "vitest";
import { App } from "./App";

test("unit_app_renders", () => {
  render(<App />);

  expect(screen.getByRole("heading", { level: 1 })).toBeDefined();
});
