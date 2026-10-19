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
        inlines: [
          { kind: 'bold', text: 'File:' },
          { kind: 'text', text: ' ' },
          { kind: 'code', text: 'plan.txt' },
        ],
      },
      {
        kind: 'item',
        marker: '2.',
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
});
