import { Alert, Button, Flex, Popconfirm, Skeleton, Tag, theme, Typography } from "antd";
import ShieldCheck from "@gravity-ui/icons/ShieldCheck";
import { useState } from "react";
import { useTranslation } from "react-i18next";
import { ErrorAlert, presentError } from "../../../shared/errors";
import { formatDateTime } from "../../../shared/format/dateTime";
import { SettingsSection } from "../../../shared/ui/SettingsSection";
import { useRevokeAllTrustedDevices, useRevokeTrustedDevice } from "../api/mutations";
import { type TrustedDeviceItem, useTrustedDevices } from "../api/queries";

function formatDays(days: number, locale: string): string {
  try {
    return new Intl.NumberFormat(locale, {
      style: "unit",
      unit: "day",
      unitDisplay: "long",
    }).format(days);
  } catch {
    return String(days);
  }
}

function reportable(error: unknown): unknown {
  const presented = presentError(error);
  return presented.presentation.silent || presented.code === "AUTH_RECENT_AUTH_REQUIRED"
    ? null
    : error;
}

interface DeviceRowProps {
  device: TrustedDeviceItem;
  revoking: boolean;
  onRevoke: (device: TrustedDeviceItem) => void;
}

function DeviceRow({ device, revoking, onRevoke }: DeviceRowProps) {
  const { t, i18n } = useTranslation("auth");
  const { token } = theme.useToken();
  const locale = i18n.language;
  const label = device.label ?? t("trustedDevices.unknownDevice");
  const facts = [
    t("trustedDevices.trustedAt", { date: formatDateTime(device.createdAt, locale) }),
    t("trustedDevices.lastUsed", { date: formatDateTime(device.lastSeenAt, locale) }),
    t("trustedDevices.expires", { date: formatDateTime(device.expiresAt, locale) }),
    ...(device.ipAtEnrollment === null
      ? []
      : [t("trustedDevices.enrolledFrom", { ip: device.ipAtEnrollment })]),
  ];
  const variant = device.isCurrent ? "revokeCurrent" : "revoke";
  return (
    <li
      data-testid="trusted-device-row"
      data-device-id={device.id}
      data-current={device.isCurrent ? "true" : "false"}
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
          <ShieldCheck aria-hidden="true" focusable="false" width="1em" height="1em" />
        </span>
        <Flex vertical gap={token.marginXXS} style={{ flex: "1 1 240px", minWidth: 0 }}>
          <Flex align="center" gap={token.marginXS} wrap>
            <Typography.Text strong>{label}</Typography.Text>
            {device.isCurrent ? (
              <Tag color="success" variant="filled" style={{ marginInlineEnd: 0 }}>
                {t("trustedDevices.current")}
              </Tag>
            ) : null}
          </Flex>
          <Flex vertical gap={2}>
            {facts.map((fact) => (
              <Typography.Text key={fact} type="secondary" style={{ fontSize: token.fontSizeSM }}>
                {fact}
              </Typography.Text>
            ))}
          </Flex>
        </Flex>
        <Popconfirm
          title={t(`trustedDevices.${variant}.title`)}
          description={t(`trustedDevices.${variant}.description`)}
          okText={t("trustedDevices.revoke.confirm")}
          cancelText={t("trustedDevices.cancel")}
          okButtonProps={{ danger: true }}
          onConfirm={() => {
            onRevoke(device);
          }}
        >
          <Button
            danger
            loading={revoking}
            aria-label={t("trustedDevices.revoke.label", { device: label })}
          >
            {t("trustedDevices.revoke.action")}
          </Button>
        </Popconfirm>
      </Flex>
    </li>
  );
}

