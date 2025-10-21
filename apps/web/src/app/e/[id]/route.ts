import { NextRequest, NextResponse } from "next/server";

const API_BASE_URL = process.env.API_BASE_URL || "http://localhost:3333";

/**
 * Short public embed endpoint: /e/{id}
 * No authentication required
 * Only works for media files (images, videos, audio)
 */
export async function GET(req: NextRequest, { params }: { params: Promise<{ id: string }> }) {
  const { id } = await params;

  if (!id) {
    return new NextResponse(JSON.stringify({ error: "File ID is required" }), {
      status: 400,
      headers: { "Content-Type": "application/json" },
    });
  }

  const url = `${API_BASE_URL}/embed/${id}`;

  try {
    const apiRes = await fetch(url, {
      method: "GET",
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

    const blob = await apiRes.blob();

    const contentType = apiRes.headers.get("content-type") || "application/octet-stream";
    const contentDisposition = apiRes.headers.get("content-disposition");
    const cacheControl = apiRes.headers.get("cache-control");

    const res = new NextResponse(blob, {
      status: apiRes.status,
      headers: {
        "Content-Type": contentType,
      },
    });

    if (contentDisposition) {
      res.headers.set("Content-Disposition", contentDisposition);
    }

    if (cacheControl) {
      res.headers.set("Cache-Control", cacheControl);
    } else {
      res.headers.set("Cache-Control", "public, max-age=31536000");
    }

    return res;
  } catch (error) {
    console.error("Error proxying embed request:", error);
    return new NextResponse(JSON.stringify({ error: "Failed to fetch file" }), {
      status: 500,
      headers: {
        "Content-Type": "application/json",
      },
    });
  }
}
