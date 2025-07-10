import { SettingsFormProps, ValidGroup } from "../types";
import { AuthProvidersSettings } from "./auth-provider-form/auth-providers-settings";
import { SettingsGroup } from "./settings-group";

const GROUP_ORDER: string[] = ["general", "email", "auth-providers", "security", "storage"];

export function SettingsForm({
  groupedConfigs,
  collapsedGroups,
  groupForms,
  onGroupSubmit,
  onToggleCollapse,
}: SettingsFormProps) {
  const sortedGroups = Object.entries(groupedConfigs).sort(([a], [b]) => {
    const indexA = GROUP_ORDER.indexOf(a);
    const indexB = GROUP_ORDER.indexOf(b);

    if (indexA === -1) return 1;
    if (indexB === -1) return -1;

    return indexA - indexB;
  });

  return (
    <div className="flex flex-col gap-6">
      {sortedGroups.map(([group, configs]) => {
        if (group === "auth-providers") {
          return <AuthProvidersSettings key={group} />;
        }

        const form = groupForms[group as ValidGroup];
        if (!form) {
          return null;
        }

        return (
          <SettingsGroup
            key={group}
            configs={configs}
            form={form}
            group={group}
            isCollapsed={collapsedGroups[group]}
            onSubmit={(data) => onGroupSubmit(group as ValidGroup, data)}
            onToggleCollapse={() => onToggleCollapse(group as ValidGroup)}
          />
        );
      })}
    </div>
  );
}
