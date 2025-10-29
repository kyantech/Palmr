"use client";

import { useState } from "react";

import { ProtectedRoute } from "@/components/auth/protected-route";
import { LoadingScreen } from "@/components/layout/loading-screen";
import { Navbar } from "@/components/layout/navbar";
import { DefaultFooter } from "@/components/ui/default-footer";
import { GenerateInviteLinkModal } from "./components/generate-invite-link-modal";
import { UserManagementModals } from "./components/user-management-modals";
import { UsersHeader } from "./components/users-header";
import { UsersTable } from "./components/users-table";
import { useUserManagement } from "./hooks/use-user-management";

export default function AdminAreaPage() {
  const {
    users,
    isLoading,
    currentUser,
    modals,
    selectedUser,
    deleteModalUser,
    statusModalUser,
    handleCreateUser,
    handleEditUser,
    handleDeleteUser,
    handleToggleUserStatus,
    onSubmit,
    formMethods,
  } = useUserManagement();

  const [isInviteModalOpen, setIsInviteModalOpen] = useState(false);

  if (isLoading) {
    return <LoadingScreen />;
  }

  return (
    <ProtectedRoute requireAdmin>
      <div className="w-full h-screen flex flex-col">
        <Navbar />
        <div className="flex-1 max-w-7xl mx-auto w-full py-8 px-6">
          <div className="flex flex-col gap-8">
            <UsersHeader onCreateUser={handleCreateUser} onGenerateInvite={() => setIsInviteModalOpen(true)} />

            <UsersTable
              currentUser={currentUser}
              users={users}
              onDelete={(user) => {
                modals.setDeleteModalUser(user);
                modals.onDeleteModalOpen();
              }}
              onEdit={handleEditUser}
              onToggleStatus={(user) => {
                modals.setStatusModalUser(user);
                modals.onStatusModalOpen();
              }}
            />
          </div>
        </div>
        <DefaultFooter />

        <UserManagementModals
          deleteModalUser={deleteModalUser}
          formMethods={formMethods}
          modals={modals}
          selectedUser={selectedUser}
          statusModalUser={statusModalUser}
          onDelete={handleDeleteUser}
          onSubmit={onSubmit}
          onToggleStatus={handleToggleUserStatus}
        />

        <GenerateInviteLinkModal isOpen={isInviteModalOpen} onClose={() => setIsInviteModalOpen(false)} />
      </div>
    </ProtectedRoute>
  );
}
