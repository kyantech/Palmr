import { NextRequest, NextResponse } from "next/server";

export async function GET(req: NextRequest) {
  const cookieHeader = req.headers.get("cookie");

  const apiRes = await fetch(`${process.env.API_BASE_URL}/app/configs`, {
    method: "GET",
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
