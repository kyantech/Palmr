"use client";

import { useCallback, useEffect, useState } from "react";
import { IconCalendar, IconEye, IconLock, IconShare } from "@tabler/icons-react";
import { useTranslations } from "next-intl";
import { toast } from "sonner";

import { FileTree, TreeFile, TreeFolder } from "@/components/tables/files-tree";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Switch } from "@/components/ui/switch";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { Textarea } from "@/components/ui/textarea";
import { createShare } from "@/http/endpoints";

interface CreateShareModalProps {
  isOpen: boolean;
  onClose: () => void;
  onSuccess: () => void;
  getAllFilesAndFolders: () => Promise<{ files: any[]; folders: any[] }>;
}

export function CreateShareModal({ isOpen, onClose, onSuccess, getAllFilesAndFolders }: CreateShareModalProps) {
  const t = useTranslations();
  const [currentTab, setCurrentTab] = useState("details");

  // Form data
  const [formData, setFormData] = useState({
    name: "",
    description: "",
    password: "",
    expiresAt: "",
    isPasswordProtected: false,
    maxViews: "",
  });

  // File selection
  const [selectedItems, setSelectedItems] = useState<string[]>([]);
  const [files, setFiles] = useState<TreeFile[]>([]);
  const [folders, setFolders] = useState<TreeFolder[]>([]);
  const [searchQuery, setSearchQuery] = useState("");

  const [isLoading, setIsLoading] = useState(false);
  const [isLoadingData, setIsLoadingData] = useState(false);

  const loadData = useCallback(async () => {
    try {
      setIsLoadingData(true);
      const data = await getAllFilesAndFolders();

      // Convert to tree format
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

  // Load files and folders when modal opens
  useEffect(() => {
    if (isOpen) {
      loadData();
      // Reset form
      setFormData({
        name: "",
        description: "",
        password: "",
        expiresAt: "",
        isPasswordProtected: false,
        maxViews: "",
      });
      setSelectedItems([]);
      setCurrentTab("details");
    }
  }, [isOpen, loadData]);

  const handleSubmit = async () => {
    if (!formData.name.trim()) {
      toast.error("Share name is required");
      return;
    }

    if (selectedItems.length === 0) {
      toast.error("Please select at least one file or folder");
      return;
    }

    try {
      setIsLoading(true);

      // Separate files and folders
      const selectedFiles = selectedItems.filter((id) => files.some((file) => file.id === id));
      const selectedFolders = selectedItems.filter((id) => folders.some((folder) => folder.id === id));

      await createShare({
        name: formData.name,
        description: formData.description || undefined,
        password: formData.isPasswordProtected ? formData.password : undefined,
        expiration: formData.expiresAt
          ? (() => {
              const dateValue = formData.expiresAt;
              if (dateValue.length === 10) {
                return new Date(dateValue + "T23:59:59").toISOString();
              }
              return new Date(dateValue).toISOString();
            })()
          : undefined,
        maxViews: formData.maxViews ? parseInt(formData.maxViews) : undefined,
        files: selectedFiles,
        folders: selectedFolders,
      });

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
  const canProceedToFiles = formData.name.trim().length > 0;
  const canSubmit = formData.name.trim().length > 0 && selectedCount > 0;

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
            <TabsList className="grid w-full grid-cols-2">
              <TabsTrigger value="details">{t("createShare.tabs.shareDetails")}</TabsTrigger>
              <TabsTrigger value="files" disabled={!canProceedToFiles}>
                {t("createShare.tabs.selectFiles")}
                {selectedCount > 0 && (
                  <span className="ml-1 text-xs bg-primary text-primary-foreground rounded-full px-2 py-0.5">
                    {selectedCount}
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
