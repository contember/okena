/**
 * ChatMarkdown.tsx — draws Claude's Markdown text (see `models/chatMarkdown`):
 * paragraphs, headings, list items and code blocks, with inline code and bold.
 */

import React, { useMemo } from 'react';
import {
  ScrollView,
  StyleSheet,
  Text,
  View,
  type StyleProp,
  type ViewStyle,
} from 'react-native';

import { parseChatMarkdown, type MdInline } from '../models/chatMarkdown';
import { OkenaColors, TerminalTheme } from '../theme';

export const ChatMarkdown: React.FC<{
  text: string;
  style?: StyleProp<ViewStyle>;
}> = ({ text, style }) => {
  const blocks = useMemo(() => parseChatMarkdown(text), [text]);
  return (
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
              <View key={index} style={styles.item}>
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
        }
      })}
    </View>
  );
};

const Inlines: React.FC<{ inlines: MdInline[] }> = ({ inlines }) => (
  <>
    {inlines.map((inline, index) =>
      inline.kind === 'text' ? (
        inline.text
      ) : (
        <Text
          key={index}
          style={inline.kind === 'code' ? styles.inlineCode : styles.bold}
        >
          {inline.text}
        </Text>
      ),
    )}
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
  inlineCode: {
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
  codeBlockText: {
    color: OkenaColors.textPrimary,
    fontFamily: TerminalTheme.fontFamily,
    fontSize: 12,
    lineHeight: 17,
    padding: 10,
  },
});
