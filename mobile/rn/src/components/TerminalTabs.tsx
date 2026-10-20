/**
 * TerminalTabs.tsx — compact terminal switcher under the workspace app bar.
 *
 * The phone shows one terminal at a time; when a project has several, this
 * horizontally-scrolling chip row lists them (desktop layout order, custom
 * name or `Terminal N`) and switches the visible one. Purely client-side: it
 * never touches the server layout. The active chip is kept scrolled into view.
 * A chip of a terminal running an agent carries its lifecycle dot and the
 * count of open Claude dialogs.
 */

import React, { useEffect, useRef } from 'react';
import { Pressable, ScrollView, StyleSheet, Text, View } from 'react-native';

import type { ProjectInfo, TerminalId } from '../native/okena';
import { terminalLabel } from '../models/terminalSelection';
import { OkenaColors } from '../theme';
import { DialogBadge, LifecycleDot } from './ClaudeIndicator';

export interface TerminalTabsProps {
  project: ProjectInfo;
  activeTerminalId: TerminalId | null;
  /** Open Claude dialogs per terminal id. */
  claudeDialogs: Record<TerminalId, number>;
  onSelect(terminalId: TerminalId): void;
}

export const TerminalTabs: React.FC<TerminalTabsProps> = ({
  project,
  activeTerminalId,
  claudeDialogs,
  onSelect,
}) => {
  const scrollRef = useRef<ScrollView>(null);
  const chipX = useRef(new Map<TerminalId, { x: number; width: number }>());
  const viewportWidth = useRef(0);

  // Keep the active chip visible (e.g. after a new terminal is auto-selected).
  useEffect(() => {
    if (!activeTerminalId) return;
    const chip = chipX.current.get(activeTerminalId);
    if (!chip || viewportWidth.current === 0) return;
    const x = Math.max(0, chip.x + chip.width / 2 - viewportWidth.current / 2);
    scrollRef.current?.scrollTo({ x, animated: true });
  }, [activeTerminalId, project.terminalIds]);

  return (
    <View style={styles.bar}>
      <ScrollView
        ref={scrollRef}
        horizontal
        showsHorizontalScrollIndicator={false}
        contentContainerStyle={styles.content}
        onLayout={(e) => {
          viewportWidth.current = e.nativeEvent.layout.width;
        }}
      >
        {project.terminalIds.map((tid) => {
          const active = tid === activeTerminalId;
          const agent = project.terminalAgentStatus[tid];
          const dialogs = claudeDialogs[tid] ?? 0;
          return (
            <Pressable
              key={tid}
              accessibilityRole="tab"
              accessibilityState={{ selected: active }}
              style={[styles.chip, active && styles.chipActive]}
              onLayout={(e) => {
                const { x, width } = e.nativeEvent.layout;
                chipX.current.set(tid, { x, width });
              }}
              onPress={() => {
                if (!active) onSelect(tid);
              }}
            >
              {agent ? <LifecycleDot lifecycle={agent.lifecycle} /> : null}
              <Text
                style={[styles.chipText, agent && styles.chipTextAfterDot, active && styles.chipTextActive]}
                numberOfLines={1}
              >
                {terminalLabel(project, tid)}
              </Text>
              {dialogs > 0 ? <DialogBadge count={dialogs} /> : null}
            </Pressable>
          );
        })}
      </ScrollView>
    </View>
  );
};

const styles = StyleSheet.create({
  bar: {
    height: 40,
    backgroundColor: OkenaColors.surface,
    borderBottomWidth: StyleSheet.hairlineWidth,
    borderBottomColor: OkenaColors.border,
  },
  content: { alignItems: 'center', paddingHorizontal: 8, gap: 6 },
  chip: {
    flexDirection: 'row',
    alignItems: 'center',
    maxWidth: 180,
    paddingHorizontal: 12,
    paddingVertical: 6,
    borderRadius: 14,
    backgroundColor: OkenaColors.surfaceElevated,
  },
  chipActive: { backgroundColor: OkenaColors.accent },
  chipText: { color: OkenaColors.textSecondary, fontSize: 13, flexShrink: 1 },
  chipTextAfterDot: { marginLeft: 6 },
  chipTextActive: { color: '#ffffff', fontWeight: '600' },
});

export default TerminalTabs;
