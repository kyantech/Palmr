"use client";

import { useEffect, useState } from "react";
import { IconCheck, IconCopy } from "@tabler/icons-react";
import { useTranslations } from "next-intl";

import { Button } from "@/components/ui/button";
import { Card, CardContent } from "@/components/ui/card";
import { Label } from "@/components/ui/label";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";

interface EmbedCodeDisplayProps {
  imageUrl: string;
  fileName: string;
}

export function EmbedCodeDisplay({ imageUrl, fileName }: EmbedCodeDisplayProps) {
  const t = useTranslations();
  const [copiedType, setCopiedType] = useState<string | null>(null);
  const [fullUrl, setFullUrl] = useState<string>("");

  useEffect(() => {
    // Get the full URL with the current browser origin
    if (typeof window !== "undefined") {
      const origin = window.location.origin;
      const url = imageUrl.startsWith("http") ? imageUrl : `${origin}${imageUrl}`;
      setFullUrl(url);
    }
  }, [imageUrl]);

  const directLink = fullUrl || imageUrl;
  const htmlCode = `<img src="${directLink}" alt="${fileName}" />`;
  const bbCode = `[img]${directLink}[/img]`;

  const copyToClipboard = async (text: string, type: string) => {
    try {
      await navigator.clipboard.writeText(text);
      setCopiedType(type);
      setTimeout(() => setCopiedType(null), 2000);
    } catch (error) {
      console.error("Failed to copy:", error);
    }
  };

  return (
    <Card>
      <CardContent className="pt-6">
        <div className="space-y-4">
          <div>
            <Label className="text-sm font-semibold">{t("embedCode.title")}</Label>
            <p className="text-xs text-muted-foreground mt-1">{t("embedCode.description")}</p>
          </div>

          <Tabs defaultValue="direct" className="w-full">
            <TabsList className="grid w-full grid-cols-3">
              <TabsTrigger value="direct">{t("embedCode.tabs.directLink")}</TabsTrigger>
              <TabsTrigger value="html">{t("embedCode.tabs.html")}</TabsTrigger>
              <TabsTrigger value="bbcode">{t("embedCode.tabs.bbcode")}</TabsTrigger>
            </TabsList>

            <TabsContent value="direct" className="space-y-2">
              <div className="flex gap-2">
                <input
                  type="text"
                  readOnly
                  value={directLink}
                  className="flex-1 px-3 py-2 text-sm border rounded-md bg-muted/50 font-mono"
                />
                <Button
                  size="sm"
                  variant="outline"
                  onClick={() => copyToClipboard(directLink, "direct")}
                  className="shrink-0"
                >
                  {copiedType === "direct" ? (
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
              <p className="text-xs text-muted-foreground">{t("embedCode.directLinkDescription")}</p>
              <p className="text-xs text-amber-600 dark:text-amber-500">{t("embedCode.urlNote")}</p>
            </TabsContent>

            <TabsContent value="html" className="space-y-2">
              <div className="flex gap-2">
                <input
                  type="text"
                  readOnly
                  value={htmlCode}
                  className="flex-1 px-3 py-2 text-sm border rounded-md bg-muted/50 font-mono"
                />
                <Button
                  size="sm"
                  variant="outline"
                  onClick={() => copyToClipboard(htmlCode, "html")}
                  className="shrink-0"
                >
                  {copiedType === "html" ? (
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
              <p className="text-xs text-muted-foreground">{t("embedCode.htmlDescription")}</p>
            </TabsContent>

            <TabsContent value="bbcode" className="space-y-2">
              <div className="flex gap-2">
                <input
                  type="text"
                  readOnly
                  value={bbCode}
                  className="flex-1 px-3 py-2 text-sm border rounded-md bg-muted/50 font-mono"
                />
                <Button
                  size="sm"
                  variant="outline"
                  onClick={() => copyToClipboard(bbCode, "bbcode")}
                  className="shrink-0"
                >
                  {copiedType === "bbcode" ? (
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
              <p className="text-xs text-muted-foreground">{t("embedCode.bbcodeDescription")}</p>
            </TabsContent>
          </Tabs>
        </div>
      </CardContent>
    </Card>
  );
}
