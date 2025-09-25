import apiInstance from "@/config/api";

export interface BulkDownloadRequest {
  fileIds: string[];
  folderIds: string[];
  zipName: string;
}

export interface BulkDownloadResponse {
  success: boolean;
  message: string;
}

export const bulkDownloadFiles = async (data: BulkDownloadRequest): Promise<Blob> => {
  const response = await apiInstance.post("/api/files/bulk-download", data, {
    responseType: "blob",
  });
  return response.data;
};

export const downloadFolder = async (folderId: string, folderName: string): Promise<Blob> => {
  const response = await apiInstance.get(`/api/files/bulk-download/folder/${folderId}/${folderName}`, {
    responseType: "blob",
  });
  return response.data;
};

export const bulkDownloadReverseShareFiles = async (data: { fileIds: string[]; zipName: string }): Promise<Blob> => {
  const response = await apiInstance.post("/api/files/bulk-download/reverse-share", data, {
    responseType: "blob",
  });
  return response.data;
};
