/**
 * ClaudeChat.tsx — the chat view of the `claude` session in one terminal,
 * shown by the workspace in place of the terminal.
 *
 * Top to bottom: a status strip (what Claude is doing), a notice when the
 * session is not connected to Okena, the transcript, the open dialog cards and
 * the composer. It opens {@link useClaudeStore} for its terminal on mount and
 * closes it on unmount; the store does the polling.
 */

import React, { useEffect, useMemo, useRef, useState } from 'react';
import {
  FlatList,
  Keyboard,
  Pressable,
  ScrollView,
  StyleSheet,
  Text,
  TextInput,
  View,
} from 'react-native';

import { ChatMarkdown } from './ChatMarkdown';
import { ClaudeDialogCard } from './ClaudeDialogCard';
import { Icon } from './Icon';
import {
  buildClaudeChatRows,
  claudeActivity,
  sameClaudeChatRow,
  type ClaudeActivity,
  type ClaudeChatRow,
} from '../models/claudeChat';
import type { ApiClaudePromptDelivery } from '../native/claude';
import type { ConnId, TerminalId } from '../native/okena';
import { useClaudeStore } from '../state';
import { OkenaColors, TerminalTheme } from '../theme';

const ACTIVITY: Record<ClaudeActivity, { label: string; color: string }> = {
  loading: { label: 'Connecting…', color: OkenaColors.textTertiary },
  needsInput: { label: 'Waiting for your answer', color: OkenaColors.warning },
  unhooked: {
    label: 'Not connected to Okena',
    color: OkenaColors.textTertiary,
  },
  working: { label: 'Working…', color: OkenaColors.accent },
  ready: { label: 'Ready', color: OkenaColors.success },
};

// Where a sent prompt went, shown briefly under the composer. A channel
// prompt needs no note: it shows up in the transcript right away.
const DELIVERY_NOTE_MS = 4000;
const DELIVERY_TEXT: Record<ApiClaudePromptDelivery, string | null> = {
  channel: null,
  pty: 'Typed into the terminal',
  queued: 'Queued until Claude finishes',
};

// Also shown after a daemon restart, until Claude's next hook event.
const UNHOOKED_HINT =
  'Claude connects at its next step when Claude Remote is on in Okena settings (Terminal). If it is off, turn it on and restart claude in this terminal.';

export interface ClaudeChatProps {
  connId: ConnId;
  terminalId: TerminalId;
  onShowTerminal: () => void;
}

