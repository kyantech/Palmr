import { Alert, Button, Checkbox, Flex, theme, Typography } from "antd";
import { useEffect, useId, useState } from "react";
import { useTranslation } from "react-i18next";

export function backupCodesDocument(
  heading: string,
  note: string,
  codes: readonly string[],
): string {
  return `${heading}\n${note}\n\n${codes.join("\n")}\n`;
}

export function backupCodesFileName(appName: string): string {
  const slug = appName
    .normalize("NFKD")
    .replace(/\p{M}+/gu, "")
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-+|-+$/g, "");
  return `${slug === "" ? "palmr" : slug}-backup-codes.txt`;
}

function saveTextFile(text: string, fileName: string) {
  const url = URL.createObjectURL(new Blob([text], { type: "text/plain;charset=utf-8" }));
  const anchor = document.createElement("a");
  anchor.href = url;
  anchor.download = fileName;
  anchor.rel = "noopener";
  anchor.style.display = "none";
  document.body.append(anchor);
  anchor.click();
  anchor.remove();
  window.setTimeout(() => {
    URL.revokeObjectURL(url);
  }, 0);
}

type CopyState = "idle" | "copied" | "failed";

interface BackupCodesPanelProps {
  codes: readonly string[];
  appName: string;
  doneLabel: string;
  onDone: () => void;
}

export function BackupCodesPanel({ codes, appName, doneLabel, onDone }: BackupCodesPanelProps) {
  const { t } = useTranslation("auth");
  const { token } = theme.useToken();
  const listId = useId();
  const [saved, setSaved] = useState(false);
  const [copy, setCopy] = useState<CopyState>("idle");

  useEffect(() => {
    if (copy === "idle") {
      return;
    }
    const timer = window.setTimeout(() => {
      setCopy("idle");
    }, 2_500);
    return () => {
      window.clearTimeout(timer);
    };
  }, [copy]);

  const copyAll = async () => {
    try {
      await navigator.clipboard.writeText(codes.join("\n"));
      setCopy("copied");
    } catch {
      setCopy("failed");
    }
  };

  const download = () => {
    saveTextFile(
      backupCodesDocument(
        t("backupCodes.file.heading", { appName }),
        t("backupCodes.file.note"),
        codes,
      ),
      backupCodesFileName(appName),
    );
  };

  return (
    <Flex vertical gap={token.margin} data-testid="backup-codes">
      <Alert
        type="warning"
        showIcon
        title={t("backupCodes.title")}
        description={t("backupCodes.description")}
      />
      <Typography.Text id={listId} strong>
        {t("backupCodes.listLabel")}
      </Typography.Text>
      <ol
        aria-labelledby={listId}
        translate="no"
        style={{
          margin: 0,
          padding: token.padding,
          listStyle: "none",
          display: "grid",
          gridTemplateColumns: "repeat(auto-fill, minmax(11.5rem, 1fr))",
          gap: `${String(token.marginXS)}px ${String(token.margin)}px`,
          background: token.colorFillQuaternary,
          border: `${String(token.lineWidth)}px ${token.lineType} ${token.colorBorderSecondary}`,
          borderRadius: token.borderRadiusLG,
        }}
      >
        {codes.map((code) => (
          <li
            key={code}
            data-testid="backup-code"
            style={{
              fontFamily: token.fontFamilyCode,
              fontSize: token.fontSize,
              letterSpacing: "0.04em",
              whiteSpace: "nowrap",
              fontVariantNumeric: "tabular-nums",
            }}
          >
            {code}
          </li>
        ))}
      </ol>
      <Flex gap={token.marginXS} wrap>
        <Button
          onClick={() => {
            void copyAll();
          }}
          aria-label={t("backupCodes.copyLabel")}
        >
          {t(copy === "copied" ? "backupCodes.copied" : "backupCodes.copy")}
        </Button>
        <Button onClick={download} aria-label={t("backupCodes.downloadLabel")}>
          {t("backupCodes.download")}
        </Button>
      </Flex>
      <div aria-live="polite" style={{ minHeight: 0 }}>
        {copy === "failed" ? (
          <Typography.Text type="danger">{t("backupCodes.copyFailed")}</Typography.Text>
        ) : null}
      </div>
      <Checkbox
        checked={saved}
        onChange={(event) => {
          setSaved(event.target.checked);
        }}
      >
        {t("backupCodes.acknowledge")}
      </Checkbox>
      <div>
        <Button type="primary" disabled={!saved} onClick={onDone}>
          {doneLabel}
        </Button>
      </div>
    </Flex>
  );
}
