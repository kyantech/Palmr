"use client";

import React, { useState } from "react";
import { IconCheck, IconCopy } from "@tabler/icons-react";
import { useTranslations } from "next-intl";
import { toast } from "sonner";

import { Button } from "@/components/ui/button";
import { Label } from "@/components/ui/label";

interface CallbackUrlDisplayProps {
  providerName: string;
}

export function CallbackUrlDisplay({ providerName }: CallbackUrlDisplayProps) {
  const t = useTranslations();
  const [copied, setCopied] = useState(false);

  const callbackUrl =
    typeof window !== "undefined"
      ? `${window.location.origin}/api/auth/providers/${providerName}/callback`
      : `/api/auth/providers/${providerName}/callback`;

  const copyToClipboard = async () => {
    try {
      await navigator.clipboard.writeText(callbackUrl);
      setCopied(true);
      toast.success(t("authProviders.form.callbackUrlCopied"));
      setTimeout(() => setCopied(false), 2000);
    } catch (err) {
      console.error("Failed to copy text: ", err);
    }
  };

  return (
    <div className="space-y-3">
      <div>
        <Label className="text-sm font-medium text-foreground">{t("authProviders.form.callbackUrl")}</Label>
        <div className="flex items-center gap-2 mb-2 border py-2 px-3 mt-2 rounded-md w-fit ">
          <div className=" rounded-md font-mono text-sm break-all font-semibold px-2">{callbackUrl}</div>
          <Button
            variant="ghost"
            size="icon"
            onClick={copyToClipboard}
            className="shrink-0"
            title={t("authProviders.form.copyCallbackUrl")}
          >
            {copied ? <IconCheck className="h-3 w-3" /> : <IconCopy className="h-3 w-3" />}
          </Button>
        </div>
        <p className="text-xs text-muted-foreground mb-2">{t("authProviders.form.callbackUrlDescription")}</p>
      </div>
    </div>
  );
}
