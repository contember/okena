/**
 * SettingsSheet.tsx — app settings, presented as a modal sheet.
 *
 * A modal (not a navigation screen) so it can open from the server list and
 * from the workspace drawer without unmounting the connected workspace.
 *
 * Terminal font size: a stepper over the settings store, a live preview in
 * the terminal font and an estimate of how many columns fit this screen.
 * Changes apply immediately to every open pane (they re-measure and resize
 * the remote PTY).
 *
 * Advanced: the terminal renderer toggle (native view or Skia) for the native
 * renderer rollout.
 */

import React from 'react';
import {
  Modal,
  Platform,
  Pressable,
  ScrollView,
  StyleSheet,
  Switch,
  Text,
  useWindowDimensions,
  View,
} from 'react-native';
import { useSafeAreaInsets } from 'react-native-safe-area-context';

import {
  DEFAULT_FONT_SIZE,
  MAX_FONT_SIZE,
  MIN_FONT_SIZE,
  useSettingsStore,
} from '../state/settingsStore';
import { OkenaColors, OkenaTypography, TerminalTheme } from '../theme';

/** JetBrains Mono advance width as a fraction of the font size (600/1000 em). */
const MONO_ADVANCE_EM = 0.6;

const PREVIEW_LINES = [
  '> fix the failing test in src/app.ts',
  '  Read 2 files, edited 1 file',
  '  accept edits on (shift+tab to cycle)',
];

export function formatFontSize(size: number): string {
  return Number.isInteger(size) ? `${size} pt` : `${size.toFixed(1)} pt`;
}

export const SettingsSheet: React.FC<{ visible: boolean; onClose: () => void }> = ({
  visible,
  onClose,
}) => {
  const insets = useSafeAreaInsets();
  const { width } = useWindowDimensions();
  const fontSize = useSettingsStore((s) => s.terminalFontSize);
  const step = useSettingsStore((s) => s.stepTerminalFontSize);
  const reset = useSettingsStore((s) => s.resetTerminalFontSize);
  const renderer = useSettingsStore((s) => s.renderer);
  const setRenderer = useSettingsStore((s) => s.setRenderer);

  const columns = Math.floor(width / (fontSize * MONO_ADVANCE_EM));
  const pageSheet = Platform.OS === 'ios';

  return (
    <Modal
      visible={visible}
      animationType="slide"
      presentationStyle={pageSheet ? 'pageSheet' : 'fullScreen'}
      onRequestClose={onClose}
    >
      <View style={[styles.root, !pageSheet && { paddingTop: insets.top }]}>
        <View style={styles.header}>
          <Text style={styles.title}>Settings</Text>
          <Pressable
            hitSlop={8}
            style={({ pressed }) => [styles.done, pressed && styles.pressed]}
            onPress={onClose}
            accessibilityRole="button"
          >
            <Text style={styles.doneText}>Done</Text>
          </Pressable>
        </View>

        <ScrollView contentContainerStyle={{ paddingBottom: insets.bottom + 24 }}>
          <Text style={styles.sectionLabel}>TERMINAL</Text>
          <View style={styles.group}>
            <View style={styles.row}>
              <View style={styles.rowText}>
                <Text style={styles.rowTitle}>Font size</Text>
                <Text style={styles.rowValue}>{formatFontSize(fontSize)}</Text>
              </View>
              <View style={styles.stepper}>
                <StepButton
                  label="A"
                  small
                  disabled={fontSize <= MIN_FONT_SIZE}
                  accessibilityLabel="Smaller font"
                  onPress={() => step(-1)}
                />
                <View style={styles.stepperDivider} />
                <StepButton
                  label="A"
                  disabled={fontSize >= MAX_FONT_SIZE}
                  accessibilityLabel="Larger font"
                  onPress={() => step(1)}
                />
              </View>
            </View>

            <View style={styles.preview}>
              {PREVIEW_LINES.map((line) => (
                <Text
                  key={line}
                  style={[
                    styles.previewText,
                    { fontSize, lineHeight: fontSize * TerminalTheme.lineHeightFactor },
                  ]}
                  numberOfLines={1}
                  ellipsizeMode="clip"
                >
                  {line}
                </Text>
              ))}
            </View>

            <View style={[styles.row, styles.rowLast]}>
              <Text style={styles.hint}>
                {`About ${columns} columns fit this screen. Pinch the terminal to zoom.`}
              </Text>
              <Pressable
                hitSlop={8}
                disabled={fontSize === DEFAULT_FONT_SIZE}
                onPress={reset}
                accessibilityRole="button"
                accessibilityLabel="Reset font size"
              >
                <Text
                  style={[
                    styles.resetText,
                    fontSize === DEFAULT_FONT_SIZE && styles.resetTextDisabled,
                  ]}
                >
                  Reset
                </Text>
              </Pressable>
            </View>
          </View>
          <Text style={styles.footnote}>
            {`Range ${MIN_FONT_SIZE} to ${MAX_FONT_SIZE} pt. Applies to every terminal; the remote terminal is resized to fit.`}
          </Text>

          <Text style={[styles.sectionLabel, styles.sectionSpacing]}>ADVANCED</Text>
          <View style={styles.group}>
            <View style={[styles.row, styles.rowLast]}>
              <View style={styles.rowText}>
                <Text style={styles.rowTitle}>Native renderer</Text>
                <Text style={styles.rowValue}>
                  {renderer === 'native' ? 'Native view' : 'Skia'}
                </Text>
              </View>
              <Switch
                value={renderer === 'native'}
                onValueChange={(on) => setRenderer(on ? 'native' : 'skia')}
                trackColor={{ true: OkenaColors.accent, false: OkenaColors.surfaceOverlay }}
                accessibilityLabel="Native terminal renderer"
              />
            </View>
          </View>
          <Text style={styles.footnote}>
            Experimental. Draws terminals with the platform's own text renderer instead of Skia.
          </Text>
        </ScrollView>
      </View>
    </Modal>
  );
};

