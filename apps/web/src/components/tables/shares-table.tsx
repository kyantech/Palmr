import { useEffect, useRef, useState } from "react";
import {
  IconCheck,
  IconChevronDown,
  IconCopy,
  IconDotsVertical,
  IconDownload,
  IconEdit,
  IconEye,
  IconFolder,
  IconLink,
  IconLock,
  IconLockOpen,
  IconMail,
  IconQrcode,
  IconTrash,
  IconUsers,
  IconX,
} from "@tabler/icons-react";
import { format } from "date-fns";
import { useTranslations } from "next-intl";

import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { Input } from "@/components/ui/input";
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "@/components/ui/table";
import { useShareContext } from "../../contexts/share-context";

export interface SharesTableProps {
  shares: any[];
  onDelete: (share: any) => void;
  onEdit: (share: any) => void;
  onUpdateName: (shareId: string, newName: string) => void;
  onUpdateDescription: (shareId: string, newDescription: string) => void;
  onUpdateSecurity?: (share: any) => void;
  onUpdateExpiration?: (share: any) => void;
  onManageFiles: (share: any) => void;
  onManageRecipients: (share: any) => void;
  onViewDetails: (share: any) => void;
  onGenerateLink: (share: any) => void;
  onCopyLink: (share: any) => void;
  onNotifyRecipients: (share: any) => void;
  onViewQrCode?: (share: any) => void;
  onDownloadShareFiles?: (share: any) => void;
  onBulkDelete?: (shares: any[]) => void;
  onBulkDownload?: (shares: any[]) => void;
  setClearSelectionCallback?: (callback: () => void) => void;
}

