"use client";

import {
  IconCalendar,
  IconChevronDown,
  IconChevronUp,
  IconEye,
  IconFile,
  IconFiles,
  IconLock,
  IconSettings,
  IconUpload,
  IconUser,
} from "@tabler/icons-react";
import { useTranslations } from "next-intl";
import { useForm } from "react-hook-form";

import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Form, FormControl, FormDescription, FormField, FormItem, FormLabel, FormMessage } from "@/components/ui/form";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Separator } from "@/components/ui/separator";
import { Textarea } from "@/components/ui/textarea";
import type { CreateReverseShareBody } from "@/http/endpoints/reverse-shares/types";
import { FileSizeInput } from "./file-size-input";
import { FileTypesTagsInput } from "./file-types-tags-input";

const DIALOG_CONFIG = {
  maxWidth: "sm:max-w-[500px] md:max-w-[650px]",
  maxHeight: "max-h-[85vh]",
  contentMaxHeight: "max-h-[calc(85vh-140px)]",
} as const;

const ICON_SIZES = {
  small: 14,
  medium: 16,
  large: 20,
} as const;

interface CreateReverseShareFormData {
  name: string;
  description?: string;
  expiration?: string;
  maxFiles?: string;
  maxFileSize?: string;
  allowedFileTypes?: string;
  password?: string;
  pageLayout?: "DEFAULT" | "WETRANSFER";
  nameFieldRequired: "HIDDEN" | "OPTIONAL" | "REQUIRED";
  emailFieldRequired: "HIDDEN" | "OPTIONAL" | "REQUIRED";
  isPasswordProtected: boolean;
  hasExpiration: boolean;
  hasFileLimits: boolean;
  hasFieldRequirements: boolean;
  noFilesLimit: boolean;
  noSizeLimit: boolean;
  allFileTypes: boolean;
}

interface CreateReverseShareModalProps {
  isOpen: boolean;
  onClose: () => void;
  onCreateReverseShare: (data: CreateReverseShareBody) => Promise<any>;
  isCreating: boolean;
}

const DEFAULT_FORM_VALUES: CreateReverseShareFormData = {
  name: "",
  description: "",
  expiration: "",
  maxFiles: "",
  maxFileSize: "",
  allowedFileTypes: "",
  password: "",
  pageLayout: "DEFAULT",
  nameFieldRequired: "OPTIONAL",
  emailFieldRequired: "OPTIONAL",
  isPasswordProtected: false,
  hasExpiration: false,
  hasFileLimits: false,
  hasFieldRequirements: false,
  noFilesLimit: true,
  noSizeLimit: true,
  allFileTypes: true,
};

