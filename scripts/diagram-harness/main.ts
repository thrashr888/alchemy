import { formatArchitecture, parseArchitecture } from "@/lib/architectureDoc";
import { ensureDocumentDiagramFonts } from "@/lib/diagramFonts";
import { renderArchitecture } from "@/lib/eraserDiagram";

// Each sample is a document a generator could have written: topology only,
// no coordinates. Add a file here to see it rendered.
const SAMPLES = import.meta.glob("./samples/*.json", { query: "?raw", import: "default" });

const main = document.getElementById("samples") as HTMLElement;
ensureDocumentDiagramFonts();

let failures = 0;

for (const [file, load] of Object.entries(SAMPLES).sort()) {
  const name = file.replace(/^.*\//, "").replace(/\.json$/, "");
  const section = document.createElement("section");
  section.id = name;
  const heading = document.createElement("h2");
  heading.textContent = name;
  section.appendChild(heading);
  main.appendChild(section);

  const content = (await load()) as string;
  const parsed = parseArchitecture(content);
  if (parsed.error) {
    failures += 1;
    section.insertAdjacentHTML("beforeend", `<p class="error"></p>`);
    section.querySelector(".error")!.textContent = parsed.error;
    continue;
  }

  const started = performance.now();
  try {
    const rendered = await renderArchitecture(parsed.doc);
    const ms = Math.round(performance.now() - started);
    heading.insertAdjacentHTML(
      "beforeend",
      `<small>${rendered.width}×${rendered.height} · ${ms} ms · ${parsed.doc.entities.length} entities</small>`,
    );
    const host = document.createElement("div");
    host.className = "frame";
    host.style.width = `${rendered.width}px`;
    host.style.height = `${rendered.height}px`;
    // The stylesheet is eraser's own (global selectors); a shadow root keeps
    // it off the page, as the app viewer does.
    host.attachShadow({ mode: "open" }).innerHTML = `<style>${rendered.css}</style>${rendered.scene}`;
    section.appendChild(host);
    if (rendered.warnings.length) {
      const list = document.createElement("ul");
      list.className = "warnings";
      for (const w of rendered.warnings) {
        const li = document.createElement("li");
        li.textContent = w;
        list.appendChild(li);
      }
      section.appendChild(list);
    }
  } catch (e) {
    failures += 1;
    const p = document.createElement("p");
    p.className = "error";
    p.textContent = e instanceof Error ? e.message : String(e);
    section.appendChild(p);
  }

  const details = document.createElement("details");
  const summary = document.createElement("summary");
  summary.textContent = "document";
  const pre = document.createElement("pre");
  pre.textContent = formatArchitecture(parsed.doc);
  details.append(summary, pre);
  section.appendChild(details);
}

document.documentElement.dataset.status = failures ? "fail" : "ok";
