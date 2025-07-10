import { NextRequest, NextResponse } from "next/server";

const API_BASE_URL = process.env.API_BASE_URL || "http://localhost:3333";

export async function GET(req: NextRequest, { params }: { params: Promise<{ alias: string }> }) {
  const cookieHeader = req.headers.get("cookie");
  const url = new URL(req.url);
  const queryParams = url.search;
  const { alias } = await params;
  const fetchUrl = `${API_BASE_URL}/shares/alias/${alias}${queryParams}`;

  const apiRes = await fetch(fetchUrl, {
    method: "GET",
    headers: {
      "Content-Type": "application/json",
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
