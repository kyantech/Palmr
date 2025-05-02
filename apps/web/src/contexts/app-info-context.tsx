import { create } from "zustand";

import { getAppInfo } from "@/http/endpoints";

interface AppInfoStore {
  appName: string;
  appLogo: string;
  firstAccess: boolean | null;
  isLoading: boolean;
  setAppName: (name: string) => void;
  setAppLogo: (logo: string) => void;
  refreshAppInfo: () => Promise<void>;
}

const updateTitle = (name: string) => {
  document.title = name;
};

export const useAppInfo = create<AppInfoStore>((set) => {
  const initialState = {
    appName: "",
    appLogo: "",
    firstAccess: null,
    isLoading: true,
  };

  const loadAppInfo = async () => {
    if (typeof window !== "undefined") {
      try {
        const response = await getAppInfo();
        console.log(
          "firstUserAccess value:",
          response.data.firstUserAccess,
          "type:",
          typeof response.data.firstUserAccess
        );
        set({
          appName: response.data.appName,
          appLogo: response.data.appLogo,
          firstAccess: response.data.firstUserAccess,
          isLoading: false,
        });
        updateTitle(response.data.appName);
      } catch (error) {
        console.error("Failed to fetch app info:", error);
        set({ isLoading: false });
      }
    }
  };

  loadAppInfo();

  return {
    ...initialState,
    setAppName: (name: string) => {
      set({ appName: name });
      updateTitle(name);
    },
    setAppLogo: (logo: string) => {
      set({ appLogo: logo });
    },
    refreshAppInfo: async () => {
      set({ isLoading: true });
      try {
        const response = await getAppInfo();
        set({
          appName: response.data.appName,
          appLogo: response.data.appLogo,
          firstAccess: response.data.firstUserAccess,
          isLoading: false,
        });
        updateTitle(response.data.appName);
      } catch (error) {
        console.error("Failed to fetch app info:", error);
        set({ isLoading: false });
      }
    },
  };
});
