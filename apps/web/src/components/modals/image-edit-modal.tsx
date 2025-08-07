"use client";

import { useCallback, useEffect, useRef, useState } from "react";
import { IconCheck, IconRotateClockwise, IconX, IconZoomIn, IconZoomOut } from "@tabler/icons-react";
import { useTranslations } from "next-intl";
import ReactCrop, { centerCrop, Crop, makeAspectCrop, PixelCrop } from "react-image-crop";

import "react-image-crop/dist/ReactCrop.css";

import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Skeleton } from "@/components/ui/skeleton";
import { Slider } from "@/components/ui/slider";

interface ImageEditModalProps {
  isOpen: boolean;
  onClose: () => void;
  onSave: (croppedImageFile: File) => void;
  imageFile: File | null;
}

function centerAspectCrop(mediaWidth: number, mediaHeight: number, aspect: number) {
  return centerCrop(
    makeAspectCrop(
      {
        unit: "%",
        width: 90,
      },
      aspect,
      mediaWidth,
      mediaHeight
    ),
    mediaWidth,
    mediaHeight
  );
}

export function ImageEditModal({ isOpen, onClose, onSave, imageFile }: ImageEditModalProps) {
  const t = useTranslations();
  const [crop, setCrop] = useState<Crop>();
  const [completedCrop, setCompletedCrop] = useState<PixelCrop>();
  const [scale, setScale] = useState(1);
  const [rotate, setRotate] = useState(0);
  const aspect = 1;
  const [imageSrc, setImageSrc] = useState<string>("");
  const [isLoading, setIsLoading] = useState(false);
  const [isImageLoading, setIsImageLoading] = useState(false);

  const imgRef = useRef<HTMLImageElement>(null);
  const previewCanvasRef = useRef<HTMLCanvasElement>(null);

  useEffect(() => {
    if (imageFile) {
      setIsImageLoading(true);
      const reader = new FileReader();
      reader.onload = (e) => {
        setImageSrc(e.target?.result as string);
        setIsImageLoading(false);
      };
      reader.onerror = () => {
        setIsImageLoading(false);
      };
      reader.readAsDataURL(imageFile);
    }
  }, [imageFile]);

  const onImageLoad = useCallback(
    (e: React.SyntheticEvent<HTMLImageElement>) => {
      if (aspect) {
        const { width, height } = e.currentTarget;
        setCrop(centerAspectCrop(width, height, aspect));
      }
    },
    [aspect]
  );

  const handleRotate = () => {
    setRotate((prev) => (prev + 90) % 360);
  };

  const handleZoomIn = () => {
    setScale((prev) => Math.min(prev + 0.1, 3));
  };

  const handleZoomOut = () => {
    setScale((prev) => Math.max(prev - 0.1, 0.5));
  };

  const handleScaleChange = (value: number[]) => {
    setScale(value[0]);
  };

  const getCroppedImage = useCallback(async (): Promise<File | null> => {
    if (!completedCrop || !imgRef.current || !previewCanvasRef.current) {
      return null;
    }

    const image = imgRef.current;
    const canvas = previewCanvasRef.current;
    const crop = completedCrop;

    const scaleX = image.naturalWidth / image.width;
    const scaleY = image.naturalHeight / image.height;
    const ctx = canvas.getContext("2d");

    if (!ctx) {
      return null;
    }

    const pixelRatio = window.devicePixelRatio;
    canvas.width = crop.width * pixelRatio * scaleX;
    canvas.height = crop.height * pixelRatio * scaleY;

    ctx.setTransform(pixelRatio, 0, 0, pixelRatio, 0, 0);
    ctx.imageSmoothingQuality = "high";

    const cropX = crop.x * scaleX;
    const cropY = crop.y * scaleY;

    const rotateRads = rotate * (Math.PI / 180);
    const centerX = image.naturalWidth / 2;
    const centerY = image.naturalHeight / 2;

    ctx.save();

    ctx.translate(-cropX, -cropY);
    ctx.translate(centerX, centerY);
    ctx.rotate(rotateRads);
    ctx.scale(scale, scale);
    ctx.translate(-centerX, -centerY);

    ctx.drawImage(image, 0, 0);

    ctx.restore();

    return new Promise<File>((resolve) => {
      canvas.toBlob(
        (blob) => {
          if (blob) {
            const file = new File([blob], "cropped-image.png", { type: "image/png" });
            resolve(file);
          }
        },
        "image/png",
        1
      );
    });
  }, [completedCrop, scale, rotate]);

  const handleSave = async () => {
    try {
      setIsLoading(true);
      const croppedImageFile = await getCroppedImage();
      if (croppedImageFile) {
        onSave(croppedImageFile);
      }
    } catch (error) {
      console.error("Error cropping image:", error);
    } finally {
      setIsLoading(false);
    }
  };

  const handleClose = () => {
    setImageSrc("");
    setCrop(undefined);
    setCompletedCrop(undefined);
    setScale(1);
    setRotate(0);
    setIsImageLoading(false);
    onClose();
  };

  return (
    <Dialog open={isOpen} onOpenChange={handleClose}>
      <DialogContent className="sm:max-w-2xl h-fit overflow-hidden flex flex-col">
        <DialogHeader>
          <DialogTitle>{t("imageEdit.title")}</DialogTitle>
        </DialogHeader>

        <div className="flex-1 overflow-auto">
          {isImageLoading ? (
            <div className="space-y-4">
              <div className="flex items-center gap-4 p-4 bg-muted/50 rounded-lg">
                <div className="flex items-center gap-2">
                  <Skeleton className="h-8 w-20" />
                  <Skeleton className="h-8 w-10" />
                  <Skeleton className="h-8 w-10" />
                </div>
                <div className="flex-1 max-w-xs">
                  <Skeleton className="h-4 w-16 mb-2" />
                  <Skeleton className="h-2 w-full" />
                </div>
              </div>
              <div className="flex justify-center">
                <Skeleton className="w-96 h-96" />
              </div>
            </div>
          ) : imageSrc ? (
            <div className="space-y-4">
              <div className="flex items-center gap-4 p-4 bg-muted/50 rounded-lg">
                <div className="flex items-center gap-2">
                  <Button variant="outline" size="sm" onClick={handleRotate} disabled={isLoading}>
                    <IconRotateClockwise className="h-4 w-4" />
                    {t("imageEdit.rotate")}
                  </Button>
                  <Button variant="outline" size="sm" onClick={handleZoomOut} disabled={isLoading || scale <= 0.5}>
                    <IconZoomOut className="h-4 w-4" />
                  </Button>
                  <Button variant="outline" size="sm" onClick={handleZoomIn} disabled={isLoading || scale >= 3}>
                    <IconZoomIn className="h-4 w-4" />
                  </Button>
                </div>
                <div className="flex-1 max-w-xs">
                  <label className="text-sm font-medium mb-2 block">
                    {t("imageEdit.zoom")}: {Math.round(scale * 100)}%
                  </label>
                  <Slider
                    value={[scale]}
                    onValueChange={handleScaleChange}
                    max={3}
                    min={0.5}
                    step={0.1}
                    className="w-full"
                    disabled={isLoading}
                  />
                </div>
              </div>

              <div className="flex justify-center">
                <ReactCrop
                  crop={crop}
                  onChange={(c) => setCrop(c)}
                  onComplete={(c) => setCompletedCrop(c)}
                  aspect={aspect}
                  minWidth={100}
                  minHeight={100}
                  keepSelection
                  className="max-w-full max-h-70%"
                >
                  <img
                    ref={imgRef}
                    alt="Crop me"
                    src={imageSrc}
                    style={{
                      transform: `scale(${scale}) rotate(${rotate}deg)`,
                      maxWidth: "100%",
                      maxHeight: "400px",
                    }}
                    onLoad={onImageLoad}
                  />
                </ReactCrop>
              </div>
            </div>
          ) : null}
        </div>

        <DialogFooter className="flex gap-2">
          <Button variant="outline" onClick={handleClose} disabled={isLoading}>
            <IconX className="h-4 w-4" />
            {t("common.cancel")}
          </Button>
          <Button onClick={handleSave} disabled={isLoading || !completedCrop}>
            <IconCheck className="h-4 w-4" />
            {isLoading ? t("common.saving") : t("common.save")}
          </Button>
        </DialogFooter>

        {/* Hidden canvas for generating cropped image */}
        <canvas
          ref={previewCanvasRef}
          style={{
            display: "none",
          }}
        />
      </DialogContent>
    </Dialog>
  );
}
