"use client";

import { useCallback, useEffect, useRef, useState } from "react";
import {
  IconCheck,
  IconCopy,
  IconDownload,
  IconEdit,
  IconExternalLink,
  IconLock,
  IconLockOpen,
  IconMail,
  IconX,
} from "@tabler/icons-react";
import { format } from "date-fns";
import { useTranslations } from "next-intl";
import QRCode from "react-qr-code";
import { toast } from "sonner";

import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Loader } from "@/components/ui/loader";
import { getShare } from "@/http/endpoints";
import { getFileIcon } from "@/utils/file-icons";
import { GenerateShareLinkModal } from "./generate-share-link-modal";
import { QrCodeModal } from "./qr-code-modal";
import { ShareExpirationModal } from "./share-expiration-modal";
import { ShareSecurityModal } from "./share-security-modal";

interface ShareDetailsModalProps {
  shareId: string | null;
  onClose: () => void;
  onUpdateName?: (shareId: string, newName: string) => Promise<void>;
  onUpdateDescription?: (shareId: string, newDescription: string) => Promise<void>;
  onGenerateLink?: (shareId: string, alias: string) => Promise<void>;
  onManageFiles?: (share: any) => void;
  onUpdateSecurity?: (shareId: string) => Promise<void>;
  onUpdateExpiration?: (shareId: string) => Promise<void>;
  refreshTrigger?: number;
  onSuccess?: () => void;
}

interface ShareFile {
  id: string;
  name: string;
  description: string | null;
  extension: string;
  size: number;
  objectName: string;
  userId: string;
  createdAt: string;
  updatedAt: string;
}

interface ShareRecipient {
  id: string;
  email: string;
  createdAt: string;
  updatedAt: string;
}

