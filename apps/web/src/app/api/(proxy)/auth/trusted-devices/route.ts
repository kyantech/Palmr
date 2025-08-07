import { NextRequest, NextResponse } from "next/server";

import { getClientHeaders } from "@/lib/proxy-utils";

const API_BASE_URL = process.env.API_BASE_URL || "http://localhost:3333";

export async function GET(req: NextRequest) {
  try {
    const cookieHeader = req.headers.get("cookie");
    const url = `${API_BASE_URL}/auth/trusted-devices`;

    const clientHeaders = getClientHeaders(req);

    const apiRes = await fetch(url, {
      method: "GET",
      headers: {
        cookie: cookieHeader || "",
        ...clientHeaders,
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
    console.error("Error proxying trusted devices request:", error);
    return NextResponse.json({ error: "Internal server error" }, { status: 500 });
  }
}

export async function DELETE(req: NextRequest) {
  try {
    const cookieHeader = req.headers.get("cookie");
    const url = `${API_BASE_URL}/auth/trusted-devices`;

    // Get real client IP and user agent headers
    const clientHeaders = getClientHeaders(req);

    const apiRes = await fetch(url, {
      method: "DELETE",
      headers: {
        cookie: cookieHeader || "",
        ...clientHeaders,
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
    console.error("Error proxying remove all trusted devices request:", error);
    return NextResponse.json({ error: "Internal server error" }, { status: 500 });
  }
}
