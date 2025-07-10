"use client";

import React, { useEffect, useRef, useState } from "react";
import { X, ZoomIn, ZoomOut } from "lucide-react";

import { cn } from "@/lib/utils";

interface ZoomableImageProps {
  src: string;
  alt: string;
  legend?: string;
  className?: string;
}

export const ZoomableImage: React.FC<ZoomableImageProps> = ({ src, alt, legend, className }) => {
  const [isZoomed, setIsZoomed] = useState(false);
  const [scale, setScale] = useState(1);
  const [position, setPosition] = useState({ x: 0, y: 0 });
  const [isDragging, setIsDragging] = useState(false);
  const [dragStart, setDragStart] = useState({ x: 0, y: 0 });
  const imageRef = useRef<HTMLImageElement>(null);
  const containerRef = useRef<HTMLDivElement>(null);

  const handleImageClick = () => {
    setIsZoomed(true);
    setScale(1);
    setPosition({ x: 0, y: 0 });
  };

  const handleCloseZoom = () => {
    setIsZoomed(false);
    setScale(1);
    setPosition({ x: 0, y: 0 });
  };

  const handleBackdropClick = (e: React.MouseEvent) => {
    if (e.target === e.currentTarget) {
      setIsZoomed(false);
      setScale(1);
      setPosition({ x: 0, y: 0 });
    }
  };

  const handleZoomIn = () => {
    setScale((prev) => Math.min(prev * 1.2, 5));
  };

  const handleZoomOut = () => {
    setScale((prev) => Math.max(prev / 1.2, 0.1));
  };

  const handleMouseDown = (e: React.MouseEvent) => {
    if (scale > 1) {
      setIsDragging(true);
      setDragStart({
        x: e.clientX - position.x,
        y: e.clientY - position.y,
      });
    }
  };

  const handleMouseMove = (e: React.MouseEvent) => {
    if (isDragging && scale > 1) {
      setPosition({
        x: e.clientX - dragStart.x,
        y: e.clientY - dragStart.y,
      });
    }
  };

  const handleMouseUp = () => {
    setIsDragging(false);
  };

  // Handle ESC key press
  useEffect(() => {
    const handleKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape" && isZoomed) {
        setIsZoomed(false);
        setScale(1);
        setPosition({ x: 0, y: 0 });
      }
    };

    if (isZoomed) {
      document.addEventListener("keydown", handleKeyDown);
    }

    return () => {
      document.removeEventListener("keydown", handleKeyDown);
    };
  }, [isZoomed]);

  // Reset dragging when scale changes
  useEffect(() => {
    if (scale <= 1) {
      setIsDragging(false);
      setPosition({ x: 0, y: 0 });
    }
  }, [scale]);

  return (
    <>
      {/* Thumbnail Image */}
      <div className="relative group cursor-pointer" onClick={handleImageClick}>
        <img
          src={src}
          alt={alt}
          className={cn(
            "w-full h-auto rounded-lg border border-gray-200 dark:border-gray-700 shadow-sm transition-all duration-300 hover:shadow-md hover:border-gray-300 dark:hover:border-gray-600",
            className
          )}
        />
        {/* Zoom Icon Overlay */}
        <div className="absolute inset-0 bg-black/0 group-hover:bg-black/10 transition-all duration-300 rounded-lg flex items-center justify-center">
          <ZoomIn className="w-8 h-8 text-white opacity-0 group-hover:opacity-100 transition-opacity duration-300 drop-shadow-lg" />
        </div>
      </div>
      {legend && <p className="text-sm text-center -mt-4 font-light text-muted-foreground">{legend}</p>}

      {/* Zoomed Modal */}
      {isZoomed && (
        <div
          ref={containerRef}
          className="fixed inset-0 z-50 bg-black/80 backdrop-blur-sm flex items-center justify-center p-4"
          onClick={handleBackdropClick}
          onMouseMove={handleMouseMove}
          onMouseUp={handleMouseUp}
          onMouseLeave={handleMouseUp}
        >
          {/* Close Button */}
          <button
            onClick={handleCloseZoom}
            className="absolute top-4 right-4 z-10 p-2 bg-white/10 hover:bg-white/20 rounded-full transition-colors duration-200"
            aria-label="Close zoom"
          >
            <X className="w-6 h-6 text-white" />
          </button>

          {/* Zoom Controls */}
          <div className="absolute top-4 left-4 z-10 flex gap-2">
            <button
              onClick={handleZoomIn}
              className="p-2 bg-white/10 hover:bg-white/20 rounded-full transition-colors duration-200"
              aria-label="Zoom in"
            >
              <ZoomIn className="w-5 h-5 text-white" />
            </button>
            <button
              onClick={handleZoomOut}
              className="p-2 bg-white/10 hover:bg-white/20 rounded-full transition-colors duration-200"
              aria-label="Zoom out"
            >
              <ZoomOut className="w-5 h-5 text-white" />
            </button>
          </div>

          {/* Zoom Level Indicator */}
          <div className="absolute top-4 left-1/2 transform -translate-x-1/2 z-10 px-3 py-1 bg-white/10 rounded-full text-white text-sm">
            {Math.round(scale * 100)}%
          </div>

          {/* Zoomed Image Container */}
          <div className="relative w-full h-full flex items-center justify-center overflow-hidden">
            <img
              ref={imageRef}
              src={src}
              alt={alt}
              className={cn(
                "max-w-[90vw] max-h-[90vh] w-auto h-auto object-contain rounded-lg border border-gray-300 dark:border-gray-600 shadow-2xl transition-transform duration-200",
                scale > 1 && "cursor-grab active:cursor-grabbing"
              )}
              style={{
                transform: `scale(${scale}) translate(${position.x / scale}px, ${position.y / scale}px)`,
              }}
              onClick={(e) => e.stopPropagation()}
              onMouseDown={handleMouseDown}
              draggable={false}
            />
          </div>
        </div>
      )}
    </>
  );
};
