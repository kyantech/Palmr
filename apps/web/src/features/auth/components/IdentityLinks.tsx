import { Alert, Button, Flex, Modal, Skeleton, theme, Typography } from "antd";
import { useState } from "react";
import { useTranslation } from "react-i18next";
import {
  ErrorAlert,
  isApiErrorCode,
  presentError,
  type ReportedError,
  ReportedErrorAlert,
} from "../../../shared/errors";
import { formatDateTime } from "../../../shared/format/dateTime";
import { SettingsSection } from "../../../shared/ui/SettingsSection";
import { useStartIdentityLink, useUnlinkIdentity } from "../api/mutations";
import { type IdentityLinkItem, useIdentityLinks } from "../api/queries";
import { type LoginProvider, ProviderIcon } from "./ProviderButtons";

export interface IdentityLinksProps {
  providers: readonly LoginProvider[];
  callbackError?: ReportedError | null;
  onUnlinked: () => Promise<void>;
}

function reportable(error: unknown): unknown {
  const presented = presentError(error);
  return presented.presentation.silent || presented.code === "AUTH_RECENT_AUTH_REQUIRED"
    ? null
    : error;
}

interface LinkRowProps {
  link: IdentityLinkItem;
  iconKey: string;
  onUnlink: (link: IdentityLinkItem) => void;
}

function LinkRow({ link, iconKey, onUnlink }: LinkRowProps) {
  const { t, i18n } = useTranslation("auth");
  const { token } = theme.useToken();
  const facts = [
    ...(link.emailAtLink === null
      ? []
      : [t("identityLinks.linkedAs", { email: link.emailAtLink })]),
    t("identityLinks.linkedAt", { date: formatDateTime(link.linkedAt, i18n.language) }),
    link.lastUsedAt === null
      ? t("identityLinks.neverUsed")
      : t("identityLinks.lastUsed", { date: formatDateTime(link.lastUsedAt, i18n.language) }),
  ];
  return (
    <li
      data-testid="identity-link-row"
      data-provider={link.providerSlug}
      data-link-id={link.id}
      style={{
        listStyle: "none",
        paddingBlock: token.padding,
        borderBottom: `${String(token.lineWidth)}px ${token.lineType} ${token.colorSplit}`,
      }}
    >
      <Flex gap={token.margin} align="flex-start" wrap>
        <span
          style={{
            display: "inline-flex",
            alignItems: "center",
            justifyContent: "center",
            flex: "none",
            width: 36,
            height: 36,
            fontSize: 18,
            borderRadius: token.borderRadius,
            color: token.colorTextSecondary,
            background: token.colorFillTertiary,
          }}
        >
          <ProviderIcon iconKey={iconKey} />
        </span>
        <Flex vertical gap={token.marginXXS} style={{ flex: "1 1 240px", minWidth: 0 }}>
          <Typography.Text strong>{link.providerDisplayName}</Typography.Text>
          <Flex vertical gap={2}>
            {facts.map((fact) => (
              <Typography.Text key={fact} type="secondary" style={{ fontSize: token.fontSizeSM }}>
                {fact}
              </Typography.Text>
            ))}
          </Flex>
        </Flex>
        <Button
          danger
          aria-label={t("identityLinks.unlink.label", { provider: link.providerDisplayName })}
          onClick={() => {
            onUnlink(link);
          }}
        >
          {t("identityLinks.unlink.action")}
        </Button>
      </Flex>
    </li>
  );
}

