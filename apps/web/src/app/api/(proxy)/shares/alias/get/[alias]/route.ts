import { NextRequest, NextResponse } from "next/server";

export async function GET(req: NextRequest, { params }: { params: { alias: string } }) {
  const cookieHeader = req.headers.get("cookie");
  const url = new URL(req.url);
  const queryParams = url.search;
  const apiRes = await fetch(`${process.env.API_BASE_URL}/shares/alias/${params.alias}${queryParams}`, {
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