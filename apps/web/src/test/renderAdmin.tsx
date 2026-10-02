import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { expect } from "vitest";
import { appRoutes } from "../app/router/routes";
import { type AdminServerOptions, installAdminServer } from "./adminServer";
import { renderSession } from "./renderSession";

export type User = ReturnType<typeof userEvent.setup>;

export function renderAdmin(path: string, options: AdminServerOptions = {}) {
  const state = installAdminServer(options);
  const session = renderSession({ routes: appRoutes, initialEntries: [path] });
  return { state, ...session, user: userEvent.setup({ delay: null }) };
}

export async function chooseOption(user: User, combobox: HTMLElement, label: string) {
  await user.click(combobox);
  const option = await screen.findByTitle(label, undefined, { timeout: 3000 });
  await user.click(option);
}

export async function findRow(name: string): Promise<HTMLElement> {
  const link = await screen.findByRole("link", { name });
  const row = link.closest("tr");
  if (row === null) {
    throw new Error(`no table row contains ${name}`);
  }
  return row;
}

export async function findDialog(title: string): Promise<HTMLElement> {
  return waitFor(() => {
    const dialog = screen
      .getAllByRole("dialog")
      .find((candidate) => within(candidate).queryAllByText(title).length > 0);
    if (dialog === undefined) {
      throw new Error(`no dialog titled ${title}`);
    }
    return dialog;
  });
}

export async function expectNoDialog(): Promise<void> {
  await waitFor(() => {
    expect(screen.queryAllByRole("dialog")).toHaveLength(0);
  });
}
