import type { ReactNode } from "react";
import { HomeLayout } from "fumadocs-ui/layouts/home";

import { baseOptions } from "@/app/layout.config";
import { Particles } from "@/components/magicui/particles";

export default function Layout({ children }: { children: ReactNode }) {
  return (
    <HomeLayout {...baseOptions}>
      <Particles className="absolute w-full" />
      {children}
    </HomeLayout>
  );
}
