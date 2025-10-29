"use client";

import { useRouter } from "next/navigation";
import { IconLanguage } from "@tabler/icons-react";
import { useLocale } from "next-intl";
import { setCookie } from "nookies";
import ReactCountryFlag from "react-country-flag";

import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";

const languages = {
  "en-US": "English",
  "pt-BR": "Português",
  "fr-FR": "Français",
  "es-ES": "Español",
  "de-DE": "Deutsch",
  "it-IT": "Italiano",
  "nl-NL": "Nederlands",
  "pl-PL": "Polski",
  "tr-TR": "Türkçe (Turkish)",
  "ru-RU": "Русский (Russian)",
  "hi-IN": "हिन्दी (Hindi)",
  "ar-SA": "العربية (Arabic)",
  "zh-CN": "中文 (Chinese)",
  "ja-JP": "日本語 (Japanese)",
  "ko-KR": "한국어 (Korean)",
  "th-TH": "ไทย (Thai)",
  "vi-VN": "Tiếng Việt (Vietnamese)",
  "uk-UA": "Українська (Ukrainian)",
  "fa-IR": "فارسی (Persian)",
  "sv-SE": "Svenska (Swedish)",
  "id-ID": "Bahasa Indonesia (Indonesian)",
  "el-GR": "Ελληνικά (Greek)",
  "he-IL": "עברית (Hebrew)",
};

const COOKIE_LANG_KEY = "NEXT_LOCALE";
const COOKIE_MAX_AGE = 365 * 24 * 60 * 60;

const RTL_LANGUAGES = ["ar-SA", "fa-IR", "he-IL"];

export function LanguageSwitcher() {
  const locale = useLocale();
  const router = useRouter();

  const changeLanguage = (fullLocale: string) => {
    const isRTL = RTL_LANGUAGES.includes(fullLocale);
    document.documentElement.dir = isRTL ? "rtl" : "ltr";

    setCookie(null, COOKIE_LANG_KEY, fullLocale, {
      maxAge: COOKIE_MAX_AGE,
      path: "/",
      sameSite: "lax",
      secure: window.location.protocol === "https:",
    });

    router.refresh();
  };

  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <Button variant="ghost" size="icon" className="h-9 w-9 p-0">
          <IconLanguage className="h-5 w-5" />
          <span className="sr-only">Change language</span>
        </Button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="end">
        {Object.entries(languages).map(([code, name]) => {
          const isCurrentLocale = locale === code;

          return (
            <DropdownMenuItem
              key={code}
              onClick={() => changeLanguage(code)}
              className={isCurrentLocale ? "bg-accent" : ""}
            >
              <ReactCountryFlag
                svg
                countryCode={code.split("-")[1]}
                style={{
                  marginRight: "8px",
                  width: "1em",
                  height: "1em",
                }}
              />
              {name}
            </DropdownMenuItem>
          );
        })}
      </DropdownMenuContent>
    </DropdownMenu>
  );
}
