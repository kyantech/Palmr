import { useTranslations } from "next-intl";

import { Card, CardContent } from "@/components/ui/card";
import { FileListProps } from "../types";
import { FilesViewManager } from "./files-view-manager";
import { Header } from "./header";
import { SearchBar } from "./search-bar";

export function FileList({ files, filteredFiles, fileManager, searchQuery, onSearch, onUpload }: FileListProps) {
  const t = useTranslations();

  return (
    <Card>
      <CardContent>
        <div className="flex flex-col gap-6">
          <Header onUpload={onUpload} />
          <SearchBar
            filteredCount={filteredFiles.length}
            searchQuery={searchQuery}
            totalFiles={files.length}
            onSearch={onSearch}
          />

          {files.length > 0 ? (
            <FilesViewManager
              files={filteredFiles}
              searchQuery={searchQuery}
              onSearch={onSearch}
              onDelete={fileManager.setFileToDelete}
              onDownload={fileManager.handleDownload}
              onPreview={fileManager.setPreviewFile}
              onRename={fileManager.setFileToRename}
              onShare={fileManager.setFileToShare}
              onBulkDelete={fileManager.handleBulkDelete}
              onBulkShare={fileManager.handleBulkShare}
              onBulkDownload={fileManager.handleBulkDownload}
              setClearSelectionCallback={fileManager.setClearSelectionCallback}
              onUpdateName={(fileId, newName) => {
                const file = filteredFiles.find((f) => f.id === fileId);
                if (file) {
                  fileManager.handleRename(fileId, newName, file.description);
                }
              }}
              onUpdateDescription={(fileId, newDescription) => {
                const file = filteredFiles.find((f) => f.id === fileId);
                if (file) {
                  fileManager.handleRename(fileId, file.name, newDescription);
                }
              }}
            />
          ) : (
            <div className="text-center py-6 flex flex-col items-center gap-2">
              <p className="text-muted-foreground">{t("files.empty.title")}</p>
            </div>
          )}
        </div>
      </CardContent>
    </Card>
  );
}
