"use client";

import { useEffect, useRef, useState } from "react";
import Link from "next/link";
import { AlertTriangle, ArrowRightIcon, X } from "lucide-react";

import { LATEST_VERSION, LATEST_VERSION_PATH } from "@/config/constants";

function useIntersectionObserver(targetRef: React.RefObject<HTMLDivElement | null>) {
  const [isIntersecting, setIsIntersecting] = useState(true);

  useEffect(() => {
    const observer = new IntersectionObserver(([entry]) => setIsIntersecting(entry.isIntersecting), {
      threshold: 0,
      rootMargin: "-10px 0px 0px 0px",
    });

    if (targetRef.current) {
      observer.observe(targetRef.current);
    }

    return () => observer.disconnect();
  }, [targetRef]);

  return isIntersecting;
}

function useClickOutside(ref: React.RefObject<HTMLDivElement | null>, callback: () => void) {
  useEffect(() => {
    const handleClickOutside = (event: MouseEvent) => {
      if (ref.current && !ref.current.contains(event.target as Node)) {
        callback();
      }
    };

    document.addEventListener("mousedown", handleClickOutside);
    return () => document.removeEventListener("mousedown", handleClickOutside);
  }, [ref, callback]);
}

function WarningContent({ onClose }: { onClose: () => void }) {
  return (
    <>
      <button
        onClick={onClose}
        className="absolute top-3 right-3 text-amber-600 dark:text-amber-400 hover:text-amber-800 dark:hover:text-amber-200 transition-colors cursor-pointer"
        aria-label="Close warning"
      >
        <X className="h-5 w-5" />
      </button>
      <div className="flex items-start pr-8">
        <AlertTriangle className="h-8 w-8 text-amber-500 dark:text-amber-400 mt-0.5 mr-3 flex-shrink-0" />
        <div className="flex-1">
          <h3 className="text-lg font-medium text-amber-800 dark:text-amber-100 mb-2">
            Deprecated version documentation
          </h3>
          <p className="text-[15px] text-amber-700 dark:text-amber-300 mb-3">
            This documentation refers to a previous version of Palmr. It may contain more complex configurations and
            bugs that have already been fixed.
          </p>
          <Link
            href={LATEST_VERSION_PATH}
            className="inline-flex items-center text-base font-medium text-amber-800 dark:text-amber-100 hover:text-amber-900 dark:hover:text-amber-200"
          >
            View latest documentation ({LATEST_VERSION}) <ArrowRightIcon className="w-4 h-4 ml-1 mt-0.5" />
          </Link>
        </div>
      </div>
    </>
  );
}

function FloatingWarning({ onClose }: { onClose: () => void }) {
  const [showContent, setShowContent] = useState(false);
  const floatingRef = useRef<HTMLDivElement>(null);

  useClickOutside(floatingRef, () => setShowContent(false));

  const toggleContent = () => setShowContent(!showContent);

  return (
    <div className="fixed bottom-6 z-50" style={{ right: "max(1.5rem, calc((100vw - 1024px) / 2 + 1.5rem))" }}>
      <div className="relative" ref={floatingRef}>
        <button
          onClick={toggleContent}
          className="w-12 h-12 bg-amber-500 dark:bg-amber-600 rounded-full flex items-center justify-center shadow-2xl hover:bg-amber-600 dark:hover:bg-amber-700 transition-all duration-200 cursor-pointer ring-2 ring-amber-300 dark:ring-amber-500 hover:ring-4 hover:ring-amber-400 dark:hover:ring-amber-400"
          aria-label="Toggle deprecated version warning"
        >
          <AlertTriangle className="h-6 w-6 text-white drop-shadow-sm" />
        </button>

        {showContent && (
          <div className="absolute bottom-full right-0 mb-2 transition-all duration-200">
            <div className="p-4 bg-amber-50 dark:bg-amber-900 rounded-md border-2 border-amber-300 dark:border-amber-600 shadow-2xl ring-1 ring-amber-200 dark:ring-amber-700 w-[600px] max-w-[calc(100vw-8rem)] relative">
              <WarningContent onClose={onClose} />
              <div className="absolute top-full right-4 w-0 h-0 border-l-4 border-r-4 border-t-4 border-l-transparent border-r-transparent border-t-amber-300 dark:border-t-amber-600"></div>
            </div>
          </div>
        )}
      </div>
    </div>
  );
}

export function VersionWarning() {
  const [isVisible, setIsVisible] = useState(true);
  const warningRef = useRef<HTMLDivElement>(null);

  const isIntersecting = useIntersectionObserver(warningRef);
  const shouldShowFloating = isVisible && !isIntersecting;

  const handleClose = () => setIsVisible(false);

  if (!isVisible) return null;

  return (
    <div className="relative">
      <div
        ref={warningRef}
        className="mb-6 p-4 bg-amber-50 dark:bg-amber-900/20 rounded-md relative border border-amber-200 dark:border-amber-800"
      >
        <WarningContent onClose={handleClose} />
      </div>

      {shouldShowFloating && <FloatingWarning onClose={handleClose} />}
    </div>
  );
}
