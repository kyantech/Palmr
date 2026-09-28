import { screen } from "@testing-library/react";
import { describe, expect, test } from "vitest";
import { renderFeature } from "../../../test/renderFeature";
import { OverviewPage } from "./OverviewPage";

describe("component_overview_placeholder", () => {
  test("contains nothing but its page title until M23-T01", async () => {
    const { view } = await renderFeature(<OverviewPage />);

    const heading = await screen.findByRole("heading", { level: 1, name: "Overview" });
    expect(heading.tagName).toBe("H1");

    const page = view.container.querySelector("[data-testid='overview-page']");
    expect(page?.textContent).toBe("Overview");
    expect(
      view.container.querySelectorAll("button, a, img, input, table, ul, ol, canvas, svg"),
    ).toHaveLength(0);
  });
});
