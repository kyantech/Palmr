import { NextRequest, NextResponse } from "next/server";

const API_BASE_URL = process.env.API_BASE_URL || "http://localhost:3333";

/**
 * Upload proxy for internal storage system
 *
 * This proxy is ONLY used when ENABLE_S3=false (internal storage mode).
 * External S3 uploads use presigned URLs directly from the client.
 *
 * Why we need this proxy:
 * 1. Security: Internal storage is not exposed to the internet
 * 2. Simplicity: No need to configure CORS on storage system
 * 3. Compatibility: Works in any network setup
 *
 * Performance note: Node.js streams the upload efficiently with minimal memory overhead
 */

async function handleUpload(req: NextRequest, method: "POST" | "PUT") {
  try {
    const { searchParams } = new URL(req.url);
    const queryString = searchParams.toString();
    const url = `${API_BASE_URL}/files/upload${queryString ? `?${queryString}` : ""}`;

    const body = req.body;

    const apiRes = await fetch(url, {
      method: method,
      headers: {
        "Content-Type": req.headers.get("content-type") || "application/octet-stream",
        cookie: req.headers.get("cookie") || "",
        ...Object.fromEntries(
          Array.from(req.headers.entries()).filter(
            ([key]) =>
              key.startsWith("authorization") ||
              key.startsWith("x-forwarded") ||
              key === "user-agent" ||
              key === "accept"
          )
        ),
      },
      body: body,
      // Required for streaming request bodies in Node.js 18+ / Next.js 15
      // See: https://nodejs.org/docs/latest-v18.x/api/fetch.html#request-duplex
      // @ts-expect-error - duplex not yet in TypeScript types but required at runtime
      duplex: "half",
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
    console.error("Error proxying upload request:", error);
    return new NextResponse(JSON.stringify({ error: "Failed to upload file" }), {
      status: 500,
      headers: {
        "Content-Type": "application/json",
      },
    });
  }
}

export async function POST(req: NextRequest) {
  return handleUpload(req, "POST");
}

export async function PUT(req: NextRequest) {
  return handleUpload(req, "PUT");
}