export const ClaudeChat: React.FC<ClaudeChatProps> = ({
  connId,
  terminalId,
  onShowTerminal,
}) => {
  const session = useClaudeStore((s) => s.session);
  const entries = useClaudeStore((s) => s.entries);
  const sessionError = useClaudeStore((s) => s.sessionError);
  const transcriptError = useClaudeStore((s) => s.transcriptError);
  const dialogAnswers = useClaudeStore((s) => s.dialogAnswers);
  const open = useClaudeStore((s) => s.open);
  const close = useClaudeStore((s) => s.close);
  const answerDialog = useClaudeStore((s) => s.answerDialog);

  useEffect(() => {
    open(connId, terminalId);
    return close;
  }, [connId, terminalId, open, close]);

  // Newest first for the inverted list, which keeps the view pinned to the
  // latest entry unless the user scrolled up.
  const rows = useMemo(() => buildClaudeChatRows(entries).reverse(), [entries]);
  const activity = claudeActivity(session);
  const pollError = sessionError ?? transcriptError;

  const dialogsRef = useRef<ScrollView>(null);
  const dialogCount = session?.dialogs.length ?? 0;
  const previousDialogCount = useRef(dialogCount);
  useEffect(() => {
    // A new dialog needs its card's buttons, which the keyboard would cover.
    if (dialogCount > previousDialogCount.current) Keyboard.dismiss();
    previousDialogCount.current = dialogCount;
  }, [dialogCount]);

  return (
    <View style={styles.root}>
      <StatusStrip activity={activity} queued={session?.queued_prompts ?? 0} />
      {activity === 'unhooked' ? (
        <View style={styles.notice}>
          <Text style={styles.noticeText}>{UNHOOKED_HINT}</Text>
          <Pressable
            hitSlop={8}
            onPress={onShowTerminal}
            accessibilityRole="button"
          >
            <Text style={styles.noticeAction}>Show terminal</Text>
          </Pressable>
        </View>
      ) : null}
      {pollError ? (
        <Text style={[styles.notice, styles.errorText]}>{pollError}</Text>
      ) : null}

      {rows.length === 0 ? (
        // Outside the list: an inverted list draws its empty component upside down.
        <View style={styles.empty}>
          <Text style={styles.emptyText}>
            {session === null
              ? 'Loading…'
              : session.has_transcript
              ? 'No messages yet'
              : 'No Claude conversation in this terminal yet'}
          </Text>
        </View>
      ) : (
        <FlatList
          style={styles.flex}
          contentContainerStyle={styles.listContent}
          inverted
          data={rows}
          keyExtractor={(row) => row.key}
          renderItem={({ item }) => <ChatRow row={item} />}
          ListHeaderComponent={activity === 'working' ? <WorkingRow /> : null}
          keyboardDismissMode="on-drag"
          keyboardShouldPersistTaps="handled"
        />
      )}

      {session && session.dialogs.length > 0 ? (
        <ScrollView
          ref={dialogsRef}
          style={styles.dialogs}
          contentContainerStyle={styles.dialogsContent}
          keyboardShouldPersistTaps="handled"
        >
          {session.dialogs.map((dialog) => (
            <ClaudeDialogCard
              key={dialog.id}
              dialog={dialog}
              answer={dialogAnswers[dialog.id]}
              onDecide={(decision) => void answerDialog(dialog.id, decision)}
              onRevealStep={() =>
                dialogsRef.current?.scrollToEnd({ animated: true })
              }
            />
          ))}
        </ScrollView>
      ) : null}

      <Composer />
    </View>
  );
};

const StatusStrip: React.FC<{ activity: ClaudeActivity; queued: number }> = ({
  activity,
  queued,
}) => {
  const { label, color } = ACTIVITY[activity];
  return (
    <View style={styles.strip} accessibilityRole="summary">
      <View style={[styles.dot, { backgroundColor: color }]} />
      <Text style={styles.stripText} numberOfLines={1}>
        {label}
      </Text>
      <View style={styles.flex} />
      {queued > 0 ? (
        <Text style={styles.stripMeta}>{`${queued} queued`}</Text>
      ) : null}
    </View>
  );
};

const WorkingRow: React.FC = () => (
  <View style={styles.working}>
    <View style={[styles.dot, { backgroundColor: OkenaColors.accent }]} />
    <Text style={styles.workingText}>Claude is working</Text>
  </View>
);

const Composer: React.FC = () => {
  const [text, setText] = useState('');
  const busy = useClaudeStore((s) => s.session?.busy === true);
  const sending = useClaudeStore((s) => s.sending);
  const lastDelivery = useClaudeStore((s) => s.lastDelivery);
  const promptError = useClaudeStore((s) => s.promptError);
  const sendPrompt = useClaudeStore((s) => s.sendPrompt);
  const interrupt = useClaudeStore((s) => s.interrupt);

  const hasText = text.trim().length > 0;
  const send = async () => {
    if (sending || !hasText) return;
    const sent = text;
    if (await sendPrompt(sent)) {
      setText((current) => (current === sent ? '' : current));
    }
  };

  const [deliveryNote, setDeliveryNote] = useState<string | null>(null);
  useEffect(() => {
    const note = lastDelivery ? DELIVERY_TEXT[lastDelivery] : null;
    setDeliveryNote(note);
    if (note === null) return undefined;
    const timer = setTimeout(() => setDeliveryNote(null), DELIVERY_NOTE_MS);
    return () => clearTimeout(timer);
  }, [lastDelivery]);

  const status = promptError ?? deliveryNote;
  // While Claude works and nothing is typed, the button stops the turn.
  const showStop = busy && !hasText;

  return (
    <View style={styles.composer}>
      {status ? (
        <Text style={[styles.status, promptError ? styles.errorText : null]}>
          {status}
        </Text>
      ) : null}
      <View style={styles.composerRow}>
        <TextInput
          style={styles.input}
          value={text}
          onChangeText={setText}
          placeholder={busy ? 'Queue a message for Claude' : 'Message Claude'}
          placeholderTextColor={OkenaColors.textTertiary}
          keyboardAppearance="dark"
          multiline
        />
        {showStop ? (
          <RoundButton
            icon="stop"
            label="Interrupt Claude"
            color={OkenaColors.error}
            onPress={() => void interrupt()}
          />
        ) : (
          <RoundButton
            icon="arrowUp"
            label="Send"
            color={OkenaColors.accent}
            disabled={sending || !hasText}
            onPress={() => void send()}
          />
        )}
      </View>
    </View>
  );
};

