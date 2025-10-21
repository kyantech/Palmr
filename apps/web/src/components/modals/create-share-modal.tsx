"use client";

import { useCallback, useEffect, useState } from "react";
import { IconCalendar, IconEye, IconLock, IconShare, IconUpload, IconX } from "@tabler/icons-react";
import { useTranslations } from "next-intl";
import { useDropzone } from "react-dropzone";
import { toast } from "sonner";

import { FileTree, TreeFile, TreeFolder } from "@/components/tables/files-tree";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Switch } from "@/components/ui/switch";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { Textarea } from "@/components/ui/textarea";
import { createShare, createShareWithFiles } from "@/http/endpoints";
import { formatFileSize } from "@/utils/format-file-size";

interface CreateShareModalProps {
  isOpen: boolean;
  onClose: () => void;
  onSuccess: () => void;
  getAllFilesAndFolders: () => Promise<{ files: any[]; folders: any[] }>;
}

export function CreateShareModal({ isOpen, onClose, onSuccess, getAllFilesAndFolders }: CreateShareModalProps) {
  const t = useTranslations();
  const [currentTab, setCurrentTab] = useState("details");

  const [formData, setFormData] = useState({
    name: "",
    description: "",
    password: "",
    expiresAt: "",
    isPasswordProtected: false,
    maxViews: "",
  });

  const [selectedItems, setSelectedItems] = useState<string[]>([]);
  const [files, setFiles] = useState<TreeFile[]>([]);
  const [folders, setFolders] = useState<TreeFolder[]>([]);
  const [searchQuery, setSearchQuery] = useState("");
  const [newFiles, setNewFiles] = useState<File[]>([]);

  const [isLoading, setIsLoading] = useState(false);
  const [isLoadingData, setIsLoadingData] = useState(false);

  const loadData = useCallback(async () => {
    try {
      setIsLoadingData(true);
      const data = await getAllFilesAndFolders();

      const treeFiles: TreeFile[] = data.files.map((file) => ({
        id: file.id,
        name: file.name,
        type: "file" as const,
        size: file.size,
        parentId: file.folderId || null,
      }));

      const treeFolders: TreeFolder[] = data.folders.map((folder) => ({
        id: folder.id,
        name: folder.name,
        type: "folder" as const,
        parentId: folder.parentId || null,
        totalSize: folder.totalSize,
      }));

      setFiles(treeFiles);
      setFolders(treeFolders);
    } catch (error) {
      console.error("Error loading files and folders:", error);
    } finally {
      setIsLoadingData(false);
    }
  }, [getAllFilesAndFolders]);

  useEffect(() => {
    if (isOpen) {
      loadData();
      setFormData({
        name: "",
        description: "",
        password: "",
        expiresAt: "",
        isPasswordProtected: false,
        maxViews: "",
      });
      setSelectedItems([]);
      setNewFiles([]);
      setCurrentTab("details");
    }
  }, [isOpen, loadData]);

  const onDrop = useCallback((acceptedFiles: File[]) => {
    setNewFiles((prev) => [...prev, ...acceptedFiles]);
  }, []);

  const { getRootProps, getInputProps, isDragActive } = useDropzone({
    onDrop,
    multiple: true,
  });

  const removeNewFile = (index: number) => {
    setNewFiles((prev) => prev.filter((_, i) => i !== index));
  };

  const handleSubmit = async () => {
    if (!formData.name.trim()) {
      toast.error("Share name is required");
      return;
    }

    const hasExistingItems = selectedItems.length > 0;
    const hasNewFiles = newFiles.length > 0;

    if (!hasExistingItems && !hasNewFiles) {
      toast.error("Please select at least one file/folder or upload new files");
      return;
    }

    try {
      setIsLoading(true);

      const selectedFiles = selectedItems.filter((id) => files.some((file) => file.id === id));
      const selectedFolders = selectedItems.filter((id) => folders.some((folder) => folder.id === id));

      const expiration = formData.expiresAt
        ? (() => {
            const dateValue = formData.expiresAt;
            if (dateValue.length === 10) {
              return new Date(dateValue + "T23:59:59").toISOString();
            }
            return new Date(dateValue).toISOString();
          })()
        : undefined;

      // Use the new endpoint if there are new files to upload
      if (hasNewFiles) {
        await createShareWithFiles({
          name: formData.name,
          description: formData.description || undefined,
          password: formData.isPasswordProtected ? formData.password : undefined,
          expiration: expiration,
          maxViews: formData.maxViews ? parseInt(formData.maxViews) : undefined,
          existingFiles: selectedFiles.length > 0 ? selectedFiles : undefined,
          existingFolders: selectedFolders.length > 0 ? selectedFolders : undefined,
          newFiles: newFiles,
        });
      } else {
        // Use the traditional endpoint if only selecting existing files
        await createShare({
          name: formData.name,
          description: formData.description || undefined,
          password: formData.isPasswordProtected ? formData.password : undefined,
          expiration: expiration,
          maxViews: formData.maxViews ? parseInt(formData.maxViews) : undefined,
          files: selectedFiles,
          folders: selectedFolders,
        });
      }

      toast.success(t("createShare.success"));
      onSuccess();
      onClose();
    } catch (error) {
      console.error("Error creating share:", error);
      toast.error(t("createShare.error"));
    } finally {
      setIsLoading(false);
    }
  };

  const handleClose = () => {
    if (!isLoading) {
      onClose();
    }
  };

  const updateFormData = (field: keyof typeof formData, value: string | boolean) => {
    setFormData((prev) => ({ ...prev, [field]: value }));
  };

  const selectedCount = selectedItems.length;
  const newFilesCount = newFiles.length;
  const totalCount = selectedCount + newFilesCount;
  const canProceedToFiles = formData.name.trim().length > 0;
  const canSubmit = formData.name.trim().length > 0 && totalCount > 0;

  return (
    <Dialog open={isOpen} onOpenChange={handleClose}>
      <DialogContent className="max-w-4xl max-h-[90vh] w-full">
        <DialogHeader>
          <DialogTitle className="flex items-center gap-2">
            <IconShare className="h-5 w-5" />
            {t("createShare.title")}
          </DialogTitle>
        </DialogHeader>

        <div className="flex flex-col gap-6 flex-1 min-h-0 w-full overflow-hidden">
          <Tabs value={currentTab} onValueChange={setCurrentTab} className="flex-1">
            <TabsList className="grid w-full grid-cols-3">
              <TabsTrigger value="details">{t("createShare.tabs.shareDetails")}</TabsTrigger>
              <TabsTrigger value="files" disabled={!canProceedToFiles}>
                {t("createShare.tabs.selectFiles")}
                {selectedCount > 0 && (
                  <span className="ml-1 text-xs bg-primary text-primary-foreground rounded-full px-2 py-0.5">
                    {selectedCount}
                  </span>
                )}
              </TabsTrigger>
              <TabsTrigger value="upload" disabled={!canProceedToFiles}>
                {t("createShare.tabs.uploadFiles") || "Upload Files"}
                {newFilesCount > 0 && (
                  <span className="ml-1 text-xs bg-primary text-primary-foreground rounded-full px-2 py-0.5">
                    {newFilesCount}
                  </span>
                )}
              </TabsTrigger>
            </TabsList>

            <TabsContent value="details" className="space-y-4 mt-4">
              <div className="space-y-2">
                <Label htmlFor="share-name">{t("createShare.nameLabel")} *</Label>
                <Input
                  id="share-name"
                  value={formData.name}
                  onChange={(e) => updateFormData("name", e.target.value)}
                  placeholder={t("createShare.namePlaceholder")}
                  required
                />
              </div>

              <div className="space-y-2">
                <Label htmlFor="share-description">{t("createShare.descriptionLabel")}</Label>
                <Textarea
                  id="share-description"
                  value={formData.description}
                  onChange={(e) => updateFormData("description", e.target.value)}
                  placeholder={t("createShare.descriptionPlaceholder")}
                  rows={3}
                />
              </div>

              <div className="flex items-center space-x-2">
                <Switch
                  id="password-protection"
                  checked={formData.isPasswordProtected}
                  onCheckedChange={(checked) => updateFormData("isPasswordProtected", checked)}
                />
                <Label htmlFor="password-protection" className="flex items-center gap-2">
                  <IconLock className="h-4 w-4" />
                  {t("createShare.passwordProtection")}
                </Label>
              </div>

              {formData.isPasswordProtected && (
                <div className="space-y-2">
                  <Label htmlFor="share-password">{t("createShare.passwordLabel")}</Label>
                  <Input
                    id="share-password"
                    type="password"
                    value={formData.password}
                    onChange={(e) => updateFormData("password", e.target.value)}
                    placeholder="Enter password"
                  />
                </div>
              )}

              <div className="space-y-2">
                <Label htmlFor="expiration" className="flex items-center gap-2">
                  <IconCalendar className="h-4 w-4" />
                  {t("createShare.expirationLabel")}
                </Label>
                <Input
                  id="expiration"
                  type="datetime-local"
                  value={formData.expiresAt}
                  onChange={(e) => updateFormData("expiresAt", e.target.value)}
                />
              </div>

              <div className="space-y-2">
                <Label htmlFor="max-views" className="flex items-center gap-2">
                  <IconEye className="h-4 w-4" />
                  {t("createShare.maxViewsLabel")}
                </Label>
                <Input
                  id="max-views"
                  type="number"
                  min="1"
                  value={formData.maxViews}
                  onChange={(e) => updateFormData("maxViews", e.target.value)}
                  placeholder={t("createShare.maxViewsPlaceholder")}
                />
              </div>

              <div className="flex justify-end">
                <Button onClick={() => setCurrentTab("files")} disabled={!canProceedToFiles}>
                  {t("createShare.nextSelectFiles")}
                </Button>
              </div>
            </TabsContent>

            <TabsContent value="files" className="space-y-4 mt-4 flex-1 min-h-0">
              <div className="space-y-2">
                <Label htmlFor="file-search">{t("common.search")}</Label>
                <Input
                  id="file-search"
                  type="search"
                  placeholder={t("searchBar.placeholder")}
                  value={searchQuery}
                  onChange={(e) => setSearchQuery(e.target.value)}
                  disabled={isLoadingData}
                />
              </div>

              <div className="text-sm text-muted-foreground">
                {selectedCount > 0 ? (
                  <span>{selectedCount} items selected</span>
                ) : (
                  <span>Select files and folders to share</span>
                )}
              </div>

              <div className="flex-1 min-h-0 w-full overflow-hidden">
                {isLoadingData ? (
                  <div className="flex items-center justify-center py-8">
                    <div className="text-sm text-muted-foreground">{t("common.loadingSimple")}</div>
                  </div>
                ) : (
                  <FileTree
                    files={files.map((file) => ({
                      id: file.id,
                      name: file.name,
                      description: "",
                      extension: "",
                      size: file.size?.toString() || "0",
                      objectName: "",
                      userId: "",
                      folderId: file.parentId,
                      createdAt: "",
                      updatedAt: "",
                    }))}
                    folders={folders.map((folder) => ({
                      id: folder.id,
                      name: folder.name,
                      description: "",
                      parentId: folder.parentId,
                      userId: "",
                      createdAt: "",
                      updatedAt: "",
                      totalSize: folder.totalSize,
                    }))}
                    selectedItems={selectedItems}
                    onSelectionChange={setSelectedItems}
                    showFiles={true}
                    showFolders={true}
                    maxHeight="400px"
                    searchQuery={searchQuery}
                  />
                )}
              </div>

              <div className="flex justify-between">
                <Button variant="outline" onClick={() => setCurrentTab("details")}>
                  {t("common.back")}
                </Button>
                <div className="space-x-2">
                  <Button variant="outline" onClick={() => setCurrentTab("upload")}>
                    {t("createShare.nextUploadFiles") || "Upload New Files"}
                  </Button>
                  <Button onClick={handleSubmit} disabled={!canSubmit || isLoading}>
                    {isLoading ? t("common.creating") : t("createShare.create")}
                  </Button>
                </div>
              </div>
            </TabsContent>

            <TabsContent value="upload" className="space-y-4 mt-4 flex-1 min-h-0">
              <div className="space-y-4">
                <div
                  {...getRootProps()}
                  className={`border-2 border-dashed rounded-lg p-8 text-center cursor-pointer transition-colors ${
                    isDragActive ? "border-primary bg-primary/5" : "border-muted-foreground/25 hover:border-primary/50"
                  }`}
                >
                  <input {...getInputProps()} />
                  <IconUpload className="h-12 w-12 mx-auto mb-4 text-muted-foreground" />
                  {isDragActive ? (
                    <p className="text-sm text-muted-foreground">Drop files here...</p>
                  ) : (
                    <div>
                      <p className="text-sm font-medium mb-1">
                        {t("createShare.upload.dragDrop") || "Drag & drop files here"}
                      </p>
                      <p className="text-xs text-muted-foreground">
                        {t("createShare.upload.orClick") || "or click to browse"}
                      </p>
                    </div>
                  )}
                </div>

                {newFiles.length > 0 && (
                  <div className="space-y-2">
                    <Label>{t("createShare.upload.selectedFiles") || "Selected Files"}</Label>
                    <div className="max-h-[200px] overflow-y-auto space-y-2">
                      {newFiles.map((file, index) => (
                        <div
                          key={index}
                          className="flex items-center justify-between p-2 bg-muted/50 rounded-md text-sm"
                        >
                          <div className="flex items-center gap-2 flex-1 min-w-0">
                            <IconUpload className="h-4 w-4 flex-shrink-0" />
                            <span className="truncate">{file.name}</span>
                            <span className="text-xs text-muted-foreground flex-shrink-0">
                              {formatFileSize(file.size)}
                            </span>
                          </div>
                          <Button
                            variant="ghost"
                            size="sm"
                            onClick={() => removeNewFile(index)}
                            className="ml-2 h-6 w-6 p-0"
                          >
                            <IconX className="h-4 w-4" />
                          </Button>
                        </div>
                      ))}
                    </div>
                  </div>
                )}

                <div className="text-sm text-muted-foreground">
                  {totalCount > 0 ? (
                    <span>
                      {totalCount} {totalCount === 1 ? "item" : "items"} selected ({selectedCount} existing,{" "}
                      {newFilesCount} new)
                    </span>
                  ) : (
                    <span>No items selected</span>
                  )}
                </div>
              </div>

              <div className="flex justify-between pt-4">
                <Button variant="outline" onClick={() => setCurrentTab("files")}>
                  {t("common.back")}
                </Button>
                <div className="space-x-2">
                  <Button variant="outline" onClick={handleClose}>
                    {t("common.cancel")}
                  </Button>
                  <Button onClick={handleSubmit} disabled={!canSubmit || isLoading}>
                    {isLoading ? t("common.creating") : t("createShare.create")}
                  </Button>
                </div>
              </div>
            </TabsContent>
          </Tabs>
        </div>
      </DialogContent>
    </Dialog>
  );
}
