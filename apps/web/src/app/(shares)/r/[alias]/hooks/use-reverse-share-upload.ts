"use client";

import { useCallback, useEffect, useState } from "react";
import { useRouter } from "next/navigation";
import { useTranslations } from "next-intl";
import { toast } from "sonner";

import { getReverseShareForUploadByAlias } from "@/http/endpoints";
import { ERROR_MESSAGES, HTTP_STATUS, type ErrorType } from "../constants";
import type { ReverseShareInfo } from "../types";

interface UseReverseShareUploadProps {
  alias: string;
}

export function useReverseShareUpload({ alias }: UseReverseShareUploadProps) {
  const router = useRouter();
  const t = useTranslations();

  const [reverseShare, setReverseShare] = useState<ReverseShareInfo | null>(null);
  const [isLoading, setIsLoading] = useState(true);
  const [isPasswordModalOpen, setIsPasswordModalOpen] = useState(false);
  const [currentPassword, setCurrentPassword] = useState("");
  const [hasUploadedSuccessfully, setHasUploadedSuccessfully] = useState(false);
  const [error, setError] = useState<{ type: ErrorType }>({ type: null });

  const redirectToHome = () => router.push("/");

  const checkIfMaxFilesReached = (reverseShareData: ReverseShareInfo): boolean => {
    if (!reverseShareData.maxFiles) return false;
    return reverseShareData.currentFileCount >= reverseShareData.maxFiles;
  };

  const handleErrorResponse = useCallback(
    (responseError: any) => {
      const status = responseError.response?.status;
      const errorMessage = responseError.response?.data?.error;

      switch (status) {
        case HTTP_STATUS.UNAUTHORIZED:
          if (errorMessage === ERROR_MESSAGES.PASSWORD_REQUIRED) {
            setIsPasswordModalOpen(true);
          } else if (errorMessage === ERROR_MESSAGES.INVALID_PASSWORD) {
            setIsPasswordModalOpen(true);
            toast.error(t("reverseShares.upload.errors.passwordIncorrect"));
          }
          break;

        case HTTP_STATUS.NOT_FOUND:
          setError({ type: "notFound" });
          break;

        case HTTP_STATUS.FORBIDDEN:
          setError({ type: "inactive" });
          break;

        case HTTP_STATUS.GONE:
          setError({ type: "expired" });
          break;

        default:
          setError({ type: "generic" });
          toast.error(t("reverseShares.upload.errors.loadFailed"));
          break;
      }
    },
    [t]
  );

  const loadReverseShare = useCallback(
    async (passwordAttempt?: string) => {
      try {
        setIsLoading(true);
        setError({ type: null });

        const response = await getReverseShareForUploadByAlias(
          alias,
          passwordAttempt ? { password: passwordAttempt } : undefined
        );

        setReverseShare(response.data.reverseShare);
        setIsPasswordModalOpen(false);
        setCurrentPassword(passwordAttempt || "");
      } catch (responseError: any) {
        console.error("Failed to load reverse share:", responseError);
        handleErrorResponse(responseError);
      } finally {
        setIsLoading(false);
      }
    },
    [alias, handleErrorResponse]
  );

  const handlePasswordSubmit = (passwordValue: string) => {
    loadReverseShare(passwordValue);
  };

  const handlePasswordModalClose = () => {
    redirectToHome();
  };

  const handleUploadSuccess = () => {
    setHasUploadedSuccessfully(true);
  };

  const resetUploadSuccess = () => {
    setHasUploadedSuccessfully(false);
  };

  useEffect(() => {
    if (alias) {
      loadReverseShare();
    }
  }, [alias, loadReverseShare]);

  const isMaxFilesReached = reverseShare ? checkIfMaxFilesReached(reverseShare) : false;
  const isWeTransferLayout = reverseShare?.pageLayout === "WETRANSFER";
  const hasError = error.type !== null || (!reverseShare && !isLoading && !isPasswordModalOpen);

  const isLinkInactive = error.type === "inactive";
  const isLinkNotFound = error.type === "notFound" || (!reverseShare && !isLoading && !isPasswordModalOpen);
  const isLinkExpired = error.type === "expired";

  return {
    reverseShare,
    currentPassword,
    alias,

    isLoading,
    isPasswordModalOpen,
    hasUploadedSuccessfully,
    error: error.type,
    isMaxFilesReached,
    isWeTransferLayout,
    hasError,

    isLinkInactive,
    isLinkNotFound,
    isLinkExpired,

    handlePasswordSubmit,
    handlePasswordModalClose,
    handleUploadSuccess,
    resetUploadSuccess,
    loadReverseShare,
    redirectToHome,
  };
}
