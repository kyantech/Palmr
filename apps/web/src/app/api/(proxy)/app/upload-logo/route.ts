import { NextRequest, NextResponse } from "next/server";

export const maxDuration = 300; // 5 minutes for logo uploads
export const dynamic = "force-dynamic";

const API_BASE_URL = process.env.API_BASE_URL || "http://localhost:3333";

export async function POST(req: NextRequest) {
  const cookieHeader = req.headers.get("cookie");
  const url = `${API_BASE_URL}/app/logo`;
  const formData = await req.formData();

  const apiRes = await fetch(url, {
    method: "POST",
    headers: {
      cookie: cookieHeader || "",
    },
    body: formData,
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
