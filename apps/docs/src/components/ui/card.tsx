import { ReactNode } from "react";
import Link from "next/link";

import { cn } from "@/lib/utils";

interface CardProps {
  title: string;
  description?: string;
  href?: string;
  icon?: ReactNode;
  className?: string;
  onClick?: () => void;
  children?: ReactNode;
}

export const Card = ({ title, description, href, icon, className, onClick, children }: CardProps) => {
  const cardContent = (
    <div
      className={cn(
        "group relative overflow-hidden rounded-xl border px-3 py-0 mt-0",
        "bg-card text-card-foreground shadow-sm",
        "hover:bg-accent/20 hover:border-accent-foreground/30 hover:shadow-md",
        "transition-all duration-300 ease-out",
        "focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2",
        "cursor-pointer",
        className
      )}
    >
      <div className="flex items-center gap-3">
        {icon && (
          <div className="flex-shrink-0 -mt-11">
            <div className="w-8 h-8 rounded-lg bg-gradient-to-br from-primary/15 to-primary/5 flex items-center justify-center group-hover:scale-105 transition-transform duration-200">
              {icon}
            </div>
          </div>
        )}
        <div className="flex-1 min-w-0">
          <h3 className="font-medium text-sm text-foreground mb-1 group-hover:text-primary transition-colors duration-200 mt-3 text-decoration-none">
            {title}
          </h3>
          {description && (
            <p className="text-xs text-muted-foreground/80 leading-relaxed line-clamp-2 group-hover:text-muted-foreground transition-colors duration-200">
              {description}
            </p>
          )}
          {children && (
            <div className="text-xs text-muted-foreground/80 leading-relaxed group-hover:text-muted-foreground transition-colors duration-200 mt-2">
              {children}
            </div>
          )}
        </div>
        <div className="flex-shrink-0 ml-2">
          <div className="w-5 h-5 rounded-full bg-muted/40 flex items-center justify-center opacity-0 group-hover:opacity-100 group-hover:bg-primary/10 transition-all duration-200">
            <svg
              className="w-3 h-3 text-muted-foreground group-hover:text-primary"
              fill="none"
              stroke="currentColor"
              viewBox="0 0 24 24"
            >
              <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2.5} d="M9 5l7 7-7 7" />
            </svg>
          </div>
        </div>
      </div>
    </div>
  );

  if (href) {
    return (
      <Link href={href} className="block no-underline">
        {cardContent}
      </Link>
    );
  }

  if (onClick) {
    return (
      <button onClick={onClick} className="block w-full text-left">
        {cardContent}
      </button>
    );
  }

  return cardContent;
};

interface CardGridProps {
  children: ReactNode;
  className?: string;
}

export const CardGrid = ({ children, className }: CardGridProps) => {
  return <div className={cn("grid grid-cols-1 sm:grid-cols-2 gap-2.5 mt-5", "max-w-4xl", className)}>{children}</div>;
};
