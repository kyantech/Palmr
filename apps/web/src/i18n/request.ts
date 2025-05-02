import { cookies } from "next/headers";
import { getRequestConfig } from "next-intl/server";

const DEFAULT_LOCALE = "en-US";

export default getRequestConfig(async ({ locale }) => {
  const cookieStore = cookies();
  const cookiesList = await cookieStore;
  const localeCookie = cookiesList.get("NEXT_LOCALE");

  const resolvedLocale = localeCookie?.value || locale || DEFAULT_LOCALE;

  try {
    return {
      locale: resolvedLocale,
      messages: (await import(`../../messages/${resolvedLocale}.json`)).default,
    };
  } catch (error) {
    console.error(error);
    return {
      locale: DEFAULT_LOCALE,
      messages: (await import(`../../messages/${DEFAULT_LOCALE}.json`)).default,
    };
  }
});
