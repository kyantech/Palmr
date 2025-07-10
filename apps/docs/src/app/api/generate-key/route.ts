import { execSync } from "child_process";
import { NextResponse } from "next/server";

export async function GET() {
  try {
    const key = execSync("openssl rand -base64 48 | tr -dc 'A-Za-z0-9' | head -c 64").toString().trim();
    return NextResponse.json({ key });
  } catch (error) {
    console.error("Failed to generate key:", error);
    return NextResponse.json({ error: "Failed to generate key" }, { status: 500 });
  }
}