export function IdentityLinks({ providers, callbackError = null, onUnlinked }: IdentityLinksProps) {
  const { t } = useTranslation("auth");
  const { token } = theme.useToken();
  const links = useIdentityLinks();
  const startLink = useStartIdentityLink();
  const unlink = useUnlinkIdentity(onUnlinked);
  const [target, setTarget] = useState<IdentityLinkItem | null>(null);

  const items = links.data?.pages.flatMap((page) => page.items) ?? [];
  const linkedSlugs = new Set(items.map((item) => item.providerSlug));
  const linkable = providers.filter((provider) => !linkedSlugs.has(provider.slug));
  const iconKeyOf = (slug: string) =>
    providers.find((provider) => provider.slug === slug)?.iconKey ?? "generic";
  const linking = startLink.isPending || startLink.isSuccess;
  const unlinkRefused =
    unlink.isError && !isApiErrorCode(unlink.error, "AUTH_RECENT_AUTH_REQUIRED");

  const closeDialog = () => {
    if (!unlink.isPending) {
      setTarget(null);
    }
  };
  const confirmUnlink = () => {
    if (target === null || unlink.isPending) {
      return;
    }
    unlink.mutate({ id: target.id });
  };

  let list;
  if (links.isPending) {
    list = <Skeleton active paragraph={{ rows: 2 }} />;
  } else if (links.isError) {
    list = <ErrorAlert error={links.error} />;
  } else if (items.length === 0) {
    list = (
      <Flex
        vertical
        gap={token.marginXXS}
        data-testid="identity-links-empty"
        style={{
          padding: token.paddingLG,
          textAlign: "center",
          border: `${String(token.lineWidth)}px dashed ${token.colorBorder}`,
          borderRadius: token.borderRadiusLG,
        }}
      >
        <Typography.Text strong>{t("identityLinks.empty.title")}</Typography.Text>
        <Typography.Text type="secondary">{t("identityLinks.empty.description")}</Typography.Text>
      </Flex>
    );
  } else {
    list = (
      <ul aria-label={t("identityLinks.listLabel")} style={{ margin: 0, padding: 0 }}>
        {items.map((link) => (
          <LinkRow
            key={link.id}
            link={link}
            iconKey={iconKeyOf(link.providerSlug)}
            onUnlink={(selected) => {
              unlink.reset();
              setTarget(selected);
            }}
          />
        ))}
      </ul>
    );
  }

  const startFailure = startLink.isError ? reportable(startLink.error) : null;
  const unlinkFailure = unlink.isError ? reportable(unlink.error) : null;

  return (
    <SettingsSection
      title={t("identityLinks.title")}
      description={t("identityLinks.description")}
      testId="settings-identity-links"
    >
      <Flex vertical gap={token.margin}>
        {callbackError === null ? null : (
          <div data-testid="identity-link-callback-error">
            <ReportedErrorAlert reported={callbackError} />
          </div>
        )}
        {startFailure === null ? null : <ErrorAlert error={startFailure} />}
        {unlinkFailure === null ? null : <ErrorAlert error={unlinkFailure} />}
        {list}
        {links.hasNextPage ? (
          <div>
            <Button
              loading={links.isFetchingNextPage}
              onClick={() => {
                void links.fetchNextPage();
              }}
            >
              {t("identityLinks.loadMore")}
            </Button>
          </div>
        ) : null}
        {links.isSuccess && linkable.length > 0 ? (
          <Flex vertical gap={token.marginXS} data-testid="identity-links-available">
            <Typography.Text strong>{t("identityLinks.available.title")}</Typography.Text>
            <Flex gap={token.marginXS} wrap>
              {linkable.map((provider) => (
                <Button
                  key={provider.slug}
                  data-provider={provider.slug}
                  icon={<ProviderIcon iconKey={provider.iconKey} />}
                  loading={startLink.isPending && startLink.variables.slug === provider.slug}
                  disabled={linking}
                  onClick={() => {
                    startLink.mutate({ slug: provider.slug });
                  }}
                >
                  {t("identityLinks.link", { provider: provider.displayName })}
                </Button>
              ))}
            </Flex>
            <Typography.Text type="secondary" style={{ fontSize: token.fontSizeSM }}>
              {t("identityLinks.available.hint")}
            </Typography.Text>
          </Flex>
        ) : null}
        {links.isSuccess && items.length === 0 && linkable.length === 0 ? (
          <Alert
            type="info"
            showIcon
            role="status"
            data-testid="identity-links-no-providers"
            title={t("identityLinks.noProviders")}
          />
        ) : null}
      </Flex>
      <Modal
        open={target !== null && !unlinkRefused}
        title={t("identityLinks.unlink.title", { provider: target?.providerDisplayName ?? "" })}
        okText={t("identityLinks.unlink.confirm")}
        cancelText={t("identityLinks.unlink.cancel")}
        okButtonProps={{ danger: true, loading: unlink.isPending }}
        cancelButtonProps={{ disabled: unlink.isPending }}
        onOk={confirmUnlink}
        onCancel={closeDialog}
        centered
        destroyOnHidden
        width={440}
        mask={{ closable: !unlink.isPending }}
        keyboard={!unlink.isPending}
      >
        <Flex vertical gap={token.marginXS}>
          <Typography.Text>{t("identityLinks.unlink.description")}</Typography.Text>
          <Typography.Text type="secondary">
            {t("identityLinks.unlink.consequence")}
          </Typography.Text>
        </Flex>
      </Modal>
    </SettingsSection>
  );
}
