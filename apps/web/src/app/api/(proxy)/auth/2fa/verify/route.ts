import { NextRequest, NextResponse } from "next/server";

const API_BASE_URL = process.env.API_BASE_URL || "http://localhost:3333";

export async function POST(req: NextRequest) {
  try {
    const body = await req.text();
    const cookieHeader = req.headers.get("cookie");
    const url = `${API_BASE_URL}/auth/2fa/verify`;

    const apiRes = await fetch(url, {
      method: "POST",
      headers: {
        "Content-Type": "application/json",
        cookie: cookieHeader || "",
        ...Object.fromEntries(Array.from(req.headers.entries()).filter(([key]) => key.startsWith("authorization"))),
      },
      body,
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
    console.error("Error proxying 2FA verify request:", error);
    return NextResponse.json({ error: "Internal server error" }, { status: 500 });
  }
}
