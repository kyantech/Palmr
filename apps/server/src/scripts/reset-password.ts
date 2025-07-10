#!/usr/bin/env node
import * as readline from "readline";
import { PrismaClient } from "@prisma/client";
import bcrypt from "bcryptjs";

const prisma = new PrismaClient();

function createReadlineInterface() {
  return readline.createInterface({
    input: process.stdin,
    output: process.stdout,
  });
}

function question(rl: readline.Interface, query: string): Promise<string> {
  return new Promise((resolve) => rl.question(query, resolve));
}

function isValidEmail(email: string): boolean {
  const emailRegex = /^[^\s@]+@[^\s@]+\.[^\s@]+$/;
  return emailRegex.test(email);
}

function isValidPassword(password: string): boolean {
  return password.length >= 8;
}

async function resetUserPassword() {
  const rl = createReadlineInterface();

  try {
    console.log("\n🔐 Palmr Password Reset Tool");
    console.log("===============================");
    console.log("This script allows you to reset a user's password directly from the Docker terminal.");
    console.log("⚠️  WARNING: This bypasses normal security checks. Use only when necessary!\n");

    let email: string;
    let user: any;

    while (true) {
      email = await question(rl, "Enter user email: ");

      if (!email.trim()) {
        console.log("❌ Email cannot be empty. Please try again.\n");
        continue;
      }

      if (!isValidEmail(email)) {
        console.log("❌ Please enter a valid email address.\n");
        continue;
      }

      user = await prisma.user.findUnique({
        where: { email: email.toLowerCase() },
        select: {
          id: true,
          firstName: true,
          lastName: true,
          email: true,
          username: true,
          isActive: true,
          isAdmin: true,
        },
      });

      if (!user) {
        console.log(`❌ No user found with email: ${email}\n`);
        const retry = await question(rl, "Try another email? (y/n): ");
        if (retry.toLowerCase() !== "y") {
          console.log("\n👋 Exiting...");
          return;
        }
        console.log();
        continue;
      }

      break;
    }

    console.log("\n✅ User found:");
    console.log(`   Name: ${user.firstName} ${user.lastName}`);
    console.log(`   Username: ${user.username}`);
    console.log(`   Email: ${user.email}`);
    console.log(`   Status: ${user.isActive ? "Active" : "Inactive"}`);
    console.log(`   Admin: ${user.isAdmin ? "Yes" : "No"}\n`);

    const confirm = await question(rl, "Do you want to reset the password for this user? (y/n): ");
    if (confirm.toLowerCase() !== "y") {
      console.log("\n👋 Operation cancelled.");
      return;
    }

    let newPassword: string;
    while (true) {
      console.log("\n🔑 Enter new password requirements:");
      console.log("   - Minimum 8 characters");

      newPassword = await question(rl, "\nEnter new password: ");

      if (!newPassword.trim()) {
        console.log("❌ Password cannot be empty. Please try again.");
        continue;
      }

      if (!isValidPassword(newPassword)) {
        console.log("❌ Password must be at least 8 characters long. Please try again.");
        continue;
      }

      const confirmPassword = await question(rl, "Confirm new password: ");

      if (newPassword !== confirmPassword) {
        console.log("❌ Passwords do not match. Please try again.");
        continue;
      }

      break;
    }

    console.log("\n🔄 Hashing password...");
    const hashedPassword = await bcrypt.hash(newPassword, 10);

    console.log("💾 Updating password in database...");
    await prisma.user.update({
      where: { id: user.id },
      data: { password: hashedPassword },
    });

    console.log("🧹 Cleaning up existing password reset tokens...");
    await prisma.passwordReset.deleteMany({
      where: {
        userId: user.id,
        used: false,
      },
    });

    console.log("\n✅ Password reset successful!");
    console.log(`   User: ${user.firstName} ${user.lastName} (${user.email})`);
    console.log("   The user can now login with the new password.");
    console.log("\n🔐 Security Note: The password has been encrypted using bcrypt with salt rounds of 10.");
  } catch (error) {
    console.error("\n❌ Error resetting password:", error);
    process.exit(1);
  } finally {
    rl.close();
    await prisma.$disconnect();
  }
}

async function listUsers() {
  try {
    console.log("\n👥 Registered Users:");
    console.log("===================");

    const users = await prisma.user.findMany({
      select: {
        firstName: true,
        lastName: true,
        email: true,
        username: true,
        isActive: true,
        isAdmin: true,
        createdAt: true,
      },
      orderBy: {
        createdAt: "desc",
      },
    });

    if (users.length === 0) {
      console.log("No users found in the system.");
      return;
    }

    users.forEach((user, index) => {
      console.log(`\n${index + 1}. ${user.firstName} ${user.lastName}`);
      console.log(`   Email: ${user.email}`);
      console.log(`   Username: ${user.username}`);
      console.log(`   Status: ${user.isActive ? "Active" : "Inactive"}`);
      console.log(`   Admin: ${user.isAdmin ? "Yes" : "No"}`);
      console.log(`   Created: ${user.createdAt.toLocaleDateString()}`);
    });
  } catch (error) {
    console.error("❌ Error listing users:", error);
  }
}

async function main() {
  const args = process.argv.slice(2);

  if (args.includes("--help") || args.includes("-h")) {
    console.log("\n🔐 Palmr Password Reset Tool");
    console.log("=============================");
    console.log("Interactive password reset tool for Docker terminal access");
    console.log("\nUsage:");
    console.log("  ./reset-password.sh         - Reset a user's password interactively");
    console.log("  ./reset-password.sh --list  - List all users in the system");
    console.log("  ./reset-password.sh --help  - Show this help message");
    console.log("\nExamples:");
    console.log("  ./reset-password.sh");
    console.log("  ./reset-password.sh --list");
    console.log("\nNote: This script must be run inside the Docker container with database access.");
    console.log("⚠️  For security, all password resets require interactive confirmation.");
    return;
  }

  if (args.includes("--list") || args.includes("-l")) {
    await listUsers();
    await prisma.$disconnect();
    return;
  }

  await resetUserPassword();
}

process.on("SIGINT", async () => {
  console.log("\n\n👋 Goodbye!");
  await prisma.$disconnect();
  process.exit(0);
});

process.on("SIGTERM", async () => {
  await prisma.$disconnect();
  process.exit(0);
});

if (require.main === module) {
  main().catch(console.error);
}
