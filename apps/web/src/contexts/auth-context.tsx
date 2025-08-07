"use client";

import { createContext, useContext, useEffect, useState } from "react";

import { getAppInfo, getCurrentUser } from "@/http/endpoints";
import type { User } from "@/http/endpoints/auth/types";

type AuthUser = Omit<User, "isAdmin">;

type AuthContextType = {
  user: AuthUser | null;
  setUser: (user: AuthUser | null) => void;
  isAuthenticated: boolean | null;
  setIsAuthenticated: (value: boolean) => void;
  isAdmin: boolean | null;
  setIsAdmin: (value: boolean) => void;
  logout: () => void;
};

const AuthContext = createContext<AuthContextType>({
  user: null,
  setUser: () => {},
  isAuthenticated: null,
  setIsAuthenticated: () => {},
  isAdmin: null,
  setIsAdmin: () => {},
  logout: () => {},
});

export function AuthProvider({ children }: { children: React.ReactNode }) {
  const [isAuthenticated, setIsAuthenticated] = useState<boolean | null>(null);
  const [user, setUser] = useState<AuthUser | null>(null);
  const [isAdmin, setIsAdmin] = useState<boolean | null>(null);

  const logout = () => {
    setUser(null);
    setIsAdmin(false);
    setIsAuthenticated(false);
  };

  useEffect(() => {
    let isMounted = true;

    const checkAuth = async () => {
      try {
        const appInfoResponse = await getAppInfo();
        const appInfo = appInfoResponse.data;

        if (!isMounted) return;

        if (appInfo.firstUserAccess) {
          setUser(null);
          setIsAdmin(false);
          setIsAuthenticated(false);
          return;
        }

        const response = await getCurrentUser();

        if (!isMounted) return;

        if (!response?.data?.user) {
          setUser(null);
          setIsAdmin(false);
          setIsAuthenticated(false);
          return;
        }

        const { isAdmin, ...userData } = response.data.user;

        setUser(userData);
        setIsAdmin(isAdmin);
        setIsAuthenticated(true);
      } catch (err) {
        if (!isMounted) return;

        console.error(err);
        setUser(null);
        setIsAdmin(false);
        setIsAuthenticated(false);
      }
    };

    checkAuth();

    return () => {
      isMounted = false;
    };
  }, []);

  return (
    <AuthContext.Provider
      value={{
        user,
        setUser,
        isAuthenticated,
        setIsAuthenticated,
        isAdmin,
        setIsAdmin,
        logout,
      }}
    >
      {children}
    </AuthContext.Provider>
  );
}

export const useAuth = () => useContext(AuthContext);
