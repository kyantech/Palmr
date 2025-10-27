import { useTranslations } from "next-intl";

import { AspectRatio } from "@/components/ui/aspect-ratio";

interface VideoPreviewProps {
  src: string;
}

export function VideoPreview({ src }: VideoPreviewProps) {
  const t = useTranslations();

  return (
    <AspectRatio ratio={16 / 9} className="bg-muted">
      <video controls className="w-full h-full rounded-lg object-contain" preload="metadata">
        <source src={src} />
        {t("filePreview.videoNotSupported")}
      </video>
    </AspectRatio>
  );
}
