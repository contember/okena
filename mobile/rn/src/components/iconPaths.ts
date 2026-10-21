/**
 * iconPaths.ts — SVG path data for the app's UI icons (drawn by `Icon.tsx`).
 *
 * Unicode symbols such as `⛶ 🗗 ⎇ ☰ ⃠` are missing from stock iOS/Android
 * fonts and render as a "?" box, so UI icons are vector paths instead. All
 * paths use a 24×24 box. `stroke` icons are drawn with round caps/joins at the
 * requested stroke width; `fill` icons are filled.
 */

export interface IconDef {
  d: string;
  fill?: boolean;
}

export const ICONS = {
  menu: { d: 'M4 6h16M4 12h16M4 18h16' },
  plus: { d: 'M12 5v14M5 12h14' },
  close: { d: 'M6 6l12 12M18 6L6 18' },
  check: { d: 'M20 6L9 17l-5-5' },
  chevronLeft: { d: 'M15 5l-7 7 7 7' },
  chevronRight: { d: 'M9 5l7 7-7 7' },
  chevronDown: { d: 'M5 9l7 7 7-7' },
  caretRight: { d: 'M9 6l7 6-7 6z', fill: true },
  caretDown: { d: 'M6 9h12l-6 7z', fill: true },
  maximize: {
    d: 'M3 8V5a2 2 0 0 1 2-2h3M16 3h3a2 2 0 0 1 2 2v3M21 16v3a2 2 0 0 1-2 2h-3M8 21H5a2 2 0 0 1-2-2v-3',
  },
  minimize: {
    d: 'M8 3v3a2 2 0 0 1-2 2H3M21 8h-3a2 2 0 0 1-2-2V3M16 21v-3a2 2 0 0 1 2-2h3M3 16h3a2 2 0 0 1 2 2v3',
  },
  gitBranch: {
    d: 'M6 3v12M3 18a3 3 0 1 0 6 0a3 3 0 1 0-6 0M15 6a3 3 0 1 0 6 0a3 3 0 1 0-6 0M18 9a9 9 0 0 1-9 9',
  },
  keyboard: {
    d: 'M4 5h16a2 2 0 0 1 2 2v10a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V7a2 2 0 0 1 2-2zM6 9h.01M10 9h.01M14 9h.01M18 9h.01M8 13h.01M12 13h.01M16 13h.01M7 16h10',
  },
  prompt: { d: 'M5 7l5 5-5 5M12 17h7' },
  chat: { d: 'M21 11.5a8.5 8.5 0 0 1-12.4 7.6L3 21l1.9-5.6A8.5 8.5 0 1 1 21 11.5z' },
  arrowUp: { d: 'M12 19V5M5 12l7-7 7 7' },
  stop: { d: 'M7 7h10v10H7z', fill: true },
  gauge: { d: 'M4 17a8 8 0 1 1 16 0M12 17l3.5-5' },
  disconnect: { d: 'M3 12a9 9 0 1 0 18 0a9 9 0 1 0-18 0M5.7 5.7l12.6 12.6' },
  bell: { d: 'M6 16v-5a6 6 0 0 1 12 0v5l2 2H4l2-2zM10 21h4' },
  dashboard: { d: 'M4 4h6v6H4zM14 4h6v6h-6zM4 14h6v6H4zM14 14h6v6h-6z' },
} satisfies Record<string, IconDef>;

export type IconName = keyof typeof ICONS;
