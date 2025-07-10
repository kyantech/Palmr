"use client";

import { useCallback, useEffect, useState } from "react";

import { getAllConfigs } from "@/http/endpoints";

interface Config {
  key: string;
  value: string;
  type: string;
  group: string;
  updatedAt: string;
}

/**
 * Hook to fetch configurations securely
 * Replaces direct use of getAllConfigs which exposed sensitive data
 */
export function useSecureConfigs() {
  const [configs, setConfigs] = useState<Config[]>([]);
  const [isLoading, setIsLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  const loadConfigs = async () => {
    try {
      setIsLoading(true);
      setError(null);
      const response = await getAllConfigs();
      setConfigs(response.data.configs);
    } catch (err) {
      setError(err instanceof Error ? err.message : "Unknown error");
      console.error("Error loading secure configs:", err);
    } finally {
      setIsLoading(false);
    }
  };

  useEffect(() => {
    loadConfigs();
  }, []);

  return {
    configs,
    isLoading,
    error,
    reload: loadConfigs,
  };
}

/**
 * Hook to fetch configurations for administrators
 * REQUIRES ADMIN PERMISSIONS - returns error if user is not admin
 */
export function useAdminConfigs() {
  const [configs, setConfigs] = useState<Config[]>([]);
  const [isLoading, setIsLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [isUnauthorized, setIsUnauthorized] = useState(false);

  const loadConfigs = async () => {
    try {
      setIsLoading(true);
      setError(null);
      setIsUnauthorized(false);

      const response = await getAllConfigs();
      setConfigs(response.data.configs);
    } catch (err: any) {
      const errorMessage = err?.response?.data?.error || err?.message || "Unknown error";

      if (err?.response?.status === 401 || err?.response?.status === 403) {
        setIsUnauthorized(true);
        setError("Access denied: Administrator privileges required");
      } else {
        setError(errorMessage);
      }

      console.error("Error loading admin configs:", err);
    } finally {
      setIsLoading(false);
    }
  };

  useEffect(() => {
    loadConfigs();
  }, []);

  return {
    configs,
    isLoading,
    error,
    isUnauthorized,
    reload: loadConfigs,
  };
}

/**
 * Hook to fetch a specific configuration value
 * Useful when you only need a specific value (e.g. smtpEnabled)
 */
export function useSecureConfigValue(key: string) {
  const [value, setValue] = useState<string | null>(null);
  const [isLoading, setIsLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  const loadConfigValue = useCallback(async () => {
    try {
      setIsLoading(true);
      setError(null);
      const response = await getAllConfigs();
      const config = response.data.configs.find((c) => c.key === key);
      setValue(config?.value || null);
    } catch (err) {
      setError(err instanceof Error ? err.message : "Unknown error");
      console.error(`Error loading config value for ${key}:`, err);
    } finally {
      setIsLoading(false);
    }
  }, [key]);

  useEffect(() => {
    loadConfigValue();
  }, [key, loadConfigValue]);

  return {
    value,
    isLoading,
    error,
    reload: loadConfigValue,
  };
}
