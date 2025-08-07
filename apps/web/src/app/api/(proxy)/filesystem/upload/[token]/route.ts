import { NextRequest, NextResponse } from "next/server";

export const maxDuration = 120000; // 2 minutes to handle large files
export const dynamic = "force-dynamic";

const API_BASE_URL = process.env.API_BASE_URL || "http://localhost:3333";

export async function PUT(req: NextRequest, { params }: { params: Promise<{ token: string }> }) {
  const { token } = await params;
  const cookieHeader = req.headers.get("cookie");
  const url = `${API_BASE_URL}/filesystem/upload/${token}`;

  const headers: Record<string, string> = {
    cookie: cookieHeader || "",
    "Content-Type": req.headers.get("Content-Type") || "application/octet-stream",
    "Content-Length": req.headers.get("Content-Length") || "0",
  };

  req.headers.forEach((value, key) => {
    if (key.startsWith("x-") || key.startsWith("X-")) {
      headers[key] = value;
    }
  });

  try {
    const apiRes = await fetch(url, {
      method: "PUT",
      headers,
      body: req.body,
      duplex: "half",
    } as RequestInit);

    const contentType = apiRes.headers.get("Content-Type") || "application/json";

    let resBody;
    if (contentType.includes("application/json")) {
      resBody = await apiRes.text();
    } else {
      resBody = await apiRes.arrayBuffer();
    }

    const res = new NextResponse(resBody, {
      status: apiRes.status,
      headers: {
        "Content-Type": contentType,
      },
    });

    const setCookie = apiRes.headers.getSetCookie?.() || [];
    if (setCookie.length > 0) {
      res.headers.set("Set-Cookie", setCookie.join(","));
    }

    return res;
  } catch (error) {
    return new NextResponse(
      JSON.stringify({
        error: "Proxy request failed",
        details: error instanceof Error ? error.message : "Unknown error",
      }),
      {
        status: 500,
        headers: {
          "Content-Type": "application/json",
        },
      }
    );
  }
}
