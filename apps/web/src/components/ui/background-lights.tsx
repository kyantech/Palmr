import { motion } from "framer-motion";

export function BackgroundLights() {
  return (
    <div className="absolute inset-0 -z-10 overflow-hidden">
      <motion.div
        animate={{
          scale: [1, 1.1, 1],
          opacity: [0.3, 0.5, 0.3],
        }}
        className="absolute -top-[20%] -left-[20%] w-[140%] h-[140%] dark:opacity-100 opacity-50"
        style={{
          background: `radial-gradient(circle, oklch(from var(--primary) l c h / 0.15) 0%, transparent 70%)`,
        }}
        transition={{
          duration: 5,
          repeat: Infinity,
          ease: "easeInOut",
        }}
      />
      <motion.div
        animate={{
          scale: [1, 1.1, 1],
          opacity: [0.3, 0.5, 0.3],
        }}
        className="absolute -bottom-[20%] -right-[20%] w-[140%] h-[140%] dark:opacity-100 opacity-50"
        style={{
          background: `radial-gradient(circle, oklch(from var(--primary) l c h / 0.15) 0%, transparent 70%)`,
        }}
        transition={{
          duration: 5,
          repeat: Infinity,
          ease: "easeInOut",
          delay: 2.5,
        }}
      />
    </div>
  );
}
