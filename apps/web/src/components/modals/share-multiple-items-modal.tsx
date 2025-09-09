"use client";

import { useEffect, useState } from "react";
import {
  IconCalendar,
  IconCopy,
  IconDownload,
  IconEye,
  IconFolder,
  IconLink,
  IconLock,
  IconShare,
} from "@tabler/icons-react";
import { useTranslations } from "next-intl";
import QRCode from "react-qr-code";
import { toast } from "sonner";

import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { ScrollArea } from "@/components/ui/scroll-area";
import { Switch } from "@/components/ui/switch";
import { createShare, createShareAlias, listFiles, listFolders } from "@/http/endpoints";
import { customNanoid } from "@/lib/utils";
import { getFileIcon } from "@/utils/file-icons";

interface BulkFile {
  id: string;
  name: string;
  description?: string;
  size: number;
  objectName: string;
  createdAt: string;
  updatedAt: string;
}

interface BulkFolder {
  id: string;
  name: string;
  description?: string;
  objectName: string;
  parentId?: string;
  userId: string;
  createdAt: string;
  updatedAt: string;
  totalSize?: string;
  _count?: {
    files: number;
    children: number;
  };
}

interface BulkItem {
  id: string;
  name: string;
  description?: string;
  size?: number;
  type: "file" | "folder";
  createdAt: string;
  updatedAt: string;
}

interface ShareMultipleItemsModalProps {
  files: BulkFile[] | null;
  folders: BulkFolder[] | null;
  isOpen: boolean;
  onClose: () => void;
  onSuccess: () => void;
}

const generateCustomId = () => customNanoid(10, "0123456789abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ");

