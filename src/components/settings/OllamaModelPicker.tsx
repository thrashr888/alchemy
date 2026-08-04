/* Pick an Ollama model from the ones actually installed.

   Typing a model name from memory is a bad ask: the exact tag matters
   (`gemma4:12b-mlx`, not `gemma4`), a typo fails silently at call time, and
   nothing in the app told you what was on the machine. Ollama already
   answers `/api/tags`, and `list_models` already wraps it — so the field is
   a list.

   "Custom…" stays, because a model can be pulled after this dialog opened,
   and a value already set must never be erased just because Ollama is down. */
import { useEffect, useState } from "react";
import { api } from "@/lib/api";
import { Input } from "../ui";

const CUSTOM = "__custom__";

/** Installed Ollama models, fetched once per mount. Empty on any failure —
    the picker degrades to a plain text field rather than blocking. */
function useInstalledModels(enabled: boolean) {
  const [models, setModels] = useState<string[]>([]);
  const [loaded, setLoaded] = useState(false);
  useEffect(() => {
    if (!enabled) return;
    let stale = false;
    api
      .listModels()
      .then((list) => {
        if (stale) return;
        setModels([...list].sort((a, b) => a.localeCompare(b)));
        setLoaded(true);
      })
      .catch(() => {
        if (!stale) setLoaded(true);
      });
    return () => {
      stale = true;
    };
  }, [enabled]);
  return { models, loaded };
}

export function OllamaModelPicker({
  value,
  onChange,
  placeholder,
  label,
  /** Text for the empty choice; omit to require a value. */
  emptyLabel,
  /** Show only models whose name matches — embedders and OCR models have
   *  no business in the chat list. */
  filter,
}: {
  value: string;
  onChange: (value: string) => void;
  placeholder?: string;
  label: string;
  emptyLabel?: string;
  filter?: (name: string) => boolean;
}) {
  const { models, loaded } = useInstalledModels(true);
  const shown = filter ? models.filter(filter) : models;
  // A value Ollama doesn't report (pulled elsewhere, renamed, or a remote
  // host) is still the user's answer — keep it selectable rather than
  // silently swapping it for something else.
  const known = !value || shown.includes(value);
  const [custom, setCustom] = useState(!known);

  useEffect(() => {
    if (!known) setCustom(true);
  }, [known]);

  if (loaded && shown.length === 0) {
    // Ollama unreachable or nothing installed: a dropdown of nothing is
    // worse than the field we had.
    return (
      <Input
        aria-label={label}
        value={value}
        onChange={(e) => onChange(e.target.value)}
        placeholder={placeholder}
      />
    );
  }

  return (
    <div className="flex flex-col gap-1.5">
      <select
        aria-label={label}
        value={custom ? CUSTOM : value}
        onChange={(e) => {
          if (e.target.value === CUSTOM) {
            setCustom(true);
            return;
          }
          setCustom(false);
          onChange(e.target.value);
        }}
        className="h-8 rounded-md border border-input bg-surface-2 px-2 text-body text-foreground focus:outline-none"
      >
        {emptyLabel !== undefined && <option value="">{emptyLabel}</option>}
        {shown.map((m) => (
          <option key={m} value={m}>
            {m}
          </option>
        ))}
        <option value={CUSTOM}>Custom…</option>
      </select>
      {custom && (
        <Input
          autoFocus
          aria-label={`${label} (custom)`}
          value={value}
          onChange={(e) => onChange(e.target.value)}
          placeholder={placeholder}
        />
      )}
      {!loaded && (
        <span className="text-micro text-subtle-foreground">
          Reading installed models…
        </span>
      )}
    </div>
  );
}
