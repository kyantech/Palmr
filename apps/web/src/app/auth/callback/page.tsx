"use client";

import { useEffect } from "react";
import { useRouter, useSearchParams } from "next/navigation";
import { toast } from "sonner";

export default function AuthCallbackPage() {
  const router = useRouter();
  const searchParams = useSearchParams();

  useEffect(() => {
    const token = searchParams.get("token");
    const error = searchParams.get("error");

    if (error) {
      let errorMessage = "Authentication failed";

      switch (error) {
        case "oauth_error":
          errorMessage = "OAuth authentication failed";
          break;
        case "missing_parameters":
          errorMessage = "Missing authentication parameters";
          break;
        case "registration_disabled":
          errorMessage = "Registration is disabled for this provider";
          break;
        case "provider_disabled":
          errorMessage = "This authentication provider is disabled";
          break;
        case "state_expired":
          errorMessage = "Authentication session expired";
          break;
        case "account_inactive":
          errorMessage = "Your account is inactive";
          break;
        default:
          errorMessage = "Authentication failed";
      }

      toast.error(errorMessage);
      router.push("/login");
      return;
    }

    if (token) {
      // Set the token in a cookie and redirect to dashboard (using Palmr's standard cookie name)
      document.cookie = `token=${token}; path=/; max-age=${7 * 24 * 60 * 60}; samesite=lax`;

      toast.success("Successfully authenticated!");
      router.push("/dashboard");
      return;
    }

    // If no token or error, redirect to login
    router.push("/login");
  }, [router, searchParams]);

  return (
    <div className="min-h-screen flex items-center justify-center">
      <div className="text-center">
        <div className="animate-spin rounded-full h-8 w-8 border-b-2 border-primary mx-auto mb-4"></div>
        <p className="text-muted-foreground">Processing authentication...</p>
      </div>
    </div>
  );
}
