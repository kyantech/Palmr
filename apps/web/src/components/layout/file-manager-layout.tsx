import { ReactNode } from "react";
import Link from "next/link";
import { IconLayoutDashboard } from "@tabler/icons-react";
import { useTranslations } from "next-intl";

import { DownloadQueueIndicator } from "@/components/download-queue-indicator";
import { Navbar } from "@/components/layout/navbar";
import {
  Breadcrumb,
  BreadcrumbItem,
  BreadcrumbLink,
  BreadcrumbList,
  BreadcrumbSeparator,
} from "@/components/ui/breadcrumb";
import { DefaultFooter } from "@/components/ui/default-footer";
import { Separator } from "@/components/ui/separator";

interface FileManagerLayoutProps {
  children: ReactNode;
  title: string;
  description?: string;
  icon: ReactNode;
  breadcrumbLabel?: string;
  showBreadcrumb?: boolean;
  pendingDownloads?: Array<{
    downloadId: string;
    fileName: string;
    objectName: string;
    startTime: number;
    status: "pending" | "queued" | "downloading" | "completed" | "failed";
  }>;
  onCancelDownload?: (downloadId: string) => void;
}

export function FileManagerLayout({
  children,
  title,
  description,
  icon,
  breadcrumbLabel,
  showBreadcrumb = true,
  pendingDownloads = [],
  onCancelDownload,
}: FileManagerLayoutProps) {
  const t = useTranslations();

  return (
    <div className="w-full min-h-screen flex flex-col">
      <Navbar />
      <div className="flex-1 max-w-7xl mx-auto w-full p-6 py-8">
        <div className="flex flex-col gap-6">
          <div className="flex flex-col gap-4">
            <div className="flex flex-row items-center gap-2">
              {icon}
              <h1 className="text-2xl font-bold">{title}</h1>
            </div>
            {description && <p className="text-muted-foreground text-sm -mt-2">{description}</p>}
            <Separator />
            {showBreadcrumb && breadcrumbLabel && (
              <Breadcrumb>
                <BreadcrumbList>
                  <BreadcrumbItem>
                    <BreadcrumbLink asChild>
                      <Link href="/dashboard" className="flex items-center">
                        <IconLayoutDashboard size={20} className="mr-2" />
                        {t("navigation.dashboard")}
                      </Link>
                    </BreadcrumbLink>
                  </BreadcrumbItem>
                  <BreadcrumbSeparator />
                  <BreadcrumbItem>
                    <span className="flex items-center gap-2">
                      {icon} {breadcrumbLabel}
                    </span>
                  </BreadcrumbItem>
                </BreadcrumbList>
              </Breadcrumb>
            )}
          </div>

          {children}
        </div>
      </div>
      <DefaultFooter />

      <DownloadQueueIndicator pendingDownloads={pendingDownloads} onCancelDownload={onCancelDownload} />
    </div>
  );
}
