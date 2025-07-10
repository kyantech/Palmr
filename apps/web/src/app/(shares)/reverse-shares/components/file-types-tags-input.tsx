"use client";

import React, { KeyboardEvent, useState } from "react";
import { IconX } from "@tabler/icons-react";

import { Badge } from "@/components/ui/badge";
import { cn } from "@/lib/utils";

interface FileTypesTagsInputProps {
  value: string[];
  onChange: (tags: string[]) => void;
  placeholder?: string;
  disabled?: boolean;
  className?: string;
}

export function FileTypesTagsInput({
  value = [],
  onChange,
  placeholder = "jpg png pdf docx...",
  disabled,
  className,
}: FileTypesTagsInputProps) {
  const [inputValue, setInputValue] = useState("");

  const addTag = () => {
    const newTag = inputValue.trim().toLowerCase();
    if (newTag && !value.includes(newTag)) {
      onChange([...value, newTag]);
      setInputValue("");
    }
  };

  const removeTag = (index: number) => {
    const newTags = value.filter((_, i) => i !== index);
    onChange(newTags);
  };

  const handleKeyDown = (e: KeyboardEvent<HTMLInputElement>) => {
    if (e.key === "Enter" || e.key === " " || e.key === "," || e.key === "|" || e.key === "-") {
      e.preventDefault();
      addTag();
    } else if (e.key === "Backspace" && inputValue === "" && value.length > 0) {
      e.preventDefault();
      removeTag(value.length - 1);
    } else if (e.key === ".") {
      e.preventDefault();
    }
  };

  const handleInputChange = (e: React.ChangeEvent<HTMLInputElement>) => {
    const sanitizedValue = e.target.value.replace(/\./g, "").toLowerCase();
    setInputValue(sanitizedValue);
  };

  const handleInputBlur = () => {
    if (inputValue.trim()) {
      addTag();
    }
  };

  return (
    <div
      data-slot="input"
      className={cn(
        "file:text-foreground placeholder:text-muted-foreground selection:bg-primary selection:text-primary-foreground dark:bg-input/30 border-input flex h-9 w-full min-w-0 rounded-md border bg-transparent px-3 py-1 text-base shadow-xs transition-[color,box-shadow] outline-none file:inline-flex file:h-7 file:border-0 file:bg-transparent file:text-sm file:font-medium disabled:pointer-events-none disabled:cursor-not-allowed disabled:opacity-50 md:text-sm",
        "focus-within:border-ring focus-within:ring-ring/50 focus-within:ring-[3px]",
        "aria-invalid:ring-destructive/20 dark:aria-invalid:ring-destructive/40 aria-invalid:border-destructive",
        "flex-wrap gap-1 min-h-9 h-auto py-1",
        disabled && "opacity-50 cursor-not-allowed",
        className
      )}
    >
      {value.map((tag, index) => (
        <Badge
          key={index}
          variant="outline"
          className={cn(
            "flex items-center gap-1 pl-2 pr-1 h-6 text-xs mt-[1px] rounded-[6px]",
            "bg-slate-300 text-gray-800 border-slate-200  hover:text-gray-800",
            "dark:bg-slate-800 dark:text-slate-200 dark:border-slate-700",
            "hover:cursor-default"
          )}
        >
          <span>{tag}</span>
          {!disabled && (
            <button
              type="button"
              onClick={() => removeTag(index)}
              className="ml-1 rounded-sm hover:bg-background/50 dark:hover:bg-background/20 flex items-center justify-center transition-colors hover:cursor-pointer"
            >
              <IconX className="h-2.5 w-2.5" />
            </button>
          )}
        </Badge>
      ))}
      <input
        type="text"
        value={inputValue}
        onChange={handleInputChange}
        onKeyDown={handleKeyDown}
        onBlur={handleInputBlur}
        placeholder={value.length === 0 ? placeholder : ""}
        disabled={disabled}
        className="flex-1 min-w-[80px] border-0 bg-transparent p-0 text-sm outline-none placeholder:text-muted-foreground disabled:cursor-not-allowed"
      />
    </div>
  );
}
