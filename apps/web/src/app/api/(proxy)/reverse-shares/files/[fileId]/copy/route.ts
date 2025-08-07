import { NextRequest, NextResponse } from "next/server";

export const maxDuration = 600000; // 10 minutes timeout for large file copies
export const dynamic = "force-dynamic";

const API_BASE_URL = process.env.API_BASE_URL || "http://localhost:3333";

export async function POST(req: NextRequest, { params }: { params: Promise<{ fileId: string }> }) {
  const { fileId } = await params;
  const cookieHeader = req.headers.get("cookie");
  const url = `${API_BASE_URL}/reverse-shares/files/${fileId}/copy`;

  try {
    const testResponse = await fetch(`${API_BASE_URL}/health`, {
      method: "GET",
      signal: AbortSignal.timeout(5000), // 5 seconds timeout
    });

    if (!testResponse.ok) {
      throw new Error(`Backend health check failed: ${testResponse.status}`);
    }

    const controller = new AbortController();
    const timeoutId = setTimeout(() => {
      controller.abort();
    }, 600000); // 10 minutes

    const apiRes = await fetch(url, {
      method: "POST",
      headers: {
        cookie: cookieHeader || "",
      },
      redirect: "manual",
      signal: controller.signal,
    });

    clearTimeout(timeoutId);

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
  } catch (error: any) {
    console.error(`Copy to my files proxy error details:`, {
      name: error.name,
      message: error.message,
      code: error.code,
      cause: error.cause,
    });

    if (error.name === "AbortError") {
      return new NextResponse(
        JSON.stringify({
          error: "Copy operation timed out",
          details: "The operation took too long to complete",
          fileId,
        }),
        {
          status: 408,
          headers: {
            "Content-Type": "application/json",
          },
        }
      );
    }

    return new NextResponse(
      JSON.stringify({
        error: "Copy operation failed",
        details: error.message || "Unknown error",
        fileId,
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
