"use client";

import { useEffect } from "react";
import { CheckCircle } from "lucide-react";
import { createPortal } from "react-dom";

interface ToastProps {
  message: string;
  duration?: number;
  onClose: () => void;
}

export function Toast({ message, duration = 2000, onClose }: ToastProps) {
  useEffect(() => {
    const timer = setTimeout(() => {
      onClose();
    }, duration);

    return () => clearTimeout(timer);
  }, [duration, onClose]);

  return createPortal(
    <div className="fixed bottom-4 right-4 z-50 animate-in fade-in slide-in-from-bottom-4">
      <div className="flex items-center gap-2 bg-white dark:bg-black/80 text-black dark:text-white px-4 py-2 rounded-md shadow-lg border border-gray-200 dark:border-gray-700">
        <CheckCircle className="h-5 w-5 text-green-400" />
        <p className="text-sm font-medium">{message}</p>
      </div>
    </div>,
    document.body
  );
}
