"use client";

import { useState } from "react";
import { Copy, X } from "lucide-react";

import { Button } from "@/components/ui/button";
import { Toast } from "@/components/ui/toast";

export function KeyGenerator() {
  const [key, setKey] = useState("");
  const [showToast, setShowToast] = useState(false);

  const generateKey = async () => {
    try {
      const response = await fetch("/api/generate-key");
      const data = await response.json();
      setKey(data.key);
    } catch (error) {
      console.error("Failed to generate key:", error);
    }
  };

  const copyToClipboard = async () => {
    if (key) {
      try {
        await navigator.clipboard.writeText(key);
        setShowToast(true);
      } catch (error) {
        console.error("Failed to copy:", error);
      }
    }
  };

  return (
    <div className="my-4 space-y-4">
      <div className="flex gap-2">
        <Button onClick={generateKey}>Generate new key</Button>
        {key && (
          <Button
            onClick={() => setKey("")}
            className="!border-red-200 !text-red-600 hover:!bg-red-50 dark:!border-red-800/50 dark:!text-red-400 dark:hover:!bg-red-950/50"
          >
            <X className="h-4 w-4 mr-1" />
            Clear
          </Button>
        )}
      </div>

      <div className="relative">
        <code className="block w-full p-3 bg-gray-100 dark:bg-black/80 rounded-md font-mono text-sm min-h-[2.5rem] border border-gray-200 dark:border-gray-700">
          {key ? (
            <>
              <span className="select-none text-gray-500 dark:text-gray-400">ENCRYPTION_KEY=</span>
              <span className="text-green-900 dark:text-green-600">{key}</span>
            </>
          ) : (
            <span className="text-gray-500 dark:text-gray-400">
              Click the button above to generate a new encryption key
            </span>
          )}
        </code>
        {key && (
          <button
            onClick={copyToClipboard}
            className="absolute right-2 top-1/2 -translate-y-1/2 p-1.5 rounded-md hover:bg-gray-200 dark:hover:bg-gray-700 transition-colors cursor-pointer"
            title="Copy to clipboard"
          >
            <Copy className="h-4 w-4 text-gray-600 dark:text-gray-400" />
          </button>
        )}
      </div>

      {key && (
        <p className="text-sm text-gray-500 dark:text-gray-400">
          Copy the key and paste it into your <code>docker-compose.yml</code> file.
        </p>
      )}

      {showToast && <Toast message="Key copied to clipboard!" onClose={() => setShowToast(false)} />}
    </div>
  );
}
