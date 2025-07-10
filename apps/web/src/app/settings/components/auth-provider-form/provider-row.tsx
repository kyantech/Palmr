"use client";

import React from "react";
import { IconEdit, IconGripVertical, IconTrash } from "@tabler/icons-react";
import { useTranslations } from "next-intl";

import { Button } from "@/components/ui/button";
import { Switch } from "@/components/ui/switch";
import { AuthProvider, EditProviderForm } from "./edit-provider-form";

interface ProviderRowProps {
  provider: AuthProvider;
  onUpdate: (updates: Partial<AuthProvider>) => void;
  onEdit: () => void;
  onDelete: () => void;
  saving: boolean;
  getIcon: (provider: AuthProvider) => React.ReactNode;
  editingProvider: AuthProvider | null;
  editProvider: (data: Partial<AuthProvider>) => void;
  onCancelEdit: () => void;
  editingFormData: Record<string, any>;
  setEditingFormData: (data: Record<string, any>) => void;
  dragHandleProps: any;
  isDragging: boolean;
  isDragDisabled: boolean;
}

export function ProviderRow({
  provider,
  onUpdate,
  onEdit,
  onDelete,
  saving,
  getIcon,
  editingProvider,
  editProvider,
  onCancelEdit,
  editingFormData,
  setEditingFormData,
  dragHandleProps,
  isDragging,
  isDragDisabled,
}: ProviderRowProps) {
  const t = useTranslations();
  const isEditing = editingProvider?.id === provider.id;

  return (
    <div className={`border rounded-lg ${isDragging ? "border-blue-300 bg-blue-50 dark:bg-blue-950/20" : ""}`}>
      <div className="flex items-center justify-between p-3">
        <div className="flex items-center gap-3">
          {!isDragDisabled ? (
            <div
              {...dragHandleProps}
              className="cursor-grab active:cursor-grabbing text-muted-foreground hover:text-foreground transition-colors"
              title={t("authProviders.dragToReorder")}
            >
              <IconGripVertical className="h-4 w-4" />
            </div>
          ) : null}

          <span className="text-lg">{getIcon(provider)}</span>
          <div>
            <div className="flex items-center gap-2">
              <div
                className={`w-2 h-2 rounded-full ${provider.enabled ? "bg-green-500" : "bg-gray-400"}`}
                title={provider.enabled ? t("authProviders.enabled") : t("authProviders.disabled")}
              />
              <span className="font-medium text-sm">{provider.displayName}</span>
            </div>
            <div className="text-xs text-muted-foreground">
              {provider.type.toUpperCase()} • {provider.name}
              {provider.isOfficial && (
                <span className="text-blue-600 dark:text-blue-400"> • {t("authProviders.officialProvider")}</span>
              )}
            </div>
          </div>
        </div>
        <div className="flex items-center gap-1">
          <Switch checked={provider.enabled} onCheckedChange={(enabled) => onUpdate({ enabled })} disabled={saving} />
          <Button variant="ghost" size="sm" onClick={onEdit} disabled={saving} title={t("authProviders.editProvider")}>
            <IconEdit className="h-3 w-3" />
          </Button>
          {!provider.isOfficial && (
            <Button
              variant="ghost"
              size="sm"
              onClick={onDelete}
              disabled={saving}
              className="text-red-600 dark:text-red-400 hover:text-red-700 dark:hover:text-red-300 hover:bg-red-50 dark:hover:bg-red-900/20"
              title={t("authProviders.deleteProvider")}
            >
              <IconTrash className="h-3 w-3" />
            </Button>
          )}
        </div>
      </div>

      {isEditing && (
        <div className="border-t border-border dark:border-border p-4 space-y-4 bg-muted/50 dark:bg-muted/20">
          <div className="flex items-center justify-between">
            <h3 className="font-medium text-foreground dark:text-foreground">{provider.displayName}</h3>
          </div>
          <EditProviderForm
            key={provider.id}
            provider={provider}
            onSave={editProvider}
            onCancel={onCancelEdit}
            saving={saving}
            editingFormData={editingFormData}
            setEditingFormData={setEditingFormData}
          />
        </div>
      )}
    </div>
  );
}
