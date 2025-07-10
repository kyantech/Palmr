"use client";

import React from "react";
import { DragDropContext, Draggable, Droppable, DropResult } from "@hello-pangea/dnd";
import { IconSettings } from "@tabler/icons-react";
import { useTranslations } from "next-intl";

import { AuthProvider } from "./edit-provider-form";
import { ProviderRow } from "./provider-row";

interface ProviderListProps {
  providers: AuthProvider[];
  filteredProviders: AuthProvider[];
  hideDisabledProviders: boolean;
  onDragEnd: (result: DropResult) => void;
  onUpdateProvider: (id: string, updates: Partial<AuthProvider>) => void;
  onEditProvider: (provider: AuthProvider) => void;
  onDeleteProvider: (provider: AuthProvider) => void;
  saving: string | null;
  getIcon: (provider: AuthProvider) => React.ReactNode;
  editingProvider: AuthProvider | null;
  editProvider: (data: Partial<AuthProvider>) => void;
  onCancelEdit: () => void;
  editingFormData: Record<string, any>;
  setEditingFormData: (data: Record<string, any>) => void;
}

export function ProviderList({
  filteredProviders,
  hideDisabledProviders,
  onDragEnd,
  onUpdateProvider,
  onEditProvider,
  onDeleteProvider,
  saving,
  getIcon,
  editingProvider,
  editProvider,
  onCancelEdit,
  editingFormData,
  setEditingFormData,
}: ProviderListProps) {
  const t = useTranslations();

  if (filteredProviders.length === 0) {
    return (
      <div className="text-center py-8 text-muted-foreground">
        <IconSettings className="h-12 w-12 mx-auto mb-4 opacity-50" />
        <p>
          {hideDisabledProviders ? t("authProviders.noProvidersEnabled") : t("authProviders.noProvidersConfigured")}
        </p>
      </div>
    );
  }

  return (
    <div className="space-y-2">
      <div className="flex items-center justify-between">
        <p className="text-sm text-muted-foreground">
          {hideDisabledProviders ? t("authProviders.dragDisabledMessage") : t("authProviders.dragEnabledMessage")}
        </p>
      </div>

      {!hideDisabledProviders ? (
        <DragDropContext onDragEnd={onDragEnd}>
          <Droppable droppableId="providers">
            {(provided) => (
              <div {...provided.droppableProps} ref={provided.innerRef} className="space-y-2">
                {filteredProviders.map((provider, index) => (
                  <Draggable key={provider.id} draggableId={provider.id} index={index}>
                    {(provided, snapshot) => (
                      <div
                        ref={provided.innerRef}
                        {...provided.draggableProps}
                        className={`transition-all ${snapshot.isDragging ? "shadow-lg scale-105" : ""}`}
                      >
                        <ProviderRow
                          provider={provider}
                          onUpdate={(updates) => onUpdateProvider(provider.id, updates)}
                          onEdit={() => onEditProvider(provider)}
                          onDelete={() => onDeleteProvider(provider)}
                          saving={saving === provider.id}
                          getIcon={getIcon}
                          editingProvider={editingProvider}
                          editProvider={editProvider}
                          onCancelEdit={onCancelEdit}
                          editingFormData={editingFormData}
                          setEditingFormData={setEditingFormData}
                          dragHandleProps={provided.dragHandleProps}
                          isDragging={snapshot.isDragging}
                          isDragDisabled={false}
                        />
                      </div>
                    )}
                  </Draggable>
                ))}
                {provided.placeholder}
              </div>
            )}
          </Droppable>
        </DragDropContext>
      ) : (
        <div className="space-y-2">
          {filteredProviders.map((provider) => (
            <ProviderRow
              key={provider.id}
              provider={provider}
              onUpdate={(updates) => onUpdateProvider(provider.id, updates)}
              onEdit={() => onEditProvider(provider)}
              onDelete={() => onDeleteProvider(provider)}
              saving={saving === provider.id}
              getIcon={getIcon}
              editingProvider={editingProvider}
              editProvider={editProvider}
              onCancelEdit={onCancelEdit}
              editingFormData={editingFormData}
              setEditingFormData={setEditingFormData}
              dragHandleProps={null}
              isDragging={false}
              isDragDisabled={true}
            />
          ))}
        </div>
      )}
    </div>
  );
}
