/**
 * chatMarkdown.ts — the subset of Markdown Claude's replies lean on, parsed
 * into blocks the chat can draw: fenced code, headings, list items and
 * paragraphs, with inline code and bold. Anything else stays literal text.
 */

export interface MdInline {
  kind: 'text' | 'code' | 'bold';
  text: string;
}

export type MdBlock =
  | { kind: 'paragraph'; inlines: MdInline[] }
  | { kind: 'heading'; inlines: MdInline[] }
  | { kind: 'item'; marker: string; inlines: MdInline[] }
  | { kind: 'code'; text: string };

const FENCE = /^\s*```/;
const HEADING = /^#{1,6}\s+(.*)$/;
const BULLET = /^\s*[-*+]\s+(.*)$/;
const ORDERED = /^\s*(\d+)[.)]\s+(.*)$/;
const INLINE = /`([^`\n]+)`|\*\*([^*\n]+)\*\*/g;

export function parseInlines(text: string): MdInline[] {
  const out: MdInline[] = [];
  let last = 0;
  for (const match of text.matchAll(INLINE)) {
    const start = match.index ?? 0;
    if (start > last) out.push({ kind: 'text', text: text.slice(last, start) });
    if (match[1] !== undefined) out.push({ kind: 'code', text: match[1] });
    else out.push({ kind: 'bold', text: match[2] ?? '' });
    last = start + match[0].length;
  }
  if (last < text.length) out.push({ kind: 'text', text: text.slice(last) });
  return out;
}

export function parseChatMarkdown(source: string): MdBlock[] {
  const blocks: MdBlock[] = [];
  const lines = source.split('\n');
  let paragraph: string[] = [];

  const flushParagraph = () => {
    if (paragraph.length > 0) {
      blocks.push({
        kind: 'paragraph',
        inlines: parseInlines(paragraph.join('\n')),
      });
      paragraph = [];
    }
  };

  // Lines of the open fence; an unclosed one runs to the end, as in a reply still streaming.
  let code: string[] | null = null;

  for (const line of lines) {
    if (code !== null) {
      if (FENCE.test(line)) {
        blocks.push({ kind: 'code', text: code.join('\n') });
        code = null;
      } else {
        code.push(line);
      }
      continue;
    }
    if (FENCE.test(line)) {
      flushParagraph();
      code = [];
      continue;
    }
    const heading = HEADING.exec(line);
    const bullet = BULLET.exec(line);
    const ordered = ORDERED.exec(line);
    if (heading) {
      flushParagraph();
      blocks.push({ kind: 'heading', inlines: parseInlines(heading[1] ?? '') });
    } else if (bullet) {
      flushParagraph();
      blocks.push({
        kind: 'item',
        marker: '•',
        inlines: parseInlines(bullet[1] ?? ''),
      });
    } else if (ordered) {
      flushParagraph();
      blocks.push({
        kind: 'item',
        marker: `${ordered[1] ?? ''}.`,
        inlines: parseInlines(ordered[2] ?? ''),
      });
    } else if (line.trim().length === 0) {
      flushParagraph();
    } else {
      paragraph.push(line);
    }
  }
  flushParagraph();
  if (code !== null) blocks.push({ kind: 'code', text: code.join('\n') });
  return blocks;
}