export function ShareMultipleItemsModal({ files, folders, isOpen, onClose, onSuccess }: ShareMultipleItemsModalProps) {
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

  useEffect(() => {
    if (isOpen && ((files && files.length > 0) || (folders && folders.length > 0))) {
      const fileCount = files ? files.length : 0;
      const folderCount = folders ? folders.length : 0;
      const totalCount = fileCount + folderCount;

      let defaultName = "";
      if (totalCount === 1) {
        if (fileCount === 1 && files) {
          defaultName = files[0].name.split(".")[0];
        } else if (folderCount === 1 && folders) {
          defaultName = folders[0].name;
        }
      } else {
        const items = [];
        if (fileCount > 0) items.push(`${fileCount} files`);
        if (folderCount > 0) items.push(`${folderCount} folders`);
        defaultName = `${items.join(" and ")} shared`;
      }

      setFormData({
        name: defaultName,
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
  }, [isOpen, files, folders]);

  const getAllFolderContents = async (folderId: string): Promise<{ files: string[]; folders: string[] }> => {
    try {
      const [filesResponse, foldersResponse] = await Promise.all([listFiles(), listFolders()]);

      const allFiles = filesResponse.data.files || [];
      const allFolders = foldersResponse.data.folders || [];

      const collectContents = (parentId: string): { files: string[]; folders: string[] } => {
        const folderFiles = allFiles.filter((f: any) => f.folderId === parentId).map((f: any) => f.id);

        const subFolders = allFolders.filter((f: any) => f.parentId === parentId);
        const subFolderIds = subFolders.map((f: any) => f.id);

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
    const fileCount = files ? files.length : 0;
    const folderCount = folders ? folders.length : 0;

    if (fileCount === 0 && folderCount === 0) return;

    try {
      setIsLoading(true);

      let allFilesToShare: string[] = files ? files.map((f) => f.id) : [];
      let allFoldersToShare: string[] = folders ? folders.map((f) => f.id) : [];

      if (folders && folders.length > 0) {
        for (const folder of folders) {
          const folderContents = await getAllFolderContents(folder.id);
          allFilesToShare = [...allFilesToShare, ...folderContents.files];
          allFoldersToShare = [...allFoldersToShare, ...folderContents.folders];
        }
      }

      const shareResponse = await createShare({
        name: formData.name,
        description: formData.description || undefined,
        password: formData.isPasswordProtected ? formData.password : undefined,
        expiration: formData.expiresAt ? new Date(formData.expiresAt).toISOString() : undefined,
        maxViews: formData.maxViews ? parseInt(formData.maxViews) : undefined,
        files: allFilesToShare,
        folders: allFoldersToShare,
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
    const qrCodeElement = document.getElementById("share-multiple-files-qr-code");
    if (qrCodeElement) {
      const canvas = qrCodeElement.querySelector("canvas");
      if (canvas) {
        const link = document.createElement("a");
        link.download = "share-multiple-files-qr-code.png";
        link.href = canvas.toDataURL("image/png");
        link.click();
      }
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

  if (!files && !folders) return null;

  const filesList = files || [];
  const foldersList = folders || [];
  const allItems: BulkItem[] = [
    ...filesList.map((file) => ({
      id: file.id,
      name: file.name,
      description: file.description,
      size: file.size,
      type: "file" as const,
      createdAt: file.createdAt,
      updatedAt: file.updatedAt,
    })),
    ...foldersList.map((folder) => ({
      id: folder.id,
      name: folder.name,
      description: folder.description,
      size: folder.totalSize ? parseInt(folder.totalSize) : undefined,
      type: "folder" as const,
      createdAt: folder.createdAt,
      updatedAt: folder.updatedAt,
    })),
  ];

  const totalSize =
    filesList.reduce((sum, file) => sum + file.size, 0) +
    foldersList.reduce((sum, folder) => sum + (folder.totalSize ? parseInt(folder.totalSize) : 0), 0);
  const formatFileSize = (bytes: number) => {
    const sizes = ["B", "KB", "MB", "GB"];
    if (bytes === 0) return "0 B";
    const i = Math.floor(Math.log(bytes) / Math.log(1024));
    return `${Math.round((bytes / Math.pow(1024, i)) * 100) / 100} ${sizes[i]}`;
  };

  return (
    <Dialog open={isOpen} onOpenChange={handleClose}>
      <DialogContent className="sm:max-w-md max-h-[90vh] overflow-hidden flex flex-col">
        <DialogHeader>
          <DialogTitle className="flex items-center gap-2">
            {step === "create" ? (
              <>
                <IconShare size={20} />
                {t("shareMultipleFiles.title")}
              </>
            ) : (
              <>
                <IconLink size={20} />
                {t("shareActions.linkTitle")}
              </>
            )}
          </DialogTitle>
        </DialogHeader>

        <div className="flex-1 overflow-auto">
          {step === "create" && (
            <div className="space-y-4">
              <div className="space-y-2">
                <Label>{t("shareMultipleFiles.shareNameLabel")} *</Label>
                <Input
                  value={formData.name}
                  onChange={(e) => setFormData({ ...formData, name: e.target.value })}
                  placeholder={t("shareMultipleFiles.shareNamePlaceholder")}
                  required
                />
              </div>

              <div className="space-y-2">
                <Label>{t("shareMultipleFiles.descriptionLabel")}</Label>
                <Input
                  value={formData.description}
                  onChange={(e) => setFormData({ ...formData, description: e.target.value })}
                  placeholder={t("shareMultipleFiles.descriptionPlaceholder")}
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

              <div className="space-y-2">
                <Label>{t("shareMultipleFiles.itemsToShare", { count: allItems.length })}</Label>
                <ScrollArea className="h-32 w-full rounded-md border p-2 bg-muted/30">
                  <div className="space-y-1">
                    {allItems.map((item) => {
                      const isFolder = item.type === "folder";
                      const { icon: FileIcon, color } = isFolder
                        ? { icon: IconFolder, color: "text-primary" }
                        : getFileIcon(item.name);

                      return (
                        <div key={item.id} className="flex justify-between items-center text-sm">
                          <div className="flex items-center gap-2 truncate flex-1">
                            <FileIcon className={`h-4 w-4 ${color} flex-shrink-0`} />
                            <span className="truncate">{item.name}</span>
                          </div>
                          <span className="text-muted-foreground ml-2">
                            {item.size ? formatFileSize(item.size) : "—"}
                          </span>
                        </div>
                      );
                    })}
                  </div>
                </ScrollArea>
                <p className="text-xs text-muted-foreground">
                  Total size: {formatFileSize(totalSize)} ({filesList.length} files, {foldersList.length} folders)
                </p>
              </div>
            </div>
          )}

          {step === "link" && (
            <div className="space-y-4">
              {!generatedLink ? (
                <>
                  <p className="text-sm text-muted-foreground">{t("shareActions.linkDescriptionFile")}</p>
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
                        id="share-multiple-files-qr-code"
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
                    <Button variant="outline" size="icon" onClick={handleCopyLink} title={t("shareActions.copyLink")}>
                      <IconCopy className="h-4 w-4" />
                    </Button>
                  </div>
                </>
              )}
            </div>
          )}
        </div>

        <DialogFooter>
          {step === "create" && (
            <>
              <Button variant="outline" onClick={handleClose}>
                {t("common.cancel")}
              </Button>
              <Button
                disabled={
                  isLoading || !formData.name.trim() || (formData.isPasswordProtected && !formData.password.trim())
                }
                onClick={handleCreateShare}
              >
                {isLoading ? <div className="animate-spin">⠋</div> : t("shareMultipleFiles.create")}
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
