import Key from "@gravity-ui/icons/Key";
import LogoGithub from "@gravity-ui/icons/LogoGithub";
import ShieldKeyhole from "@gravity-ui/icons/ShieldKeyhole";
import { Button, Flex, theme } from "antd";
import { useTranslation } from "react-i18next";
import type { components } from "../../../shared/api/schema";

export type LoginProvider = components["schemas"]["BootstrapProvider"];

export function ProviderIcon({ iconKey }: { iconKey: string }) {
  const common = { "aria-hidden": true, focusable: false, width: "1em", height: "1em" } as const;
  switch (iconKey) {
    case "github":
      return <LogoGithub {...common} />;
    case "generic":
      return <ShieldKeyhole {...common} />;
    default:
      return <Key {...common} />;
  }
}

interface ProviderButtonsProps {
  providers: readonly LoginProvider[];
  pendingSlug?: string | null;
  onSelect?: ((slug: string) => void) | undefined;
}

export function ProviderButtons({ providers, pendingSlug = null, onSelect }: ProviderButtonsProps) {
  const { t } = useTranslation("auth");
  const { token } = theme.useToken();
  const pending = pendingSlug !== null;
  return (
    <Flex vertical gap={token.marginSM} data-testid="login-providers">
      {providers.map((provider) => (
        <Button
          key={provider.slug}
          size="large"
          block
          icon={<ProviderIcon iconKey={provider.iconKey} />}
          data-provider={provider.slug}
          data-icon-key={provider.iconKey}
          loading={pendingSlug === provider.slug}
          disabled={onSelect === undefined || (pending && pendingSlug !== provider.slug)}
          onClick={() => {
            if (!pending) {
              onSelect?.(provider.slug);
            }
          }}
        >
          {t("login.continueWith", { provider: provider.displayName })}
        </Button>
      ))}
    </Flex>
  );
}
