"use client";

import { useEffect, useState } from "react";
import { IconBell, IconCheck, IconMail, IconPlus, IconTrash, IconUsers, IconX } from "@tabler/icons-react";
import { useTranslations } from "next-intl";
import { toast } from "sonner";

import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { Input } from "@/components/ui/input";
import { Separator } from "@/components/ui/separator";
import { useShareContext } from "@/contexts/share-context";
import { addRecipients, notifyRecipients, removeRecipients } from "@/http/endpoints";

interface Recipient {
  id: string;
  email: string;
  createdAt: string;
  updatedAt: string;
}

interface RecipientSelectorProps {
  shareId: string;
  selectedRecipients: Recipient[];
  shareAlias?: string;
  onSuccess: () => void;
}

export function RecipientSelector({ shareId, selectedRecipients, shareAlias, onSuccess }: RecipientSelectorProps) {
  const t = useTranslations();
  const { smtpEnabled } = useShareContext();
  const [recipients, setRecipients] = useState<string[]>(selectedRecipients?.map((recipient) => recipient.email) || []);
  const [newRecipient, setNewRecipient] = useState("");
  const [selectedForAction, setSelectedForAction] = useState<Set<string>>(new Set());
  const [isAddingRecipient, setIsAddingRecipient] = useState(false);

  useEffect(() => {
    setRecipients(selectedRecipients?.map((recipient) => recipient.email) || []);
    setSelectedForAction(new Set());
  }, [selectedRecipients]);

  const isValidEmail = (email: string) => {
    const emailRegex = /^[^\s@]+@[^\s@]+\.[^\s@]+$/;
    return emailRegex.test(email);
  };

  const handleAddRecipient = async () => {
    if (!newRecipient.trim()) return;

    if (!isValidEmail(newRecipient)) {
      toast.error(t("recipientSelector.invalidEmail"));
      return;
    }

    if (recipients.includes(newRecipient)) {
      toast.error(t("recipientSelector.duplicateEmail"));
      return;
    }

    setIsAddingRecipient(true);
    try {
      await addRecipients(shareId, { emails: [newRecipient] });
      setRecipients([...recipients, newRecipient]);
      setNewRecipient("");
      toast.success(t("recipientSelector.addSuccess"));
      onSuccess();
    } catch {
      toast.error(t("recipientSelector.addError"));
    } finally {
      setIsAddingRecipient(false);
    }
  };

  const handleRemoveRecipient = async (email: string) => {
    try {
      await removeRecipients(shareId, { emails: [email] });
      setRecipients(recipients.filter((r) => r !== email));
      setSelectedForAction((prev) => {
        const newSet = new Set(prev);
        newSet.delete(email);
        return newSet;
      });
      toast.success(t("recipientSelector.removeSuccess"));
      onSuccess();
    } catch {
      toast.error(t("recipientSelector.removeError"));
    }
  };

  const handleRemoveSelected = async () => {
    const emailsToRemove = Array.from(selectedForAction);
    try {
      await removeRecipients(shareId, { emails: emailsToRemove });
      setRecipients(recipients.filter((r) => !selectedForAction.has(r)));
      setSelectedForAction(new Set());
      toast.success(t("recipientSelector.bulkRemoveSuccess", { count: emailsToRemove.length }));
      onSuccess();
    } catch {
      toast.error(t("recipientSelector.bulkRemoveError"));
    }
  };

  const handleNotifySelected = async () => {
    if (!shareAlias) return;

    const emailsToNotify = Array.from(selectedForAction);
    const link = `${window.location.origin}/s/${shareAlias}`;
    const loadingToast = toast.loading(t("recipientSelector.sendingNotifications"));

    try {
      await notifyRecipients(shareId, { shareLink: link });
      toast.dismiss(loadingToast);
      toast.success(t("recipientSelector.bulkNotifySuccess", { count: emailsToNotify.length }));
      setSelectedForAction(new Set());
    } catch {
      toast.dismiss(loadingToast);
      toast.error(t("recipientSelector.bulkNotifyError"));
    }
  };

  const handleNotifyAll = async () => {
    if (!shareAlias) return;

    const link = `${window.location.origin}/s/${shareAlias}`;
    const loadingToast = toast.loading(t("recipientSelector.sendingNotifications"));

    try {
      await notifyRecipients(shareId, { shareLink: link });
      toast.dismiss(loadingToast);
      toast.success(t("recipientSelector.notifySuccess"));
    } catch {
      toast.dismiss(loadingToast);
      toast.error(t("recipientSelector.notifyError"));
    }
  };

  const handleSelectAll = (checked: boolean) => {
    if (checked) {
      setSelectedForAction(new Set(recipients));
    } else {
      setSelectedForAction(new Set());
    }
  };

  const handleSelectRecipient = (email: string, checked: boolean) => {
    const newSelected = new Set(selectedForAction);
    if (checked) {
      newSelected.add(email);
    } else {
      newSelected.delete(email);
    }
    setSelectedForAction(newSelected);
  };

  const isAllSelected = recipients.length > 0 && selectedForAction.size === recipients.length;
  const hasSelection = selectedForAction.size > 0;

  return (
    <div className="space-y-6">
      <div className="space-y-4">
        <div className="flex items-center gap-2">
          <IconPlus className="h-5 w-5 text-primary" />
          <h3 className="text-lg font-medium">{t("recipientSelector.addRecipient")}</h3>
        </div>
        <div className="flex flex-col sm:flex-row gap-3">
          <div className="relative flex-1">
            <IconMail className="absolute left-3 top-1/2 -translate-y-1/2 text-muted-foreground h-4 w-4" />
            <Input
              className="pl-9 h-10"
              placeholder={t("recipientSelector.emailPlaceholder")}
              value={newRecipient}
              onChange={(e) => setNewRecipient(e.target.value)}
              onKeyDown={(e) => e.key === "Enter" && !isAddingRecipient && handleAddRecipient()}
              disabled={isAddingRecipient}
            />
          </div>
          <Button
            onClick={handleAddRecipient}
            disabled={!newRecipient.trim() || isAddingRecipient}
            className="h-10 px-6 sm:w-auto w-full"
          >
            {isAddingRecipient ? (
              <div className="flex items-center gap-2">
                <div className="h-4 w-4 animate-spin rounded-full border-2 border-background border-t-transparent" />
                {t("common.loading")}
              </div>
            ) : (
              <>
                <IconPlus className="h-4 w-4" />
                {t("recipientSelector.add")}
              </>
            )}
          </Button>
        </div>
      </div>

      <Separator />

      <div className="space-y-4">
        <div className="flex flex-col sm:flex-row sm:items-center sm:justify-between gap-3">
          <div className="flex items-center gap-2">
            <IconUsers className="h-5 w-5 text-primary" />
            <h3 className="text-lg font-medium">{t("recipientSelector.recipients", { count: recipients.length })}</h3>
          </div>

          {recipients.length > 0 && shareAlias && smtpEnabled === "true" && (
            <Button variant="outline" size="sm" onClick={handleNotifyAll} className="sm:w-auto w-full">
              <IconBell className="h-4 w-4" />
              {t("recipientSelector.notifyAll")}
            </Button>
          )}
        </div>

        {hasSelection && (
          <div className="flex flex-col sm:flex-row sm:items-center sm:justify-between gap-3 p-3 bg-blue-50 dark:bg-blue-950/30 border border-blue-200 dark:border-blue-800 rounded-lg">
            <div className="flex items-center gap-2">
              <IconCheck className="h-4 w-4 text-blue-600" />
              <span className="text-sm font-medium text-blue-900 dark:text-blue-100">
                {t("recipientSelector.selectedCount", { count: selectedForAction.size })}
              </span>
            </div>
            <div className="flex flex-col sm:flex-row items-stretch sm:items-center gap-2">
              {smtpEnabled === "true" && shareAlias && (
                <Button variant="outline" size="sm" onClick={handleNotifySelected} className="sm:w-auto w-full">
                  <IconBell className="h-4 w-4" />
                  {t("recipientSelector.notifySelected")}
                </Button>
              )}
              <Button variant="destructive" size="sm" onClick={handleRemoveSelected} className="sm:w-auto w-full">
                <IconTrash className="h-4 w-4" />
                {t("recipientSelector.removeSelected")}
              </Button>
              <Button
                variant="ghost"
                size="sm"
                onClick={() => setSelectedForAction(new Set())}
                className="h-8 w-8 p-0 self-center"
                title={t("common.cancel")}
              >
                <IconX className="h-4 w-4" />
              </Button>
            </div>
          </div>
        )}

        <div className="border rounded-lg overflow-hidden">
          {recipients.length === 0 ? (
            <div className="text-center py-12 px-6">
              <div className="mx-auto w-16 h-16 bg-muted rounded-full flex items-center justify-center mb-4">
                <IconUsers className="h-8 w-8 text-muted-foreground" />
              </div>
              <h4 className="text-lg font-medium mb-2">{t("recipientSelector.noRecipients")}</h4>
              <p className="text-sm text-muted-foreground mb-4">{t("recipientSelector.noRecipientsDescription")}</p>
            </div>
          ) : (
            <>
              <div className="flex items-center gap-3 p-4 border-b bg-muted/30">
                <Checkbox
                  checked={isAllSelected}
                  onCheckedChange={handleSelectAll}
                  aria-label={t("recipientSelector.selectAll")}
                />
                <span className="text-sm font-medium text-muted-foreground">{t("recipientSelector.selectAll")}</span>
              </div>

              <div className="divide-y max-h-80 overflow-y-auto">
                {recipients.map((email, index) => {
                  const isSelected = selectedForAction.has(email);
                  return (
                    <div
                      key={index}
                      className={`flex items-center gap-3 p-4 hover:bg-muted/50 transition-colors ${
                        isSelected ? "bg-blue-50 dark:bg-blue-950/30" : ""
                      }`}
                    >
                      <Checkbox
                        checked={isSelected}
                        onCheckedChange={(checked) => handleSelectRecipient(email, checked as boolean)}
                        aria-label={t("recipientSelector.selectRecipient", { email })}
                      />

                      <div className="flex items-center gap-3 flex-1 min-w-0">
                        <div className="w-8 h-8 bg-primary/10 rounded-full flex items-center justify-center flex-shrink-0">
                          <IconMail className="h-4 w-4 text-primary" />
                        </div>
                        <span className="truncate font-medium">{email}</span>
                      </div>

                      <div className="flex items-center gap-1">
                        {smtpEnabled === "true" && shareAlias && (
                          <Button
                            variant="ghost"
                            size="sm"
                            className="h-8 w-8 p-0 text-blue-600 hover:text-blue-700 hover:bg-blue-100 dark:hover:bg-blue-900/30"
                            onClick={async () => {
                              const link = `${window.location.origin}/s/${shareAlias}`;
                              const loadingToast = toast.loading(t("recipientSelector.sendingNotifications"));

                              try {
                                await notifyRecipients(shareId, { shareLink: link });
                                toast.dismiss(loadingToast);
                                toast.success(t("recipientSelector.singleNotifySuccess", { email }));
                              } catch {
                                toast.dismiss(loadingToast);
                                toast.error(t("recipientSelector.singleNotifyError"));
                              }
                            }}
                            title={t("recipientSelector.notifySingle")}
                          >
                            <IconBell className="h-4 w-4" />
                          </Button>
                        )}

                        <Button
                          variant="ghost"
                          size="sm"
                          className="h-8 w-8 p-0 text-destructive hover:text-destructive hover:bg-destructive/10"
                          onClick={() => handleRemoveRecipient(email)}
                          title={t("recipientSelector.removeSingle")}
                        >
                          <IconTrash className="h-4 w-4" />
                        </Button>
                      </div>
                    </div>
                  );
                })}
              </div>
            </>
          )}
        </div>
      </div>
    </div>
  );
}
