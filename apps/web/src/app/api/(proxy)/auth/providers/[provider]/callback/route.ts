import { NextRequest, NextResponse } from "next/server";

const API_BASE_URL = process.env.API_BASE_URL || "http://localhost:3333";

export async function GET(request: NextRequest, { params }: { params: Promise<{ provider: string }> }) {
  try {
    const { provider } = await params;
    const url = new URL(request.url);
    const queryString = url.search;
    const originalHost = request.headers.get("host") || url.host;
    const originalProtocol = request.headers.get("x-forwarded-proto") || url.protocol.replace(":", "");
    const callbackUrl = `${API_BASE_URL}/auth/providers/${provider}/callback${queryString}`;

    const apiRes = await fetch(callbackUrl, {
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
        const response = NextResponse.redirect(location);

        const setCookieHeaders = apiRes.headers.getSetCookie?.() || [];
        if (setCookieHeaders.length > 0) {
          setCookieHeaders.forEach((cookie) => {
            response.headers.append("set-cookie", cookie);
          });
        } else {
          const singleCookie = apiRes.headers.get("set-cookie");
          if (singleCookie) {
            response.headers.set("set-cookie", singleCookie);
          }
        }

        return response;
      }
    }

    let data;
    try {
      data = await apiRes.json();
    } catch {
      return new NextResponse(null, {
        status: apiRes.status,
        headers: Object.fromEntries(apiRes.headers.entries()),
      });
    }

    return NextResponse.json(data, {
      status: apiRes.status,
      headers: {
        "Content-Type": "application/json",
      },
    });
  } catch (error) {
    console.error(`Error proxying callback request:`, error);
    return NextResponse.json({ success: false, error: "Internal server error" }, { status: 500 });
  }
}
