"use client";

import { FilePreviewModal } from "@/components/modals/file-preview-modal";
import type { ReverseShareFile } from "@/http/endpoints/reverse-shares/types";

interface ReverseShareFilePreviewModalProps {
  isOpen: boolean;
  onClose: () => void;
  file: ReverseShareFile | null;
}

export function ReverseShareFilePreviewModal({ isOpen, onClose, file }: ReverseShareFilePreviewModalProps) {
  if (!file) return null;

  const adaptedFile = {
    ...file,
    description: file.description ?? undefined,
  };

  return <FilePreviewModal isOpen={isOpen} onClose={onClose} file={adaptedFile} isReverseShare={true} />;
}
