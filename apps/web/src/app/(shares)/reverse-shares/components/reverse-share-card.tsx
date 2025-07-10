import { useEffect, useRef, useState } from "react";
import {
  IconCheck,
  IconCopy,
  IconDotsVertical,
  IconEdit,
  IconExternalLink,
  IconEye,
  IconFile,
  IconFileUnknown,
  IconLink,
  IconLock,
  IconLockOpen,
  IconToggleLeft,
  IconToggleRight,
  IconTrash,
  IconX,
} from "@tabler/icons-react";
import { useTranslations } from "next-intl";

import { Button } from "@/components/ui/button";
import { Card, CardContent } from "@/components/ui/card";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { Input } from "@/components/ui/input";
import { ReverseShare } from "../hooks/use-reverse-shares";
import { EditPasswordModal } from "./edit-password-modal";

interface ReverseShareCardProps {
  reverseShare: ReverseShare;
  onCopyLink: (reverseShare: ReverseShare) => void;
  onDelete: (reverseShare: ReverseShare) => void;
  onEdit: (reverseShare: ReverseShare) => void;
  onGenerateLink: (reverseShare: ReverseShare) => void;
  onViewDetails: (reverseShare: ReverseShare) => void;
  onViewFiles: (reverseShare: ReverseShare) => void;
  onUpdateReverseShare?: (id: string, data: any) => Promise<any>;
  onToggleActive?: (id: string, isActive: boolean) => Promise<any>;
  onUpdatePassword?: (id: string, data: { hasPassword: boolean; password?: string }) => Promise<any>;
}

