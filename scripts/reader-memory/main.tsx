import React from "react";
import { createRoot } from "react-dom/client";
import { PdfPageView } from "../../src/components/PdfPageView";
import { api } from "../../src/lib/api";
import "../../src/index.css";

const NativeObserver = window.IntersectionObserver;
let observerCount = 0;
window.IntersectionObserver = class extends NativeObserver {
  private disconnected = false;
  constructor(callback: IntersectionObserverCallback, options?: IntersectionObserverInit) {
    super(callback, options); observerCount++;
  }
  disconnect() { if (!this.disconnected) { observerCount--; this.disconnected = true; } super.disconnect(); }
};
const calls: Array<{ path: string; page: number; width: number }> = [];
let active = 0;
let peak = 0;
const canvas = document.createElement("canvas");
canvas.width = 700; canvas.height = 906;
const ctx = canvas.getContext("2d")!;
const pixels = ctx.createImageData(canvas.width, canvas.height);
let seed = 42;
for (let i = 0; i < pixels.data.length; i += 4) {
  seed = (seed * 1664525 + 1013904223) >>> 0;
  pixels.data[i] = seed & 255; pixels.data[i + 1] = (seed >>> 8) & 255;
  pixels.data[i + 2] = (seed >>> 16) & 255; pixels.data[i + 3] = 255;
}
ctx.putImageData(pixels, 0, 0);
api.pdfLocalPath = async (id) => id;
api.pdfPageCount = async () => 40;
api.pdfPageImage = async (path, page, width) => {
  calls.push({ path, page, width }); peak = Math.max(peak, ++active);
  await new Promise((resolve) => setTimeout(resolve, 40));
  // Distinct encoded bitmap per page, just as the production command returns.
  ctx.fillStyle = "white"; ctx.fillRect(0, 0, 350, 60); ctx.fillStyle = "black";
  ctx.font = "24px sans-serif"; ctx.fillText(`${path} — page ${page}`, 15, 40);
  const url = canvas.toDataURL(); active--; return url;
};
const root = createRoot(document.getElementById("root")!);
function open(id = "first.pdf") {
  root.render(<div style={{ display: "flex", height: "100vh", width: 760 }}><PdfPageView key={id} sourceId={id} title={id} /></div>);
}
open();
Object.assign(window, { readerMemory: { calls, get peak() { return peak; }, get observers() { return observerCount; }, open,
  close: () => root.render(null) } });