const RoundButton: React.FC<{
  icon: 'stop' | 'arrowUp';
  label: string;
  color: string;
  disabled?: boolean;
  onPress: () => void;
}> = ({ icon, label, color, disabled, onPress }) => (
  <Pressable
    style={({ pressed }) => [
      styles.roundBtn,
      { backgroundColor: color },
      disabled && styles.disabled,
      pressed && styles.pressed,
    ]}
    disabled={disabled}
    onPress={onPress}
    accessibilityRole="button"
    accessibilityLabel={label}
  >
    <Icon
      name={icon}
      size={icon === 'stop' ? 16 : 20}
      strokeWidth={2.5}
      color="#ffffff"
    />
  </Pressable>
);

const ChatRowContent: React.FC<{ row: ClaudeChatRow }> = ({ row }) => {
  switch (row.kind) {
    case 'user':
      return (
        <View style={styles.userBubble}>
          <Text style={styles.userText} selectable>
            {row.text}
          </Text>
          {row.viaChannel ? <Text style={styles.via}>from phone</Text> : null}
        </View>
      );
    case 'assistant':
      return <ChatMarkdown text={row.text} style={styles.assistant} />;
    case 'tool':
      return <ToolRow row={row} />;
  }
};

// Every append rebuilds the row objects; only rows whose content changed re-render.
const ChatRow = React.memo(ChatRowContent, (previous, next) =>
  sameClaudeChatRow(previous.row, next.row),
);

const ToolRow: React.FC<{ row: Extract<ClaudeChatRow, { kind: 'tool' }> }> = ({
  row,
}) => {
  const [expanded, setExpanded] = useState(false);
  const pending = row.result === undefined;
  const failed = row.result?.is_error === true;
  return (
    <Pressable
      style={styles.tool}
      disabled={pending}
      onPress={() => setExpanded((value) => !value)}
      accessibilityRole="button"
      accessibilityState={{ expanded, disabled: pending }}
    >
      <View style={styles.toolHead}>
        {pending ? (
          <View style={styles.toolPending} />
        ) : (
          <Icon
            name={expanded ? 'caretDown' : 'caretRight'}
            size={12}
            color={failed ? OkenaColors.error : OkenaColors.textTertiary}
            style={styles.toolMark}
          />
        )}
        <Text
          style={[styles.toolText, styles.toolName, failed && styles.errorText]}
        >
          {row.use?.name ?? 'Tool result'}
        </Text>
        {row.use?.summary ? (
          // Middle: keeps both a command's start and a path's file name.
          <Text
            style={[styles.toolText, styles.toolSummary]}
            numberOfLines={expanded ? undefined : 1}
            ellipsizeMode="middle"
          >
            {row.use.summary}
          </Text>
        ) : null}
      </View>
      {expanded && row.result ? (
        <Text
          style={[styles.toolResult, failed && styles.errorText]}
          selectable
        >
          {row.result.text}
        </Text>
      ) : null}
    </Pressable>
  );
};

