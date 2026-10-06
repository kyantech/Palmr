import ArrowChevronDown from "@gravity-ui/icons/ArrowChevronDown";
import ArrowChevronUp from "@gravity-ui/icons/ArrowChevronUp";
import Grip from "@gravity-ui/icons/Grip";
import { Button, Flex, Switch, Tag, theme, Typography } from "antd";
import type { DragEventHandler } from "react";
import { useTranslation } from "react-i18next";
import { formatDateTime } from "../../../shared/format/dateTime";
import type { Provider } from "../types";
import { ProviderChecks, type CheckLine } from "./ProviderChecks";
import { parseValidationError } from "./providerFormModel";
import { RedirectUri } from "./RedirectUri";

export interface ProviderDragProps {
  dragging: boolean;
  over: boolean;
  onDragStart: DragEventHandler<HTMLElement>;
  onDragOver: DragEventHandler<HTMLElement>;
  onDrop: DragEventHandler<HTMLElement>;
  onDragEnd: DragEventHandler<HTMLElement>;
}

interface ProviderRowProps {
  provider: Provider;
  position: number;
  total: number;
  busy: boolean;
  testing: boolean;
  checks: readonly CheckLine[] | null;
  drag: ProviderDragProps;
  onEdit: (provider: Provider) => void;
  onTest: (provider: Provider) => void;
  onDelete: (provider: Provider) => void;
  onToggle: (provider: Provider, enabled: boolean) => void;
  onMove: (provider: Provider, delta: -1 | 1) => void;
}

function ValidationTag({ provider }: { provider: Provider }) {
  const { t, i18n } = useTranslation("admin");
  if (provider.validatedAt !== null && provider.validationError === null) {
    return (
      <Tag
        color="success"
        variant="filled"
        data-testid="provider-validation"
        data-state="validated"
      >
        {t("providers.row.validation.validated", {
          date: formatDateTime(provider.validatedAt, i18n.language),
        })}
      </Tag>
    );
  }
  if (provider.validationError !== null) {
    return (
      <Tag color="error" variant="filled" data-testid="provider-validation" data-state="failed">
        {t("providers.row.validation.failed")}
      </Tag>
    );
  }
  return (
    <Tag variant="filled" data-testid="provider-validation" data-state="untested">
      {t("providers.row.validation.untested")}
    </Tag>
  );
}

