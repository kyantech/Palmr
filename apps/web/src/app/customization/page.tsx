"use client";

import { IconPalette } from "@tabler/icons-react";
import { useTranslations } from "next-intl";

import { ProtectedRoute } from "@/components/auth/protected-route";
import { FileManagerLayout } from "@/components/layout/file-manager-layout";
import { BackgroundPickerForm } from "./components/background-picker-form";
import { ColorPickerForm } from "./components/color-picker-form";
import { FontPickerForm } from "./components/font-picker-form";
import { RadiusPickerForm } from "./components/radius-picker-form";
import { ThemePickerForm } from "./components/theme-picker-form";

export default function CustomizationPage() {
  const t = useTranslations();

  return (
    <ProtectedRoute>
      <FileManagerLayout
        breadcrumbLabel={t("customization.breadcrumb")}
        icon={<IconPalette size={20} />}
        title={t("customization.pageTitle")}
      >
        <div className="flex flex-col gap-6">
          <ThemePickerForm />
          <ColorPickerForm />
          <FontPickerForm />
          <RadiusPickerForm />
          <BackgroundPickerForm />
        </div>
      </FileManagerLayout>
    </ProtectedRoute>
  );
}
