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
      className="mr-1 select-none rounded-full border border-warning/40 bg-warning/15 px-2 py-0.5 text-badge font-semibold tracking-wide text-warning"
      title={`Dev build · ${build.commit}`}
    >
      DEV
    </span>
  );
}
