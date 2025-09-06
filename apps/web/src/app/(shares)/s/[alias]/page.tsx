"use client";

import { LoadingScreen } from "@/components/layout/loading-screen";
import { DefaultFooter } from "@/components/ui/default-footer";
import { PasswordModal } from "./components/password-modal";
import { ShareDetails } from "./components/share-details";
import { ShareHeader } from "./components/share-header";
import { ShareNotFound } from "./components/share-not-found";
import { usePublicShare } from "./hooks/use-public-share";

export default function PublicSharePage() {
  const {
    isLoading,
    share,
    password,
    isPasswordModalOpen,
    isPasswordError,
    setPassword,
    handlePasswordSubmit,
    handleDownload,
    handleBulkDownload,
    handleSelectedItemsBulkDownload,
  } = usePublicShare();

  if (isLoading) {
    return <LoadingScreen />;
  }

  return (
    <div className="min-h-screen flex flex-col">
      <ShareHeader />

      <main className="flex-1 container mx-auto px-6 py-8">
        <div className="max-w-5xl mx-auto space-y-6">
          {!isPasswordModalOpen && !share && <ShareNotFound />}
          {share && (
            <ShareDetails
              share={share}
              password={password}
              onDownload={handleDownload}
              onBulkDownload={handleBulkDownload}
              onSelectedItemsBulkDownload={handleSelectedItemsBulkDownload}
            />
          )}
        </div>
      </main>

      <DefaultFooter />

      <PasswordModal
        isError={isPasswordError}
        isOpen={isPasswordModalOpen}
        password={password}
        onPasswordChange={setPassword}
        onSubmit={handlePasswordSubmit}
      />
    </div>
  );
}
