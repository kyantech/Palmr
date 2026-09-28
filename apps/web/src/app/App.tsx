import { createQueryClient } from "../shared/api/queryClient";
import { AppTree } from "./AppTree";
import { createI18n } from "./i18n/i18n";
import { browserLanguages } from "./i18n/resolveLocale";
import { createAppRouter } from "./router/routes";
import { readCspNonce } from "./theme/cspNonce";

const cspNonce = readCspNonce();
const queryClient = createQueryClient();
const i18n = createI18n();
const router = createAppRouter();
const languages = browserLanguages();

export function App() {
  return (
    <AppTree
      queryClient={queryClient}
      i18n={i18n}
      router={router}
      cspNonce={cspNonce}
      browserLanguages={languages}
    />
  );
}
