import { NextRequest, NextResponse } from "next/server";

const API_BASE_URL = process.env.API_BASE_URL || "http://localhost:3333";

export async function DELETE(req: NextRequest, { params }: { params: Promise<{ downloadId: string }> }) {
  const { downloadId } = await params;
  const cookieHeader = req.headers.get("cookie");
  const url = `${API_BASE_URL}/filesystem/download-queue/${downloadId}`;

  try {
    const apiRes = await fetch(url, {
      method: "DELETE",
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
  } catch (error) {
    console.error("Error proxying cancel download request:", error);
    return NextResponse.json({ error: "Internal server error" }, { status: 500 });
  }
}
