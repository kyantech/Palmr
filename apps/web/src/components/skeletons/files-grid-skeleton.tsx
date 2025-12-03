import { Skeleton } from "@/components/ui/skeleton";

interface FilesGridSkeletonProps {
  itemCount?: number;
}

export function FilesGridSkeleton({ itemCount = 12 }: FilesGridSkeletonProps) {
  return (
    <>
      {/* Select All Checkbox Skeleton */}
      <div className="flex items-center gap-2 px-2 mb-4">
        <Skeleton className="h-4 w-4 rounded" />
        <Skeleton className="h-4 w-24" />
      </div>

      <div className="grid grid-cols-1 sm:grid-cols-2 md:grid-cols-3 lg:grid-cols-4 xl:grid-cols-5 gap-4">
        {Array.from({ length: itemCount }).map((_, index) => (
          <div key={index} className="border rounded-lg p-3 space-y-3">
            {/* Icon/Preview skeleton */}
            <div className="flex flex-col items-center space-y-3">
              <Skeleton className="w-16 h-16 rounded-lg" />

              {/* File name skeleton */}
              <div className="w-full space-y-1">
                <Skeleton className="h-4 w-full" />

                {/* Description skeleton (optional, 50% chance) */}
                {index % 2 === 0 && <Skeleton className="h-3 w-3/4" />}

                {/* Size and date skeleton */}
                <div className="space-y-1">
                  <Skeleton className="h-3 w-16" />
                  <Skeleton className="h-3 w-24" />
                </div>
              </div>
            </div>
          </div>
        ))}
      </div>
    </>
  );
}
