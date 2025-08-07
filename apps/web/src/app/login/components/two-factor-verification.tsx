"use client";

import { useState } from "react";
import { IconShield } from "@tabler/icons-react";
import { useTranslations } from "next-intl";

import { Button } from "@/components/ui/button";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { Checkbox } from "@/components/ui/checkbox";
import { Input } from "@/components/ui/input";
import { InputOTP, InputOTPGroup, InputOTPSeparator, InputOTPSlot } from "@/components/ui/input-otp";
import { Label } from "@/components/ui/label";

interface TwoFactorVerificationProps {
  twoFactorCode: string;
  setTwoFactorCode: (code: string) => void;
  onSubmit: (rememberDevice?: boolean) => void;
  error?: string;
  isSubmitting: boolean;
}

export function TwoFactorVerification({
  twoFactorCode,
  setTwoFactorCode,
  onSubmit,
  error,
  isSubmitting,
}: TwoFactorVerificationProps) {
  const t = useTranslations();
  const [showBackupCode, setShowBackupCode] = useState(false);
  const [rememberDevice, setRememberDevice] = useState(false);

  const handleSubmit = (e: React.FormEvent) => {
    e.preventDefault();
    onSubmit(rememberDevice);
  };

  const handleCodeChange = (value: string) => {
    setTwoFactorCode(value);
  };

  const handleBackupCodeChange = (e: React.ChangeEvent<HTMLInputElement>) => {
    const value = e.target.value.toUpperCase();
    setTwoFactorCode(value);
  };

  return (
    <Card className="w-full max-w-md mx-auto">
      <CardHeader className="text-center">
        <div className="flex justify-center mb-4">
          <div className="p-3 rounded-full bg-primary/10">
            <IconShield className="h-8 w-8 text-primary" />
          </div>
        </div>
        <CardTitle>{t("twoFactor.verification.title")}</CardTitle>
        <CardDescription>
          {showBackupCode ? t("twoFactor.verification.backupDescription") : t("twoFactor.verification.description")}
        </CardDescription>
      </CardHeader>
      <CardContent>
        <form onSubmit={handleSubmit} className="space-y-4">
          <div>
            <Label htmlFor="twoFactorCode" className="mb-2">
              {showBackupCode ? t("twoFactor.verification.backupCode") : t("twoFactor.verification.verificationCode")}
            </Label>
            {showBackupCode ? (
              <Input
                id="twoFactorCode"
                type="text"
                placeholder={t("twoFactor.verification.backupCodePlaceholder")}
                value={twoFactorCode}
                onChange={handleBackupCodeChange}
                className="text-center tracking-widest font-mono"
                maxLength={9}
              />
            ) : (
              <div className="flex justify-center">
                <InputOTP maxLength={6} value={twoFactorCode} onChange={handleCodeChange}>
                  <InputOTPGroup>
                    <InputOTPSlot index={0} />
                    <InputOTPSlot index={1} />
                    <InputOTPSlot index={2} />
                  </InputOTPGroup>
                  <InputOTPSeparator />
                  <InputOTPGroup>
                    <InputOTPSlot index={3} />
                    <InputOTPSlot index={4} />
                    <InputOTPSlot index={5} />
                  </InputOTPGroup>
                </InputOTP>
              </div>
            )}
          </div>

          <div className="flex items-center space-x-2">
            <Checkbox
              id="rememberDevice"
              checked={rememberDevice}
              onCheckedChange={(checked) => setRememberDevice(checked as boolean)}
            />
            <Label htmlFor="rememberDevice" className="text-sm font-normal cursor-pointer">
              {t("twoFactor.verification.rememberDevice")}
            </Label>
          </div>

          <Button
            type="submit"
            className="w-full"
            disabled={isSubmitting || twoFactorCode.length < (showBackupCode ? 8 : 6)}
          >
            {isSubmitting ? t("twoFactor.verification.verifying") : t("twoFactor.verification.verify")}
          </Button>

          {error && (
            <div className="text-sm text-destructive text-center bg-destructive/10 p-3 rounded-md">{error}</div>
          )}

          <div className="text-center">
            <Button
              type="button"
              variant="link"
              onClick={() => {
                setShowBackupCode(!showBackupCode);
                setTwoFactorCode("");
              }}
              className="text-sm text-muted-foreground hover:text-primary transition-colors"
            >
              {showBackupCode
                ? t("twoFactor.verification.useAuthenticatorCode")
                : t("twoFactor.verification.useBackupCode")}
            </Button>
          </div>
        </form>
      </CardContent>
    </Card>
  );
}
