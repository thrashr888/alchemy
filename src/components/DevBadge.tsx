import { useEffect, useState } from "react";
import { api } from "@/lib/api";
import type { BuildInfo } from "@/lib/types";

/** Title-bar dev-build marker: dev and the installed app share a data dir
 *  and look identical — this chip tells the windows apart. Renders nothing
 *  on release builds. */
export function DevBadge() {
  const [build, setBuild] = useState<BuildInfo | null>(null);
  useEffect(() => {
    api
      .buildInfo()
      .then(setBuild)
      .catch(() => {});
  }, []);
  if (build?.profile !== "dev") return null;
  return (
    <span
      className="mr-1 select-none rounded-full border border-[#e8a33d]/40 bg-[#e8a33d]/15 px-2 py-0.5 text-[10px] font-semibold tracking-wide text-[#e8a33d] [[data-scheme=light]_&]:text-[#7a5200]"
      title={`Dev build · ${build.commit}`}
    >
      DEV
    </span>
  );
}
