import { forwardRef } from "react";
import { useTranslations } from "next-intl";

import { Input } from "@/components/ui/input";

interface RedirectUriInputProps {
  value: string;
  onChange: (value: string) => void;
  disabled?: boolean;
  error?: any;
  placeholder?: string;
}

const CALLBACK_PATH = "/api/auth/providers/callback";

export const RedirectUriInput = forwardRef<HTMLInputElement, RedirectUriInputProps>(
  ({ value, onChange, disabled, error, placeholder }, ref) => {
    const t = useTranslations();

    const getBaseUrl = (fullUrl: string) => {
      if (!fullUrl) return "";
      return fullUrl.replace(CALLBACK_PATH, "");
    };

    const buildFullUrl = (baseUrl: string) => {
      if (!baseUrl) return "";
      return `${baseUrl}${CALLBACK_PATH}`;
    };

    const baseUrl = getBaseUrl(value || "");

    const handleInputChange = (e: React.ChangeEvent<HTMLInputElement>) => {
      const newBaseUrl = e.target.value;
      const fullUrl = buildFullUrl(newBaseUrl);
      onChange(fullUrl);
    };

    return (
      <div className="space-y-2">
        <div className="relative">
          <Input
            ref={ref}
            value={baseUrl}
            onChange={handleInputChange}
            placeholder={placeholder || t("settings.redirectUri.placeholder")}
            disabled={disabled}
            aria-invalid={!!error}
            className="pr-32"
          />
          <div className="absolute inset-y-0 right-0 flex items-center pr-3">
            <span className="text-xs text-muted-foreground font-mono bg-muted px-2 py-1 rounded border">
              {CALLBACK_PATH}
            </span>
          </div>
        </div>

        {baseUrl && (
          <div className="text-xs text-muted-foreground bg-muted/30 border border-muted rounded-md p-3">
            <div className="font-medium mb-1 text-foreground">{t("settings.redirectUri.previewLabel")}</div>
            <code className="text-foreground break-all font-mono">{buildFullUrl(baseUrl)}</code>
          </div>
        )}
      </div>
    );
  }
);

RedirectUriInput.displayName = "RedirectUriInput";
