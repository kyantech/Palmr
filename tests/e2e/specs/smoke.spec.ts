import { expect, test } from "@playwright/test";

test("e2e_shell_loads", async ({ page }) => {
  const pageErrors: string[] = [];
  page.on("pageerror", (error) => pageErrors.push(error.message));

  const response = await page.goto("/");
  expect(response).not.toBeNull();
  expect(response?.ok()).toBe(true);
  await expect(
    page.getByRole("heading", { name: "Palmr", exact: true }),
  ).toBeVisible();
  expect(pageErrors).toEqual([]);
});
