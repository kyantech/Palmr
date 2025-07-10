import nodemailer from "nodemailer";

import { ConfigService } from "../config/service";

interface SmtpConfig {
  smtpEnabled: string;
  smtpHost: string;
  smtpPort: string;
  smtpUser: string;
  smtpPass: string;
  smtpSecure?: string;
  smtpNoAuth?: string;
  smtpTrustSelfSigned?: string;
}

export class EmailService {
  private configService = new ConfigService();

  private async createTransporter() {
    const smtpEnabled = await this.configService.getValue("smtpEnabled");
    if (smtpEnabled !== "true") {
      return null;
    }

    const port = Number(await this.configService.getValue("smtpPort"));
    const smtpSecure = (await this.configService.getValue("smtpSecure")) || "auto";
    const smtpNoAuth = await this.configService.getValue("smtpNoAuth");
    const smtpTrustSelfSigned = await this.configService.getValue("smtpTrustSelfSigned");

    let secure = false;
    let requireTLS = false;

    if (smtpSecure === "ssl") {
      secure = true;
    } else if (smtpSecure === "tls") {
      requireTLS = true;
    } else if (smtpSecure === "none") {
      secure = false;
      requireTLS = false;
    } else if (smtpSecure === "auto") {
      if (port === 465) {
        secure = true;
      } else if (port === 587 || port === 25) {
        requireTLS = true;
      }
    }

    const transportConfig: any = {
      host: await this.configService.getValue("smtpHost"),
      port: port,
      secure: secure,
      requireTLS: requireTLS,
    };

    if (smtpSecure !== "none") {
      transportConfig.tls = {
        rejectUnauthorized: smtpTrustSelfSigned === "true" ? false : true,
      };
    }

    if (smtpNoAuth !== "true") {
      transportConfig.auth = {
        user: await this.configService.getValue("smtpUser"),
        pass: await this.configService.getValue("smtpPass"),
      };
    }

    return nodemailer.createTransport(transportConfig);
  }

  async testConnection(config?: SmtpConfig) {
    let smtpConfig: SmtpConfig;

    if (config) {
      // Use provided configuration
      smtpConfig = config;
    } else {
      // Fallback to saved configuration
      smtpConfig = {
        smtpEnabled: await this.configService.getValue("smtpEnabled"),
        smtpHost: await this.configService.getValue("smtpHost"),
        smtpPort: await this.configService.getValue("smtpPort"),
        smtpUser: await this.configService.getValue("smtpUser"),
        smtpPass: await this.configService.getValue("smtpPass"),
        smtpSecure: (await this.configService.getValue("smtpSecure")) || "auto",
        smtpNoAuth: await this.configService.getValue("smtpNoAuth"),
        smtpTrustSelfSigned: await this.configService.getValue("smtpTrustSelfSigned"),
      };
    }

    if (smtpConfig.smtpEnabled !== "true") {
      throw new Error("SMTP is not enabled");
    }

    const port = Number(smtpConfig.smtpPort);
    const smtpSecure = smtpConfig.smtpSecure || "auto";
    const smtpNoAuth = smtpConfig.smtpNoAuth;

    let secure = false;
    let requireTLS = false;

    if (smtpSecure === "ssl") {
      secure = true;
    } else if (smtpSecure === "tls") {
      requireTLS = true;
    } else if (smtpSecure === "none") {
      secure = false;
      requireTLS = false;
    } else if (smtpSecure === "auto") {
      if (port === 465) {
        secure = true;
      } else if (port === 587 || port === 25) {
        requireTLS = true;
      }
    }

    const transportConfig: any = {
      host: smtpConfig.smtpHost,
      port: port,
      secure: secure,
      requireTLS: requireTLS,
    };

    if (smtpSecure !== "none") {
      transportConfig.tls = {
        rejectUnauthorized: smtpConfig.smtpTrustSelfSigned === "true" ? false : true,
      };
    }

    if (smtpNoAuth !== "true") {
      transportConfig.auth = {
        user: smtpConfig.smtpUser,
        pass: smtpConfig.smtpPass,
      };
    }

    const transporter = nodemailer.createTransport(transportConfig);

    try {
      await transporter.verify();
      return { success: true, message: "SMTP connection successful" };
    } catch (error: any) {
      throw new Error(`SMTP connection failed: ${error.message}`);
    }
  }

  async sendPasswordResetEmail(to: string, resetToken: string, origin: string) {
    const transporter = await this.createTransporter();
    if (!transporter) {
      throw new Error("SMTP is not enabled");
    }

    const fromName = await this.configService.getValue("smtpFromName");
    const fromEmail = await this.configService.getValue("smtpFromEmail");
    const appName = await this.configService.getValue("appName");

    await transporter.sendMail({
      from: `"${fromName}" <${fromEmail}>`,
      to,
      subject: `${appName} - Password Reset Request`,
      html: `
        <h1>${appName} - Password Reset Request</h1>
        <p>Click the link below to reset your password:</p>
        <a href="${origin}/reset-password?token=${resetToken}">
          Reset Password
        </a>
        <p>This link will expire in 1 hour.</p>
      `,
    });
  }

  async sendShareNotification(to: string, shareLink: string, shareName?: string) {
    const transporter = await this.createTransporter();
    if (!transporter) {
      throw new Error("SMTP is not enabled");
    }

    const fromName = await this.configService.getValue("smtpFromName");
    const fromEmail = await this.configService.getValue("smtpFromEmail");
    const appName = await this.configService.getValue("appName");

    const shareTitle = shareName || "Files";

    await transporter.sendMail({
      from: `"${fromName}" <${fromEmail}>`,
      to,
      subject: `${appName} - ${shareTitle} shared with you`,
      html: `
        <h1>${appName} - Shared Files</h1>
        <p>Someone has shared "${shareTitle}" with you.</p>
        <p>Click the link below to access the shared files:</p>
        <a href="${shareLink}">
          Access Shared Files
        </a>
        <p>Note: This share may have an expiration date or view limit.</p>
      `,
    });
  }
}