export function CreateReverseShareModal({
  isOpen,
  onClose,
  onCreateReverseShare,
  isCreating,
}: CreateReverseShareModalProps) {
  const t = useTranslations();

  const form = useForm<CreateReverseShareFormData>({
    defaultValues: DEFAULT_FORM_VALUES,
  });

  const watchedValues = {
    isPasswordProtected: form.watch("isPasswordProtected"),
    hasExpiration: form.watch("hasExpiration"),
    hasFileLimits: form.watch("hasFileLimits"),
    hasFieldRequirements: form.watch("hasFieldRequirements"),
    noFilesLimit: form.watch("noFilesLimit"),
    noSizeLimit: form.watch("noSizeLimit"),
    allFileTypes: form.watch("allFileTypes"),
  };

  const buildPayload = (formData: CreateReverseShareFormData): CreateReverseShareBody => {
    const payload: CreateReverseShareBody = {
      name: formData.name,
      pageLayout: formData.pageLayout || "DEFAULT",
      nameFieldRequired: formData.nameFieldRequired,
      emailFieldRequired: formData.emailFieldRequired,
    };

    if (formData.description?.trim()) {
      payload.description = formData.description.trim();
    }

    if (formData.hasExpiration && formData.expiration) {
      payload.expiration = new Date(formData.expiration).toISOString();
    }

    if (formData.isPasswordProtected && formData.password?.trim()) {
      payload.password = formData.password.trim();
    }

    if (formData.hasFileLimits) {
      const maxFiles = parseInt(formData.maxFiles || "0");
      const maxFileSize = parseInt(formData.maxFileSize || "0");

      if (maxFiles > 0) {
        payload.maxFiles = maxFiles;
      }

      if (maxFileSize > 0) {
        payload.maxFileSize = maxFileSize;
      }
    }

    if (formData.allowedFileTypes?.trim()) {
      payload.allowedFileTypes = formData.allowedFileTypes.trim();
    }

    return payload;
  };

  const handleSubmit = async (formData: CreateReverseShareFormData) => {
    try {
      const payload = buildPayload(formData);
      await onCreateReverseShare(payload);
      form.reset();
    } catch {
      // Error handling is managed by the hook
    }
  };

  const handleClose = () => {
    form.reset();
    onClose();
  };

  const toggleSection = (sectionKey: keyof CreateReverseShareFormData, resetFields?: string[]) => {
    return () => {
      const currentValue = form.getValues(sectionKey) as boolean;
      const newValue = !currentValue;
      form.setValue(sectionKey, newValue);

      if (!newValue && resetFields) {
        resetFields.forEach((field) => {
          form.setValue(field as keyof CreateReverseShareFormData, "" as any);
        });
      }
    };
  };

  const renderSectionToggle = (isExpanded: boolean, icon: React.ReactNode, label: string, onToggle: () => void) => (
    <div className="flex items-center gap-1">
      <Label className="flex items-center gap-2">
        {icon}
        {label}
      </Label>
      <Button type="button" variant="ghost" size="icon" className="h-6 w-6" onClick={onToggle}>
        {isExpanded ? <IconChevronUp size={ICON_SIZES.small} /> : <IconChevronDown size={ICON_SIZES.small} />}
      </Button>
    </div>
  );

  const renderCheckboxOption = (
    id: string,
    checked: boolean,
    onCheckedChange: (checked: boolean) => void,
    label: string
  ) => (
    <div className="flex items-center gap-2">
      <Checkbox id={id} checked={checked} onCheckedChange={onCheckedChange} />
      <label htmlFor={id} className="text-sm text-muted-foreground cursor-pointer">
        {label}
      </label>
    </div>
  );

  return (
    <Dialog open={isOpen} onOpenChange={handleClose}>
      <DialogContent className={`${DIALOG_CONFIG.maxWidth} ${DIALOG_CONFIG.maxHeight} overflow-hidden`}>
        <DialogHeader>
          <DialogTitle className="flex items-center gap-2">
            <IconUpload size={ICON_SIZES.large} />
            {t("reverseShares.modals.create.title")}
          </DialogTitle>
          <DialogDescription>{t("reverseShares.modals.create.description")}</DialogDescription>
        </DialogHeader>

        <div className={`overflow-y-auto ${DIALOG_CONFIG.contentMaxHeight} py-2`}>
          <Form {...form}>
            <form onSubmit={form.handleSubmit(handleSubmit)} className="space-y-6">
              {/* Basic Information */}
              <div className="space-y-4">
                <FormField
                  control={form.control}
                  name="name"
                  rules={{ required: t("validation.nameRequired") }}
                  render={({ field }) => (
                    <FormItem>
                      <FormLabel>{t("reverseShares.form.name.label")}</FormLabel>
                      <FormControl>
                        <Input placeholder={t("reverseShares.form.name.placeholder")} {...field} />
                      </FormControl>
                      <FormMessage />
                    </FormItem>
                  )}
                />

                <FormField
                  control={form.control}
                  name="description"
                  render={({ field }) => (
                    <FormItem>
                      <FormLabel>{t("reverseShares.form.description.label")}</FormLabel>
                      <FormControl>
                        <Textarea placeholder={t("reverseShares.form.description.placeholder")} rows={3} {...field} />
                      </FormControl>
                      <FormDescription>{t("reverseShares.form.description.description")}</FormDescription>
                      <FormMessage />
                    </FormItem>
                  )}
                />

                <FormField
                  control={form.control}
                  name="pageLayout"
                  render={({ field }) => (
                    <FormItem>
                      <FormLabel className="flex items-center gap-2">
                        <IconSettings size={ICON_SIZES.medium} />
                        {t("reverseShares.form.pageLayout.label")}
                      </FormLabel>
                      <Select onValueChange={field.onChange} defaultValue={field.value}>
                        <FormControl>
                          <SelectTrigger>
                            <SelectValue placeholder={t("reverseShares.form.pageLayout.placeholder")} />
                          </SelectTrigger>
                        </FormControl>
                        <SelectContent>
                          <SelectItem value="DEFAULT">{t("reverseShares.form.pageLayout.options.default")}</SelectItem>
                          <SelectItem value="WETRANSFER">
                            {t("reverseShares.form.pageLayout.options.wetransfer")}
                          </SelectItem>
                        </SelectContent>
                      </Select>
                      <FormDescription>{t("reverseShares.form.pageLayout.description")}</FormDescription>
                      <FormMessage />
                    </FormItem>
                  )}
                />
              </div>

              <Separator />

              {/* Expiration Settings */}
              <div className="space-y-4">
                {renderSectionToggle(
                  watchedValues.hasExpiration,
                  <IconCalendar size={ICON_SIZES.medium} />,
                  t("reverseShares.labels.configureExpiration"),
                  toggleSection("hasExpiration")
                )}

                {watchedValues.hasExpiration && (
                  <FormField
                    control={form.control}
                    name="expiration"
                    render={({ field }) => (
                      <FormItem>
                        <FormLabel>{t("reverseShares.form.expiration.label")}</FormLabel>
                        <FormControl>
                          <Input type="datetime-local" {...field} />
                        </FormControl>
                        <FormDescription>{t("reverseShares.form.expiration.description")}</FormDescription>
                        <FormMessage />
                      </FormItem>
                    )}
                  />
                )}
              </div>

              <Separator />

              {/* Password Protection */}
              <div className="space-y-4">
                {renderSectionToggle(
                  watchedValues.isPasswordProtected,
                  <IconLock size={ICON_SIZES.medium} />,
                  t("reverseShares.labels.protectWithPassword"),
                  toggleSection("isPasswordProtected", ["password"])
                )}

                {watchedValues.isPasswordProtected && (
                  <FormField
                    control={form.control}
                    name="password"
                    rules={{
                      required: watchedValues.isPasswordProtected ? t("validation.passwordRequired") : false,
                    }}
                    render={({ field }) => (
                      <FormItem>
                        <FormLabel>{t("reverseShares.form.password.label")}</FormLabel>
                        <FormControl>
                          <Input
                            type="password"
                            placeholder={t("reverseShares.form.password.placeholder")}
                            {...field}
                          />
                        </FormControl>
                        <FormDescription>{t("reverseShares.form.password.description")}</FormDescription>
                        <FormMessage />
                      </FormItem>
                    )}
                  />
                )}
              </div>

              <Separator />

              {/* File Limits */}
              <div className="space-y-4">
                {renderSectionToggle(
                  watchedValues.hasFileLimits,
                  <IconFile size={ICON_SIZES.medium} />,
                  t("reverseShares.labels.configureLimits"),
                  toggleSection("hasFileLimits", ["maxFiles", "maxFileSize", "allowedFileTypes"])
                )}

                {watchedValues.hasFileLimits && (
                  <div className="space-y-4">
                    <FormField
                      control={form.control}
                      name="maxFiles"
                      render={({ field }) => (
                        <FormItem>
                          <FormLabel className="flex items-center gap-2">
                            <IconEye size={ICON_SIZES.medium} />
                            {t("reverseShares.form.maxFiles.label")}
                          </FormLabel>
                          <div className="space-y-3">
                            {renderCheckboxOption(
                              "no-files-limit-create",
                              watchedValues.noFilesLimit,
                              (checked) => {
                                form.setValue("noFilesLimit", !!checked);
                                if (checked) field.onChange("0");
                              },
                              t("reverseShares.form.maxFiles.noLimit")
                            )}
                            {!watchedValues.noFilesLimit && (
                              <FormControl>
                                <Input
                                  type="number"
                                  min="1"
                                  placeholder={t("reverseShares.form.maxFiles.placeholder")}
                                  {...field}
                                />
                              </FormControl>
                            )}
                          </div>
                          <FormDescription className="text-xs">
                            {t("reverseShares.form.maxFiles.description")}
                          </FormDescription>
                          <FormMessage />
                        </FormItem>
                      )}
                    />

                    <FormField
                      control={form.control}
                      name="maxFileSize"
                      render={({ field }) => (
                        <FormItem>
                          <FormLabel className="flex items-center gap-2">
                            <IconFiles size={ICON_SIZES.medium} />
                            {t("reverseShares.form.maxFileSize.label")}
                          </FormLabel>
                          <div className="space-y-3">
                            {renderCheckboxOption(
                              "no-size-limit-create",
                              watchedValues.noSizeLimit,
                              (checked) => {
                                form.setValue("noSizeLimit", !!checked);
                                if (checked) field.onChange("0");
                              },
                              t("reverseShares.form.maxFileSize.noLimit")
                            )}
                            {!watchedValues.noSizeLimit && (
                              <FormControl>
                                <FileSizeInput
                                  value={field.value || ""}
                                  onChange={field.onChange}
                                  placeholder={t("reverseShares.form.maxFileSize.placeholder")}
                                />
                              </FormControl>
                            )}
                          </div>
                          <FormDescription className="text-xs">
                            {t("reverseShares.form.maxFileSize.description")}
                          </FormDescription>
                          <FormMessage />
                        </FormItem>
                      )}
                    />

                    <FormField
                      control={form.control}
                      name="allowedFileTypes"
                      render={({ field }) => (
                        <FormItem>
                          <FormLabel>{t("reverseShares.form.allowedFileTypes.label")}</FormLabel>
                          <div className="space-y-3">
                            {renderCheckboxOption(
                              "all-file-types-create",
                              watchedValues.allFileTypes,
                              (checked) => {
                                form.setValue("allFileTypes", !!checked);
                                if (checked) field.onChange("");
                              },
                              t("reverseShares.form.allowedFileTypes.allTypes")
                            )}
                            {!watchedValues.allFileTypes && (
                              <FormControl>
                                <FileTypesTagsInput
                                  value={field.value ? field.value.split(",").filter(Boolean) : []}
                                  onChange={(tags) => field.onChange(tags.join(","))}
                                  placeholder="jpg png pdf docx"
                                />
                              </FormControl>
                            )}
                          </div>
                          <FormDescription className="text-xs">
                            {t("reverseShares.form.allowedFileTypes.description")}
                          </FormDescription>
                          <FormMessage />
                        </FormItem>
                      )}
                    />
                  </div>
                )}
              </div>

              <Separator />

              {/* Field Requirements */}
              <div className="space-y-4">
                {renderSectionToggle(
                  watchedValues.hasFieldRequirements,
                  <IconUser size={ICON_SIZES.medium} />,
                  t("reverseShares.form.fieldRequirements.title"),
                  toggleSection("hasFieldRequirements")
                )}

                {watchedValues.hasFieldRequirements && (
                  <div className="bg-gray-50 dark:bg-gray-800/50 rounded-lg p-4 space-y-4">
                    <div className="grid grid-cols-1 md:grid-cols-2 gap-4">
                      <FormField
                        control={form.control}
                        name="nameFieldRequired"
                        render={({ field }) => (
                          <FormItem>
                            <FormLabel className="flex items-center gap-2 font-medium">
                              <IconUser size={ICON_SIZES.small} />
                              {t("reverseShares.form.nameFieldRequired.label")}
                            </FormLabel>
                            <Select onValueChange={field.onChange} defaultValue={field.value}>
                              <FormControl>
                                <SelectTrigger className="bg-white dark:bg-gray-900">
                                  <SelectValue />
                                </SelectTrigger>
                              </FormControl>
                              <SelectContent>
                                <SelectItem value="HIDDEN">
                                  <div className="flex items-center gap-2">
                                    <div className="w-2 h-2 rounded-full bg-gray-400" />
                                    {t("reverseShares.labels.fieldOptions.hidden")}
                                  </div>
                                </SelectItem>
                                <SelectItem value="OPTIONAL">
                                  <div className="flex items-center gap-2">
                                    <div className="w-2 h-2 rounded-full bg-blue-500" />
                                    {t("reverseShares.labels.fieldOptions.optional")}
                                  </div>
                                </SelectItem>
                                <SelectItem value="REQUIRED">
                                  <div className="flex items-center gap-2">
                                    <div className="w-2 h-2 rounded-full bg-red-500" />
                                    {t("reverseShares.labels.fieldOptions.required")}
                                  </div>
                                </SelectItem>
                              </SelectContent>
                            </Select>
                            <FormMessage />
                          </FormItem>
                        )}
                      />

                      <FormField
                        control={form.control}
                        name="emailFieldRequired"
                        render={({ field }) => (
                          <FormItem>
                            <FormLabel className="flex items-center gap-2 font-medium">
                              <IconUser size={ICON_SIZES.small} />
                              {t("reverseShares.form.emailFieldRequired.label")}
                            </FormLabel>
                            <Select onValueChange={field.onChange} defaultValue={field.value}>
                              <FormControl>
                                <SelectTrigger className="bg-white dark:bg-gray-900">
                                  <SelectValue />
                                </SelectTrigger>
                              </FormControl>
                              <SelectContent>
                                <SelectItem value="HIDDEN">
                                  <div className="flex items-center gap-2">
                                    <div className="w-2 h-2 rounded-full bg-gray-400" />
                                    {t("reverseShares.labels.fieldOptions.hidden")}
                                  </div>
                                </SelectItem>
                                <SelectItem value="OPTIONAL">
                                  <div className="flex items-center gap-2">
                                    <div className="w-2 h-2 rounded-full bg-blue-500" />
                                    {t("reverseShares.labels.fieldOptions.optional")}
                                  </div>
                                </SelectItem>
                                <SelectItem value="REQUIRED">
                                  <div className="flex items-center gap-2">
                                    <div className="w-2 h-2 rounded-full bg-red-500" />
                                    {t("reverseShares.labels.fieldOptions.required")}
                                  </div>
                                </SelectItem>
                              </SelectContent>
                            </Select>
                            <FormMessage />
                          </FormItem>
                        )}
                      />
                    </div>

                    <div className="text-xs text-muted-foreground bg-blue-50 dark:bg-blue-950/20 p-3 rounded-md border border-blue-200 dark:border-blue-800">
                      <div className="flex items-start gap-2">
                        <IconSettings size={12} className="mt-0.5 text-blue-600 dark:text-blue-400" />
                        <div className="space-y-1">
                          <p className="font-medium text-blue-900 dark:text-blue-100">Field Configuration:</p>
                          <ul className="space-y-0.5 text-blue-800 dark:text-blue-200">
                            <li>
                              • <strong>Hidden:</strong> Field won't appear in the upload form
                            </li>
                            <li>
                              • <strong>Optional:</strong> Field appears but isn't required
                            </li>
                            <li>
                              • <strong>Required:</strong> Field appears and must be filled
                            </li>
                          </ul>
                        </div>
                      </div>
                    </div>
                  </div>
                )}
              </div>

              <DialogFooter className="gap-2">
                <Button type="button" variant="outline" onClick={handleClose} disabled={isCreating}>
                  {t("common.cancel")}
                </Button>
                <Button type="submit" disabled={isCreating}>
                  {isCreating ? (
                    <div className="flex items-center gap-2">
                      <div className="animate-spin">⠋</div>
                      {t("common.creating")}
                    </div>
                  ) : (
                    t("reverseShares.form.submit")
                  )}
                </Button>
              </DialogFooter>
            </form>
          </Form>
        </div>
      </DialogContent>
    </Dialog>
  );
}
