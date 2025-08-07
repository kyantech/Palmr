"use client";

import { useCallback, useEffect, useState } from "react";
import { DropResult } from "@hello-pangea/dnd";
import { useTranslations } from "next-intl";
import { toast } from "sonner";

import {
  createProvider,
  deleteProvider as deleteProviderEndpoint,
  getAllProviders,
  updateProvider as updateProviderEndpoint,
  updateProvidersOrder as updateProvidersOrderEndpoint,
} from "@/http/endpoints";
import type { AuthProvider, NewProvider } from "@/http/endpoints/auth/types";

export function useAuthProviders() {
  const t = useTranslations();
  const [providers, setProviders] = useState<AuthProvider[]>([]);
  const [loading, setLoading] = useState(true);
  const [saving, setSaving] = useState<string | null>(null);
  const [editingProvider, setEditingProvider] = useState<AuthProvider | null>(null);
  const [editingFormData, setEditingFormData] = useState<Record<string, any>>({});
  const [hideDisabledProviders, setHideDisabledProviders] = useState<boolean>(false);
  const [providerToDelete, setProviderToDelete] = useState<{
    id: string;
    name: string;
    displayName: string;
  } | null>(null);
  const [isDeleting, setIsDeleting] = useState(false);

  useEffect(() => {
    const savedState = localStorage.getItem("hideDisabledProviders");
    if (savedState !== null) {
      setHideDisabledProviders(JSON.parse(savedState));
    }
  }, []);

  const loadProviders = useCallback(async () => {
    try {
      setLoading(true);
      const response = await getAllProviders();
      const data = response.data;

      if (data.success) {
        setProviders(data.data.sort((a: AuthProvider, b: AuthProvider) => a.sortOrder - b.sortOrder));
      } else {
        toast.error(t("authProviders.messages.loadFailed"));
      }
    } catch (error) {
      console.error("Error loading providers:", error);
      toast.error(t("authProviders.messages.loadFailed"));
    } finally {
      setLoading(false);
    }
  }, [t]);

  useEffect(() => {
    loadProviders();
  }, [loadProviders]);

  const updateProvider = async (id: string, updates: Partial<AuthProvider>) => {
    try {
      setSaving(id);
      const response = await updateProviderEndpoint(id, updates);
      const data = response.data;

      if (data.success) {
        setProviders((prev) => prev.map((p) => (p.id === id ? { ...p, ...updates } : p)));
        toast.success(t("authProviders.messages.providerUpdated"));
      } else {
        toast.error(t("authProviders.messages.updateFailed"));
      }
    } catch (error) {
      console.error("Error updating provider:", error);
      toast.error(t("authProviders.messages.updateFailed"));
    } finally {
      setSaving(null);
    }
  };

  const addProvider = async (newProvider: NewProvider) => {
    try {
      setSaving("new");
      const response = await createProvider({
        name: newProvider.name.toLowerCase().replace(/\s+/g, "-"),
        displayName: newProvider.displayName,
        type: newProvider.type,
        icon: newProvider.icon,
        clientId: newProvider.clientId,
        clientSecret: newProvider.clientSecret,
        scope: newProvider.scope || (newProvider.type === "oidc" ? "openid profile email" : "user:email"),
        ...(newProvider.issuerUrl ? { issuerUrl: newProvider.issuerUrl } : {}),
        ...(newProvider.authorizationEndpoint ? { authorizationEndpoint: newProvider.authorizationEndpoint } : {}),
        ...(newProvider.tokenEndpoint ? { tokenEndpoint: newProvider.tokenEndpoint } : {}),
        ...(newProvider.userInfoEndpoint ? { userInfoEndpoint: newProvider.userInfoEndpoint } : {}),
      });

      const data = response.data;

      if (data.success) {
        await loadProviders();
        toast.success(t("authProviders.messages.providerAdded"));
      } else {
        toast.error(t("authProviders.messages.addFailed"));
      }
    } catch (error) {
      console.error("Error adding provider:", error);
      toast.error(t("authProviders.messages.addFailed"));
    } finally {
      setSaving(null);
    }
  };

  const editProvider = async (providerData: Partial<AuthProvider>) => {
    if (!editingProvider || !providerData.name || !providerData.displayName) {
      toast.error(t("authProviders.messages.fillRequiredFields"));
      return;
    }

    try {
      setSaving(editingProvider.id);
      const response = await updateProviderEndpoint(editingProvider.id, {
        ...providerData,
        name: providerData.name?.toLowerCase().replace(/\s+/g, "-"),
      });

      const data = response.data;

      if (data.success) {
        await loadProviders();
        setEditingProvider(null);
        toast.success(t("authProviders.messages.providerUpdated"));
      } else {
        toast.error(t("authProviders.messages.updateFailed"));
      }
    } catch (error) {
      console.error("Error updating provider:", error);
      toast.error(t("authProviders.messages.updateFailed"));
    } finally {
      setSaving(null);
    }
  };

  const deleteProvider = async (id: string) => {
    try {
      setIsDeleting(true);
      const response = await deleteProviderEndpoint(id);
      const data = response.data;

      if (data.success) {
        setProviders((prev) => prev.filter((p) => p.id !== id));
        toast.success(t("authProviders.messages.providerDeleted"));
        setProviderToDelete(null);
      } else {
        toast.error(t("authProviders.messages.deleteFailed"));
      }
    } catch (error) {
      console.error("Error deleting provider:", error);
      toast.error(t("authProviders.messages.deleteFailed"));
    } finally {
      setIsDeleting(false);
    }
  };

  const handleDragEnd = async (result: DropResult) => {
    if (!result.destination) return;

    const items = Array.from(providers);
    const [reorderedItem] = items.splice(result.source.index, 1);
    items.splice(result.destination.index, 0, reorderedItem);

    const updatedItems = items.map((provider, index) => ({
      ...provider,
      sortOrder: index + 1,
    }));
    setProviders(updatedItems);

    const updatedProviders = updatedItems.map((provider) => ({
      id: provider.id,
      sortOrder: provider.sortOrder,
    }));

    try {
      const response = await updateProvidersOrderEndpoint({ providers: updatedProviders });
      const data = response.data;

      if (data.success) {
        toast.success(t("authProviders.messages.providerOrderUpdated"));
      } else {
        toast.error(t("authProviders.messages.orderUpdateFailed"));
        await loadProviders();
      }
    } catch (error) {
      console.error("Error updating provider order:", error);
      toast.error(t("authProviders.messages.orderUpdateFailed"));
      await loadProviders();
    }
  };

  const handleHideDisabledProvidersChange = (checked: boolean) => {
    setHideDisabledProviders(checked);
    localStorage.setItem("hideDisabledProviders", JSON.stringify(checked));
  };

  const handleEditProvider = (provider: AuthProvider) => {
    if (editingProvider?.id === provider.id) {
      setEditingProvider(null);
    } else {
      setEditingProvider(provider);
    }
  };

  const handleDeleteProvider = (provider: AuthProvider) => {
    setProviderToDelete({
      id: provider.id,
      name: provider.name,
      displayName: provider.displayName,
    });
  };

  const handleCancelEdit = () => {
    setEditingProvider(null);
    setEditingFormData({});
  };

  const enabledCount = providers.filter((p) => p.enabled).length;
  const filteredProviders = hideDisabledProviders ? providers.filter((p) => p.enabled) : providers;

  return {
    // State
    providers,
    loading,
    saving,
    editingProvider,
    editingFormData,
    hideDisabledProviders,
    providerToDelete,
    isDeleting,

    // Computed
    enabledCount,
    filteredProviders,

    // Actions
    loadProviders,
    updateProvider,
    addProvider,
    editProvider,
    deleteProvider,
    handleDragEnd,
    handleHideDisabledProvidersChange,
    handleEditProvider,
    handleDeleteProvider,
    handleCancelEdit,

    // Setters
    setEditingFormData,
    setProviderToDelete,
  };
}
