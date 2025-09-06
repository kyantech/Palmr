import { Fragment } from "react";
import { IconChevronRight, IconHome } from "@tabler/icons-react";
import { Button } from "@/components/ui/button";

interface ShareBreadcrumbNavigationProps {
  path: Array<{
    id: string;
    name: string;
    description?: string | null;
  }>;
  onNavigate: (folderId?: string) => void;
}

export function ShareBreadcrumbNavigation({ path, onNavigate }: ShareBreadcrumbNavigationProps) {
  return (
    <div className="flex items-center gap-1 text-sm text-muted-foreground">
      <Button
        variant="ghost"
        size="sm"
        className="h-8 px-2 text-muted-foreground hover:text-foreground"
        onClick={() => onNavigate()}
      >
        <IconHome className="h-4 w-4 mr-1" />
        Share Root
      </Button>

      {path.map((folder, index) => (
        <Fragment key={folder.id}>
          <IconChevronRight className="h-4 w-4" />
          <Button
            variant="ghost"
            size="sm"
            className="h-8 px-2 text-muted-foreground hover:text-foreground"
            onClick={() => onNavigate(folder.id)}
          >
            {folder.name}
          </Button>
        </Fragment>
      ))}
    </div>
  );
}
