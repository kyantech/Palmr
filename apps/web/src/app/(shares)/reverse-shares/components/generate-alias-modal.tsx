"use client";

import React, { useEffect, useState } from "react";
import { IconCopy, IconDice, IconLink } from "@tabler/icons-react";
import { useTranslations } from "next-intl";
import { useForm } from "react-hook-form";

import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Form, FormControl, FormField, FormItem, FormLabel, FormMessage } from "@/components/ui/form";
import { Input } from "@/components/ui/input";
import { customNanoid } from "@/lib/utils";
import { ReverseShare } from "../hooks/use-reverse-shares";

interface GenerateAliasFormData {
  alias: string;
}

interface GenerateAliasModalProps {
  reverseShare: ReverseShare | null;
  isOpen: boolean;
  onClose: () => void;
  onCreateAlias: (reverseShareId: string, alias: string) => Promise<void>;
  onCopyLink: (reverseShare: ReverseShare) => void;
}

const generateDefaultAlias = () => customNanoid(10, "0123456789abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ");

export function GenerateAliasModal({
  reverseShare,
  isOpen,
  onClose,
  onCreateAlias,
  onCopyLink,
}: GenerateAliasModalProps) {
  const t = useTranslations();
  const [isSubmitting, setIsSubmitting] = useState(false);
  const [origin, setOrigin] = useState("");

  useEffect(() => {
    setOrigin(window.location.origin);
  }, []);

  const form = useForm<GenerateAliasFormData>({
    defaultValues: {
      alias: "",
    },
  });

  React.useEffect(() => {
    if (reverseShare) {
      if (reverseShare.alias?.alias) {
        form.setValue("alias", reverseShare.alias.alias);
      } else {
        form.setValue("alias", generateDefaultAlias());
      }
    }
  }, [reverseShare, form]);

  const onSubmit = async (data: GenerateAliasFormData) => {
    if (!reverseShare) return;

    setIsSubmitting(true);
    try {
      await onCreateAlias(reverseShare.id, data.alias);
      onClose();
    } catch {
    } finally {
      setIsSubmitting(false);
    }
  };

  const handleCopyLink = () => {
    if (reverseShare) {
      onCopyLink(reverseShare);
    }
  };

  const hasExistingAlias = reverseShare?.alias?.alias;
  const currentLink = hasExistingAlias ? `${origin}/r/${hasExistingAlias}` : null;

  return (
    <Dialog open={isOpen} onOpenChange={onClose}>
      <DialogContent className="sm:max-w-lg max-w-[95vw] overflow-hidden">
        <DialogHeader>
          <DialogTitle className="flex items-center gap-2">
            <IconLink size={20} />
            {hasExistingAlias ? t("reverseShares.modals.alias.editTitle") : t("reverseShares.modals.alias.createTitle")}
          </DialogTitle>
          <DialogDescription>
            {hasExistingAlias
              ? t("reverseShares.modals.alias.editDescription")
              : t("reverseShares.modals.alias.createDescription")}
          </DialogDescription>
        </DialogHeader>

        <Form {...form}>
          <form onSubmit={form.handleSubmit(onSubmit)} className="space-y-4">
            <FormField
              control={form.control}
              name="alias"
              rules={{
                required: t("reverseShares.modals.alias.validation.required"),
                minLength: {
                  value: 3,
                  message: t("reverseShares.modals.alias.validation.minLength"),
                },
                maxLength: {
                  value: 50,
                  message: t("reverseShares.modals.alias.validation.maxLength"),
                },
                pattern: {
                  value: /^[a-zA-Z0-9-_]+$/,
                  message: t("reverseShares.modals.alias.validation.pattern"),
                },
              }}
              render={({ field }) => (
                <FormItem>
                  <div className="flex items-center justify-between">
                    <FormLabel>{t("reverseShares.modals.alias.aliasLabel")}</FormLabel>
                    <Button
                      type="button"
                      variant="ghost"
                      size="sm"
                      className="h-6 px-2 text-xs text-muted-foreground hover:text-foreground"
                      onClick={() => {
                        const randomAlias = generateDefaultAlias();
                        field.onChange(randomAlias);
                      }}
                      title={t("reverseShares.modals.alias.randomTooltip")}
                    >
                      <IconDice className="h-4 w-4" />
                    </Button>
                  </div>
                  <FormControl>
                    <Input
                      placeholder={t("reverseShares.modals.alias.aliasPlaceholder")}
                      maxLength={50}
                      className="max-w-full"
                      {...field}
                      onChange={(e) => {
                        const value = e.target.value
                          .replace(/\s+/g, "-")
                          .replace(/[^a-zA-Z0-9-_]/g, "")
                          .toLowerCase();
                        field.onChange(value);
                      }}
                    />
                  </FormControl>

                  {field.value && field.value.length >= 3 && (
                    <div className="mt-2 p-2 bg-primary/5 border border-primary/20 rounded-md overflow-hidden">
                      <label className="text-xs text-muted-foreground block mb-1">
                        {t("reverseShares.modals.alias.preview")}
                      </label>
                      <code className="block text-xs font-mono text-primary bg-primary/10 px-2 py-1 rounded w-full truncate">
                        {origin}/r/{field.value}
                      </code>
                    </div>
                  )}

                  <p className="text-xs text-muted-foreground">{t("reverseShares.modals.alias.help")}</p>
                  <FormMessage />
                </FormItem>
              )}
            />

            {currentLink && (
              <div className="space-y-2">
                <label className="text-sm font-medium">{t("reverseShares.modals.alias.currentLink")}</label>
                <div className="flex items-center gap-2 p-3 bg-muted rounded-md min-w-0 max-w-full overflow-hidden">
                  <div className="flex-1 min-w-0 max-w-full overflow-hidden">
                    <code className="block text-sm font-mono bg-background px-2 py-1 rounded border w-full truncate">
                      {currentLink}
                    </code>
                  </div>
                  <Button
                    type="button"
                    variant="outline"
                    size="sm"
                    onClick={handleCopyLink}
                    className="shrink-0"
                    title={t("reverseShares.modals.alias.copyCurrentLink")}
                  >
                    <IconCopy className="h-4 w-4" />
                  </Button>
                </div>
              </div>
            )}

            <DialogFooter className="gap-2">
              <Button type="button" variant="outline" onClick={onClose} disabled={isSubmitting}>
                {t("reverseShares.modals.alias.cancel")}
              </Button>
              <Button
                type="submit"
                disabled={isSubmitting || !form.formState.isValid || form.watch("alias").length < 3}
              >
                {isSubmitting ? (
                  <div className="flex items-center gap-2">
                    <div className="animate-spin">⠋</div>
                    {hasExistingAlias
                      ? t("reverseShares.modals.alias.updating")
                      : t("reverseShares.modals.alias.creating")}
                  </div>
                ) : hasExistingAlias ? (
                  t("reverseShares.modals.alias.update")
                ) : (
                  t("reverseShares.modals.alias.create")
                )}
              </Button>
            </DialogFooter>
          </form>
        </Form>
      </DialogContent>
    </Dialog>
  );
}