export function ShareDetailsModal({
  shareId,
  onClose,
  onUpdateName,
  onUpdateDescription,
  onGenerateLink,
  onManageFiles,
  onUpdateSecurity,
  onUpdateExpiration,
  refreshTrigger,
  onSuccess,
}: ShareDetailsModalProps) {
  const t = useTranslations();
  const [share, setShare] = useState<any>(null);
  const [isLoading, setIsLoading] = useState(false);
  const [editingField, setEditingField] = useState<{ field: "name" | "description" } | null>(null);
  const [editValue, setEditValue] = useState("");
  const [pendingChanges, setPendingChanges] = useState<{ name?: string; description?: string }>({});
  const [showLinkModal, setShowLinkModal] = useState(false);
  const [showSecurityModal, setShowSecurityModal] = useState(false);
  const [showExpirationModal, setShowExpirationModal] = useState(false);
  const [showQrCodeModal, setShowQrCodeModal] = useState(false);
  const [isDownloading, setIsDownloading] = useState(false);
  const inputRef = useRef<HTMLInputElement>(null);

  const loadShareDetails = useCallback(async () => {
    if (!shareId) return;
    setIsLoading(true);
    try {
      const response = await getShare(shareId);
      setShare(response.data.share);
    } catch {
      toast.error(t("shareDetails.loadError"));
    } finally {
      setIsLoading(false);
    }
  }, [shareId, t]);

  useEffect(() => {
    if (shareId) {
      loadShareDetails();
    }
  }, [shareId, loadShareDetails]);

  useEffect(() => {
    if (editingField && inputRef.current) {
      inputRef.current.focus();
      inputRef.current.select();
    }
  }, [editingField]);

  useEffect(() => {
    setPendingChanges({});
  }, [share]);

  useEffect(() => {
    if (refreshTrigger) {
      loadShareDetails();
    }
  }, [refreshTrigger, loadShareDetails]);

  const formatDate = (dateString: string | null) => {
    if (!dateString) return t("shareDetails.notAvailable");
    try {
      return format(new Date(dateString), "MM/dd/yyyy HH:mm");
    } catch {
      return t("shareDetails.invalidDate");
    }
  };

  const startEdit = (field: "name" | "description", currentValue: string) => {
    setEditingField({ field });
    setEditValue(currentValue || "");
  };

  const saveEdit = async () => {
    if (!editingField || !shareId) return;

    const { field } = editingField;

    setPendingChanges((prev) => ({
      ...prev,
      [field]: editValue,
    }));

    try {
      if (field === "name" && onUpdateName) {
        await onUpdateName(shareId, editValue);
      } else if (field === "description" && onUpdateDescription) {
        await onUpdateDescription(shareId, editValue);
      }

      await loadShareDetails();
      if (onSuccess) {
        onSuccess();
      }
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

  const getDisplayValue = (field: "name" | "description") => {
    const pendingChange = pendingChanges[field];
    if (pendingChange !== undefined) {
      return pendingChange;
    }
    return field === "name" ? share?.name : share?.description;
  };

  const handleCopyLink = () => {
    if (share?.alias?.alias) {
      const link = `${window.location.origin}/s/${share.alias.alias}`;
      navigator.clipboard.writeText(link);
      toast.success(t("shareDetails.linkCopied"));
    }
  };

  const handleOpenLink = () => {
    if (share?.alias?.alias) {
      const link = `${window.location.origin}/s/${share.alias.alias}`;
      window.open(link, "_blank");
    }
  };

  const downloadQRCode = () => {
    setIsDownloading(true);

    const svg = document.getElementById("share-details-qr-code");
    if (!svg) {
      setIsDownloading(false);
      return;
    }

    const canvas = document.createElement("canvas");
    const ctx = canvas.getContext("2d");

    const padding = 20;
    canvas.width = 200 + padding * 2;
    canvas.height = 200 + padding * 2;

    if (ctx) {
      ctx.fillStyle = "#FFFFFF";
      ctx.fillRect(0, 0, canvas.width, canvas.height);

      const svgData = new XMLSerializer().serializeToString(svg);
      const img = new Image();

      img.onload = () => {
        ctx.drawImage(img, padding, padding, 200, 200);

        const link = document.createElement("a");
        link.download = `${share?.name?.replace(/[^a-z0-9]/gi, "-").toLowerCase() || "share"}-qr-code.png`;
        link.href = canvas.toDataURL("image/png");
        link.click();

        setIsDownloading(false);
      };

      img.src = `data:image/svg+xml;base64,${btoa(svgData)}`;
    } else {
      setIsDownloading(false);
    }
  };

  const handleLinkGenerated = async () => {
    setShowLinkModal(false);
    await loadShareDetails();
    if (onSuccess) {
      onSuccess();
    }
  };

  const handleSecurityUpdated = async () => {
    setShowSecurityModal(false);
    await loadShareDetails();
    if (onSuccess) {
      onSuccess();
    }
  };

  const handleExpirationUpdated = async () => {
    setShowExpirationModal(false);
    await loadShareDetails();
    if (onSuccess) {
      onSuccess();
    }
  };

  if (!share) return null;

  const shareLink = share?.alias?.alias ? `${window.location.origin}/s/${share.alias.alias}` : null;
  const isEditingName = editingField?.field === "name";
  const isEditingDescription = editingField?.field === "description";
  const displayName = getDisplayValue("name");
  const displayDescription = getDisplayValue("description");

  return (
    <>
      <Dialog open={!!shareId} onOpenChange={() => onClose()}>
        <DialogContent className="sm:max-w-[600px]">
          <DialogHeader>
            <DialogTitle>{t("shareDetails.title")}</DialogTitle>
            <DialogDescription>{t("shareDetails.subtitle")}</DialogDescription>
          </DialogHeader>
          <div className="py-4">
            {isLoading ? (
              <div className="flex justify-center py-8">
                <Loader size="lg" />
              </div>
            ) : (
              <div className="space-y-4">
                <div className="grid grid-cols-3 gap-3">
                  <div className="text-center p-2 bg-muted/30 rounded-lg">
                    <p className="text-lg font-semibold text-green-600">{share.views || 0}</p>
                    <p className="text-xs text-muted-foreground">{t("shareDetails.views")}</p>
                  </div>
                  <div className="text-center p-2 bg-muted/30 rounded-lg">
                    <p className="text-lg font-semibold text-green-600">{share.files?.length || 0}</p>
                    <p className="text-xs text-muted-foreground">{t("shareDetails.files")}</p>
                  </div>
                  <div className="text-center p-2 bg-muted/30 rounded-lg">
                    <p className="text-lg font-semibold text-green-600">{share.recipients?.length || 0}</p>
                    <p className="text-xs text-muted-foreground">{t("shareDetails.recipients")}</p>
                  </div>
                </div>

                <div className="grid grid-cols-2 gap-4">
                  {/* Basic Information */}
                  <div className="space-y-3">
                    <div className="flex items-center gap-2 border-b pb-2">
                      <h3 className="text-base font-medium text-foreground">{t("shareDetails.basicInfo")}</h3>
                    </div>

                    <div>
                      <div className="flex items-center gap-2 mb-1">
                        <label className="text-sm font-medium text-muted-foreground">{t("shareDetails.name")}</label>
                        {onUpdateName && !isEditingName && (
                          <Button
                            size="icon"
                            variant="ghost"
                            className="h-5 w-5 text-muted-foreground hover:text-foreground"
                            onClick={() => startEdit("name", displayName || "")}
                          >
                            <IconEdit className="h-3 w-3" />
                          </Button>
                        )}
                      </div>
                      {isEditingName ? (
                        <div className="flex items-center gap-2">
                          <Input
                            ref={inputRef}
                            value={editValue}
                            onChange={(e) => setEditValue(e.target.value)}
                            onKeyDown={handleKeyDown}
                            className="h-8 flex-1 text-sm"
                            onClick={(e) => e.stopPropagation()}
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
                        <span className="text-sm font-medium block">{displayName || t("shareDetails.untitled")}</span>
                      )}
                    </div>

                    <div>
                      <div className="flex items-center gap-2 mb-1">
                        <label className="text-sm font-medium text-muted-foreground">
                          {t("shareDetails.description")}
                        </label>
                        {onUpdateDescription && !isEditingDescription && (
                          <Button
                            size="icon"
                            variant="ghost"
                            className="h-5 w-5 text-muted-foreground hover:text-foreground"
                            onClick={() => startEdit("description", displayDescription || "")}
                          >
                            <IconEdit className="h-3 w-3" />
                          </Button>
                        )}
                      </div>
                      {isEditingDescription ? (
                        <div className="flex items-center gap-2">
                          <Input
                            ref={inputRef}
                            value={editValue}
                            onChange={(e) => setEditValue(e.target.value)}
                            onKeyDown={handleKeyDown}
                            className="h-8 flex-1 text-sm"
                            placeholder={t("shareDetails.noDescription")}
                            onClick={(e) => e.stopPropagation()}
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
                        <span className="text-sm block">{displayDescription || t("shareDetails.noDescription")}</span>
                      )}
                    </div>
                  </div>

                  {/* QR Code */}
                  {shareLink && (
                    <div className="space-y-3">
                      <div className="flex items-center gap-2 border-b pb-2">
                        <h3
                          className="text-base font-medium text-foreground cursor-pointer"
                          onClick={() => setShowQrCodeModal(true)}
                        >
                          {t("shareDetails.qrCode", { defaultValue: "QR Code" })}
                        </h3>
                        <Button
                          size="icon"
                          variant="ghost"
                          className="h-5 w-5 text-muted-foreground hover:text-foreground"
                          onClick={downloadQRCode}
                          disabled={isDownloading}
                          title={t("shareDetails.downloadQrCode")}
                        >
                          <IconDownload className="h-3 w-3" />
                        </Button>
                      </div>
                      <div className="flex flex-col items-start justify-start ">
                        <div
                          className="p-2 bg-white rounded-lg cursor-pointer hover:opacity-80 transition-opacity duration-300"
                          onClick={() => setShowQrCodeModal(true)}
                          title={t("shareDetails.clickToEnlargeQrCode", { defaultValue: "Click to enlarge QR Code" })}
                        >
                          <QRCode
                            id="share-details-qr-code"
                            value={shareLink}
                            size={100}
                            level="H"
                            fgColor="#000000"
                            bgColor="#FFFFFF"
                          />
                        </div>
                      </div>
                    </div>
                  )}
                </div>

                <div className="space-y-3">
                  <div className="flex items-center gap-2 border-b pb-2">
                    <h3 className="text-base font-medium text-foreground">{t("shareDetails.shareLink")}</h3>
                    {onGenerateLink && (
                      <Button
                        size="icon"
                        variant="ghost"
                        className="h-5 w-5 text-muted-foreground hover:text-foreground"
                        onClick={() => setShowLinkModal(true)}
                        title={shareLink ? t("shareDetails.editLink") : t("shareDetails.generateLink")}
                      >
                        <IconEdit className="h-3 w-3" />
                      </Button>
                    )}
                  </div>
                  {shareLink ? (
                    <div className="flex gap-2">
                      <Input value={shareLink} readOnly className="flex-1 bg-muted/30 text-sm h-8" />
                      <Button
                        variant="outline"
                        size="icon"
                        className="h-8 w-8"
                        onClick={handleCopyLink}
                        title={t("shareDetails.copyLink")}
                      >
                        <IconCopy className="h-3.5 w-3.5" />
                      </Button>
                      <Button
                        variant="outline"
                        size="icon"
                        className="h-8 w-8"
                        onClick={handleOpenLink}
                        title={t("shareDetails.openLink")}
                      >
                        <IconExternalLink className="h-3.5 w-3.5" />
                      </Button>
                    </div>
                  ) : (
                    <div className="flex items-center justify-between p-2 bg-muted/20 rounded-lg">
                      <p className="text-sm text-muted-foreground">{t("shareDetails.noLink")}</p>
                    </div>
                  )}
                </div>

                <div className="grid grid-cols-2 gap-4">
                  <div className="space-y-3">
                    <div className="flex items-center gap-2 border-b pb-2">
                      <h3 className="text-base font-medium text-foreground">{t("shareDetails.dates")}</h3>
                      {onUpdateExpiration && (
                        <Button
                          size="icon"
                          variant="ghost"
                          className="h-5 w-5 text-muted-foreground hover:text-foreground"
                          onClick={() => setShowExpirationModal(true)}
                          title={t("shareDetails.editExpiration")}
                        >
                          <IconEdit className="h-3 w-3" />
                        </Button>
                      )}
                    </div>
                    <div className="space-y-2">
                      <div>
                        <div className="text-xs font-medium text-muted-foreground">{t("shareDetails.created")}</div>
                        <div className="text-sm">{formatDate(share.createdAt)}</div>
                      </div>
                      <div>
                        <div className="text-xs font-medium text-muted-foreground">{t("shareDetails.expires")}</div>
                        <div className="text-sm">
                          {share.expiration ? formatDate(share.expiration) : t("shareDetails.never")}
                        </div>
                      </div>
                    </div>
                  </div>

                  <div className="space-y-3">
                    <div className="flex items-center gap-2 border-b pb-2">
                      <h3 className="text-base font-medium text-foreground">{t("shareDetails.security")}</h3>
                      {onUpdateSecurity && (
                        <Button
                          size="icon"
                          variant="ghost"
                          className="h-5 w-5 text-muted-foreground hover:text-foreground"
                          onClick={() => setShowSecurityModal(true)}
                          title={t("shareDetails.editSecurity")}
                        >
                          <IconEdit className="h-3 w-3" />
                        </Button>
                      )}
                    </div>
                    <div className="flex flex-col gap-2">
                      {share.security?.hasPassword ? (
                        <Badge variant="secondary" className="bg-yellow-500/20 text-yellow-700 border-yellow-200 w-fit">
                          <IconLock className="h-3 w-3 mr-1" />
                          {t("shareDetails.passwordProtected")}
                        </Badge>
                      ) : (
                        <Badge variant="secondary" className="bg-green-500/20 text-green-700 border-green-200 w-fit">
                          <IconLockOpen className="h-3 w-3 mr-1" />
                          {t("shareDetails.publicAccess")}
                        </Badge>
                      )}
                      {share.security?.maxViews && (
                        <Badge variant="secondary" className="bg-blue-500/20 text-blue-700 border-blue-200 w-fit">
                          {t("shareDetails.maxViews")} {share.security.maxViews}
                        </Badge>
                      )}
                    </div>
                  </div>
                </div>

                {share.files && share.files.length > 0 && (
                  <div className="space-y-3">
                    <div className="flex items-center gap-2 border-b pb-2">
                      <h3 className="text-base font-medium text-foreground">{t("shareDetails.files")}</h3>
                      {onManageFiles && (
                        <Button
                          size="icon"
                          variant="ghost"
                          className="h-5 w-5 text-muted-foreground hover:text-foreground"
                          onClick={() => onManageFiles(share)}
                          title={t("sharesTable.actions.manageFiles")}
                        >
                          <IconEdit className="h-3 w-3" />
                        </Button>
                      )}
                    </div>
                    <div className="border rounded-lg bg-muted/10 p-2">
                      <div className="grid gap-1 max-h-32 overflow-y-auto">
                        {share.files.map((file: ShareFile) => {
                          const { icon: FileIcon, color } = getFileIcon(file.name);
                          return (
                            <div
                              key={file.id}
                              className="flex items-center gap-2 p-2 bg-background rounded border mr-2"
                            >
                              <FileIcon className={`h-3.5 w-3.5 ${color} flex-shrink-0`} />
                              <div className="flex-1 min-w-0">
                                <div className="text-xs font-medium truncate max-w-[280px]" title={file.name}>
                                  {file.name}
                                </div>
                                {file.description && (
                                  <div
                                    className="text-xs text-muted-foreground truncate max-w-[280px]"
                                    title={file.description}
                                  >
                                    {file.description}
                                  </div>
                                )}
                              </div>
                            </div>
                          );
                        })}
                      </div>
                    </div>
                  </div>
                )}

                {share.recipients && share.recipients.length > 0 && (
                  <div className="space-y-3">
                    <h3 className="text-base font-medium text-foreground border-b pb-2">
                      {t("shareDetails.recipients")}
                    </h3>
                    <div className="flex flex-wrap gap-1">
                      {share.recipients.map((recipient: ShareRecipient) => (
                        <Badge
                          key={recipient.id}
                          variant="secondary"
                          className="bg-blue-500/20 text-blue-700 border-blue-200 text-xs"
                        >
                          <IconMail className="h-3 w-3 mr-1" />
                          {recipient.email}
                        </Badge>
                      ))}
                    </div>
                  </div>
                )}
              </div>
            )}
          </div>
          <DialogFooter>
            <Button onClick={onClose}>{t("common.close")}</Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
      {showLinkModal && shareId && (
        <GenerateShareLinkModal
          shareId={shareId}
          share={share}
          onClose={() => setShowLinkModal(false)}
          onSuccess={handleLinkGenerated}
          onGenerate={onGenerateLink || (() => Promise.resolve())}
        />
      )}
      {showSecurityModal && shareId && onUpdateSecurity && (
        <ShareSecurityModal
          shareId={shareId}
          share={share}
          onClose={() => setShowSecurityModal(false)}
          onSuccess={handleSecurityUpdated}
        />
      )}
      {showExpirationModal && shareId && onUpdateExpiration && (
        <ShareExpirationModal
          shareId={shareId}
          share={share}
          onClose={() => setShowExpirationModal(false)}
          onSuccess={handleExpirationUpdated}
        />
      )}
      {showQrCodeModal && shareLink && (
        <QrCodeModal
          isOpen={showQrCodeModal}
          onClose={() => setShowQrCodeModal(false)}
          shareLink={shareLink}
          shareName={share?.name || "Share"}
        />
      )}
    </>
  );
}
