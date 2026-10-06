import CircleCheck from "@gravity-ui/icons/CircleCheck";
import CircleXmark from "@gravity-ui/icons/CircleXmark";
import { Flex, theme, Typography } from "antd";
import { useTranslation } from "react-i18next";

export interface CheckLine {
  readonly name: string;
  readonly ok: boolean;
  readonly detail?: string | null | undefined;
}

const KNOWN_CHECKS: readonly string[] = [
  "discovery",
  "jwks",
  "token_endpoint",
  "authorization_endpoint",
  "userinfo_endpoint",
  "redirect_uri",
];

interface ProviderChecksProps {
  checks: readonly CheckLine[];
  label: string;
  testId?: string;
}

export function ProviderChecks({ checks, label, testId = "provider-checks" }: ProviderChecksProps) {
  const { t } = useTranslation("admin");
  const { token } = theme.useToken();
  return (
    <ul
      aria-label={label}
      data-testid={testId}
      style={{ margin: 0, padding: 0, listStyle: "none", display: "grid", gap: token.marginXXS }}
    >
      {checks.map((check) => {
        const Icon = check.ok ? CircleCheck : CircleXmark;
        return (
          <li key={check.name} data-check={check.name} data-ok={check.ok ? "true" : "false"}>
            <Flex gap={token.marginXS} align="center" wrap>
              <Icon
                aria-hidden="true"
                focusable="false"
                width="1em"
                height="1em"
                style={{ color: check.ok ? token.colorSuccess : token.colorError }}
              />
              <Typography.Text>
                {KNOWN_CHECKS.includes(check.name)
                  ? t(`providers.checks.names.${check.name}`)
                  : check.name}
              </Typography.Text>
              <Typography.Text type={check.ok ? "success" : "danger"}>
                {t(check.ok ? "providers.checks.passed" : "providers.checks.failed")}
              </Typography.Text>
              {check.detail === undefined || check.detail === null ? null : (
                <Typography.Text type="secondary" code>
                  {check.detail}
                </Typography.Text>
              )}
            </Flex>
          </li>
        );
      })}
    </ul>
  );
}