const styles = StyleSheet.create({
  root: { flex: 1, backgroundColor: OkenaColors.background },
  flex: { flex: 1 },
  strip: {
    height: 32,
    flexDirection: 'row',
    alignItems: 'center',
    paddingHorizontal: 14,
    borderBottomWidth: StyleSheet.hairlineWidth,
    borderBottomColor: OkenaColors.border,
  },
  dot: { width: 8, height: 8, borderRadius: 4, marginRight: 8 },
  stripText: {
    color: OkenaColors.textSecondary,
    fontSize: 13,
    fontWeight: '500',
  },
  stripMeta: { color: OkenaColors.textTertiary, fontSize: 12 },
  notice: {
    paddingHorizontal: 14,
    paddingVertical: 10,
    backgroundColor: OkenaColors.surfaceElevated,
    borderBottomWidth: StyleSheet.hairlineWidth,
    borderBottomColor: OkenaColors.border,
  },
  noticeText: {
    color: OkenaColors.textSecondary,
    fontSize: 13,
    lineHeight: 18,
  },
  noticeAction: {
    color: OkenaColors.accent,
    fontSize: 13,
    fontWeight: '600',
    marginTop: 6,
  },
  errorText: { color: OkenaColors.error },
  empty: {
    flex: 1,
    alignItems: 'center',
    justifyContent: 'center',
    padding: 24,
  },
  emptyText: {
    color: OkenaColors.textTertiary,
    fontSize: 14,
    textAlign: 'center',
  },
  listContent: { paddingHorizontal: 14, paddingVertical: 12 },
  working: { flexDirection: 'row', alignItems: 'center', paddingVertical: 8 },
  workingText: { color: OkenaColors.textTertiary, fontSize: 13 },
  userBubble: {
    alignSelf: 'flex-end',
    maxWidth: '85%',
    backgroundColor: OkenaColors.accentSoft,
    borderRadius: 16,
    borderBottomRightRadius: 4,
    paddingHorizontal: 12,
    paddingVertical: 8,
    marginVertical: 8,
  },
  userText: { color: OkenaColors.textPrimary, fontSize: 15, lineHeight: 21 },
  via: {
    color: OkenaColors.textTertiary,
    fontSize: 11,
    marginTop: 2,
    textAlign: 'right',
  },
  assistant: { marginVertical: 6 },
  tool: { paddingVertical: 5 },
  toolHead: { flexDirection: 'row', alignItems: 'flex-start' },
  toolMark: { marginTop: 2 },
  toolPending: {
    width: 8,
    height: 8,
    marginTop: 4,
    marginHorizontal: 2,
    borderRadius: 4,
    borderWidth: 1.5,
    borderColor: OkenaColors.accent,
  },
  toolText: { fontFamily: TerminalTheme.fontFamily, fontSize: 12 },
  toolName: {
    color: OkenaColors.textSecondary,
    fontWeight: '700',
    marginLeft: 6,
  },
  toolSummary: { flex: 1, color: OkenaColors.textTertiary, marginLeft: 10 },
  toolResult: {
    color: OkenaColors.textSecondary,
    fontSize: 11,
    lineHeight: 16,
    fontFamily: TerminalTheme.fontFamily,
    backgroundColor: OkenaColors.surface,
    borderRadius: 8,
    padding: 10,
    marginTop: 6,
  },
  dialogs: { maxHeight: '60%', flexGrow: 0 },
  dialogsContent: { paddingHorizontal: 10, paddingTop: 10 },
  composer: {
    borderTopWidth: StyleSheet.hairlineWidth,
    borderTopColor: OkenaColors.border,
    backgroundColor: OkenaColors.surface,
    paddingHorizontal: 10,
    paddingVertical: 8,
  },
  status: {
    color: OkenaColors.textTertiary,
    fontSize: 12,
    marginBottom: 6,
    marginLeft: 6,
  },
  composerRow: { flexDirection: 'row', alignItems: 'flex-end' },
  input: {
    flex: 1,
    minHeight: 40,
    maxHeight: 140,
    backgroundColor: OkenaColors.surfaceElevated,
    borderRadius: 20,
    borderWidth: 1,
    borderColor: OkenaColors.borderLight,
    paddingHorizontal: 16,
    paddingTop: 10,
    paddingBottom: 10,
    color: OkenaColors.textPrimary,
    fontSize: 15,
  },
  roundBtn: {
    width: 40,
    height: 40,
    borderRadius: 20,
    marginLeft: 8,
    alignItems: 'center',
    justifyContent: 'center',
  },
  disabled: { opacity: 0.35 },
  pressed: { opacity: 0.7 },
});
