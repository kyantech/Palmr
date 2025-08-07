import { ReverseShare } from "../hooks/use-reverse-shares";
import { EmptyReverseSharesState } from "./empty-reverse-shares-state";
import { ReverseShareCard } from "./reverse-share-card";

interface ReverseSharesCardsContainerProps {
  reverseShares: ReverseShare[];
  onCopyLink: (reverseShare: ReverseShare) => void;
  onDelete: (reverseShare: ReverseShare) => void;
  onEdit: (reverseShare: ReverseShare) => void;
  onGenerateLink: (reverseShare: ReverseShare) => void;
  onViewDetails: (reverseShare: ReverseShare) => void;
  onViewFiles: (reverseShare: ReverseShare) => void;
  onViewQrCode?: (reverseShare: ReverseShare) => void;
  onCreateReverseShare: () => void;
  onUpdateReverseShare?: (id: string, data: any) => Promise<any>;
  onToggleActive?: (id: string, isActive: boolean) => Promise<any>;
  onUpdatePassword?: (id: string, data: { hasPassword: boolean; password?: string }) => Promise<any>;
}

export function ReverseSharesCardsContainer({
  reverseShares,
  onCopyLink,
  onDelete,
  onEdit,
  onGenerateLink,
  onViewDetails,
  onViewFiles,
  onViewQrCode,
  onCreateReverseShare,
  onUpdateReverseShare,
  onToggleActive,
  onUpdatePassword,
}: ReverseSharesCardsContainerProps) {
  if (reverseShares.length === 0) {
    return <EmptyReverseSharesState onCreateReverseShare={onCreateReverseShare} />;
  }

  return (
    <div className="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-3 gap-6">
      {reverseShares.map((reverseShare) => (
        <ReverseShareCard
          key={reverseShare.id}
          reverseShare={reverseShare}
          onCopyLink={onCopyLink}
          onDelete={onDelete}
          onEdit={onEdit}
          onGenerateLink={onGenerateLink}
          onViewDetails={onViewDetails}
          onViewFiles={onViewFiles}
          onViewQrCode={onViewQrCode}
          onUpdateReverseShare={onUpdateReverseShare}
          onToggleActive={onToggleActive}
          onUpdatePassword={onUpdatePassword}
        />
      ))}
    </div>
  );
}
