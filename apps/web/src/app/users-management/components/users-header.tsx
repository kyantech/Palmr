import Link from "next/link";
import { IconLayoutDashboard, IconLink, IconUserPlus, IconUsers } from "@tabler/icons-react";
import { useTranslations } from "next-intl";

import {
  Breadcrumb,
  BreadcrumbItem,
  BreadcrumbLink,
  BreadcrumbList,
  BreadcrumbSeparator,
} from "@/components/ui/breadcrumb";
import { Button } from "@/components/ui/button";
import { Separator } from "@/components/ui/separator";
import { UsersHeaderProps } from "../types";

export function UsersHeader({ onCreateUser, onGenerateInvite }: UsersHeaderProps) {
  const t = useTranslations();

  return (
    <div className="flex flex-col gap-4">
      <div className="flex flex-row items-center justify-between">
        <div className="flex items-center gap-2">
          <IconUsers className="text-2xl" />
          <h1 className="text-2xl font-bold">{t("users.header.title")}</h1>
        </div>
        <div className="flex gap-2">
          <Button variant="outline" className="font-semibold" onClick={onGenerateInvite}>
            <IconLink size={18} />
            {t("users.invite.button")}
          </Button>
          <Button className="font-semibold" onClick={onCreateUser}>
            <IconUserPlus size={18} />
            {t("users.header.addUser")}
          </Button>
        </div>
      </div>
      <Separator />
      <Breadcrumb>
        <BreadcrumbList>
          <BreadcrumbItem>
            <BreadcrumbLink asChild>
              <Link href="/dashboard" className="flex items-center">
                <IconLayoutDashboard size={20} className="mr-2" />
                {t("common.dashboard")}
              </Link>
            </BreadcrumbLink>
          </BreadcrumbItem>
          <BreadcrumbSeparator />
          <BreadcrumbItem>
            <span className="flex items-center gap-2">
              <IconUsers size={20} /> {t("users.header.management")}
            </span>
          </BreadcrumbItem>
        </BreadcrumbList>
      </Breadcrumb>
    </div>
  );
}
