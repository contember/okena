/**
 * chatMarkdown.ts — the subset of Markdown Claude's replies lean on, parsed
 * into blocks the chat can draw: fenced code, headings, (nested) list items,
 * GFM tables, block quotes, rules and paragraphs, with inline code, bold,
 * italic, strikethrough and links. Anything else stays literal text.
 */

export type MdInline =
  | { kind: 'text' | 'code' | 'bold' | 'italic' | 'strike'; text: string }
  | { kind: 'link'; text: string; url: string };

export type MdAlign = 'left' | 'center' | 'right';

export type MdBlock =
  | { kind: 'paragraph'; inlines: MdInline[] }
  | { kind: 'heading'; inlines: MdInline[] }
  | { kind: 'item'; marker: string; depth: number; inlines: MdInline[] }
  | { kind: 'code'; text: string }
  | { kind: 'quote'; blocks: MdBlock[] }
  | { kind: 'rule' }
  | {
      kind: 'table';
      align: MdAlign[];
      header: MdInline[][];
      rows: MdInline[][][];
    };

const FENCE = /^\s*```/;
const HEADING = /^#{1,6}\s+(.*)$/;
const BULLET = /^(\s*)[-*+]\s+(.*)$/;
const ORDERED = /^(\s*)(\d+)[.)]\s+(.*)$/;
const TASK = /^\[([ xX])\]\s+(.*)$/;
const QUOTE = /^\s*>\s?(.*)$/;
const RULE = /^\s*([-*_])(\s*\1){2,}\s*$/;
const TABLE_DELIMITER = /^\s*\|?\s*:?-+:?\s*(\|\s*:?-+:?\s*)*\|?\s*$/;
// `_` emphasis is left out: it would mangle snake_case identifiers in prose.
const INLINE =
  /`([^`\n]+)`|\*\*([^*\n]+)\*\*|~~([^~\n]+)~~|\[([^\]\n]+)\]\(([^)\s]+)\)|\*([^*\s][^*\n]*?)\*/g;

export function parseInlines(text: string): MdInline[] {
  const out: MdInline[] = [];
  let last = 0;
  for (const match of text.matchAll(INLINE)) {
    const start = match.index ?? 0;
    if (start > last) out.push({ kind: 'text', text: text.slice(last, start) });
    if (match[1] !== undefined) out.push({ kind: 'code', text: match[1] });
    else if (match[2] !== undefined) out.push({ kind: 'bold', text: match[2] });
    else if (match[3] !== undefined)
      out.push({ kind: 'strike', text: match[3] });
    else if (match[4] !== undefined)
      out.push({ kind: 'link', text: match[4], url: match[5] ?? '' });
    else out.push({ kind: 'italic', text: match[6] ?? '' });
    last = start + match[0].length;
  }
  if (last < text.length) out.push({ kind: 'text', text: text.slice(last) });
  return out;
}

/** Cells of a table row; the outer pipes are optional and `\|` is a literal pipe. */
function splitTableRow(line: string): string[] {
  let row = line.trim();
  if (row.startsWith('|')) row = row.slice(1);
  if (row.endsWith('|') && !row.endsWith('\\|')) row = row.slice(0, -1);
  const cells: string[] = [];
  let cell = '';
  for (let i = 0; i < row.length; i++) {
    const char = row[i];
    if (char === '\\' && row[i + 1] === '|') {
      cell += '|';
      i++;
    } else if (char === '|') {
      cells.push(cell.trim());
      cell = '';
    } else {
      cell += char;
    }
  }
  cells.push(cell.trim());
  return cells;
}

function cellAlign(delimiter: string): MdAlign {
  const left = delimiter.startsWith(':');
  const right = delimiter.endsWith(':');
  if (left && right) return 'center';
  return right ? 'right' : 'left';
}

function isTableStart(line: string, next: string | undefined): boolean {
  return (
    line.includes('|') &&
    next !== undefined &&
    next.includes('-') &&
    TABLE_DELIMITER.test(next) &&
    splitTableRow(line).length === splitTableRow(next).length
  );
}

/** Leading indentation as list depth: two spaces (or a tab) per level. */
function listDepth(indent: string): number {
  const width = indent.replace(/\t/g, '  ').length;
  return Math.floor(width / 2);
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

  for (let i = 0; i < lines.length; i++) {
    const line = lines[i] ?? '';
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
    if (isTableStart(line, lines[i + 1])) {
      flushParagraph();
      const header = splitTableRow(line);
      const align = splitTableRow(lines[i + 1] ?? '').map(cellAlign);
      const rows: MdInline[][][] = [];
      i += 2;
      for (; i < lines.length; i++) {
        const row = lines[i] ?? '';
        if (row.trim().length === 0 || !row.includes('|')) break;
        const cells = splitTableRow(row);
        // GFM pads short rows and drops the excess of long ones.
        rows.push(
          header.map((_, column) => parseInlines(cells[column] ?? '')),
        );
      }
      i--;
      blocks.push({
        kind: 'table',
        align,
        header: header.map(parseInlines),
        rows,
      });
      continue;
    }
    const quote = QUOTE.exec(line);
    if (quote) {
      flushParagraph();
      const quoted = [quote[1] ?? ''];
      for (; i + 1 < lines.length; i++) {
        const next = QUOTE.exec(lines[i + 1] ?? '');
        if (!next) break;
        quoted.push(next[1] ?? '');
      }
      blocks.push({ kind: 'quote', blocks: parseChatMarkdown(quoted.join('\n')) });
      continue;
    }
    if (RULE.test(line)) {
      flushParagraph();
      blocks.push({ kind: 'rule' });
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
      const text = bullet[2] ?? '';
      const task = TASK.exec(text);
      blocks.push({
        kind: 'item',
        marker: task ? (task[1] === ' ' ? '☐' : '☑') : '•',
        depth: listDepth(bullet[1] ?? ''),
        inlines: parseInlines(task ? task[2] ?? '' : text),
      });
    } else if (ordered) {
      flushParagraph();
      blocks.push({
        kind: 'item',
        marker: `${ordered[2] ?? ''}.`,
        depth: listDepth(ordered[1] ?? ''),
        inlines: parseInlines(ordered[3] ?? ''),
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
