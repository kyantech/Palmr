import { NextRequest, NextResponse } from "next/server";

const API_BASE_URL = process.env.API_BASE_URL || "http://localhost:3333";

export async function DELETE(req: NextRequest, { params }: { params: Promise<{ shareId: string }> }) {
  const cookieHeader = req.headers.get("cookie");
  const body = await req.text();
  const { shareId } = await params;

  const requestData = JSON.parse(body);

  const itemsBody = {
    files: requestData.files || [],
    folders: [],
  };

  const url = `${API_BASE_URL}/shares/${shareId}/items`;

  const apiRes = await fetch(url, {
    method: "DELETE",
    headers: {
      "Content-Type": "application/json",
      cookie: cookieHeader || "",
    },
    body: JSON.stringify(itemsBody),
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
