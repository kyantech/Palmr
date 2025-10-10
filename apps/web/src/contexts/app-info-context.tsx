import { create } from "zustand";

import { getAppInfo } from "@/http/endpoints";

interface AppInfoStore {
  appName: string;
  appLogo: string;
  appDescription: string;
  showAppDescription: boolean;
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
    appDescription: "",
    showAppDescription: false,
    firstAccess: null,
    isLoading: true,
  };

  const loadAppInfo = async () => {
    if (typeof window !== "undefined") {
      try {
        const response = await getAppInfo();
        set({
          appName: response.data.appName,
          appLogo: response.data.appLogo,
          appDescription: response.data.appDescription,
          showAppDescription: response.data.showAppDescription,
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
          appDescription: response.data.appDescription,
          showAppDescription: response.data.showAppDescription,
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
