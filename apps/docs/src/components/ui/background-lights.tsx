import { motion } from "motion/react";

export function BackgroundLights() {
  return (
    <div className="absolute inset-0 -z-10 overflow-hidden">
      <motion.div
        animate={{
          scale: [1, 1.1, 1],
          opacity: [0.3, 0.5, 0.3],
        }}
        className="absolute -top-[20%] -left-[20%] w-[140%] h-[140%] bg-[radial-gradient(circle,rgba(34,197,94,0.15)_0%,transparent_70%)] dark:opacity-100 opacity-50"
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
        className="absolute -bottom-[20%] -right-[20%] w-[140%] h-[140%] bg-[radial-gradient(circle,rgba(34,197,94,0.15)_0%,transparent_70%)] dark:opacity-100 opacity-50"
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
