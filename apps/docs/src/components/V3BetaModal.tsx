"use client";

import { useEffect, useState } from "react";
import { usePathname, useRouter } from "next/navigation";
import { AlertTriangle } from "lucide-react";

import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle } from "@/components/ui/dialog";

const V3_BETA_MODAL_KEY = "palmr-v3-beta-modal-shown";

export function V3BetaModal() {
  const [isOpen, setIsOpen] = useState(false);
  const router = useRouter();
  const pathname = usePathname();

  useEffect(() => {
    // Verifica se está na rota v3-beta
    if (pathname?.includes("/docs/v3-beta")) {
      // Verifica se o modal já foi mostrado antes
      const hasSeenModal = localStorage.getItem(V3_BETA_MODAL_KEY);

      if (!hasSeenModal) {
        setIsOpen(true);
      }
    }
  }, [pathname]);

  const handleClose = () => {
    // Marca como visto no localStorage
    localStorage.setItem(V3_BETA_MODAL_KEY, "true");
    setIsOpen(false);
  };

  const handleGoToQuickStart = () => {
    handleClose();
    router.push("/docs/v3-beta/quick-start");
  };

  return (
    <Dialog open={isOpen} onOpenChange={handleClose}>
      <DialogContent className="w-[600px] max-w-[90vw] sm:max-w-[600px] border-2 border-amber-400 shadow-2xl backdrop-blur-md">
        <DialogHeader>
          <div className="flex items-center gap-3 mb-2">
            <div className="p-2">
              <AlertTriangle className="h-8 w-8 text-amber-600" />
            </div>
            <DialogTitle className="text-xl font-bold">Important Changes</DialogTitle>
          </div>
          <DialogDescription className="text-left space-y-4 pt-2 text-base leading-relaxed">
            <div className="bg-amber-50 dark:bg-amber-900/20 border border-amber-200 dark:border-amber-800 rounded-lg p-4 mb-4">
              <p className="text-amber-800 dark:text-amber-200 text-sm font-medium">
                <strong>Note:</strong> This alert is only relevant if you are upgrading from a version prior to
                v3.0.0-beta. If you're new to Palmr, you can safely follow the documentation normally.
              </p>
            </div>
            <p>
              Major changes have been made starting from version <strong className="text-amber-600">v3.3.0-beta</strong>
              .
            </p>
            <p>
              This has made it necessary to update the{" "}
              <code className="bg-gray-100 dark:bg-gray-900 px-2 py-1 rounded text-sm font-mono">
                docker-compose.yaml
              </code>{" "}
              configuration to work perfectly with the new changes.
            </p>
            <p>For more details, please check the updated documentation in the quick start guide.</p>
          </DialogDescription>
        </DialogHeader>
        <div className="flex gap-3 justify-end pt-6">
          <Button variant="outline" onClick={handleClose} className="px-6">
            Got it
          </Button>
          <Button onClick={handleGoToQuickStart} className="px-6 ">
            View Quick Start Guide
          </Button>
        </div>
      </DialogContent>
    </Dialog>
  );
}
