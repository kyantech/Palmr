"use client";

import { useEffect, useState } from "react";
import { IconCheck, IconCopy } from "@tabler/icons-react";
import { useTranslations } from "next-intl";

import { Button } from "@/components/ui/button";
import { Card, CardContent } from "@/components/ui/card";
import { Label } from "@/components/ui/label";

interface MediaEmbedLinkProps {
  fileId: string;
}

export function MediaEmbedLink({ fileId }: MediaEmbedLinkProps) {
  const t = useTranslations();
  const [copied, setCopied] = useState(false);
  const [embedUrl, setEmbedUrl] = useState<string>("");

  useEffect(() => {
    if (typeof window !== "undefined") {
      const origin = window.location.origin;
      const url = `${origin}/e/${fileId}`;
      setEmbedUrl(url);
    }
  }, [fileId]);

  const copyToClipboard = async () => {
    try {
      await navigator.clipboard.writeText(embedUrl);
      setCopied(true);
      setTimeout(() => setCopied(false), 2000);
    } catch (error) {
      console.error("Failed to copy:", error);
    }
  };

  return (
    <Card>
      <CardContent>
        <div className="space-y-3">
          <div>
            <Label className="text-sm font-semibold">{t("embedCode.title")}</Label>
            <p className="text-xs text-muted-foreground mt-1">{t("embedCode.directLinkDescription")}</p>
          </div>

          <div className="flex gap-2">
            <input
              type="text"
              readOnly
              value={embedUrl}
              className="flex-1 px-3 py-2 text-sm border rounded-md bg-muted/50 font-mono"
            />
            <Button size="default" variant="outline" onClick={copyToClipboard} className="shrink-0 h-full">
              {copied ? (
                <>
                  <IconCheck className="h-4 w-4 mr-1" />
                  {t("common.copied")}
                </>
              ) : (
                <>
                  <IconCopy className="h-4 w-4 mr-1" />
                  {t("common.copy")}
                </>
              )}
            </Button>
          </div>
        </div>
      </CardContent>
    </Card>
  );
}