export function ProviderRow({
  provider,
  position,
  total,
  busy,
  testing,
  checks,
  drag,
  onEdit,
  onTest,
  onDelete,
  onToggle,
  onMove,
}: ProviderRowProps) {
  const { t } = useTranslation("admin");
  const { token } = theme.useToken();
  const persistedFailures = parseValidationError(provider.validationError);
  const shownChecks: readonly CheckLine[] | null =
    checks ??
    (persistedFailures.length === 0
      ? null
      : persistedFailures.map((failure) => ({
          name: failure.name,
          ok: false,
          detail: failure.reason,
        })));
  return (
    <li
      data-testid="provider-row"
      data-provider-id={provider.id}
      data-provider-slug={provider.slug}
      data-dragging={drag.dragging ? "true" : "false"}
      onDragOver={drag.onDragOver}
      onDrop={drag.onDrop}
      style={{
        listStyle: "none",
        padding: token.padding,
        marginBottom: token.marginSM,
        border: `${String(token.lineWidth)}px ${token.lineType} ${
          drag.over ? token.colorPrimary : token.colorBorderSecondary
        }`,
        borderRadius: token.borderRadiusLG,
        background: token.colorBgContainer,
        opacity: drag.dragging ? 0.55 : 1,
      }}
    >
      <Flex gap={token.margin} align="flex-start" wrap>
        <Flex vertical align="center" gap={token.marginXXS} style={{ flex: "none" }}>
          <Button
            type="text"
            size="small"
            aria-label={t("providers.row.moveUp", { name: provider.displayName })}
            disabled={busy || position === 1}
            icon={<ArrowChevronUp aria-hidden="true" focusable="false" width="1em" height="1em" />}
            onClick={() => {
              onMove(provider, -1);
            }}
          />
          <span
            role="presentation"
            title={t("providers.row.drag")}
            data-testid="provider-drag-handle"
            draggable
            onDragStart={drag.onDragStart}
            onDragEnd={drag.onDragEnd}
            style={{ cursor: "grab", fontSize: 18, color: token.colorTextTertiary, lineHeight: 1 }}
          >
            <Grip aria-hidden="true" focusable="false" width="1em" height="1em" />
          </span>
          <Button
            type="text"
            size="small"
            aria-label={t("providers.row.moveDown", { name: provider.displayName })}
            disabled={busy || position === total}
            icon={
              <ArrowChevronDown aria-hidden="true" focusable="false" width="1em" height="1em" />
            }
            onClick={() => {
              onMove(provider, 1);
            }}
          />
        </Flex>
        <Flex vertical gap={token.marginXS} style={{ flex: "1 1 320px", minWidth: 0 }}>
          <Flex align="center" gap={token.marginXS} wrap>
            <Typography.Text strong style={{ fontSize: token.fontSizeLG }}>
              {provider.displayName}
            </Typography.Text>
            <Tag variant="filled" style={{ marginInlineEnd: 0 }}>
              {t(`providers.row.protocol.${provider.protocol}`)}
            </Tag>
            <Typography.Text type="secondary" code>
              {provider.slug}
            </Typography.Text>
            <Typography.Text type="secondary" style={{ fontSize: token.fontSizeSM }}>
              {t("providers.row.position", { position, total })}
            </Typography.Text>
          </Flex>
          <Flex gap={token.marginXXS} wrap>
            <Tag
              color={provider.enabled ? "processing" : "default"}
              variant="filled"
              data-testid="provider-enabled"
              data-enabled={provider.enabled ? "true" : "false"}
            >
              {t(provider.enabled ? "providers.row.enabled" : "providers.row.disabled")}
            </Tag>
            <ValidationTag provider={provider} />
            <Tag
              variant="filled"
              data-testid="provider-secret"
              data-configured={provider.clientSecretConfigured ? "true" : "false"}
            >
              {t(
                provider.clientSecretConfigured
                  ? "providers.row.secret.configured"
                  : "providers.row.secret.missing",
              )}
            </Tag>
            <Tag variant="filled" data-testid="provider-auto-provision">
              {t(
                provider.autoProvision
                  ? "providers.row.autoProvision.on"
                  : "providers.row.autoProvision.off",
              )}
            </Tag>
            <Tag variant="filled" data-testid="provider-email-linking">
              {t(
                provider.allowEmailLinking
                  ? "providers.row.emailLinking.on"
                  : "providers.row.emailLinking.off",
              )}
            </Tag>
            <Tag variant="filled">
              {t("providers.row.linkedUsers", { total: provider.linkedUserCount })}
            </Tag>
          </Flex>
          <RedirectUri value={provider.redirectUri} />
          {shownChecks === null ? null : (
            <ProviderChecks
              checks={shownChecks}
              label={t("providers.checks.title", { name: provider.displayName })}
            />
          )}
        </Flex>
        <Flex vertical align="flex-end" gap={token.marginXS} style={{ flex: "none" }}>
          <Flex align="center" gap={token.marginXS}>
            <Switch
              checked={provider.enabled}
              disabled={busy}
              aria-label={t("providers.row.toggle", { name: provider.displayName })}
              onChange={(checked) => {
                onToggle(provider, checked);
              }}
            />
          </Flex>
          <Flex gap={token.marginXS} wrap justify="flex-end">
            <Button
              loading={testing}
              disabled={busy}
              aria-label={t("providers.row.testLabel", { name: provider.displayName })}
              onClick={() => {
                onTest(provider);
              }}
            >
              {t("providers.row.test")}
            </Button>
            <Button
              disabled={busy}
              aria-label={t("providers.row.editLabel", { name: provider.displayName })}
              onClick={() => {
                onEdit(provider);
              }}
            >
              {t("providers.row.edit")}
            </Button>
            <Button
              danger
              disabled={busy}
              aria-label={t("providers.row.deleteLabel", { name: provider.displayName })}
              onClick={() => {
                onDelete(provider);
              }}
            >
              {t("providers.row.delete")}
            </Button>
          </Flex>
        </Flex>
      </Flex>
    </li>
  );
}
