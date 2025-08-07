import { NextRequest, NextResponse } from "next/server";

const API_BASE_URL = process.env.API_BASE_URL || "http://localhost:3333";

export async function POST(req: NextRequest) {
  try {
    const cookieHeader = req.headers.get("cookie");
    const url = `${API_BASE_URL}/auth/2fa/backup-codes`;

    const apiRes = await fetch(url, {
      method: "POST",
      headers: {
        cookie: cookieHeader || "",
        ...Object.fromEntries(Array.from(req.headers.entries()).filter(([key]) => key.startsWith("authorization"))),
      },
      redirect: "manual",
    });

    const resBody = await apiRes.text();
    const res = new NextResponse(resBody, {
      status: apiRes.status,
      headers: {
        "Content-Type": "application/json",
      },
    });

    const setCookie = apiRes.headers.getSetCookie?.() || [];
    if (setCookie.length > 0) {
      res.headers.set("Set-Cookie", setCookie.join(","));
    }

    return res;
  } catch (error) {
    console.error("Error proxying 2FA backup codes request:", error);
    return NextResponse.json({ error: "Internal server error" }, { status: 500 });
  }
}
