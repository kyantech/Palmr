"use client";

import { useState } from "react";
import {
  IconAlertTriangle,
  IconCalendar,
  IconClock,
  IconCopy,
  IconDeviceDesktop,
  IconDeviceMobile,
  IconDevices,
  IconDownload,
  IconEye,
  IconEyeClosed,
  IconKey,
  IconShield,
  IconShieldCheck,
  IconTrash,
} from "@tabler/icons-react";
import { useTranslations } from "next-intl";

import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { InputOTP, InputOTPGroup, InputOTPSlot } from "@/components/ui/input-otp";
import { Label } from "@/components/ui/label";
import { Separator } from "@/components/ui/separator";
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "@/components/ui/table";
import { useTrustedDevices } from "../hooks/use-trusted-devices";
import { useTwoFactor } from "../hooks/use-two-factor";

export function TwoFactorForm() {
  const t = useTranslations();
  const {
    isLoading,
    status,
    setupData,
    backupCodes,
    verificationCode,
    disablePassword,
    isSetupModalOpen,
    isDisableModalOpen,
    isBackupCodesModalOpen,
    setVerificationCode,
    setDisablePassword,
    setIsSetupModalOpen,
    setIsDisableModalOpen,
    setIsBackupCodesModalOpen,
    startSetup,
    verifySetup,
    disable2FA,
    generateNewBackupCodes,
    downloadBackupCodes,
    copyBackupCodes,
  } = useTwoFactor();

  const {
    isLoading: devicesLoading,
    devices,
    isRemoveModalOpen,
    isRemoveAllModalOpen,
    deviceToRemove,
    isRemoving,
    setIsRemoveModalOpen,
    setIsRemoveAllModalOpen,
    handleRemoveDevice,
    confirmRemoveDevice,
    handleRemoveAllDevices,
    confirmRemoveAllDevices,
    formatDeviceName,
    formatDate,
  } = useTrustedDevices();

  const [showPassword, setShowPassword] = useState(false);

  const getDeviceIcon = (userAgent: string) => {
    if (!userAgent) return IconDevices;

    if (userAgent.includes("iPhone") || userAgent.includes("Android") || userAgent.includes("Mobile")) {
      return IconDeviceMobile;
    }

    return IconDeviceDesktop;
  };

  if (isLoading) {
    return (
      <Card>
        <CardHeader>
          <CardTitle className="flex items-center gap-2">
            <IconShield className="h-5 w-5" />
            {t("twoFactor.title")}
          </CardTitle>
          <CardDescription>{t("common.loadingSimple")}</CardDescription>
        </CardHeader>
      </Card>
    );
  }

  return (
    <>
      <Card>
        <CardHeader>
          <CardTitle className="flex items-center gap-2">
            {status.enabled ? (
              <IconShieldCheck className="h-5 w-5 text-green-600" />
            ) : (
              <IconShield className="h-5 w-5" />
            )}
            {t("twoFactor.title")}
          </CardTitle>
          <CardDescription>{status.enabled ? t("twoFactor.enabled") : t("twoFactor.description")}</CardDescription>
        </CardHeader>
        <CardContent className="space-y-4">
          <div className="flex flex-col sm:flex-row sm:items-center sm:justify-between gap-4">
            <div>
              <p className="font-medium">
                {t("twoFactor.status.label")}{" "}
                {status.enabled ? t("twoFactor.status.enabled") : t("twoFactor.status.disabled")}
              </p>
              {status.enabled && (
                <p className="text-sm text-muted-foreground">
                  {t("twoFactor.backupCodes.available", { count: status.availableBackupCodes })}
                </p>
              )}
            </div>
            <div className="flex flex-col sm:flex-row gap-2 w-full sm:w-auto">
              {status.enabled ? (
                <>
                  <Button
                    variant="outline"
                    onClick={generateNewBackupCodes}
                    disabled={isLoading}
                    className="w-full sm:w-auto"
                  >
                    <IconKey className="h-4 w-4" />
                    {t("twoFactor.backupCodes.generateNew")}
                  </Button>
                  <Button
                    variant="destructive"
                    onClick={() => setIsDisableModalOpen(true)}
                    disabled={isLoading}
                    className="w-full sm:w-auto"
                  >
                    {t("twoFactor.buttons.disable2FA")}
                  </Button>
                </>
              ) : (
                <Button onClick={startSetup} disabled={isLoading} className="w-full sm:w-auto">
                  <IconShield className="h-4 w-4" />
                  {t("twoFactor.buttons.enable2FA")}
                </Button>
              )}
            </div>
          </div>

          {/* Trusted Devices Section - Only shown when 2FA is enabled */}
          {status.enabled && (
            <>
              <Separator className="my-6" />

              <div className="space-y-4">
                <div className="flex items-center justify-between">
                  <div className="flex items-center gap-2">
                    <IconDevices className="h-5 w-5 text-muted-foreground" />
                    <h3 className="text-lg font-semibold">{t("twoFactor.trustedDevices.title")}</h3>
                  </div>
                  {devices.length > 0 && (
                    <Button
                      variant="ghost"
                      size="sm"
                      onClick={handleRemoveAllDevices}
                      disabled={isRemoving}
                      className="text-muted-foreground hover:text-destructive"
                    >
                      <IconTrash className="h-4 w-4" />
                      {t("twoFactor.trustedDevices.removeAll")}
                    </Button>
                  )}
                </div>
                <p className="text-sm text-muted-foreground">{t("twoFactor.trustedDevices.description")}</p>

                {devicesLoading ? (
                  <div className="text-center py-8">
                    <p className="text-muted-foreground">{t("twoFactor.trustedDevices.loading")}</p>
                  </div>
                ) : devices.length === 0 ? (
                  <div className="text-center py-12">
                    <IconDevices className="h-12 w-12 mx-auto text-muted-foreground mb-4" />
                    <p className="text-muted-foreground text-sm">{t("twoFactor.trustedDevices.noDevices")}</p>
                    <p className="text-xs text-muted-foreground/70 mt-2">
                      {t("twoFactor.trustedDevices.noDevicesDescription")}
                    </p>
                  </div>
                ) : (
                  <div className="border rounded-lg overflow-hidden">
                    <Table>
                      <TableHeader>
                        <TableRow className="bg-muted/50">
                          <TableHead className="w-12"></TableHead>
                          <TableHead className="font-semibold">
                            {t("twoFactor.trustedDevices.tableHeaders.device")}
                          </TableHead>
                          <TableHead className="font-semibold">
                            {t("twoFactor.trustedDevices.tableHeaders.added")}
                          </TableHead>
                          <TableHead className="font-semibold">
                            {t("twoFactor.trustedDevices.tableHeaders.expires")}
                          </TableHead>
                          <TableHead className="font-semibold">
                            {t("twoFactor.trustedDevices.tableHeaders.lastUsed")}
                          </TableHead>
                          <TableHead className="font-semibold">
                            {t("twoFactor.trustedDevices.tableHeaders.ipAddress")}
                          </TableHead>
                          <TableHead className="w-20 text-center font-semibold">
                            {t("twoFactor.trustedDevices.tableHeaders.actions")}
                          </TableHead>
                        </TableRow>
                      </TableHeader>
                      <TableBody>
                        {devices.map((device) => {
                          const isExpired = new Date(device.expiresAt) < new Date();
                          const DeviceIcon = getDeviceIcon(device.userAgent || "");

                          return (
                            <TableRow key={device.id}>
                              <TableCell className="text-center">
                                <DeviceIcon className="h-4 w-4 text-muted-foreground" />
                              </TableCell>
                              <TableCell>
                                <div className="flex items-center gap-2">
                                  <span className={`font-medium ${isExpired ? "text-muted-foreground" : ""}`}>
                                    {formatDeviceName(device)}
                                  </span>
                                  {isExpired && (
                                    <Badge variant="secondary" className="text-xs text-destructive">
                                      {t("twoFactor.trustedDevices.status.expired")}
                                    </Badge>
                                  )}
                                </div>
                              </TableCell>
                              <TableCell>
                                <div className="flex items-center gap-1 text-sm text-muted-foreground">
                                  <IconCalendar className="h-3 w-3" />
                                  <span>{formatDate(device.createdAt)}</span>
                                </div>
                              </TableCell>
                              <TableCell>
                                <div
                                  className={`flex items-center gap-1 text-sm ${isExpired ? "text-destructive" : "text-muted-foreground"}`}
                                >
                                  <IconClock className="h-3 w-3" />
                                  <span>{formatDate(device.expiresAt)}</span>
                                </div>
                              </TableCell>
                              <TableCell>
                                {device.lastUsedAt ? (
                                  <div className="flex items-center gap-1 text-sm text-muted-foreground">
                                    <IconClock className="h-3 w-3" />
                                    <span>{formatDate(device.lastUsedAt)}</span>
                                  </div>
                                ) : (
                                  <span className="text-sm text-muted-foreground">
                                    {t("twoFactor.trustedDevices.status.never")}
                                  </span>
                                )}
                              </TableCell>
                              <TableCell>
                                <span className="text-sm text-muted-foreground font-mono">{device.ipAddress}</span>
                              </TableCell>
                              <TableCell className="text-center">
                                <Button
                                  variant="ghost"
                                  size="sm"
                                  onClick={() => handleRemoveDevice(device)}
                                  disabled={isRemoving}
                                  className="text-muted-foreground hover:text-destructive h-8 w-8 p-0"
                                >
                                  <IconTrash className="h-4 w-4" />
                                </Button>
                              </TableCell>
                            </TableRow>
                          );
                        })}
                      </TableBody>
                    </Table>
                  </div>
                )}
              </div>
            </>
          )}
        </CardContent>
      </Card>

      {/* Setup Modal */}
      <Dialog open={isSetupModalOpen} onOpenChange={setIsSetupModalOpen}>
        <DialogContent className="max-w-md">
          <DialogHeader>
            <DialogTitle>{t("twoFactor.setup.title")}</DialogTitle>
            <DialogDescription>{t("twoFactor.setup.description")}</DialogDescription>
          </DialogHeader>

          {setupData && (
            <div className="space-y-4">
              {/* QR Code */}
              <div className="flex justify-center">
                <img src={setupData.qrCode} alt="2FA QR Code" className="w-48 h-48 border rounded-lg" />
              </div>

              {/* Manual Entry */}
              <div>
                <Label className="text-sm font-medium">{t("twoFactor.setup.manualEntryKey")}</Label>
                <div className="flex items-center gap-2 mt-1">
                  <Input value={setupData.manualEntryKey} readOnly className="font-mono text-xs" />
                  <Button
                    size="sm"
                    variant="outline"
                    onClick={() => navigator.clipboard.writeText(setupData.manualEntryKey)}
                  >
                    <IconCopy className="h-4 w-4" />
                  </Button>
                </div>
              </div>

              {/* Verification Code */}
              <div>
                <Label htmlFor="verification-code" className="mb-2">
                  {t("twoFactor.setup.verificationCode")}
                </Label>
                <div className="flex justify-start">
                  <InputOTP maxLength={6} value={verificationCode} onChange={setVerificationCode}>
                    <InputOTPGroup>
                      <InputOTPSlot index={0} />
                      <InputOTPSlot index={1} />
                      <InputOTPSlot index={2} />
                      <InputOTPSlot index={3} />
                      <InputOTPSlot index={4} />
                      <InputOTPSlot index={5} />
                    </InputOTPGroup>
                  </InputOTP>
                </div>
                <span className="text-sm text-muted-foreground mt-1">
                  {t("twoFactor.setup.verificationCodeDescription")}
                </span>
              </div>
            </div>
          )}

          <DialogFooter>
            <Button variant="outline" onClick={() => setIsSetupModalOpen(false)} disabled={isLoading}>
              {t("twoFactor.setup.cancel")}
            </Button>
            <Button onClick={verifySetup} disabled={isLoading || !verificationCode || verificationCode.length !== 6}>
              {t("twoFactor.setup.verifyAndEnable")}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>

      {/* Disable Modal */}
      <Dialog open={isDisableModalOpen} onOpenChange={setIsDisableModalOpen}>
        <DialogContent className="max-w-md">
          <DialogHeader>
            <DialogTitle>{t("twoFactor.disable.title")}</DialogTitle>
            <DialogDescription>{t("twoFactor.disable.description")}</DialogDescription>
          </DialogHeader>

          <div className="space-y-4">
            <div>
              <Label htmlFor="disable-password" className="mb-2">
                {t("twoFactor.disable.password")}
              </Label>
              <div className="relative">
                <Input
                  id="disable-password"
                  type={showPassword ? "text" : "password"}
                  placeholder={t("twoFactor.disable.passwordPlaceholder")}
                  value={disablePassword}
                  onChange={(e) => setDisablePassword(e.target.value)}
                />
                <Button
                  type="button"
                  variant="ghost"
                  size="sm"
                  className="absolute right-0 top-0 h-full px-3 py-2 hover:bg-transparent"
                  onClick={() => setShowPassword(!showPassword)}
                >
                  {showPassword ? (
                    <IconEye className="h-4 w-4 text-muted-foreground" />
                  ) : (
                    <IconEyeClosed className="h-4 w-4 text-muted-foreground" />
                  )}
                </Button>
              </div>
            </div>
          </div>

          <DialogFooter>
            <Button variant="outline" onClick={() => setIsDisableModalOpen(false)} disabled={isLoading}>
              {t("twoFactor.disable.cancel")}
            </Button>
            <Button variant="destructive" onClick={disable2FA} disabled={isLoading || !disablePassword}>
              {t("twoFactor.disable.confirm")}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>

      {/* Backup Codes Modal */}
      <Dialog open={isBackupCodesModalOpen} onOpenChange={setIsBackupCodesModalOpen}>
        <DialogContent className="max-w-md">
          <DialogHeader>
            <DialogTitle>{t("twoFactor.backupCodes.title")}</DialogTitle>
            <DialogDescription>{t("twoFactor.backupCodes.description")}</DialogDescription>
          </DialogHeader>

          <div className="space-y-4">
            <div className="bg-muted p-4 rounded-lg">
              <div className="grid grid-cols-2 gap-2 font-mono text-sm">
                {backupCodes.map((code, index) => (
                  <div key={index} className="text-center py-1">
                    {code}
                  </div>
                ))}
              </div>
            </div>

            <div className="flex gap-2">
              <Button variant="outline" onClick={downloadBackupCodes} className="flex-1">
                <IconDownload className="h-4 w-4" />
                {t("twoFactor.backupCodes.download")}
              </Button>
              <Button variant="outline" onClick={copyBackupCodes} className="flex-1">
                <IconCopy className="h-4 w-4" />
                {t("twoFactor.backupCodes.copyToClipboard")}
              </Button>
            </div>

            <div className="text-sm text-muted-foreground">
              {t.raw("twoFactor.backupCodes.instructions").map((instruction: string, index: number) => (
                <p key={index}>{instruction}</p>
              ))}
            </div>
          </div>

          <DialogFooter>
            <Button onClick={() => setIsBackupCodesModalOpen(false)}>{t("twoFactor.backupCodes.savedMessage")}</Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>

      {/* Remove Device Modal */}
      <Dialog open={isRemoveModalOpen} onOpenChange={setIsRemoveModalOpen}>
        <DialogContent className="max-w-md">
          <DialogHeader>
            <DialogTitle className="flex items-center gap-2">
              <IconAlertTriangle className="h-5 w-5 text-destructive" />
              {t("twoFactor.trustedDevices.modals.removeDevice.title")}
            </DialogTitle>
            <DialogDescription>{t("twoFactor.trustedDevices.confirmRemove")}</DialogDescription>
          </DialogHeader>

          {deviceToRemove && (
            <div className="p-4 bg-muted rounded-lg">
              <p className="font-medium">{formatDeviceName(deviceToRemove)}</p>
              <p className="text-sm text-muted-foreground">
                {t("twoFactor.trustedDevices.modals.removeDevice.added")} {formatDate(deviceToRemove.createdAt)}
              </p>
              <p className="text-xs text-muted-foreground">
                {t("twoFactor.trustedDevices.modals.removeDevice.ip")} {deviceToRemove.ipAddress}
              </p>
            </div>
          )}

          <DialogFooter>
            <Button variant="outline" onClick={() => setIsRemoveModalOpen(false)} disabled={isRemoving}>
              {t("twoFactor.trustedDevices.modals.buttons.cancel")}
            </Button>
            <Button variant="destructive" onClick={confirmRemoveDevice} disabled={isRemoving}>
              {isRemoving
                ? t("twoFactor.trustedDevices.modals.buttons.removing")
                : t("twoFactor.trustedDevices.modals.buttons.removeDevice")}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>

      {/* Remove All Devices Modal */}
      <Dialog open={isRemoveAllModalOpen} onOpenChange={setIsRemoveAllModalOpen}>
        <DialogContent className="max-w-md">
          <DialogHeader>
            <DialogTitle className="flex items-center gap-2">
              <IconAlertTriangle className="h-5 w-5 text-destructive" />
              {t("twoFactor.trustedDevices.modals.removeAllDevices.title")}
            </DialogTitle>
            <DialogDescription>{t("twoFactor.trustedDevices.confirmRemoveAll")}</DialogDescription>
          </DialogHeader>

          <div className="p-4 bg-muted rounded-lg">
            <p className="text-sm">
              {t("twoFactor.trustedDevices.modals.removeAllDevices.description", { count: devices.length })}
            </p>
          </div>

          <DialogFooter>
            <Button variant="outline" onClick={() => setIsRemoveAllModalOpen(false)} disabled={isRemoving}>
              {t("twoFactor.trustedDevices.modals.buttons.cancel")}
            </Button>
            <Button variant="destructive" onClick={confirmRemoveAllDevices} disabled={isRemoving}>
              {isRemoving
                ? t("twoFactor.trustedDevices.modals.buttons.removing")
                : t("twoFactor.trustedDevices.modals.buttons.removeAllDevices")}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </>
  );
}
