/**
 * ClaudeDialogSheet.tsx — a bottom sheet with the open Claude dialogs of one
 * terminal, opened from the dashboard so a question is answered without
 * leaving it. It follows the terminal with {@link useClaudeStore} while
 * mounted and closes itself once no dialog is left.
 */

import React, { useEffect, useRef } from 'react';
import {
  ActivityIndicator,
  KeyboardAvoidingView,
  Modal,
  Platform,
  Pressable,
  ScrollView,
  StyleSheet,
  Text,
  View,
} from 'react-native';
import { useSafeAreaInsets } from 'react-native-safe-area-context';

import { ClaudeDialogCard } from './ClaudeDialogCard';
import { Icon } from './Icon';
import type { ConnId, TerminalId } from '../native/okena';
import { useClaudeStore } from '../state';
import { OkenaColors } from '../theme';

export interface ClaudeDialogSheetProps {
  connId: ConnId;
  terminalId: TerminalId;
  /** Project and terminal name. */
  title: string;
  onClose: () => void;
  onOpenChat: () => void;
}

export const ClaudeDialogSheet: React.FC<ClaudeDialogSheetProps> = ({
  connId,
  terminalId,
  title,
  onClose,
  onOpenChat,
}) => {
  const insets = useSafeAreaInsets();
  const session = useClaudeStore((s) => s.session);
  const sessionError = useClaudeStore((s) => s.sessionError);
  const dialogAnswers = useClaudeStore((s) => s.dialogAnswers);
  const open = useClaudeStore((s) => s.open);
  const close = useClaudeStore((s) => s.close);
  const answerDialog = useClaudeStore((s) => s.answerDialog);
  const scrollRef = useRef<ScrollView>(null);

  useEffect(() => {
    open(connId, terminalId);
    return close;
  }, [connId, terminalId, open, close]);

  // Answered here or elsewhere: nothing is left to answer.
  const answeredAll = session !== null && session.dialogs.length === 0;
  useEffect(() => {
    if (answeredAll) onClose();
  }, [answeredAll, onClose]);

  return (
    <Modal visible transparent animationType="slide" onRequestClose={onClose}>
      <KeyboardAvoidingView
        style={styles.root}
        behavior={Platform.OS === 'ios' ? 'padding' : undefined}
      >
        <Pressable
          style={[StyleSheet.absoluteFill, styles.backdrop]}
          onPress={onClose}
          accessibilityLabel="Close"
        />
        <View style={[styles.sheet, { paddingBottom: insets.bottom + 16 }]}>
          <View style={styles.handle} />
          <View style={styles.header}>
            <Text style={styles.title} numberOfLines={1}>
              {title}
            </Text>
            <Pressable
              hitSlop={8}
              style={styles.closeButton}
              onPress={onClose}
              accessibilityRole="button"
              accessibilityLabel="Close"
            >
              <Icon name="close" size={18} color={OkenaColors.textSecondary} />
            </Pressable>
          </View>
          {sessionError ? <Text style={styles.error}>{sessionError}</Text> : null}
          {session === null ? (
            <ActivityIndicator style={styles.loading} color={OkenaColors.textSecondary} />
          ) : (
            <ScrollView
              ref={scrollRef}
              style={styles.dialogs}
              keyboardShouldPersistTaps="handled"
            >
              {session.dialogs.map((dialog) => (
                <ClaudeDialogCard
                  key={dialog.id}
                  dialog={dialog}
                  answer={dialogAnswers[dialog.id]}
                  onDecide={(decision) => void answerDialog(dialog.id, decision)}
                  onRevealStep={() => scrollRef.current?.scrollToEnd({ animated: true })}
                />
              ))}
            </ScrollView>
          )}
          <Pressable
            style={styles.chatButton}
            onPress={onOpenChat}
            accessibilityRole="button"
          >
            <Icon name="chat" size={16} color={OkenaColors.textPrimary} />
            <Text style={styles.chatButtonText}>Open chat</Text>
          </Pressable>
        </View>
      </KeyboardAvoidingView>
    </Modal>
  );
};

const styles = StyleSheet.create({
  root: { flex: 1, justifyContent: 'flex-end' },
  backdrop: { backgroundColor: OkenaColors.backdrop },
  sheet: {
    maxHeight: '85%',
    paddingHorizontal: 12,
    borderTopLeftRadius: 20,
    borderTopRightRadius: 20,
    borderTopWidth: StyleSheet.hairlineWidth,
    borderTopColor: OkenaColors.borderLight,
    backgroundColor: OkenaColors.surfaceElevated,
  },
  handle: {
    alignSelf: 'center',
    width: 36,
    height: 4,
    borderRadius: 2,
    marginTop: 8,
    backgroundColor: OkenaColors.borderLight,
  },
  header: {
    flexDirection: 'row',
    alignItems: 'center',
    paddingHorizontal: 4,
    paddingVertical: 8,
  },
  title: {
    flex: 1,
    color: OkenaColors.textSecondary,
    fontSize: 13,
    fontWeight: '600',
  },
  closeButton: { padding: 8 },
  error: {
    color: OkenaColors.error,
    fontSize: 13,
    paddingHorizontal: 4,
    paddingBottom: 8,
  },
  loading: { paddingVertical: 32 },
  dialogs: { flexGrow: 0 },
  chatButton: {
    flexDirection: 'row',
    alignItems: 'center',
    justifyContent: 'center',
    gap: 8,
    minHeight: 44,
    marginTop: 8,
    borderRadius: 10,
    borderWidth: StyleSheet.hairlineWidth,
    borderColor: OkenaColors.borderLight,
  },
  chatButtonText: { color: OkenaColors.textPrimary, fontSize: 14, fontWeight: '500' },
});
