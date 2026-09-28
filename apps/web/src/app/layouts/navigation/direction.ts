import { useTranslation } from "react-i18next";
import { isLocaleCode, localeDirection, type TextDirection } from "../../i18n/catalog";

export function useTextDirection(): TextDirection {
  const { i18n } = useTranslation();
  const language = i18n.resolvedLanguage;
  return isLocaleCode(language) ? localeDirection(language) : "ltr";
}
