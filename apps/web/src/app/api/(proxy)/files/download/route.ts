import { NextRequest, NextResponse } from "next/server";

const API_BASE_URL = process.env.API_BASE_URL || "http://localhost:3333";

export async function GET(req: NextRequest) {
  try {
    const { searchParams } = new URL(req.url);
    const queryString = searchParams.toString();
    const url = `${API_BASE_URL}/files/download${queryString ? `?${queryString}` : ""}`;

    const apiRes = await fetch(url, {
      method: "GET",
      headers: {
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
      redirect: "manual",
    });

    if (!apiRes.ok) {
      const errorText = await apiRes.text();
      return new NextResponse(errorText, {
        status: apiRes.status,
        headers: {
          "Content-Type": "application/json",
        },
      });
    }

    // Stream the file content
    const contentType = apiRes.headers.get("content-type") || "application/octet-stream";
    const contentDisposition = apiRes.headers.get("content-disposition");
    const contentLength = apiRes.headers.get("content-length");
    const cacheControl = apiRes.headers.get("cache-control");

    const res = new NextResponse(apiRes.body, {
      status: apiRes.status,
      headers: {
        "Content-Type": contentType,
      },
    });

    if (contentDisposition) {
      res.headers.set("Content-Disposition", contentDisposition);
    }
    if (contentLength) {
      res.headers.set("Content-Length", contentLength);
    }
    if (cacheControl) {
      res.headers.set("Cache-Control", cacheControl);
    }

    return res;
  } catch (error) {
    console.error("Error proxying download request:", error);
    return new NextResponse(JSON.stringify({ error: "Failed to download file" }), {
      status: 500,
      headers: {
        "Content-Type": "application/json",
      },
    });
  }
}
