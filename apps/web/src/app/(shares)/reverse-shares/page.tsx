"use client";

import { ProtectedRoute } from "@/components/auth/protected-route";
import { LoadingScreen } from "@/components/layout/loading-screen";
import { Navbar } from "@/components/layout/navbar";
import { Card, CardContent } from "@/components/ui/card";
import { DefaultFooter } from "@/components/ui/default-footer";
import { ReverseSharesCardsContainer } from "./components/reverse-shares-cards-container";
import { ReverseSharesHeader } from "./components/reverse-shares-header";
import { ReverseSharesModals } from "./components/reverse-shares-modals";
import { ReverseSharesSearch } from "./components/reverse-shares-search";
import { useReverseShares } from "./hooks/use-reverse-shares";

export default function ReverseSharesPage() {
  const {
    reverseShares,
    isLoading,
    searchQuery,
    setSearchQuery,
    filteredReverseShares,
    reverseShareToViewDetails,
    reverseShareToGenerateLink,
    reverseShareToDelete,
    reverseShareToEdit,
    reverseShareToViewFiles,
    isDeleting,
    isCreateModalOpen,
    isCreating,
    isUpdating,
    setIsCreateModalOpen,
    handleCopyLink,
    handleDeleteReverseShare,
    handleCreateReverseShare,
    handleUpdateReverseShare,
    setReverseShareToViewDetails,
    setReverseShareToGenerateLink,
    setReverseShareToDelete,
    setReverseShareToEdit,
    setReverseShareToViewFiles,
    handleCreateAlias,
    handleUpdatePassword,
    handleUpdateReverseShareData,
    handleToggleActive,
    loadReverseShares,
    refreshReverseShare,
  } = useReverseShares();

  if (isLoading) {
    return <LoadingScreen />;
  }

  return (
    <ProtectedRoute>
      <div className="w-full h-screen flex flex-col">
        <Navbar />
        <div className="flex-1 max-w-7xl mx-auto w-full px-6 py-8">
          <div className="flex flex-col gap-8">
            <ReverseSharesHeader />
            <Card>
              <CardContent>
                <div className="flex flex-col gap-6">
                  <ReverseSharesSearch
                    filteredCount={filteredReverseShares.length}
                    searchQuery={searchQuery}
                    totalReverseShares={reverseShares.length}
                    onCreateReverseShare={() => setIsCreateModalOpen(true)}
                    onSearchChange={setSearchQuery}
                    onRefresh={loadReverseShares}
                    isRefreshing={isLoading}
                  />

                  <ReverseSharesCardsContainer
                    reverseShares={filteredReverseShares}
                    onCopyLink={handleCopyLink}
                    onDelete={setReverseShareToDelete}
                    onEdit={setReverseShareToEdit}
                    onGenerateLink={setReverseShareToGenerateLink}
                    onViewDetails={setReverseShareToViewDetails}
                    onViewFiles={setReverseShareToViewFiles}
                    onCreateReverseShare={() => setIsCreateModalOpen(true)}
                    onUpdateReverseShare={handleUpdateReverseShareData}
                    onToggleActive={handleToggleActive}
                    onUpdatePassword={handleUpdatePassword}
                  />
                </div>
              </CardContent>
            </Card>

            <ReverseSharesModals
              isCreateModalOpen={isCreateModalOpen}
              onCloseCreateModal={() => setIsCreateModalOpen(false)}
              onCreateReverseShare={handleCreateReverseShare}
              isCreating={isCreating}
              reverseShareToEdit={reverseShareToEdit}
              onCloseEditModal={() => setReverseShareToEdit(null)}
              onUpdateReverseShare={handleUpdateReverseShare}
              isUpdating={isUpdating}
              reverseShareToGenerateLink={reverseShareToGenerateLink}
              reverseShareToViewDetails={reverseShareToViewDetails}
              reverseShareToDelete={reverseShareToDelete}
              reverseShareToViewFiles={reverseShareToViewFiles}
              isDeleting={isDeleting}
              onCloseGenerateLink={() => setReverseShareToGenerateLink(null)}
              onCloseViewDetails={() => setReverseShareToViewDetails(null)}
              onCloseDeleteModal={() => setReverseShareToDelete(null)}
              onCloseViewFiles={() => setReverseShareToViewFiles(null)}
              onConfirmDelete={handleDeleteReverseShare}
              onCreateAlias={handleCreateAlias}
              onCopyLink={handleCopyLink}
              onUpdateReverseShareData={handleUpdateReverseShareData}
              onUpdatePassword={handleUpdatePassword}
              onToggleActive={handleToggleActive}
              onRefreshData={loadReverseShares}
              refreshReverseShare={refreshReverseShare}
            />
          </div>
        </div>
        <DefaultFooter />
      </div>
    </ProtectedRoute>
  );
}
