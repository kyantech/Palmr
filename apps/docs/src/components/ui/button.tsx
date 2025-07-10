import { ButtonHTMLAttributes, forwardRef } from "react";

import { cn } from "@/lib/utils";

interface ButtonProps extends ButtonHTMLAttributes<HTMLButtonElement> {
  className?: string;
}

export const Button = forwardRef<HTMLButtonElement, ButtonProps>(({ className, children, ...props }, ref) => {
  return (
    <button
      ref={ref}
      className={cn(
        "inline-flex items-center justify-center rounded-md px-4 py-2",
        "text-base font-medium transition-all duration-200 cursor-pointer",
        "text-gray-700 dark:text-gray-200",
        "border border-gray-300 dark:border-gray-600",
        "hover:border-gray-400 dark:hover:border-gray-500",
        "hover:bg-gray-50 dark:hover:bg-gray-800/50",
        "hover:shadow-sm",
        "focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-gray-400",
        "dark:focus-visible:ring-gray-500 focus-visible:ring-offset-2",
        "disabled:pointer-events-none disabled:opacity-50",
        className
      )}
      {...props}
    >
      {children}
    </button>
  );
});

Button.displayName = "Button";
