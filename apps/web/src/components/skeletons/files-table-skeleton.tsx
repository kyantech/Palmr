import { Skeleton } from "@/components/ui/skeleton";

interface FilesTableSkeletonProps {
  rowCount?: number;
}

export function FilesTableSkeleton({ rowCount = 10 }: FilesTableSkeletonProps) {
  return (
    <div className="rounded-md border">
      <div className="w-full">
        {/* Table Header */}
        <div className="border-b bg-muted/50">
          <div className="grid grid-cols-[auto_1fr_120px_120px_80px] gap-4 p-4">
            <Skeleton className="h-4 w-4" /> {/* Checkbox */}
            <Skeleton className="h-4 w-24" /> {/* Name */}
            <Skeleton className="h-4 w-16" /> {/* Size */}
            <Skeleton className="h-4 w-20" /> {/* Modified */}
            <Skeleton className="h-4 w-12" /> {/* Actions */}
          </div>
        </div>

        {/* Table Rows */}
        <div className="divide-y">
          {Array.from({ length: rowCount }).map((_, index) => (
            <div key={index} className="grid grid-cols-[auto_1fr_120px_120px_80px] gap-4 p-4 hover:bg-muted/50">
              {/* Checkbox */}
              <div className="flex items-center gap-2 min-w-0">
                <Skeleton className="h-4 w-4" />
              </div>

              {/* Name column with icon */}
              <div className="flex items-center gap-2 min-w-0">
                <Skeleton className="h-6 w-6 rounded flex-shrink-0" />
                <Skeleton className="h-4 w-full max-w-[200px]" />
              </div>

              {/* Size */}
              <div className="flex items-center">
                <Skeleton className="h-4 w-16" />
              </div>

              {/* Modified date */}
              <div className="flex items-center">
                <Skeleton className="h-4 w-24" />
              </div>

              {/* Actions */}
              <div className="flex items-center justify-end gap-2">
                <Skeleton className="h-6 w-6 rounded" />
                <Skeleton className="h-6 w-6 rounded" />
              </div>
            </div>
          ))}
        </div>
      </div>
    </div>
  );
}
