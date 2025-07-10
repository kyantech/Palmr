import { motion } from "framer-motion";
import { Palmtree } from "lucide-react";
import { useTranslations } from "next-intl";

import { HomeHeaderProps } from "../types";

const fadeInUpAnimation = {
  animate: { opacity: 1, y: 0 },
  initial: { opacity: 0, y: 20 },
  transition: { duration: 0.5 },
};

const titleAnimation = {
  animate: { scale: 1 },
  initial: { scale: 0.9 },
  transition: { duration: 0.3 },
};

const slideInAnimation = (delay: number, direction: -1 | 1) => ({
  animate: { opacity: 1, x: 0 },
  initial: { opacity: 0, x: 20 * direction },
  transition: { delay, duration: 0.5 },
});

export function HomeHeader({ title }: HomeHeaderProps) {
  const t = useTranslations();

  return (
    <motion.div {...fadeInUpAnimation} className="inline-block max-w-xl text-center justify-center">
      <div className="flex flex-col gap-8">
        <motion.span
          {...titleAnimation}
          className="text-4xl lg:text-6xl font-extrabold tracking-tight flex mx-auto items-end gap-3"
        >
          <Palmtree className="h-18 w-18" /> {title}
        </motion.span>
        <div className="flex flex-col gap-2">
          <motion.span
            {...slideInAnimation(0.2, -1)}
            className="text-4xl lg:text-5xl font-semibold tracking-tight text-primary"
          >
            {t("home.header.fileSharing")}
          </motion.span>
          <motion.span
            {...slideInAnimation(0.4, 1)}
            className="text-[2.3rem] lg:text-5xl leading-9 font-semibold tracking-tight"
          >
            {t("home.header.tagline")}
          </motion.span>
        </div>
      </div>
    </motion.div>
  );
}
