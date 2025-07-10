"use client";

import { useState } from "react";
import { IconFlask, IconInfoCircle, IconLoader } from "@tabler/icons-react";
import { useTranslations } from "next-intl";
import { toast } from "sonner";

import { Button } from "@/components/ui/button";
import { testSmtpConnection } from "@/http/endpoints/app";

interface SmtpTestButtonProps {
  smtpEnabled: string;
  getFormValues: () => {
    smtpEnabled: string;
    smtpHost: string;
    smtpPort: string;
    smtpUser: string;
    smtpPass: string;
    smtpSecure: string;
    smtpNoAuth: string;
    smtpTrustSelfSigned: string;
  };
}

export function SmtpTestButton({ smtpEnabled, getFormValues }: SmtpTestButtonProps) {
  const [isLoading, setIsLoading] = useState(false);
  const t = useTranslations();

  const handleTestConnection = async () => {
    const formValues = getFormValues();

    if (formValues.smtpEnabled !== "true") {
      toast.error(t("settings.messages.smtpNotEnabled"));
      return;
    }

    if (!formValues.smtpHost || !formValues.smtpPort) {
      toast.error(t("settings.messages.smtpMissingHostPort"));
      return;
    }

    if (formValues.smtpNoAuth !== "true" && (!formValues.smtpUser || !formValues.smtpPass)) {
      toast.error(t("settings.messages.smtpMissingAuth"));
      return;
    }

    setIsLoading(true);
    try {
      const response = await testSmtpConnection({
        smtpConfig: {
          smtpEnabled: formValues.smtpEnabled,
          smtpHost: formValues.smtpHost,
          smtpPort: formValues.smtpPort,
          smtpUser: formValues.smtpUser,
          smtpPass: formValues.smtpPass,
          smtpSecure: formValues.smtpSecure,
          smtpNoAuth: formValues.smtpNoAuth,
          smtpTrustSelfSigned: formValues.smtpTrustSelfSigned,
        },
      });

      if (response.data.success) {
        toast.success(t("settings.messages.smtpTestSuccess"));
      } else {
        toast.error(t("settings.messages.smtpTestGenericError"));
      }
    } catch (error: any) {
      const errorMessage = error?.response?.data?.error || error?.message || t("common.unexpectedError");
      toast.error(t("settings.messages.smtpTestFailed", { error: errorMessage }));
    } finally {
      setIsLoading(false);
    }
  };

  return (
    <div className="flex items-center gap-2">
      <Button
        type="button"
        variant="outline"
        onClick={handleTestConnection}
        disabled={isLoading || smtpEnabled !== "true"}
        className="flex items-center gap-2"
      >
        {isLoading ? <IconLoader className="h-4 w-4 animate-spin" /> : <IconFlask className="h-4 w-4" />}
        {isLoading ? t("settings.buttons.testing") : t("settings.buttons.testSmtp")}
      </Button>
      <div className="relative group">
        <IconInfoCircle className="h-4 w-4 text-muted-foreground hover:text-foreground cursor-help transition-colors" />
        <div className="absolute left-1/2 -translate-x-1/2 bottom-full mb-2 px-3 py-2 bg-popover text-popover-foreground text-xs rounded-md border shadow-md opacity-0 group-hover:opacity-100 transition-opacity pointer-events-none z-50 w-72 text-wrap">
          {t("settings.tooltips.testSmtp")}
          <div className="absolute top-full left-1/2 -translate-x-1/2 border-4 border-transparent border-t-popover"></div>
        </div>
      </div>
    </div>
  );
}