export function SharesTable({
  shares,
  onDelete,
  onEdit,
  onUpdateName,
  onUpdateDescription,
  onUpdateSecurity,
  onUpdateExpiration,
  onManageFiles,
  onManageRecipients,
  onViewDetails,
  onGenerateLink,
  onCopyLink,
  onNotifyRecipients,
  onViewQrCode,
  onDownloadShareFiles,
  onBulkDelete,
  onBulkDownload,
  setClearSelectionCallback,
}: SharesTableProps) {
  const t = useTranslations();
  const { smtpEnabled } = useShareContext();
  const [editingField, setEditingField] = useState<{ shareId: string; field: "name" | "description" } | null>(null);
  const [editValue, setEditValue] = useState("");
  const [hoveredField, setHoveredField] = useState<{
    shareId: string;
    field: "name" | "description" | "security" | "expiration" | "files" | "recipients";
  } | null>(null);
  const [pendingChanges, setPendingChanges] = useState<{ [shareId: string]: { name?: string; description?: string } }>(
    {}
  );
  const [selectedShares, setSelectedShares] = useState<Set<string>>(new Set());
  const inputRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    if (editingField && inputRef.current) {
      inputRef.current.focus();
      inputRef.current.select();
    }
  }, [editingField]);

  useEffect(() => {
    setPendingChanges({});
  }, [shares]);

  useEffect(() => {
    setSelectedShares(new Set());
  }, [shares]);

  useEffect(() => {
    const clearSelection = () => setSelectedShares(new Set());
    setClearSelectionCallback?.(clearSelection);
  }, [setClearSelectionCallback]);

  const startEdit = (shareId: string, field: "name" | "description", currentValue: string) => {
    setEditingField({ shareId, field });
    setEditValue(currentValue || "");
  };

  const saveEdit = () => {
    if (!editingField) return;

    const { shareId, field } = editingField;

    setPendingChanges((prev) => ({
      ...prev,
      [shareId]: { ...prev[shareId], [field]: editValue },
    }));

    if (field === "name") {
      onUpdateName(shareId, editValue);
    } else {
      onUpdateDescription(shareId, editValue);
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

  const getDisplayValue = (share: any, field: "name" | "description") => {
    const pendingChange = pendingChanges[share.id];
    if (pendingChange && pendingChange[field] !== undefined) {
      return pendingChange[field];
    }
    return field === "name" ? share.name : share.description;
  };

  const handleSelectAll = (checked: boolean) => {
    if (checked) {
      setSelectedShares(new Set(shares.map((share) => share.id)));
    } else {
      setSelectedShares(new Set());
    }
  };

  const handleSelectShare = (shareId: string, checked: boolean) => {
    const newSelected = new Set(selectedShares);
    if (checked) {
      newSelected.add(shareId);
    } else {
      newSelected.delete(shareId);
    }
    setSelectedShares(newSelected);
  };

  const getSelectedShares = () => {
    return shares.filter((share) => selectedShares.has(share.id));
  };

  const isAllSelected = shares.length > 0 && selectedShares.size === shares.length;

  const handleBulkDelete = () => {
    const selectedShareObjects = getSelectedShares();

    if (selectedShareObjects.length === 0) return;

    if (onBulkDelete) {
      onBulkDelete(selectedShareObjects);
    }
  };

  const handleBulkDownload = () => {
    const selectedShareObjects = getSelectedShares();

    if (selectedShareObjects.length === 0) return;

    if (onBulkDownload) {
      onBulkDownload(selectedShareObjects);
    }
  };

  const showBulkActions = selectedShares.size > 0 && (onBulkDelete || onBulkDownload);

  return (
    <div className="space-y-4">
      {showBulkActions && (
        <div className="flex items-center justify-between p-4 bg-muted/30 border rounded-lg">
          <div className="flex items-center gap-3">
            <span className="text-sm font-medium text-foreground">
              {t("sharesTable.bulkActions.selected", { count: selectedShares.size })}
            </span>
          </div>
          <div className="flex items-center gap-2">
            <DropdownMenu>
              <DropdownMenuTrigger asChild>
                <Button variant="default" size="sm" className="gap-2">
                  {t("sharesTable.bulkActions.actions")}
                  <IconChevronDown className="h-4 w-4" />
                </Button>
              </DropdownMenuTrigger>
              <DropdownMenuContent align="end" className="w-[200px]">
                {onBulkDownload && (
                  <DropdownMenuItem className="cursor-pointer py-2" onClick={handleBulkDownload}>
                    <IconDownload className="h-4 w-4" />
                    {t("sharesTable.bulkActions.download")}
                  </DropdownMenuItem>
                )}
                {onBulkDelete && (
                  <DropdownMenuItem
                    onClick={handleBulkDelete}
                    className="cursor-pointer py-2 text-destructive focus:text-destructive"
                  >
                    <IconTrash className="h-4 w-4" />
                    {t("sharesTable.bulkActions.delete")}
                  </DropdownMenuItem>
                )}
              </DropdownMenuContent>
            </DropdownMenu>
            <Button variant="outline" size="sm" onClick={() => setSelectedShares(new Set())}>
              {t("common.cancel")}
            </Button>
          </div>
        </div>
      )}

      <div className="rounded-lg shadow-sm overflow-hidden border">
        <Table>
          <TableHeader>
            <TableRow className="border-b-0">
              <TableHead className="h-10 w-[50px] text-xs font-bold text-muted-foreground bg-muted/50 px-4 rounded-tl-lg">
                <Checkbox
                  checked={isAllSelected}
                  onCheckedChange={handleSelectAll}
                  aria-label={t("sharesTable.selectAll")}
                />
              </TableHead>
              <TableHead className="h-10 text-xs font-bold text-muted-foreground bg-muted/50 px-4">
                {t("sharesTable.columns.name")}
              </TableHead>
              <TableHead className="h-10 text-xs font-bold text-muted-foreground bg-muted/50 px-4">
                {t("sharesTable.columns.description")}
              </TableHead>
              <TableHead className="h-10 text-xs font-bold text-muted-foreground bg-muted/50 px-4">
                {t("sharesTable.columns.createdAt")}
              </TableHead>
              <TableHead className="h-10 text-xs font-bold text-muted-foreground bg-muted/50 px-4">
                {t("sharesTable.columns.expiresAt")}
              </TableHead>
              <TableHead className="h-10 text-xs font-bold text-muted-foreground bg-muted/50 px-4">
                {t("sharesTable.columns.status")}
              </TableHead>
              <TableHead className="h-10 text-xs font-bold text-muted-foreground bg-muted/50 px-4">
                {t("sharesTable.columns.security")}
              </TableHead>
              <TableHead className="h-10 text-xs font-bold text-muted-foreground bg-muted/50 px-4">
                {t("sharesTable.columns.files")}
              </TableHead>
              <TableHead className="h-10 text-xs font-bold text-muted-foreground bg-muted/50 px-4">
                {t("sharesTable.columns.recipients")}
              </TableHead>
              <TableHead className="h-10 w-[70px] text-xs font-bold text-muted-foreground bg-muted/50 px-4 rounded-tr-lg">
                {t("sharesTable.columns.actions")}
              </TableHead>
            </TableRow>
          </TableHeader>
          <TableBody>
            {shares.map((share) => {
              const isEditingName = editingField?.shareId === share.id && editingField?.field === "name";
              const isEditingDescription = editingField?.shareId === share.id && editingField?.field === "description";
              const isHoveringName = hoveredField?.shareId === share.id && hoveredField?.field === "name";
              const isHoveringDescription = hoveredField?.shareId === share.id && hoveredField?.field === "description";
              const isHoveringSecurity = hoveredField?.shareId === share.id && hoveredField?.field === "security";
              const isHoveringExpiration = hoveredField?.shareId === share.id && hoveredField?.field === "expiration";
              const isHoveringFiles = hoveredField?.shareId === share.id && hoveredField?.field === "files";
              const isHoveringRecipients = hoveredField?.shareId === share.id && hoveredField?.field === "recipients";
              const isSelected = selectedShares.has(share.id);
              const displayName = getDisplayValue(share, "name");
              const displayDescription = getDisplayValue(share, "description");

              return (
                <TableRow key={share.id} className="hover:bg-muted/50 transition-colors border-0">
                  <TableCell className="h-12 px-4 border-0">
                    <Checkbox
                      checked={isSelected}
                      onCheckedChange={(checked: boolean) => handleSelectShare(share.id, checked)}
                      aria-label={t("sharesTable.selectShare", { shareName: share.name })}
                    />
                  </TableCell>
                  <TableCell className="h-12 px-4 border-0">
                    <div
                      className="flex items-center gap-1 min-w-0"
                      onMouseEnter={() => setHoveredField({ shareId: share.id, field: "name" })}
                      onMouseLeave={() => setHoveredField(null)}
                    >
                      {isEditingName ? (
                        <div className="flex items-center gap-1 flex-1">
                          <Input
                            ref={inputRef}
                            value={editValue}
                            onChange={(e) => setEditValue(e.target.value)}
                            onKeyDown={handleKeyDown}
                            className="h-8 text-sm font-medium min-w-[200px]"
                            onClick={(e) => e.stopPropagation()}
                          />
                          <Button
                            size="icon"
                            variant="ghost"
                            className="h-8 w-8 text-green-600 hover:text-green-700 flex-shrink-0"
                            onClick={(e) => {
                              e.stopPropagation();
                              saveEdit();
                            }}
                          >
                            <IconCheck className="h-4 w-4" />
                          </Button>
                          <Button
                            size="icon"
                            variant="ghost"
                            className="h-8 w-8 text-red-600 hover:text-red-700 flex-shrink-0"
                            onClick={(e) => {
                              e.stopPropagation();
                              cancelEdit();
                            }}
                          >
                            <IconX className="h-4 w-4" />
                          </Button>
                        </div>
                      ) : (
                        <div className="flex items-center gap-1 flex-1 min-w-0">
                          <span className="truncate max-w-[120px] font-medium" title={displayName}>
                            {displayName}
                          </span>
                          <div className="w-6 flex justify-center flex-shrink-0">
                            {isHoveringName && (
                              <Button
                                size="icon"
                                variant="ghost"
                                className="h-6 w-6 text-muted-foreground hover:text-foreground hidden sm:block"
                                onClick={(e) => {
                                  e.stopPropagation();
                                  startEdit(share.id, "name", displayName);
                                }}
                              >
                                <IconEdit className="h-3 w-3" />
                              </Button>
                            )}
                          </div>
                        </div>
                      )}
                    </div>
                  </TableCell>
                  <TableCell className="h-12 px-4">
                    <div
                      className="flex items-center gap-1 min-w-0"
                      onMouseEnter={() => setHoveredField({ shareId: share.id, field: "description" })}
                      onMouseLeave={() => setHoveredField(null)}
                    >
                      {isEditingDescription ? (
                        <div className="flex items-center gap-1 flex-1">
                          <Input
                            ref={inputRef}
                            value={editValue}
                            onChange={(e) => setEditValue(e.target.value)}
                            onKeyDown={handleKeyDown}
                            className="h-8 text-sm min-w-[250px]"
                            placeholder={t("shareActions.addDescriptionPlaceholder")}
                            onClick={(e) => e.stopPropagation()}
                          />
                          <Button
                            size="icon"
                            variant="ghost"
                            className="h-8 w-8 text-green-600 hover:text-green-700 flex-shrink-0"
                            onClick={(e) => {
                              e.stopPropagation();
                              saveEdit();
                            }}
                          >
                            <IconCheck className="h-4 w-4" />
                          </Button>
                          <Button
                            size="icon"
                            variant="ghost"
                            className="h-8 w-8 text-red-600 hover:text-red-700 flex-shrink-0"
                            onClick={(e) => {
                              e.stopPropagation();
                              cancelEdit();
                            }}
                          >
                            <IconX className="h-4 w-4" />
                          </Button>
                        </div>
                      ) : (
                        <div className="flex items-center gap-1 flex-1 min-w-0">
                          <span
                            className="text-muted-foreground truncate max-w-[100px]"
                            title={displayDescription || "-"}
                          >
                            {displayDescription || "-"}
                          </span>
                          <div className="w-6 flex justify-center flex-shrink-0">
                            {isHoveringDescription && (
                              <Button
                                size="icon"
                                variant="ghost"
                                className="h-6 w-6 text-muted-foreground hover:text-foreground hidden sm:block"
                                onClick={(e) => {
                                  e.stopPropagation();
                                  startEdit(share.id, "description", displayDescription || "");
                                }}
                              >
                                <IconEdit className="h-3 w-3" />
                              </Button>
                            )}
                          </div>
                        </div>
                      )}
                    </div>
                  </TableCell>
                  <TableCell className="h-12 px-4">{format(new Date(share.createdAt), "MM/dd/yyyy HH:mm")}</TableCell>
                  <TableCell className="h-12 px-4">
                    <div
                      className="flex items-center gap-1 min-w-0"
                      onMouseEnter={() => setHoveredField({ shareId: share.id, field: "expiration" })}
                      onMouseLeave={() => setHoveredField(null)}
                    >
                      <span className="text-sm">
                        {share.expiration
                          ? format(new Date(share.expiration), "MM/dd/yyyy HH:mm")
                          : t("sharesTable.never")}
                      </span>
                      <div className="w-6 flex justify-center flex-shrink-0">
                        {isHoveringExpiration && onUpdateExpiration && (
                          <Button
                            size="icon"
                            variant="ghost"
                            className="h-6 w-6 text-muted-foreground hover:text-foreground hidden sm:block"
                            onClick={(e) => {
                              e.stopPropagation();
                              onUpdateExpiration(share);
                            }}
                          >
                            <IconEdit className="h-3 w-3" />
                          </Button>
                        )}
                      </div>
                    </div>
                  </TableCell>
                  <TableCell className="h-12 px-4">
                    <Badge
                      variant="secondary"
                      className={
                        !share.expiration || new Date(share.expiration) > new Date()
                          ? "bg-green-500/20 hover:bg-green-500/30 text-green-500"
                          : "bg-red-500/20 hover:bg-red-500/30 text-red-500"
                      }
                    >
                      {!share.expiration
                        ? t("sharesTable.status.neverExpires")
                        : new Date(share.expiration) > new Date()
                          ? t("sharesTable.status.active")
                          : t("sharesTable.status.expired")}
                    </Badge>
                  </TableCell>
                  <TableCell className="h-12 px-4">
                    <div
                      className="flex items-center gap-1 min-w-0"
                      onMouseEnter={() => setHoveredField({ shareId: share.id, field: "security" })}
                      onMouseLeave={() => setHoveredField(null)}
                    >
                      <Badge
                        variant="secondary"
                        className={`flex items-center gap-1 ${
                          share.security.hasPassword
                            ? "bg-yellow-500/20 hover:bg-yellow-500/30 text-yellow-500"
                            : "bg-green-500/20 hover:bg-green-500/30 text-green-500"
                        }`}
                      >
                        {share.security.hasPassword ? (
                          <IconLock className="h-4 w-4" />
                        ) : (
                          <IconLockOpen className="h-4 w-4" />
                        )}
                        {share.security.hasPassword
                          ? t("sharesTable.security.protected")
                          : t("sharesTable.security.public")}
                      </Badge>
                      <div className="w-6 flex justify-center flex-shrink-0">
                        {isHoveringSecurity && onUpdateSecurity && (
                          <Button
                            size="icon"
                            variant="ghost"
                            className="h-6 w-6 text-muted-foreground hover:text-foreground hidden sm:block"
                            onClick={(e) => {
                              e.stopPropagation();
                              onUpdateSecurity(share);
                            }}
                          >
                            <IconEdit className="h-3 w-3" />
                          </Button>
                        )}
                      </div>
                    </div>
                  </TableCell>
                  <TableCell className="h-12 px-4">
                    <div
                      className="flex items-center gap-1 min-w-0"
                      onMouseEnter={() => setHoveredField({ shareId: share.id, field: "files" })}
                      onMouseLeave={() => setHoveredField(null)}
                    >
                      <span className="text-sm">
                        {share.files?.length || 0} {t("sharesTable.filesCount")}
                      </span>
                      <div className="w-6 flex justify-center flex-shrink-0">
                        {isHoveringFiles && onManageFiles && (
                          <Button
                            size="icon"
                            variant="ghost"
                            className="h-6 w-6 text-muted-foreground hover:text-foreground hidden sm:block"
                            onClick={(e) => {
                              e.stopPropagation();
                              onManageFiles(share);
                            }}
                          >
                            <IconEdit className="h-3 w-3" />
                          </Button>
                        )}
                      </div>
                    </div>
                  </TableCell>
                  <TableCell className="h-12 px-4">
                    <div
                      className="flex items-center gap-1 min-w-0"
                      onMouseEnter={() => setHoveredField({ shareId: share.id, field: "recipients" })}
                      onMouseLeave={() => setHoveredField(null)}
                    >
                      <span className="text-sm">
                        {share.recipients?.length || 0} {t("sharesTable.recipientsCount")}
                      </span>
                      <div className="w-6 flex justify-center flex-shrink-0">
                        {isHoveringRecipients && onManageRecipients && (
                          <Button
                            size="icon"
                            variant="ghost"
                            className="h-6 w-6 text-muted-foreground hover:text-foreground hidden sm:block"
                            onClick={(e) => {
                              e.stopPropagation();
                              onManageRecipients(share);
                            }}
                          >
                            <IconEdit className="h-3 w-3" />
                          </Button>
                        )}
                      </div>
                    </div>
                  </TableCell>
                  <TableCell className="h-12 px-4 text-right">
                    <DropdownMenu>
                      <DropdownMenuTrigger asChild>
                        <Button variant="ghost" size="icon" className="h-8 w-8 hover:bg-muted cursor-pointer">
                          <IconDotsVertical className="h-4 w-4" />
                          <span className="sr-only">{t("sharesTable.actions.menu")}</span>
                        </Button>
                      </DropdownMenuTrigger>
                      <DropdownMenuContent align="end" className="w-[200px]">
                        <DropdownMenuItem className="cursor-pointer py-2" onClick={() => onEdit(share)}>
                          <IconEdit className="h-4 w-4" />
                          {t("sharesTable.actions.edit")}
                        </DropdownMenuItem>
                        <DropdownMenuItem className="cursor-pointer py-2" onClick={() => onManageFiles(share)}>
                          <IconFolder className="h-4 w-4" />
                          {t("sharesTable.actions.manageFiles")}
                        </DropdownMenuItem>
                        <DropdownMenuItem className="cursor-pointer py-2" onClick={() => onManageRecipients(share)}>
                          <IconUsers className="h-4 w-4" />
                          {t("sharesTable.actions.manageRecipients")}
                        </DropdownMenuItem>
                        <DropdownMenuItem className="cursor-pointer py-2" onClick={() => onViewDetails(share)}>
                          <IconEye className="h-4 w-4" />
                          {t("sharesTable.actions.viewDetails")}
                        </DropdownMenuItem>
                        <DropdownMenuItem className="cursor-pointer py-2" onClick={() => onGenerateLink(share)}>
                          <IconLink className="h-4 w-4" />
                          {share.alias ? t("sharesTable.actions.editLink") : t("sharesTable.actions.generateLink")}
                        </DropdownMenuItem>
                        {share.alias && (
                          <DropdownMenuItem className="cursor-pointer py-2" onClick={() => onCopyLink(share)}>
                            <IconCopy className="h-4 w-4" />
                            {t("sharesTable.actions.copyLink")}
                          </DropdownMenuItem>
                        )}
                        {share.alias && onViewQrCode && (
                          <DropdownMenuItem className="cursor-pointer py-2" onClick={() => onViewQrCode(share)}>
                            <IconQrcode className="h-4 w-4" />
                            {t("sharesTable.actions.viewQrCode", { defaultValue: "View QR Code" })}
                          </DropdownMenuItem>
                        )}
                        {share.recipients?.length > 0 && share.alias && smtpEnabled === "true" && (
                          <DropdownMenuItem className="cursor-pointer py-2" onClick={() => onNotifyRecipients(share)}>
                            <IconMail className="h-4 w-4" />
                            {t("sharesTable.actions.notifyRecipients")}
                          </DropdownMenuItem>
                        )}
                        {onDownloadShareFiles && share.files && share.files.length > 0 && (
                          <DropdownMenuItem
                            className="cursor-pointer py-2"
                            onClick={() => {
                              onDownloadShareFiles(share);
                            }}
                          >
                            <IconDownload className="h-4 w-4" />
                            {t("sharesTable.actions.downloadShareFiles")}
                          </DropdownMenuItem>
                        )}
                        <DropdownMenuItem
                          onClick={() => onDelete(share)}
                          className="cursor-pointer py-2 text-destructive focus:text-destructive"
                        >
                          <IconTrash className="h-4 w-4" />
                          {t("sharesTable.actions.delete")}
                        </DropdownMenuItem>
                      </DropdownMenuContent>
                    </DropdownMenu>
                  </TableCell>
                </TableRow>
              );
            })}
          </TableBody>
        </Table>
      </div>
    </div>
  );
}
