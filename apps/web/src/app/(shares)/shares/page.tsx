"use client";

import { ProtectedRoute } from "@/components/auth/protected-route";
import { LoadingScreen } from "@/components/layout/loading-screen";
import { Navbar } from "@/components/layout/navbar";
import { Card, CardContent } from "@/components/ui/card";
import { DefaultFooter } from "@/components/ui/default-footer";
import { useDisclosure } from "@/hooks/use-disclosure";
import { useEnhancedFileManager } from "@/hooks/use-enhanced-file-manager";
import { useShareManager } from "@/hooks/use-share-manager";
import { SharesHeader } from "./components/shares-header";
import { SharesModals } from "./components/shares-modals";
import { SharesSearch } from "./components/shares-search";
import { SharesTableContainer } from "./components/shares-table-container";
import { useShares } from "./hooks/use-shares";

export default function SharesPage() {
  const {
    shares,
    isLoading,
    searchQuery,
    setSearchQuery,
    filteredShares,
    shareToGenerateLink,
    handleCopyLink,
    loadShares,
    setShareToGenerateLink,
    smtpEnabled,
  } = useShares();

  const { isOpen: isCreateModalOpen, onOpen: onOpenCreateModal, onClose: onCloseCreateModal } = useDisclosure();
  const shareManager = useShareManager(loadShares);
  const fileManager = useEnhancedFileManager(loadShares);

  if (isLoading) {
    return <LoadingScreen />;
  }

  return (
    <ProtectedRoute>
      <div className="w-full h-screen flex flex-col">
        <Navbar />
        <div className="flex-1 max-w-7xl mx-auto w-full px-6 py-8">
          <div className="flex flex-col gap-8">
            <SharesHeader />
            <Card>
              <CardContent>
                <div className="flex flex-col gap-6">
                  <SharesSearch
                    filteredCount={filteredShares.length}
                    searchQuery={searchQuery}
                    totalShares={shares.length}
                    onCreateShare={onOpenCreateModal}
                    onSearchChange={setSearchQuery}
                  />

                  <SharesTableContainer
                    shareManager={shareManager}
                    shares={filteredShares}
                    onCopyLink={handleCopyLink}
                    onCreateShare={onOpenCreateModal}
                  />
                </div>
              </CardContent>
            </Card>

            <SharesModals
              isCreateModalOpen={isCreateModalOpen}
              shareManager={shareManager}
              fileManager={fileManager}
              shareToGenerateLink={shareToGenerateLink}
              shareToViewDetails={shareManager.shareToViewDetails}
              smtpEnabled={smtpEnabled}
              onCloseCreateModal={onCloseCreateModal}
              onCloseGenerateLink={() => setShareToGenerateLink(null)}
              onCloseViewDetails={() => shareManager.setShareToViewDetails(null)}
              onSuccess={loadShares}
            />
          </div>
        </div>
        <DefaultFooter />
      </div>
    </ProtectedRoute>
  );
}
