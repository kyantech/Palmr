import { NextResponse } from "next/server";

const API_BASE_URL = process.env.API_BASE_URL || "http://localhost:3333";

export async function GET() {
  try {
    const url = `${API_BASE_URL}/auth/config`;

    const apiRes = await fetch(url, {
      method: "GET",
      headers: {
        "Content-Type": "application/json",
      },
      redirect: "manual",
    });

    const resBody = await apiRes.text();
    const res = new NextResponse(resBody, {
      status: apiRes.status,
      statusText: apiRes.statusText,
    });

    apiRes.headers.forEach((value, key) => {
      res.headers.set(key, value);
    });

    return res;
  } catch (error) {
    console.error("Error proxying auth config request:", error);
    return NextResponse.json({ error: "Internal server error" }, { status: 500 });
  }
}
