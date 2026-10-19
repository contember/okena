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

  for (let i = 0; i < lines.length; i++) {
    const line = lines[i]!;
    if (FENCE.test(line)) {
      flushParagraph();
      const code: string[] = [];
      // An unclosed fence runs to the end, as in a reply still streaming.
      for (i++; i < lines.length && !FENCE.test(lines[i]!); i++)
        code.push(lines[i]!);
      blocks.push({ kind: 'code', text: code.join('\n') });
      continue;
    }
    const heading = HEADING.exec(line);
    const bullet = BULLET.exec(line);
    const ordered = ORDERED.exec(line);
    if (heading) {
      flushParagraph();
      blocks.push({ kind: 'heading', inlines: parseInlines(heading[1]!) });
    } else if (bullet) {
      flushParagraph();
      blocks.push({
        kind: 'item',
        marker: '•',
        inlines: parseInlines(bullet[1]!),
      });
    } else if (ordered) {
      flushParagraph();
      blocks.push({
        kind: 'item',
        marker: `${ordered[1]}.`,
        inlines: parseInlines(ordered[2]!),
      });
    } else if (line.trim().length === 0) {
      flushParagraph();
    } else {
      paragraph.push(line);
    }
  }
  flushParagraph();
  return blocks;
}
