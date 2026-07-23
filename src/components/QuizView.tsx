import { useState } from "react";
import { Check, RotateCcw, X } from "lucide-react";
import { Markdown } from "./Markdown";
import { cn } from "@/lib/utils";

/**
 * Native quiz renderer. The generator emits `## Questions` (numbered, options
 * A-D one per line) plus `## Answer Key` (see rag::artifact_spec); we parse
 * that into an answerable quiz with immediate grading. Falls back to Markdown
 * when the content doesn't parse, so a quiz never arrives broken.
 */

interface Option {
  letter: string;
  text: string;
}

interface Question {
  n: number;
  text: string;
  options: Option[];
  answer: string;
  explanation: string;
}

/** Parse the quiz spec; null when it isn't a usable quiz. */
export function parseQuiz(md: string): Question[] | null {
  const keySplit = md.split(/^##\s*Answer\s*Key\s*$/im);
  if (keySplit.length < 2) return null;
  const [questionPart, keyPart] = keySplit;

  // Answer key: `<n>. <letter> — <explanation>` (tolerant of ) : - variants).
  const answers = new Map<number, { letter: string; explanation: string }>();
  for (const line of keyPart.split("\n")) {
    const m = /^\s*(\d+)[.)]\s*\*{0,2}([A-D])\*{0,2}\s*[—:–-]?\s*(.*)$/.exec(line);
    if (m) answers.set(Number(m[1]), { letter: m[2], explanation: m[3].trim() });
    else if (answers.size > 0 && line.trim()) {
      // Wrapped explanation lines belong to the previous entry.
      const last = [...answers.values()].pop()!;
      last.explanation = `${last.explanation} ${line.trim()}`.trim();
    }
  }

  const questions: Question[] = [];
  let current: Omit<Question, "answer" | "explanation"> | null = null;
  const push = () => {
    if (!current) return;
    const key = answers.get(current.n);
    if (key && current.options.length >= 2) {
      questions.push({ ...current, answer: key.letter, explanation: key.explanation });
    }
    current = null;
  };
  for (const raw of questionPart.split("\n")) {
    const line = raw.trim();
    const q = /^(\d+)[.)]\s+(.*)$/.exec(line);
    const opt = /^([A-D])[.)]\s+(.*)$/.exec(line);
    if (q) {
      push();
      current = { n: Number(q[1]), text: q[2].trim(), options: [] };
    } else if (opt && current) {
      current.options.push({ letter: opt[1], text: opt[2].trim() });
    } else if (line && current) {
      // Wrapped text extends the last option, or the question stem.
      const last = current.options[current.options.length - 1];
      if (last) last.text = `${last.text} ${line}`;
      else current.text = `${current.text} ${line}`;
    }
  }
  push();
  return questions.length >= 3 ? questions : null;
}

export function QuizView({ content }: { content: string }) {
  const questions = parseQuiz(content);
  if (!questions) return <Markdown>{content}</Markdown>;
  return <Quiz questions={questions} />;
}

function Quiz({ questions }: { questions: Question[] }) {
  const [picks, setPicks] = useState<Record<number, string>>({});
  const answered = Object.keys(picks).length;
  const correct = questions.filter((q) => picks[q.n] === q.answer).length;

  return (
    <div className="flex flex-col gap-4">
      <div className="sticky top-0 z-10 flex items-center gap-2 rounded-md border border-border bg-elevated px-3 py-1.5 text-caption text-muted-foreground">
        <span className="tabular-nums">
          {answered} / {questions.length} answered
          {answered > 0 && (
            <>
              {" · "}
              <span className={correct === answered ? "text-success" : undefined}>
                {correct} correct
              </span>
            </>
          )}
        </span>
        {answered > 0 && (
          <button
            type="button"
            onClick={() => setPicks({})}
            className="ml-auto inline-flex items-center gap-1 rounded px-1.5 py-0.5 transition-colors hover:text-foreground"
          >
            <RotateCcw className="h-3 w-3" />
            Reset
          </button>
        )}
      </div>
      {questions.map((q) => {
        const pick = picks[q.n];
        return (
          <div key={q.n} className="flex flex-col gap-1.5">
            <div className="text-[0.84375rem] font-medium text-foreground">
              {q.n}. {q.text}
            </div>
            <div className="flex flex-col gap-1">
              {q.options.map((o) => {
                const chosen = pick === o.letter;
                const isRight = o.letter === q.answer;
                return (
                  <button
                    key={o.letter}
                    type="button"
                    disabled={!!pick}
                    onClick={() => setPicks((p) => ({ ...p, [q.n]: o.letter }))}
                    className={cn(
                      "flex items-start gap-2 rounded-md border px-2.5 py-1.5 text-left text-body transition-colors",
                      !pick && "border-border hover:border-border-strong hover:bg-surface-2",
                      pick && isRight && "border-success/60 bg-success/10",
                      pick && chosen && !isRight && "border-destructive/60 bg-destructive/10",
                      pick && !chosen && !isRight && "border-border opacity-60",
                    )}
                  >
                    <span className="mt-px shrink-0 font-medium text-muted-foreground">
                      {o.letter})
                    </span>
                    <span className="flex-1 text-foreground/90">{o.text}</span>
                    {pick && isRight && <Check className="mt-0.5 h-3.5 w-3.5 shrink-0 text-success" />}
                    {pick && chosen && !isRight && (
                      <X className="mt-0.5 h-3.5 w-3.5 shrink-0 text-destructive" />
                    )}
                  </button>
                );
              })}
            </div>
            {pick && q.explanation && (
              <div className="rounded-md bg-surface-2 px-2.5 py-1.5 text-caption leading-relaxed text-muted-foreground">
                {q.explanation}
              </div>
            )}
          </div>
        );
      })}
    </div>
  );
}
