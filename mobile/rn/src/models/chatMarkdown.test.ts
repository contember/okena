/// <reference types="jest" />
// tsconfig pins `types` to react/react-native, so pull jest globals in here.

import { parseChatMarkdown, parseInlines } from './chatMarkdown';

describe('parseInlines', () => {
  it('splits inline code and bold out of text', () => {
    expect(parseInlines('Edit `plan.txt` **now**, then stop')).toEqual([
      { kind: 'text', text: 'Edit ' },
      { kind: 'code', text: 'plan.txt' },
      { kind: 'text', text: ' ' },
      { kind: 'bold', text: 'now' },
      { kind: 'text', text: ', then stop' },
    ]);
  });

  it('splits italic, strikethrough and links', () => {
    expect(
      parseInlines('*a* ~~b~~ [c](https://x.y/z) snake_case_name'),
    ).toEqual([
      { kind: 'italic', text: 'a' },
      { kind: 'text', text: ' ' },
      { kind: 'strike', text: 'b' },
      { kind: 'text', text: ' ' },
      { kind: 'link', text: 'c', url: 'https://x.y/z' },
      { kind: 'text', text: ' snake_case_name' },
    ]);
  });

  it('keeps unmatched markers literal', () => {
    expect(parseInlines('a ` b ** c')).toEqual([
      { kind: 'text', text: 'a ` b ** c' },
    ]);
  });
});

describe('parseChatMarkdown', () => {
  it('parses headings, list items, paragraphs and fenced code', () => {
    const source = [
      '## Answers',
      '- **File:** `plan.txt`',
      '2. tests',
      '',
      'First line',
      'second line',
      '```ts',
      'const a = 1;',
      '',
      'const b = 2;',
      '```',
      'After',
    ].join('\n');
    expect(parseChatMarkdown(source)).toEqual([
      { kind: 'heading', inlines: [{ kind: 'text', text: 'Answers' }] },
      {
        kind: 'item',
        marker: '•',
        depth: 0,
        inlines: [
          { kind: 'bold', text: 'File:' },
          { kind: 'text', text: ' ' },
          { kind: 'code', text: 'plan.txt' },
        ],
      },
      {
        kind: 'item',
        marker: '2.',
        depth: 0,
        inlines: [{ kind: 'text', text: 'tests' }],
      },
      {
        kind: 'paragraph',
        inlines: [{ kind: 'text', text: 'First line\nsecond line' }],
      },
      { kind: 'code', text: 'const a = 1;\n\nconst b = 2;' },
      { kind: 'paragraph', inlines: [{ kind: 'text', text: 'After' }] },
    ]);
  });

  it('runs an unclosed fence to the end', () => {
    expect(parseChatMarkdown('```\nls -la')).toEqual([
      { kind: 'code', text: 'ls -la' },
    ]);
  });

  it('parses a GFM table with alignment, escaped pipes and ragged rows', () => {
    const source = [
      'Intro',
      '| Name | Size |',
      '|:-----|-----:|',
      '| `a\\|b` | 1 |',
      '| c |',
      '| d | 2 | extra |',
      'After',
    ].join('\n');
    expect(parseChatMarkdown(source)).toEqual([
      { kind: 'paragraph', inlines: [{ kind: 'text', text: 'Intro' }] },
      {
        kind: 'table',
        align: ['left', 'right'],
        header: [
          [{ kind: 'text', text: 'Name' }],
          [{ kind: 'text', text: 'Size' }],
        ],
        rows: [
          [[{ kind: 'code', text: 'a|b' }], [{ kind: 'text', text: '1' }]],
          [[{ kind: 'text', text: 'c' }], []],
          [[{ kind: 'text', text: 'd' }], [{ kind: 'text', text: '2' }]],
        ],
      },
      { kind: 'paragraph', inlines: [{ kind: 'text', text: 'After' }] },
    ]);
  });

  it('keeps a pipe line without a delimiter row as a paragraph', () => {
    expect(parseChatMarkdown('a | b\nc')).toEqual([
      { kind: 'paragraph', inlines: [{ kind: 'text', text: 'a | b\nc' }] },
    ]);
  });

  it('parses quotes, rules, nested and task items', () => {
    expect(
      parseChatMarkdown('> **Note**\n> - x\n\n---\n- [x] done\n  - [ ] todo'),
    ).toEqual([
      {
        kind: 'quote',
        blocks: [
          { kind: 'paragraph', inlines: [{ kind: 'bold', text: 'Note' }] },
          {
            kind: 'item',
            marker: '•',
            depth: 0,
            inlines: [{ kind: 'text', text: 'x' }],
          },
        ],
      },
      { kind: 'rule' },
      {
        kind: 'item',
        marker: '☑',
        depth: 0,
        inlines: [{ kind: 'text', text: 'done' }],
      },
      {
        kind: 'item',
        marker: '☐',
        depth: 1,
        inlines: [{ kind: 'text', text: 'todo' }],
      },
    ]);
  });
});
