import { prisma } from "../src/shared/prisma";
// import bcrypt from "bcryptjs";
import crypto from "node:crypto";
import { env } from '../src/env';

const defaultConfigs = [
  // General Configurations
  {
    key: "appName",
    value: "Palmr. ",
    type: "string",
    group: "general",
  },
  {
    key: "showHomePage",
    value: "true",
    type: "boolean",
    group: "general",
  },
  {
    key: "appDescription",
    value: "Secure and simple file sharing - Your personal cloud",
    type: "string",
    group: "general",
  },
  {
    key: "appLogo",
    value: "https://i.ibb.co/V0hdRtjV/logo.png",
    type: "string",
    group: "general",
  },
  {
    key: "firstUserAccess",
    value: "true",
    type: "boolean",
    group: "general",
  },
  // Storage Configurations
  {
    key: "maxFileSize",
    value: env.MAX_FILESIZE, // default 1GiB in bytes - 1073741824
    type: "bigint",
    group: "storage",
  },
  {
    key: "maxTotalStoragePerUser",
    value: "10737418240", // 10GB in bytes
    type: "bigint",
    group: "storage",
  },
  // Security Configurations
  {
    key: "jwtSecret",
    value: crypto.randomBytes(64).toString("hex"),
    type: "string",
    group: "security",
  },
  {
    key: "maxLoginAttempts",
    value: "5",
    type: "number",
    group: "security",
  },
  {
    key: "loginBlockDuration",
    value: "600", // 10 minutes in seconds
    type: "number",
    group: "security",
  },
  {
    key: "passwordMinLength",
    value: "8",
    type: "number",
    group: "security",
  },
  // Email Configurations
  {
    key: "smtpEnabled",
    value: "false",
    type: "boolean",
    group: "email",
  },
  {
    key: "smtpHost",
    value: "smtp.gmail.com",
    type: "string",
    group: "email",
  },
  {
    key: "smtpPort",
    value: "587",
    type: "number",
    group: "email",
  },
  {
    key: "smtpUser",
    value: "your-email@gmail.com",
    type: "string",
    group: "email",
  },
  {
    key: "smtpPass",
    value: "your-app-specific-password",
    type: "string",
    group: "email",
  },
  {
    key: "smtpFromName",
    value: "Palmr",
    type: "string",
    group: "email",
  },
  {
    key: "smtpFromEmail",
    value: "noreply@palmr.app",
    type: "string",
    group: "email",
  },
  {
    key: "passwordResetTokenExpiration",
    value: "3600",
    type: "number",
    group: "security",
  }
];

async function main() {
  // const existingUsers = await prisma.user.count();

  // if (existingUsers === 0) {
  //   const adminEmail = "admin@example.com";
  //   const adminPassword = "admin123";
  //   const hashedPassword = await bcrypt.hash(adminPassword, 10);

  //   const adminUser = await prisma.user.upsert({
  //     where: { email: adminEmail },
  //     update: {},
  //     create: {
  //       firstName: "Admin",
  //       lastName: "User",
  //       username: "admin",
  //       email: adminEmail,
  //       password: hashedPassword,
  //       isAdmin: true,
  //       isActive: true,
  //     },
  //   });

  //   console.log("Admin user seeded:", adminUser);
  // } else {
  //   console.log("Users already exist, skipping admin user creation...");
  // }

  console.log("Seeding app configurations...");

  for (const config of defaultConfigs) {
    if (config.key === "jwtSecret") {
      const existingSecret = await prisma.appConfig.findUnique({
        where: { key: "jwtSecret" },
      });

      if (existingSecret) {
        console.log("JWT secret already exists, skipping...");
        continue;
      }
    }

    await prisma.appConfig.upsert({
      where: { key: config.key },
      update: config,
      create: config,
    });
  }

  console.log("App configurations seeded successfully!");
}

main()
  .catch((error) => {
    console.error("Error during seed:", error);
    process.exit(1);
  })
  .finally(async () => {
    await prisma.$disconnect();
  });
