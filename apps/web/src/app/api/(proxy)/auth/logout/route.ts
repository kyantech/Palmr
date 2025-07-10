import { NextRequest, NextResponse } from "next/server";

const API_BASE_URL = process.env.API_BASE_URL || "http://localhost:3333";

export async function POST(req: NextRequest) {
  const cookieHeader = req.headers.get("cookie");
  const url = `${API_BASE_URL}/auth/logout`;

  const apiRes = await fetch(url, {
    method: "POST",
    headers: {
      cookie: cookieHeader || "",
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
}
