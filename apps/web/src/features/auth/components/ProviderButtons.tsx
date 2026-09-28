import { Button, Flex, theme } from "antd";
import { useTranslation } from "react-i18next";
import type { components } from "../../../shared/api/schema";

export type LoginProvider = components["schemas"]["BootstrapProvider"];

interface ProviderButtonsProps {
  providers: readonly LoginProvider[];
  onSelect?: ((slug: string) => void) | undefined;
}

export function sortProviders(providers: readonly LoginProvider[]): LoginProvider[] {
  return [...providers].sort(
    (a, b) => a.sortOrder - b.sortOrder || a.displayName.localeCompare(b.displayName),
  );
}

export function ProviderButtons({ providers, onSelect }: ProviderButtonsProps) {
  const { t } = useTranslation("auth");
  const { token } = theme.useToken();
  return (
    <Flex vertical gap={token.marginSM} data-testid="login-providers">
      {sortProviders(providers).map((provider) => (
        <Button
          key={provider.slug}
          size="large"
          block
          data-provider={provider.slug}
          data-icon-key={provider.iconKey}
          disabled={onSelect === undefined}
          onClick={() => onSelect?.(provider.slug)}
        >
          {t("login.continueWith", { provider: provider.displayName })}
        </Button>
      ))}
    </Flex>
  );
}
