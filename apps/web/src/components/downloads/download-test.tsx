"use client";

import React from "react";

import { Button } from "@/components/ui/button";
import { useDownloadManager } from "@/hooks/use-download-manager";

/**
 * Test component for download functionality
 * This can be temporarily added to any page to test downloads
 */
export function DownloadTest() {
  const { startDownload, activeDownloads } = useDownloadManager();

  const testDownload = async () => {
    console.log("🧪 Test download button clicked");
    try {
      // Test with a sample file - you can replace with actual objectName and fileName
      await startDownload("test-object-name", "test-file.pdf");
      console.log("🧪 Test download completed");
    } catch (error) {
      console.error("🧪 Test download failed:", error);
    }
  };

  console.log("🧪 DownloadTest render - activeDownloads:", activeDownloads);

  return (
    <div className="p-4 border rounded-lg bg-muted/50">
      <h3 className="font-semibold mb-2">Download Test</h3>
      <p className="text-sm text-muted-foreground mb-4">Active downloads: {activeDownloads.length}</p>
      <Button onClick={testDownload} variant="outline">
        Test Download
      </Button>
      <div className="mt-2 text-xs text-muted-foreground">Check console for debug logs</div>
    </div>
  );
}
