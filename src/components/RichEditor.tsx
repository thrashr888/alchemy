import { useEffect, useState } from "react";
import { useEditor, EditorContent, type Editor } from "@tiptap/react";
import StarterKit from "@tiptap/starter-kit";
import { Markdown } from "tiptap-markdown";
import { cn } from "@/lib/utils";
import { Modal, Button, Input } from "./ui";
import {
  Bold,
  Italic,
  Heading1,
  Heading2,
  List,
  ListOrdered,
  Quote,
  Code,
  Link2,
  Undo2,
  Redo2,
} from "lucide-react";

// tiptap-markdown augments editor.storage with a `markdown` helper it doesn't type.
function getMarkdown(editor: Editor): string {
  const storage = editor.storage as unknown as Record<string, { getMarkdown?: () => string }>;
  return storage.markdown?.getMarkdown?.() ?? "";
}

/** WYSIWYG note editor. Value in/out is Markdown (via tiptap-markdown). */
export function RichEditor({
  value,
  onChange,
  fill = false,
}: {
  value: string;
  onChange: (markdown: string) => void;
  /** Stretch to the parent's full height (reader pane) instead of the
   *  self-sizing modal behavior (min 240px, capped at 52vh). */
  fill?: boolean;
}) {
  const editor = useEditor({
    extensions: [
      StarterKit.configure({ link: { openOnClick: false } }),
      Markdown.configure({ html: false, transformPastedText: true }),
    ],
    content: value,
    onUpdate: ({ editor }) => onChange(getMarkdown(editor)),
    editorProps: {
      attributes: {
        class: fill
          ? "prose max-w-none h-full overflow-y-auto px-3 py-2.5 focus:outline-none"
          : "prose max-w-none min-h-[240px] max-h-[52vh] overflow-y-auto px-3 py-2.5 focus:outline-none",
      },
    },
  });

  if (!editor) return null;
  return (
    <div
      className={cn(
        "overflow-hidden rounded-md border border-input bg-surface-2",
        fill && "flex h-full flex-col",
      )}
    >
      <Toolbar editor={editor} />
      <EditorContent
        editor={editor}
        className={fill ? "min-h-0 flex-1 [&>div]:h-full" : undefined}
      />
    </div>
  );
}

function Toolbar({ editor }: { editor: Editor }) {
  // Force re-render on each transaction so active states stay in sync.
  const [, bump] = useState(0);
  const [linkOpen, setLinkOpen] = useState(false);
  const [linkUrl, setLinkUrl] = useState("");
  useEffect(() => {
    const update = () => bump((n) => n + 1);
    editor.on("transaction", update);
    return () => {
      editor.off("transaction", update);
    };
  }, [editor]);

  const openLink = () => {
    const prev = editor.getAttributes("link").href as string | undefined;
    setLinkUrl(prev ?? "https://");
    setLinkOpen(true);
  };
  const applyLink = () => {
    const url = linkUrl.trim();
    if (url === "") {
      editor.chain().focus().unsetLink().run();
    } else {
      editor.chain().focus().extendMarkRange("link").setLink({ href: url }).run();
    }
    setLinkOpen(false);
  };

  return (
    <>
    <div className="flex flex-wrap items-center gap-0.5 border-b border-border bg-surface px-1.5 py-1">
      <Btn on={editor.isActive("bold")} onClick={() => editor.chain().focus().toggleBold().run()} title="Bold">
        <Bold className="h-3.5 w-3.5" />
      </Btn>
      <Btn on={editor.isActive("italic")} onClick={() => editor.chain().focus().toggleItalic().run()} title="Italic">
        <Italic className="h-3.5 w-3.5" />
      </Btn>
      <Sep />
      <Btn
        on={editor.isActive("heading", { level: 1 })}
        onClick={() => editor.chain().focus().toggleHeading({ level: 1 }).run()}
        title="Heading 1"
      >
        <Heading1 className="h-3.5 w-3.5" />
      </Btn>
      <Btn
        on={editor.isActive("heading", { level: 2 })}
        onClick={() => editor.chain().focus().toggleHeading({ level: 2 }).run()}
        title="Heading 2"
      >
        <Heading2 className="h-3.5 w-3.5" />
      </Btn>
      <Sep />
      <Btn on={editor.isActive("bulletList")} onClick={() => editor.chain().focus().toggleBulletList().run()} title="Bullet list">
        <List className="h-3.5 w-3.5" />
      </Btn>
      <Btn on={editor.isActive("orderedList")} onClick={() => editor.chain().focus().toggleOrderedList().run()} title="Numbered list">
        <ListOrdered className="h-3.5 w-3.5" />
      </Btn>
      <Btn on={editor.isActive("blockquote")} onClick={() => editor.chain().focus().toggleBlockquote().run()} title="Quote">
        <Quote className="h-3.5 w-3.5" />
      </Btn>
      <Btn on={editor.isActive("code")} onClick={() => editor.chain().focus().toggleCode().run()} title="Inline code">
        <Code className="h-3.5 w-3.5" />
      </Btn>
      <Btn on={editor.isActive("link")} onClick={openLink} title="Link">
        <Link2 className="h-3.5 w-3.5" />
      </Btn>
      <Sep />
      <Btn onClick={() => editor.chain().focus().undo().run()} title="Undo">
        <Undo2 className="h-3.5 w-3.5" />
      </Btn>
      <Btn onClick={() => editor.chain().focus().redo().run()} title="Redo">
        <Redo2 className="h-3.5 w-3.5" />
      </Btn>
    </div>

    <Modal
      open={linkOpen}
      onClose={() => setLinkOpen(false)}
      title="Add link"
      footer={
        <div className="flex justify-end gap-2">
          <Button variant="ghost" onClick={() => setLinkOpen(false)}>
            Cancel
          </Button>
          <Button variant="primary" onClick={applyLink}>
            {linkUrl.trim() ? "Apply" : "Remove link"}
          </Button>
        </div>
      }
    >
      <Input
        autoFocus
        value={linkUrl}
        onChange={(e) => setLinkUrl(e.target.value)}
        onKeyDown={(e) => e.key === "Enter" && applyLink()}
        placeholder="https://example.com"
      />
    </Modal>
    </>
  );
}

function Btn({
  on,
  onClick,
  title,
  children,
}: {
  on?: boolean;
  onClick: () => void;
  title: string;
  children: React.ReactNode;
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      title={title}
      className={cn(
        "flex h-7 w-7 items-center justify-center rounded transition-colors",
        on
          ? "bg-primary/15 text-citation"
          : "text-muted-foreground hover:bg-surface-2 hover:text-foreground",
      )}
    >
      {children}
    </button>
  );
}

function Sep() {
  return <div className="mx-0.5 h-4 w-px bg-border" />;
}
