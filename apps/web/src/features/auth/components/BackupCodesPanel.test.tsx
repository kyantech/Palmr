import { screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, expect, test, vi } from "vitest";
import { BACKUP_CODES, TOTP_SECRET } from "../../../test/authServer";
import { renderFeature } from "../../../test/renderFeature";
import { stubMatchMedia } from "../../../test/renderSession";
import { BackupCodesPanel, backupCodesDocument, backupCodesFileName } from "./BackupCodesPanel";

let blobs: Blob[];
let anchors: { download: string; href: string }[];

beforeEach(() => {
  stubMatchMedia();
  blobs = [];
  anchors = [];
  vi.spyOn(URL, "createObjectURL").mockImplementation((object) => {
    blobs.push(object as Blob);
    return "blob:backup-codes";
  });
  vi.spyOn(URL, "revokeObjectURL").mockImplementation(() => undefined);
  vi.spyOn(HTMLAnchorElement.prototype, "click").mockImplementation(function (
    this: HTMLAnchorElement,
  ) {
    anchors.push({ download: this.download, href: this.href });
  });
});

afterEach(() => {
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

async function renderPanel(onDone = vi.fn()) {
  await renderFeature(
    <BackupCodesPanel codes={BACKUP_CODES} appName="Acme Files" doneLabel="Done" onDone={onDone} />,
  );
  await screen.findByText("Save your backup codes");
  return { onDone, user: userEvent.setup() };
}

test("unit_backup_code_download_contains_the_full_current_codes_and_nothing_else", async () => {
  const { user } = await renderPanel();

  await user.click(
    screen.getByRole("button", { name: "Download the backup codes as a text file" }),
  );

  expect(anchors).toEqual([{ download: "acme-files-backup-codes.txt", href: "blob:backup-codes" }]);
  expect(blobs).toHaveLength(1);
  const [blob] = blobs;
  if (blob === undefined) {
    throw new Error("no file was produced");
  }
  expect(blob.type).toBe("text/plain;charset=utf-8");
  const text = await blob.text();
  expect(text).toBe(
    [
      "Acme Files backup codes",
      "Each code can be used once to sign in if you can't use your authenticator app.",
      "",
      ...BACKUP_CODES,
      "",
    ].join("\n"),
  );
  for (const code of BACKUP_CODES) {
    expect(code).toMatch(/^[A-Z2-7]{4}-[A-Z2-7]{4}-[A-Z2-7]{4}-[A-Z2-7]{4}$/);
  }
  expect(text).not.toContain(TOTP_SECRET);
  expect(text).not.toMatch(/otpauth|mfa|token|password/i);
  expect(document.querySelector("a[download]")).toBeNull();
});

test("copy puts every code on the clipboard and the result is announced", async () => {
  const { user } = await renderPanel();

  await user.click(screen.getByRole("button", { name: "Copy all backup codes" }));

  expect(await navigator.clipboard.readText()).toBe(BACKUP_CODES.join("\n"));
  expect(await screen.findByText("Copied")).toBeDefined();
});

test("the codes cannot be dismissed until the user confirms they were saved", async () => {
  const { user, onDone } = await renderPanel();

  const done = screen.getByRole<HTMLButtonElement>("button", { name: "Done" });
  expect(done.disabled).toBe(true);
  await user.click(
    screen.getByRole("checkbox", { name: "I saved these backup codes somewhere safe" }),
  );
  await user.click(done);

  expect(onDone).toHaveBeenCalledTimes(1);
  expect(screen.getAllByTestId("backup-code").map((item) => item.textContent)).toEqual(
    BACKUP_CODES,
  );
});

test("the document and file name are built only from non-secret inputs", () => {
  expect(backupCodesDocument("H", "N", ["A", "B"])).toBe("H\nN\n\nA\nB\n");
  expect(backupCodesFileName("Palmr")).toBe("palmr-backup-codes.txt");
  expect(backupCodesFileName("  Équipe / Files  ")).toBe("equipe-files-backup-codes.txt");
  expect(backupCodesFileName("文件")).toBe("palmr-backup-codes.txt");
});
