import { IconFolder } from "@tabler/icons-react";

export function ShareEmptyState() {
  return (
    <div className="text-center py-16">
      <div className="flex justify-center mb-6">
        <div className="relative">
          <IconFolder className="h-24 w-24 text-muted-foreground/30" />
          <div className="absolute inset-0 flex items-center justify-center">
            <div className="text-4xl">📭</div>
          </div>
        </div>
      </div>
      <h3 className="text-lg font-semibold text-foreground mb-2">This share is empty</h3>
      <p className="text-muted-foreground max-w-sm mx-auto">
        There are no files or folders shared in this location yet.
      </p>
    </div>
  );
}
