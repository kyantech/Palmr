"use client";

import React, { useState } from "react";
import { IconChevronDown, IconChevronUp, IconSettings } from "@tabler/icons-react";
import { useTranslations } from "next-intl";

import { Badge } from "@/components/ui/badge";
import { Card, CardContent, CardHeader } from "@/components/ui/card";
import { Checkbox } from "@/components/ui/checkbox";
import { renderIconByName } from "@/components/ui/icon-picker";
import { Label } from "@/components/ui/label";
import { Separator } from "@/components/ui/separator";
import { useAuthProviders } from "../../hooks/use-auth-providers";
import { AddProviderForm } from "./add-provider-form";
import { AuthProviderDeleteModal } from "./auth-provider-delete-modal";
import { ProviderList } from "./provider-list";

export function AuthProvidersSettings() {
  const t = useTranslations();

  const [isCollapsed, setIsCollapsed] = useState(true);
  const [showAddForm, setShowAddForm] = useState(false);

  const {
    providers,
    loading,
    saving,
    editingProvider,
    editingFormData,
    hideDisabledProviders,
    providerToDelete,
    isDeleting,
    enabledCount,
    filteredProviders,
    updateProvider,
    addProvider,
    editProvider,
    deleteProvider,
    handleDragEnd,
    handleHideDisabledProvidersChange,
    handleEditProvider,
    handleDeleteProvider,
    handleCancelEdit,
    setEditingFormData,
    setProviderToDelete,
  } = useAuthProviders();

  const getProviderIcon = (provider: any) => {
    const iconName = provider.icon || "FaCog";
    return renderIconByName(iconName, "w-5 h-5");
  };

  const handleToggleAddForm = () => {
    setShowAddForm(!showAddForm);
  };

  const handleConfirmDelete = async () => {
    if (providerToDelete) {
      await deleteProvider(providerToDelete.id);
    }
  };

  return (
    <Card className="p-6 gap-0">
      <CardHeader
        className="flex flex-row items-center justify-between cursor-pointer p-0"
        onClick={() => setIsCollapsed(!isCollapsed)}
      >
        <div className="flex flex-row items-center gap-8">
          <IconSettings className="text-xl text-muted-foreground" />
          <div className="flex flex-col gap-1">
            <h2 className="text-xl font-semibold">{t("authProviders.title")}</h2>
            <p className="text-sm text-muted-foreground">
              {t("authProviders.description")}
              {enabledCount > 0 && (
                <Badge variant="secondary" className="ml-2">
                  {t("authProviders.enabledCount", { count: enabledCount })}
                </Badge>
              )}
            </p>
          </div>
        </div>
        {isCollapsed ? (
          <IconChevronDown className="text-muted-foreground" />
        ) : (
          <IconChevronUp className="text-muted-foreground" />
        )}
      </CardHeader>

      <CardContent className={`${isCollapsed ? "hidden" : "block"} px-0`}>
        <Separator className="my-6" />

        {loading ? (
          <div className="flex items-center justify-center py-8">
            <IconSettings className="h-6 w-6 animate-spin" />
            {t("authProviders.loadingProviders")}
          </div>
        ) : (
          <div className="space-y-4">
            <div className={showAddForm ? "space-y-4" : "flex justify-between items-center"}>
              <div className="text-sm text-muted-foreground">
                {hideDisabledProviders
                  ? t("authProviders.enabledOfTotal", { enabled: filteredProviders.length, total: providers.length })
                  : t("authProviders.providersConfigured", { count: providers.length })}
              </div>

              <AddProviderForm
                showAddForm={showAddForm}
                onToggleForm={handleToggleAddForm}
                onAddProvider={addProvider}
                saving={saving === "new"}
              />
            </div>

            {providers.length > 0 && (
              <div className="flex items-center space-x-2 py-2">
                <Checkbox
                  id="hideDisabledProviders"
                  checked={hideDisabledProviders}
                  onCheckedChange={handleHideDisabledProvidersChange}
                />
                <Label htmlFor="hideDisabledProviders" className="text-sm cursor-pointer">
                  {t("authProviders.hideDisabledProviders")}
                </Label>
              </div>
            )}

            <ProviderList
              providers={providers}
              filteredProviders={filteredProviders}
              hideDisabledProviders={hideDisabledProviders}
              onDragEnd={handleDragEnd}
              onUpdateProvider={updateProvider}
              onEditProvider={handleEditProvider}
              onDeleteProvider={handleDeleteProvider}
              saving={saving}
              getIcon={getProviderIcon}
              editingProvider={editingProvider}
              editProvider={editProvider}
              onCancelEdit={handleCancelEdit}
              editingFormData={editingFormData}
              setEditingFormData={setEditingFormData}
            />
          </div>
        )}

        <AuthProviderDeleteModal
          provider={providerToDelete}
          isOpen={!!providerToDelete}
          onConfirm={handleConfirmDelete}
          onClose={() => setProviderToDelete(null)}
          isDeleting={isDeleting}
        />
      </CardContent>
    </Card>
  );
}