const StepButton: React.FC<{
  label: string;
  small?: boolean;
  disabled: boolean;
  accessibilityLabel: string;
  onPress: () => void;
}> = ({ label, small = false, disabled, accessibilityLabel, onPress }) => (
  <Pressable
    style={({ pressed }) => [styles.stepButton, pressed && !disabled && styles.stepPressed]}
    disabled={disabled}
    onPress={onPress}
    accessibilityRole="button"
    accessibilityLabel={accessibilityLabel}
    accessibilityState={{ disabled }}
  >
    <Text
      style={[
        styles.stepLabel,
        small ? styles.stepLabelSmall : styles.stepLabelLarge,
        disabled && styles.stepLabelDisabled,
      ]}
    >
      {label}
    </Text>
  </Pressable>
);

const styles = StyleSheet.create({
  root: { flex: 1, backgroundColor: OkenaColors.background },
  header: {
    flexDirection: 'row',
    alignItems: 'center',
    justifyContent: 'space-between',
    paddingHorizontal: 20,
    paddingTop: 16,
    paddingBottom: 12,
  },
  title: { ...OkenaTypography.title },
  done: { minHeight: 44, justifyContent: 'center', paddingHorizontal: 4 },
  doneText: { color: OkenaColors.accent, fontSize: 17, fontWeight: '600' },
  pressed: { opacity: 0.6 },
  sectionLabel: {
    color: OkenaColors.textTertiary,
    fontSize: 12,
    fontWeight: '600',
    letterSpacing: 0.6,
    marginHorizontal: 32,
    marginTop: 12,
    marginBottom: 6,
  },
  sectionSpacing: { marginTop: 28 },
  group: {
    marginHorizontal: 16,
    borderRadius: 12,
    backgroundColor: OkenaColors.surfaceElevated,
    overflow: 'hidden',
  },
  row: {
    flexDirection: 'row',
    alignItems: 'center',
    minHeight: 56,
    paddingHorizontal: 16,
    paddingVertical: 8,
  },
  rowLast: { minHeight: 48 },
  rowText: { flex: 1 },
  rowTitle: { color: OkenaColors.textPrimary, fontSize: 16 },
  rowValue: { color: OkenaColors.textSecondary, fontSize: 13, marginTop: 2 },
  stepper: {
    flexDirection: 'row',
    alignItems: 'center',
    borderRadius: 9,
    backgroundColor: OkenaColors.surfaceOverlay,
    borderWidth: StyleSheet.hairlineWidth,
    borderColor: OkenaColors.borderLight,
    overflow: 'hidden',
  },
  stepperDivider: { width: StyleSheet.hairlineWidth, height: 22, backgroundColor: OkenaColors.borderLight },
  stepButton: { width: 52, height: 40, alignItems: 'center', justifyContent: 'center' },
  stepPressed: { backgroundColor: OkenaColors.keyBgPressed },
  stepLabel: { color: OkenaColors.textPrimary, fontWeight: '600' },
  stepLabelSmall: { fontSize: 13 },
  stepLabelLarge: { fontSize: 19 },
  stepLabelDisabled: { color: OkenaColors.textTertiary },
  preview: {
    marginHorizontal: 16,
    paddingHorizontal: 12,
    paddingVertical: 10,
    borderRadius: 8,
    backgroundColor: TerminalTheme.bgColor,
    borderWidth: StyleSheet.hairlineWidth,
    borderColor: OkenaColors.border,
  },
  previewText: { color: TerminalTheme.fgColor, fontFamily: TerminalTheme.fontFamily },
  hint: { flex: 1, color: OkenaColors.textSecondary, fontSize: 13, paddingRight: 12 },
  resetText: { color: OkenaColors.accent, fontSize: 15, fontWeight: '600' },
  resetTextDisabled: { color: OkenaColors.textTertiary },
  footnote: {
    color: OkenaColors.textTertiary,
    fontSize: 12,
    marginHorizontal: 32,
    marginTop: 8,
  },
});

export default SettingsSheet;
