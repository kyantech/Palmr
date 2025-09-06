"use client";

import { useEffect, useState } from "react";
import { IconCalendar, IconCopy, IconDownload, IconEye, IconLink, IconLock, IconShare } from "@tabler/icons-react";
import { useTranslations } from "next-intl";
import QRCode from "react-qr-code";
import { toast } from "sonner";

import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Switch } from "@/components/ui/switch";
import { createShare, createShareAlias, listFiles, listFolders } from "@/http/endpoints";
import { customNanoid } from "@/lib/utils";

interface File {
  id: string;
  name: string;
  description?: string;
  size: number;
  objectName: string;
  createdAt: string;
  updatedAt: string;
}

interface Folder {
  id: string;
  name: string;
  description?: string;
  objectName: string;
  parentId?: string;
  userId: string;
  createdAt: string;
  updatedAt: string;
}

interface ShareItemModalProps {
  isOpen: boolean;
  file?: File | null;
  folder?: Folder | null;
  onClose: () => void;
  onSuccess: () => void;
}

const generateCustomId = () => customNanoid(10, "0123456789abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ");

export function ShareItemModal({ isOpen, file, folder, onClose, onSuccess }: ShareItemModalProps) {
  const t = useTranslations();
  const [step, setStep] = useState<"create" | "link">("create");
  const [shareId, setShareId] = useState<string | null>(null);
  const [formData, setFormData] = useState({
    name: "",
    description: "",
    password: "",
    expiresAt: "",
    isPasswordProtected: false,
    maxViews: "",
  });
  const [alias, setAlias] = useState(() => generateCustomId());
  const [generatedLink, setGeneratedLink] = useState("");
  const [isLoading, setIsLoading] = useState(false);

  const item = file || folder;
  const itemType = file ? "file" : "folder";

  useEffect(() => {
    if (isOpen && item) {
      const baseName = file ? file.name.split(".")[0] : folder!.name;
      setFormData({
        name: baseName,
        description: "",
        password: "",
        expiresAt: "",
        isPasswordProtected: false,
        maxViews: "",
      });
      setAlias(generateCustomId());
      setStep("create");
      setShareId(null);
      setGeneratedLink("");
    }
  }, [isOpen, item, file, folder]);

  // Recursively get all files and folders within a folder
  const getAllFolderContents = async (folderId: string): Promise<{ files: string[]; folders: string[] }> => {
    try {
      // Get all files and folders
      const [filesResponse, foldersResponse] = await Promise.all([listFiles(), listFolders()]);

      const allFiles = filesResponse.data.files || [];
      const allFolders = foldersResponse.data.folders || [];

      // Find all files and subfolders that belong to this folder (recursively)
      const collectContents = (parentId: string): { files: string[]; folders: string[] } => {
        const folderFiles = allFiles.filter((f: any) => f.folderId === parentId).map((f: any) => f.id);

        const subFolders = allFolders.filter((f: any) => f.parentId === parentId);
        const subFolderIds = subFolders.map((f: any) => f.id);

        // Recursively get contents of subfolders
        let allSubFiles: string[] = [...folderFiles];
        let allSubFolders: string[] = [...subFolderIds];

        subFolders.forEach((subFolder: any) => {
          const subContents = collectContents(subFolder.id);
          allSubFiles = [...allSubFiles, ...subContents.files];
          allSubFolders = [...allSubFolders, ...subContents.folders];
        });

        return { files: allSubFiles, folders: allSubFolders };
      };

      return collectContents(folderId);
    } catch (error) {
      console.error("Error getting folder contents:", error);
      return { files: [], folders: [] };
    }
  };

  const handleCreateShare = async () => {
    if (!item) return;

    try {
      setIsLoading(true);

      let filesToShare: string[] = [];
      let foldersToShare: string[] = [];

      if (file) {
        filesToShare = [file.id];
      } else if (folder) {
        // Get all contents of the folder recursively
        const folderContents = await getAllFolderContents(folder.id);
        filesToShare = folderContents.files;
        foldersToShare = [folder.id, ...folderContents.folders];
      }

      const shareResponse = await createShare({
        name: formData.name,
        description: formData.description || undefined,
        password: formData.isPasswordProtected ? formData.password : undefined,
        expiration: formData.expiresAt ? new Date(formData.expiresAt).toISOString() : undefined,
        maxViews: formData.maxViews ? parseInt(formData.maxViews) : undefined,
        files: filesToShare,
        folders: foldersToShare,
      });

      const newShareId = shareResponse.data.share.id;
      setShareId(newShareId);

      toast.success(t("createShare.success"));
      setStep("link");
    } catch {
      toast.error(t("createShare.error"));
    } finally {
      setIsLoading(false);
    }

    setFormData({
      name: "",
      description: "",
      password: "",
      expiresAt: "",
      isPasswordProtected: false,
      maxViews: "",
    });
  };

  const handleGenerateLink = async () => {
    if (!shareId) return;

    try {
      setIsLoading(true);
      await createShareAlias(shareId, { alias });
      const link = `${window.location.origin}/s/${alias}`;
      setGeneratedLink(link);
      toast.success(t("generateShareLink.success"));
    } catch {
      toast.error(t("generateShareLink.error"));
    } finally {
      setIsLoading(false);
    }
  };

  const handleCopyLink = () => {
    navigator.clipboard.writeText(generatedLink);
    toast.success(t("generateShareLink.copied"));
  };

  const downloadQRCode = () => {
    const canvas = document.getElementById("share-item-qr-code") as HTMLCanvasElement;
    if (canvas) {
      const link = document.createElement("a");
      link.download = `share-${itemType}-qr-code.png`;
      link.href = canvas.toDataURL("image/png");
      document.body.appendChild(link);
      link.click();
      document.body.removeChild(link);
    }
  };

  const handleClose = () => {
    onClose();
    setTimeout(() => {
      setStep("create");
      setShareId(null);
      setGeneratedLink("");
      setFormData({
        name: "",
        description: "",
        password: "",
        expiresAt: "",
        isPasswordProtected: false,
        maxViews: "",
      });
    }, 300);
  };

  const handleSuccess = () => {
    onSuccess();
    handleClose();
  };

  return (
    <Dialog open={isOpen} onOpenChange={handleClose}>
      <DialogContent className="max-w-md">
        <DialogHeader>
          <DialogTitle className="flex items-center gap-2">
            {step === "create" ? (
              <>
                <IconShare size={20} />
                {itemType === "file" ? t("shareActions.fileTitle") : t("shareActions.folderTitle")}
              </>
            ) : (
              <>
                <IconLink size={20} />
                {t("shareActions.linkTitle")}
              </>
            )}
          </DialogTitle>
        </DialogHeader>

        {step === "create" && (
          <div className="flex flex-col gap-4">
            <div className="space-y-2">
              <Label>{t("createShare.nameLabel")}</Label>
              <Input
                value={formData.name}
                onChange={(e) => setFormData({ ...formData, name: e.target.value })}
                placeholder={t("createShare.namePlaceholder")}
              />
            </div>

            <div className="space-y-2">
              <Label>{t("createShare.descriptionLabel")}</Label>
              <Input
                value={formData.description}
                onChange={(e) => setFormData({ ...formData, description: e.target.value })}
                placeholder={t("createShare.descriptionPlaceholder")}
              />
            </div>

            <div className="space-y-2">
              <Label className="flex items-center gap-2">
                <IconCalendar size={16} />
                {t("createShare.expirationLabel")}
              </Label>
              <Input
                placeholder={t("createShare.expirationPlaceholder")}
                type="datetime-local"
                value={formData.expiresAt}
                onChange={(e) => setFormData({ ...formData, expiresAt: e.target.value })}
              />
            </div>

            <div className="space-y-2">
              <Label className="flex items-center gap-2">
                <IconEye size={16} />
                {t("createShare.maxViewsLabel")}
              </Label>
              <Input
                min="1"
                placeholder={t("createShare.maxViewsPlaceholder")}
                type="number"
                value={formData.maxViews}
                onChange={(e) => setFormData({ ...formData, maxViews: e.target.value })}
              />
            </div>

            <div className="flex items-center gap-2">
              <Switch
                checked={formData.isPasswordProtected}
                onCheckedChange={(checked) =>
                  setFormData({
                    ...formData,
                    isPasswordProtected: checked,
                    password: "",
                  })
                }
                id="password-protection"
              />
              <Label htmlFor="password-protection" className="flex items-center gap-2">
                <IconLock size={16} />
                {t("createShare.passwordProtection")}
              </Label>
            </div>

            {formData.isPasswordProtected && (
              <div className="space-y-2">
                <Label>{t("createShare.passwordLabel")}</Label>
                <Input
                  type="password"
                  value={formData.password}
                  onChange={(e) => setFormData({ ...formData, password: e.target.value })}
                  placeholder={t("createShare.passwordLabel")}
                />
              </div>
            )}
          </div>
        )}

        {step === "link" && (
          <div className="space-y-4">
            {!generatedLink ? (
              <>
                <p className="text-sm text-muted-foreground">
                  {itemType === "file"
                    ? t("shareActions.linkDescriptionFile")
                    : t("shareActions.linkDescriptionFolder")}
                </p>
                <div className="space-y-2">
                  <Label>{t("shareActions.aliasLabel")}</Label>
                  <Input
                    placeholder={t("shareActions.aliasPlaceholder")}
                    value={alias}
                    onChange={(e) => setAlias(e.target.value)}
                  />
                </div>
              </>
            ) : (
              <>
                <div className="flex flex-col items-center justify-center">
                  <div className="p-4 bg-white rounded-lg">
                    <svg style={{ display: "none" }} /> {/* For SSR safety */}
                    <QRCode
                      id="share-item-qr-code"
                      value={generatedLink}
                      size={250}
                      level="H"
                      fgColor="#000000"
                      bgColor="#FFFFFF"
                    />
                  </div>
                </div>
                <p className="text-sm text-muted-foreground">{t("shareActions.linkReady")}</p>
                <div className="flex gap-2">
                  <Input readOnly value={generatedLink} className="flex-1" />
                  <Button size="icon" variant="outline" onClick={handleCopyLink} title={t("shareActions.copyLink")}>
                    <IconCopy className="h-4 w-4" />
                  </Button>
                </div>
              </>
            )}
          </div>
        )}

        <DialogFooter>
          {step === "create" && (
            <>
              <Button variant="outline" onClick={handleClose}>
                {t("common.cancel")}
              </Button>
              <Button disabled={isLoading || !formData.name.trim()} onClick={handleCreateShare}>
                {isLoading ? <div className="animate-spin">⠋</div> : t("createShare.create")}
              </Button>
            </>
          )}

          {step === "link" && !generatedLink && (
            <>
              <Button variant="outline" onClick={() => setStep("create")}>
                {t("common.back")}
              </Button>
              <Button disabled={!alias || isLoading} onClick={handleGenerateLink}>
                {isLoading ? <div className="animate-spin">⠋</div> : t("shareActions.generateLink")}
              </Button>
            </>
          )}

          {step === "link" && generatedLink && (
            <>
              <Button variant="outline" onClick={handleSuccess}>
                {t("common.close")}
              </Button>
              <Button onClick={downloadQRCode}>
                <IconDownload className="h-4 w-4" />
                {t("qrCodeModal.download")}
              </Button>
            </>
          )}
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