export function ReverseShareCard({
  reverseShare,
  onCopyLink,
  onDelete,
  onEdit,
  onGenerateLink,
  onViewDetails,
  onViewFiles,
  onUpdateReverseShare,
  onToggleActive,
  onUpdatePassword,
}: ReverseShareCardProps) {
  const t = useTranslations();
  const [origin, setOrigin] = useState("");
  const [editingField, setEditingField] = useState<{ field: string } | null>(null);
  const [editValue, setEditValue] = useState("");
  const [pendingChanges, setPendingChanges] = useState<Record<string, any>>({});
  const [showPasswordModal, setShowPasswordModal] = useState(false);
  const inputRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    setOrigin(window.location.origin);
  }, []);

  useEffect(() => {
    if (editingField && inputRef.current) {
      inputRef.current.focus();
      inputRef.current.select();
    }
  }, [editingField]);

  const fileCount = reverseShare.files?.length || 0;
  const hasAlias = Boolean(reverseShare.alias?.alias);
  const isExpired = reverseShare.expiration ? new Date(reverseShare.expiration) < new Date() : false;
  const hasPassword = reverseShare.hasPassword;

  const formatFileSize = (sizeInBytes: number) => {
    if (sizeInBytes === 0) return "0 B";
    const units = ["B", "KB", "MB", "GB"];
    const k = 1024;
    const i = Math.floor(Math.log(sizeInBytes) / Math.log(k));
    return `${parseFloat((sizeInBytes / Math.pow(k, i)).toFixed(1))} ${units[i]}`;
  };

  const totalSize = reverseShare.files?.reduce((acc, file) => acc + parseInt(file.size), 0) || 0;

  const startEdit = (field: string, currentValue: any) => {
    setEditingField({ field });
    setEditValue(currentValue?.toString() || "");
  };

  const saveEdit = async () => {
    if (!editingField || !onUpdateReverseShare) return;

    const { field } = editingField;
    let processedValue: string | number | null | boolean = editValue;

    if (field === "isActive") {
      processedValue = editValue === "true";
    }

    setPendingChanges((prev) => ({
      ...prev,
      [field]: processedValue,
    }));

    try {
      await onUpdateReverseShare(reverseShare.id, { [field]: processedValue });
    } catch (error) {
      console.error("Failed to update:", error);
      setPendingChanges((prev) => {
        const newState = { ...prev };
        delete newState[field];
        return newState;
      });
    }

    setEditingField(null);
    setEditValue("");
  };

  const cancelEdit = () => {
    setEditingField(null);
    setEditValue("");
  };

  const handleKeyDown = (e: React.KeyboardEvent) => {
    if (e.key === "Enter") {
      saveEdit();
    } else if (e.key === "Escape") {
      cancelEdit();
    }
  };

  const getDisplayValue = (field: string) => {
    const pendingChange = pendingChanges[field];
    if (pendingChange !== undefined) {
      return pendingChange;
    }
    return (reverseShare as any)?.[field];
  };

  const handleToggleActive = async () => {
    if (onToggleActive) {
      await onToggleActive(reverseShare.id, !reverseShare.isActive);
    }
  };

  const handleTogglePassword = async () => {
    if (onUpdatePassword) {
      setShowPasswordModal(true);
    }
  };

  const handleOpenInNewTab = () => {
    if (hasAlias && origin) {
      const url = `${origin}/r/${reverseShare.alias?.alias}`;
      window.open(url, "_blank", "noopener,noreferrer");
    }
  };

  return (
    <>
      <Card className="group relative overflow-hidden hover:shadow-lg transition-all duration-300 bg-background/50 dark:bg-foreground/5 border border-border/50 py-1">
        <CardContent className="p-4 space-y-3">
          {/* Header: Nome, Status e Ações */}
          <div className="flex items-start justify-between gap-1">
            <div className="flex-1 min-w-0">
              {editingField?.field === "name" ? (
                <div className="flex items-center gap-2">
                  <Input
                    ref={inputRef}
                    value={editValue}
                    onChange={(e) => setEditValue(e.target.value)}
                    onKeyDown={handleKeyDown}
                    className="h-8 text-lg font-bold border-primary/50 focus:border-primary"
                  />
                  <Button
                    size="icon"
                    variant="ghost"
                    className="h-6 w-6 text-green-600 hover:text-green-700"
                    onClick={saveEdit}
                  >
                    <IconCheck className="h-3 w-3" />
                  </Button>
                  <Button
                    size="icon"
                    variant="ghost"
                    className="h-6 w-6 text-red-600 hover:text-red-700"
                    onClick={cancelEdit}
                  >
                    <IconX className="h-3 w-3" />
                  </Button>
                </div>
              ) : (
                <div className="flex items-center gap-2 group/title">
                  {/* Bolinha de status */}
                  <div
                    className={`w-2 h-2 rounded-full shrink-0 shadow-sm ${
                      isExpired ? "bg-red-500" : reverseShare.isActive ? "bg-green-500" : "bg-red-500"
                    }`}
                    title={
                      isExpired
                        ? t("reverseShares.status.expired")
                        : reverseShare.isActive
                          ? t("reverseShares.status.active")
                          : t("reverseShares.status.inactive")
                    }
                  />
                  <h3 className="text-lg font-bold text-foreground truncate">
                    {getDisplayValue("name") || t("reverseShares.card.untitled")}
                  </h3>
                  {onUpdateReverseShare && (
                    <Button
                      size="icon"
                      variant="ghost"
                      className="h-4 w-4 opacity-0 group-hover/title:opacity-100 transition-opacity text-muted-foreground hover:text-foreground"
                      onClick={() => startEdit("name", getDisplayValue("name"))}
                    >
                      <IconEdit className="h-3 w-3" />
                    </Button>
                  )}
                </div>
              )}
            </div>

            <div className="flex items-center gap-1">
              <Button
                variant="outline"
                size="sm"
                className="h-6 w-6 p-0 hover:bg-background/80 rounded-sm"
                onClick={() => onViewDetails(reverseShare)}
                title={t("reverseShares.card.viewDetails")}
              >
                <IconEye className="h-3 w-3" />
              </Button>

              <Button
                variant="outline"
                size="sm"
                className="h-6 w-6 p-0 hover:bg-background/80 rounded-sm"
                onClick={() => onEdit(reverseShare)}
                title={t("reverseShares.actions.edit")}
              >
                <IconEdit className="h-3 w-3" />
              </Button>

              <DropdownMenu>
                <DropdownMenuTrigger asChild>
                  <Button variant="outline" size="sm" className="h-6 w-6 p-0 hover:bg-background/80 rounded-sm">
                    <IconDotsVertical className="h-3 w-3" />
                  </Button>
                </DropdownMenuTrigger>
                <DropdownMenuContent align="end">
                  <DropdownMenuItem onClick={() => onCopyLink(reverseShare)}>
                    <IconCopy className="h-4 w-4" />
                    {t("reverseShares.card.copyLink")}
                  </DropdownMenuItem>

                  {hasAlias && (
                    <DropdownMenuItem onClick={handleOpenInNewTab}>
                      <IconExternalLink className="h-4 w-4" />
                      {t("reverseShares.card.openInNewTab")}
                    </DropdownMenuItem>
                  )}

                  {hasAlias && (
                    <DropdownMenuItem onClick={() => onGenerateLink(reverseShare)}>
                      <IconLink className="h-4 w-4" />
                      {hasAlias ? t("reverseShares.card.editLink") : t("reverseShares.card.createLink")}
                    </DropdownMenuItem>
                  )}

                  <DropdownMenuItem onClick={() => onEdit(reverseShare)}>
                    <IconEdit className="h-4 w-4" />
                    {t("reverseShares.actions.edit")}
                  </DropdownMenuItem>

                  <DropdownMenuItem onClick={() => onViewFiles(reverseShare)}>
                    <IconFile className="h-4 w-4" />
                    {t("reverseShares.actions.viewFiles")}
                  </DropdownMenuItem>

                  <DropdownMenuItem className="text-destructive" onClick={() => onDelete(reverseShare)}>
                    <IconTrash className="h-4 w-4" />
                    {t("reverseShares.card.delete")}
                  </DropdownMenuItem>
                </DropdownMenuContent>
              </DropdownMenu>
            </div>
          </div>

          {/* Descrição compacta */}
          {editingField?.field === "description" ? (
            <div className="flex items-center gap-2">
              <Input
                ref={inputRef}
                value={editValue}
                onChange={(e) => setEditValue(e.target.value)}
                onKeyDown={handleKeyDown}
                className="h-7 text-sm border-primary/50 focus:border-primary"
                placeholder={t("reverseShares.card.addDescriptionPlaceholder")}
              />
              <Button
                size="icon"
                variant="ghost"
                className="h-5 w-5 text-green-600 hover:text-green-700"
                onClick={saveEdit}
              >
                <IconCheck className="h-3 w-3" />
              </Button>
              <Button
                size="icon"
                variant="ghost"
                className="h-5 w-5 text-red-600 hover:text-red-700"
                onClick={cancelEdit}
              >
                <IconX className="h-3 w-3" />
              </Button>
            </div>
          ) : (
            <div className="flex items-start gap-2 group/desc">
              <p className="text-xs text-muted-foreground line-clamp-1 flex-1">
                {getDisplayValue("description") || t("reverseShares.card.noDescription")}
              </p>
              {onUpdateReverseShare && (
                <Button
                  size="icon"
                  variant="ghost"
                  className="h-4 w-4 opacity-0 group-hover/desc:opacity-100 transition-opacity text-muted-foreground hover:text-foreground"
                  onClick={() => startEdit("description", getDisplayValue("description"))}
                >
                  <IconEdit className="h-3 w-3" />
                </Button>
              )}
            </div>
          )}

          {/* Link em destaque */}
          {hasAlias && (
            <div
              className={`rounded-md p-2 ${
                reverseShare.isActive && !isExpired
                  ? "bg-primary/5 border border-primary/20"
                  : "bg-red-50 dark:bg-red-950/20 border border-red-200 dark:border-red-800"
              }`}
            >
              <div className="flex items-center gap-2">
                <IconLink
                  className={`h-3 w-3 shrink-0 ${
                    reverseShare.isActive && !isExpired ? "text-primary" : "text-red-500"
                  }`}
                />
                <code
                  className={`text-xs font-mono px-2 py-1 rounded flex-1 truncate ${
                    reverseShare.isActive && !isExpired
                      ? "text-primary bg-primary/10"
                      : "text-red-500 bg-red-100 dark:bg-red-900/30"
                  }`}
                >
                  {origin}/r/{reverseShare.alias?.alias}
                </code>
                <Button
                  variant="ghost"
                  size="sm"
                  className={`h-5 w-5 p-0 ${
                    reverseShare.isActive && !isExpired
                      ? "text-primary hover:text-primary/80"
                      : "text-red-500 hover:text-red-600"
                  }`}
                  onClick={() => onCopyLink(reverseShare)}
                  title={t("reverseShares.card.copyLinkTitle")}
                >
                  <IconCopy className="h-3 w-3" />
                </Button>
              </div>
            </div>
          )}

          {/* Grid compacto: Estatísticas + Controles */}
          <div className="grid grid-cols-4 gap-2">
            {/* Arquivos */}
            <Button
              variant="ghost"
              className="bg-muted/20 rounded-md p-2 text-center border border-border/50 hover:bg-muted/40 transition-colors h-auto"
              onClick={() => onViewFiles(reverseShare)}
              title={t("reverseShares.actions.viewFiles")}
            >
              <div className="flex flex-col items-center gap-1">
                <IconFile className="h-4 w-4 text-blue-600" />
                <p className="text-xs font-medium text-foreground leading-none">{fileCount}</p>
                <p className="text-xs text-muted-foreground">{t("reverseShares.labels.files")}</p>
              </div>
            </Button>

            {/* Tamanho */}
            <div className="bg-muted/20 rounded-md p-2 text-center border border-border/50">
              <div className="flex items-center justify-center mb-2">
                <IconFileUnknown className="h-4 w-4 text-green-600" />
              </div>
              <p className="text-xs font-medium text-foreground leading-none">{formatFileSize(totalSize)}</p>
              <p className="text-xs text-muted-foreground">{t("reverseShares.labels.size")}</p>
            </div>

            {/* Status Toggle */}
            <div className="bg-muted/20 rounded-md overflow-hidden border border-border/50">
              {onToggleActive ? (
                <Button
                  variant="ghost"
                  className="w-full h-full p-2 hover:bg-muted/40 transition-colors text-center"
                  onClick={handleToggleActive}
                  title={`${t("common.click")} ${reverseShare.isActive ? t("reverseShares.modals.details.deactivate") : t("reverseShares.modals.details.activate")}`}
                >
                  <div className="flex flex-col items-center gap-1">
                    {reverseShare.isActive ? (
                      <IconToggleRight className="h-4 w-4 text-green-600" />
                    ) : (
                      <IconToggleLeft className="h-4 w-4 text-red-600" />
                    )}
                    <p className="text-xs font-medium leading-none">
                      {reverseShare.isActive ? t("reverseShares.status.active") : t("reverseShares.status.inactive")}
                    </p>
                    <p className="text-xs text-muted-foreground">{t("reverseShares.labels.status")}</p>
                  </div>
                </Button>
              ) : (
                <div className="p-2 text-center">
                  <div className="flex flex-col items-center gap-1">
                    {reverseShare.isActive ? (
                      <IconToggleRight className="h-4 w-4 text-green-600" />
                    ) : (
                      <IconToggleLeft className="h-4 w-4 text-red-600" />
                    )}
                    <p className="text-xs font-medium leading-none">
                      {reverseShare.isActive ? t("reverseShares.status.active") : t("reverseShares.status.inactive")}
                    </p>
                    <p className="text-xs text-muted-foreground">{t("reverseShares.labels.status")}</p>
                  </div>
                </div>
              )}
            </div>

            {/* Proteção Toggle */}
            <div className="bg-muted/20 rounded-md overflow-hidden border border-border/50">
              {onUpdatePassword ? (
                <Button
                  variant="ghost"
                  className="w-full h-full p-2 hover:bg-muted/40 transition-colors text-center"
                  onClick={handleTogglePassword}
                  title={t("reverseShares.labels.configureProtection")}
                >
                  <div className="flex flex-col items-center gap-1">
                    {hasPassword ? (
                      <IconLock className="h-4 w-4 text-yellow-600" />
                    ) : (
                      <IconLockOpen className="h-4 w-4 text-green-600" />
                    )}
                    <p className="text-xs font-medium leading-none">
                      {hasPassword ? t("reverseShares.status.protected") : t("reverseShares.status.public")}
                    </p>
                    <p className="text-xs text-muted-foreground">{t("reverseShares.labels.access")}</p>
                  </div>
                </Button>
              ) : (
                <div className="p-2 text-center">
                  <div className="flex flex-col items-center gap-1">
                    {hasPassword ? (
                      <IconLock className="h-4 w-4 text-yellow-600" />
                    ) : (
                      <IconLockOpen className="h-4 w-4 text-green-600" />
                    )}
                    <p className="text-xs font-medium leading-none">
                      {hasPassword ? t("reverseShares.status.protected") : t("reverseShares.status.public")}
                    </p>
                    <p className="text-xs text-muted-foreground">{t("reverseShares.labels.access")}</p>
                  </div>
                </div>
              )}
            </div>
          </div>

          {/* CTA para criar link */}
          {!hasAlias && (
            <Button
              variant="outline"
              size="sm"
              onClick={() => onGenerateLink(reverseShare)}
              className="w-full h-8 text-sm"
            >
              <IconLink className="h-3 w-3" />
              {t("reverseShares.card.createLinkCTA")}
            </Button>
          )}
        </CardContent>
      </Card>

      {/* Modal de Senha */}
      {showPasswordModal && onUpdatePassword && (
        <EditPasswordModal
          reverseShare={reverseShare}
          isOpen={showPasswordModal}
          onClose={() => setShowPasswordModal(false)}
          onUpdatePassword={async (id: string, data: { hasPassword: boolean; password?: string }) => {
            await onUpdatePassword(id, data);
            setShowPasswordModal(false);
          }}
        />
      )}
    </>
  );
}
