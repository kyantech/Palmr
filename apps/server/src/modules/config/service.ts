import { prisma } from "../../shared/prisma";

export class ConfigService {
  async getValue(key: string): Promise<string> {
    const config = await prisma.appConfig.findUnique({
      where: { key },
    });

    if (!config) {
      throw new Error(`Configuration ${key} not found`);
    }

    return config.value;
  }

  async setValue(key: string, value: string): Promise<void> {
    await prisma.appConfig.update({
      where: { key },
      data: { value },
    });
  }

  async validatePasswordAuthDisable(): Promise<boolean> {
    const enabledProviders = await prisma.authProvider.findMany({
      where: { enabled: true },
    });

    return enabledProviders.length > 0;
  }

  async validateAllProvidersDisable(): Promise<boolean> {
    const passwordAuthEnabled = await this.getValue("passwordAuthEnabled");
    return passwordAuthEnabled === "true";
  }

  async getGroupConfigs(group: string) {
    const configs = await prisma.appConfig.findMany({
      where: { group },
    });

    return configs.reduce((acc, curr) => {
      let value: any = curr.value;

      switch (curr.type) {
        case "number":
          value = Number(value);
          break;
        case "boolean":
          value = value === "true";
          break;
        case "json":
          value = JSON.parse(value);
          break;
        case "bigint":
          value = BigInt(value);
          break;
      }

      return { ...acc, [curr.key]: value };
    }, {});
  }
}
