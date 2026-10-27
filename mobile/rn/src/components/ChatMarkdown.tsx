/**
 * ChatMarkdown.tsx — draws Claude's Markdown text (see `models/chatMarkdown`):
 * paragraphs, headings, list items, tables, quotes, rules and code blocks.
 */

import React, { useMemo } from 'react';
import {
  Linking,
  ScrollView,
  StyleSheet,
  Text,
  View,
  type StyleProp,
  type ViewStyle,
} from 'react-native';

import {
  parseChatMarkdown,
  type MdBlock,
  type MdInline,
} from '../models/chatMarkdown';
import { OkenaColors, TerminalTheme } from '../theme';

export const ChatMarkdown: React.FC<{
  text: string;
  style?: StyleProp<ViewStyle>;
}> = ({ text, style }) => {
  const blocks = useMemo(() => parseChatMarkdown(text), [text]);
  return <Blocks blocks={blocks} style={style} />;
};

const Blocks: React.FC<{
  blocks: MdBlock[];
  style?: StyleProp<ViewStyle>;
}> = ({ blocks, style }) => (
  <View style={[styles.root, style]}>
    {blocks.map((block, index) => {
      switch (block.kind) {
        case 'code':
          return (
            <ScrollView key={index} horizontal style={styles.codeBlock}>
              <Text style={styles.codeBlockText} selectable>
                {block.text}
              </Text>
            </ScrollView>
          );
        case 'heading':
          return (
            <Text key={index} style={[styles.text, styles.bold]} selectable>
              <Inlines inlines={block.inlines} />
            </Text>
          );
        case 'item':
          return (
            <View
              key={index}
              style={[styles.item, { paddingLeft: block.depth * 16 }]}
            >
              <Text style={[styles.text, styles.itemMarker]}>
                {block.marker}
              </Text>
              <Text style={[styles.text, styles.itemText]} selectable>
                <Inlines inlines={block.inlines} />
              </Text>
            </View>
          );
        case 'paragraph':
          return (
            <Text key={index} style={styles.text} selectable>
              <Inlines inlines={block.inlines} />
            </Text>
          );
        case 'quote':
          return (
            <Blocks key={index} blocks={block.blocks} style={styles.quote} />
          );
        case 'rule':
          return <View key={index} style={styles.rule} />;
        case 'table':
          return <Table key={index} table={block} />;
      }
    })}
  </View>
);

type TableBlock = Extract<MdBlock, { kind: 'table' }>;

const CELL_CHAR_WIDTH = 8;
const CELL_MIN_WIDTH = 48;
const CELL_MAX_WIDTH = 220;

function inlinesLength(inlines: MdInline[]): number {
  return inlines.reduce((sum, inline) => sum + inline.text.length, 0);
}

/** Fixed column widths from the longest cell, so rows line up while cells wrap. */
function columnWidths(table: TableBlock): number[] {
  return table.header.map((head, column) => {
    const longest = Math.max(
      inlinesLength(head),
      ...table.rows.map((row) => inlinesLength(row[column] ?? [])),
    );
    return Math.min(
      CELL_MAX_WIDTH,
      Math.max(CELL_MIN_WIDTH, longest * CELL_CHAR_WIDTH + 16),
    );
  });
}

const Table: React.FC<{ table: TableBlock }> = ({ table }) => {
  const widths = useMemo(() => columnWidths(table), [table]);
  const renderRow = (cells: MdInline[][], header: boolean, key: number) => (
    <View key={key} style={[styles.tableRow, header && styles.tableHeader]}>
      {cells.map((cell, column) => (
        <Text
          key={column}
          style={[
            styles.tableCell,
            header && styles.bold,
            {
              width: widths[column],
              textAlign: table.align[column] ?? 'left',
            },
          ]}
          selectable
        >
          <Inlines inlines={cell} />
        </Text>
      ))}
    </View>
  );
  return (
    <ScrollView horizontal style={styles.table}>
      <View>
        {renderRow(table.header, true, -1)}
        {table.rows.map((row, index) => renderRow(row, false, index))}
      </View>
    </ScrollView>
  );
};

const Inlines: React.FC<{ inlines: MdInline[] }> = ({ inlines }) => (
  <>
    {inlines.map((inline, index) => {
      switch (inline.kind) {
        case 'text':
          return inline.text;
        case 'link':
          // Only web links: the reply may quote untrusted text, and other schemes open apps.
          if (!/^https?:\/\//i.test(inline.url)) return inline.text;
          return (
            <Text
              key={index}
              style={styles.link}
              onPress={() => void Linking.openURL(inline.url).catch(() => {})}
            >
              {inline.text}
            </Text>
          );
        default:
          return (
            <Text key={index} style={styles[inline.kind]}>
              {inline.text}
            </Text>
          );
      }
    })}
  </>
);

const styles = StyleSheet.create({
  root: { gap: 8 },
  text: {
    color: OkenaColors.textPrimary,
    fontSize: 15,
    lineHeight: 22,
  },
  bold: { fontWeight: '700' },
  italic: { fontStyle: 'italic' },
  strike: { textDecorationLine: 'line-through' },
  link: { color: OkenaColors.accent, textDecorationLine: 'underline' },
  code: {
    fontFamily: TerminalTheme.fontFamily,
    fontSize: 13,
    color: OkenaColors.accent,
  },
  item: { flexDirection: 'row' },
  itemMarker: { width: 22, color: OkenaColors.textSecondary },
  itemText: { flex: 1 },
  codeBlock: {
    backgroundColor: OkenaColors.surface,
    borderRadius: 8,
    borderWidth: StyleSheet.hairlineWidth,
    borderColor: OkenaColors.borderLight,
  },
  quote: {
    borderLeftWidth: 3,
    borderLeftColor: OkenaColors.borderLight,
    paddingLeft: 10,
  },
  rule: {
    height: StyleSheet.hairlineWidth,
    backgroundColor: OkenaColors.borderLight,
  },
  table: {
    borderRadius: 8,
    borderWidth: StyleSheet.hairlineWidth,
    borderColor: OkenaColors.borderLight,
  },
  tableRow: {
    flexDirection: 'row',
    borderBottomWidth: StyleSheet.hairlineWidth,
    borderBottomColor: OkenaColors.borderLight,
  },
  tableHeader: { backgroundColor: OkenaColors.surface },
  tableCell: {
    color: OkenaColors.textPrimary,
    fontSize: 13,
    lineHeight: 18,
    paddingHorizontal: 8,
    paddingVertical: 6,
  },
  codeBlockText: {
    color: OkenaColors.textPrimary,
    fontFamily: TerminalTheme.fontFamily,
    fontSize: 12,
    lineHeight: 17,
    padding: 10,
  },
});
