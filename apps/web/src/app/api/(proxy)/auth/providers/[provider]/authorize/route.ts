import { NextRequest, NextResponse } from "next/server";

const API_BASE_URL = process.env.API_BASE_URL || "http://localhost:3333";

export async function GET(request: NextRequest, { params }: { params: Promise<{ provider: string }> }) {
  try {
    const { provider } = await params;
    const url = new URL(request.url);
    const queryString = url.search;
    const originalHost = request.headers.get("host") || url.host;
    const originalProtocol = request.headers.get("x-forwarded-proto") || url.protocol.replace(":", "");
    const authorizeUrl = `${API_BASE_URL}/auth/providers/${provider}/authorize${queryString}`;

    const apiRes = await fetch(authorizeUrl, {
      method: "GET",
      headers: {
        "Content-Type": "application/json",
        "x-forwarded-host": originalHost,
        "x-forwarded-proto": originalProtocol,
        ...Object.fromEntries(
          Array.from(request.headers.entries()).filter(
            ([key]) => key.startsWith("authorization") || key.startsWith("cookie")
          )
        ),
      },
      redirect: "manual",
    });

    if (apiRes.status >= 300 && apiRes.status < 400) {
      const location = apiRes.headers.get("location");
      if (location) {
        return NextResponse.redirect(location);
      }
    }

    const data = await apiRes.json();

    return NextResponse.json(data, {
      status: apiRes.status,
      headers: {
        "Content-Type": "application/json",
      },
    });
  } catch (error) {
    console.error(`Error proxying authorize request:`, error);
    return NextResponse.json({ success: false, error: "Internal server error" }, { status: 500 });
  }
}
