import type { BaseLayoutProps } from "fumadocs-ui/layouts/shared";
import { Github, Palmtree } from "lucide-react";

import { LATEST_VERSION_PATH } from "@/config/constants";

export const baseOptions: BaseLayoutProps = {
  nav: {
    title: (
      <div className="flex items-start gap-1.5">
        <Palmtree className="text-[#52822D]" />
        <span className="text-xl font-medium">Palmr.</span>
      </div>
    ),
  },
  links: [
    {
      text: "Docs",
      url: LATEST_VERSION_PATH,
      active: "nested-url",
    },
    {
      text: "Github",
      url: "https://github.com/kyantech/Palmr",
      active: "nested-url",
      icon: (
        <>
          <Github fill="currentColor" />
        </>
      ),
    },
  ],
};