export function TrustedDevices() {
  const { t, i18n } = useTranslation("auth");
  const { token } = theme.useToken();
  const devices = useTrustedDevices();
  const revokeDevice = useRevokeTrustedDevice();
  const revokeAll = useRevokeAllTrustedDevices();
  const [failure, setFailure] = useState<unknown>(null);
  const [notice, setNotice] = useState<"revoked" | "allRevoked" | null>(null);

  const pages = devices.data?.pages ?? [];
  const items = pages.flatMap((page) => page.items);
  const ordered = [
    ...items.filter((item) => item.isCurrent),
    ...items.filter((item) => !item.isCurrent),
  ];
  const policy = pages[0]?.policy;
  const hasAny = items.length > 0 || devices.hasNextPage;

  const revoke = (device: TrustedDeviceItem) => {
    setFailure(null);
    setNotice(null);
    revokeDevice.mutate(
      { id: device.id },
      {
        onSuccess: () => {
          setNotice("revoked");
        },
        onError: (error) => {
          setFailure(reportable(error));
        },
      },
    );
  };

  const removeAll = () => {
    setFailure(null);
    setNotice(null);
    revokeAll.mutate(undefined, {
      onSuccess: () => {
        setNotice("allRevoked");
      },
      onError: (error) => {
        setFailure(reportable(error));
      },
    });
  };

  let list;
  if (devices.isPending) {
    list = <Skeleton active paragraph={{ rows: 3 }} />;
  } else if (devices.isError) {
    list = <ErrorAlert error={devices.error} />;
  } else if (ordered.length === 0) {
    list = (
      <Flex
        vertical
        gap={token.marginXXS}
        data-testid="trusted-devices-empty"
        style={{
          padding: token.paddingLG,
          textAlign: "center",
          border: `${String(token.lineWidth)}px dashed ${token.colorBorder}`,
          borderRadius: token.borderRadiusLG,
        }}
      >
        <Typography.Text strong>{t("trustedDevices.empty.title")}</Typography.Text>
        <Typography.Text type="secondary">{t("trustedDevices.empty.description")}</Typography.Text>
      </Flex>
    );
  } else {
    list = (
      <ul aria-label={t("trustedDevices.listLabel")} style={{ margin: 0, padding: 0 }}>
        {ordered.map((device) => (
          <DeviceRow
            key={device.id}
            device={device}
            revoking={revokeDevice.isPending && revokeDevice.variables.id === device.id}
            onRevoke={revoke}
          />
        ))}
      </ul>
    );
  }

  return (
    <SettingsSection
      title={t("trustedDevices.title")}
      description={t("trustedDevices.description")}
      testId="settings-trusted-devices"
      extra={
        <Popconfirm
          title={t("trustedDevices.revokeAll.title")}
          description={t("trustedDevices.revokeAll.description")}
          okText={t("trustedDevices.revokeAll.confirm")}
          cancelText={t("trustedDevices.cancel")}
          okButtonProps={{ danger: true }}
          onConfirm={removeAll}
          disabled={!hasAny}
        >
          <Button loading={revokeAll.isPending} disabled={!hasAny}>
            {t("trustedDevices.revokeAll.action")}
          </Button>
        </Popconfirm>
      }
    >
      <Flex vertical gap={token.margin}>
        {policy === undefined ? null : policy.enabled ? (
          <Typography.Text type="secondary" data-testid="trusted-devices-policy">
            {t("trustedDevices.policy.enabled", {
              duration: formatDays(policy.durationDays, i18n.language),
            })}
          </Typography.Text>
        ) : (
          <Alert
            type="info"
            showIcon
            data-testid="trusted-devices-policy"
            title={t("trustedDevices.policy.disabled")}
          />
        )}
        {failure === null ? null : <ErrorAlert error={failure} />}
        {notice === null ? null : (
          <Alert
            type="success"
            showIcon
            role="status"
            title={t(`trustedDevices.notice.${notice}`)}
          />
        )}
        {list}
        {devices.hasNextPage ? (
          <div>
            <Button
              loading={devices.isFetchingNextPage}
              onClick={() => {
                void devices.fetchNextPage();
              }}
            >
              {t("trustedDevices.loadMore")}
            </Button>
          </div>
        ) : null}
      </Flex>
    </SettingsSection>
  );
}
