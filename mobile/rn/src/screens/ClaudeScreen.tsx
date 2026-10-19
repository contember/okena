/**
 * ClaudeScreen.tsx — follow and steer the `claude` session of one terminal.
 *
 * Layout, top to bottom: app bar (back, terminal name, lifecycle dot,
 * channel / hooks indicators), the transcript, the open dialog cards, and the
 * composer. The screen opens {@link useClaudeStore} for its terminal on mount
 * and closes it on unmount; the store does the polling. Back (button or the
 * Android hardware key) and a terminal that disappears from the workspace both
 * return to the workspace.
 */

import React, { useEffect, useMemo, useState } from 'react';
import {
  BackHandler,
  FlatList,
  KeyboardAvoidingView,
  Platform,
  Pressable,
  ScrollView,
  StyleSheet,
  Text,
  TextInput,
  View,
} from 'react-native';

import { ClaudeDialogCard } from '../components/ClaudeDialogCard';
import { LifecycleDot } from '../components/ClaudeIndicator';
import {
  buildClaudeChatRows,
  sameClaudeChatRow,
  type ClaudeChatRow,
} from '../models/claudeChat';
import type { ApiClaudePromptDelivery } from '../native/claude';
import { navigate, useNavStore } from '../navigation';
import {
  useClaudeStore,
  useConnectionStore,
  useWorkspaceStore,
} from '../state';
import { OkenaColors, TerminalTheme } from '../theme';

const DELIVERY_TEXT: Record<ApiClaudePromptDelivery, string> = {
  channel: 'Sent',
  pty: 'Typed into the terminal',
  queued: 'Queued until Claude finishes',
};

const HOOKS_HINT =
  'Answering dialogs needs Claude remote enabled in Okena settings and claude restarted in this pane.';

export const ClaudeScreen: React.FC = () => {
  const terminalId = useNavStore((s) => s.claudeTerminalId);
  const connId = useConnectionStore((s) => s.connId);
  const projects = useWorkspaceStore((s) => s.projects);

  const session = useClaudeStore((s) => s.session);
  const entries = useClaudeStore((s) => s.entries);
  const sessionError = useClaudeStore((s) => s.sessionError);
  const transcriptError = useClaudeStore((s) => s.transcriptError);
  const pollError = sessionError ?? transcriptError;
  const dialogError = useClaudeStore((s) => s.dialogError);
  const open = useClaudeStore((s) => s.open);
  const close = useClaudeStore((s) => s.close);
  const answerDialog = useClaudeStore((s) => s.answerDialog);

  useEffect(() => {
    if (connId === null || terminalId === null) return undefined;
    open(connId, terminalId);
    return close;
  }, [connId, terminalId, open, close]);

  useEffect(() => {
    const subscription = BackHandler.addEventListener(
      'hardwareBackPress',
      () => {
        navigate('workspace');
        return true;
      },
    );
    return () => subscription.remove();
  }, []);

  const project = projects.find(
    (p) => terminalId !== null && p.terminalIds.includes(terminalId),
  );
  const terminalName =
    (terminalId !== null ? project?.terminalNames[terminalId] : undefined) ??
    'Terminal';
  const lifecycle =
    terminalId !== null
      ? project?.terminalAgentStatus[terminalId]?.lifecycle
      : undefined;

  // An empty project list means the workspace has not loaded (yet), not that
  // the terminal is gone.
  const terminalGone =
    terminalId === null || (projects.length > 0 && project === undefined);
  useEffect(() => {
    if (terminalGone) navigate('workspace');
  }, [terminalGone]);

  // Newest first for the inverted list, which keeps the view pinned to the
  // latest entry unless the user scrolled up.
  const rows = useMemo(() => buildClaudeChatRows(entries).reverse(), [entries]);

  return (
    <View style={styles.root}>
      <View style={styles.appBar}>
        <Pressable
          hitSlop={8}
          style={styles.appBarBtn}
          onPress={() => navigate('workspace')}
          accessibilityRole="button"
          accessibilityLabel="Back"
        >
          <Text style={styles.appBarIcon}>{'‹'}</Text>
        </Pressable>
        <LifecycleDot lifecycle={lifecycle} />
        <Text style={styles.title} numberOfLines={1}>
          {terminalName}
        </Text>
        <View style={styles.flexSpacer} />
        {session ? (
          <>
            <Attachment label="channel" attached={session.channel_attached} />
            <Attachment label="hooks" attached={session.hooks_attached} />
          </>
        ) : null}
      </View>

      <KeyboardAvoidingView
        style={styles.flex}
        behavior={Platform.OS === 'ios' ? 'padding' : undefined}
      >
        {session && !session.hooks_attached ? (
          <Text style={styles.hint}>{HOOKS_HINT}</Text>
        ) : null}
        {pollError ? <Text style={styles.errorBanner}>{pollError}</Text> : null}

        <FlatList
          style={styles.flex}
          contentContainerStyle={styles.listContent}
          inverted
          data={rows}
          keyExtractor={(row) => row.key}
          renderItem={({ item }) => <ChatRow row={item} />}
          ListEmptyComponent={
            <Text style={styles.empty}>
              {session === null
                ? 'Loading…'
                : session.has_transcript
                ? 'No messages yet'
                : 'No Claude transcript for this terminal'}
            </Text>
          }
        />

        {session && session.dialogs.length > 0 ? (
          <ScrollView
            style={styles.dialogs}
            contentContainerStyle={styles.dialogsContent}
          >
            {session.dialogs.map((dialog) => (
              <ClaudeDialogCard
                key={dialog.id}
                dialog={dialog}
                onDecide={(decision) => void answerDialog(dialog.id, decision)}
              />
            ))}
          </ScrollView>
        ) : null}
        {dialogError ? <Text style={styles.error}>{dialogError}</Text> : null}

        <Composer />
      </KeyboardAvoidingView>
    </View>
  );
};

const Attachment: React.FC<{ label: string; attached: boolean }> = ({
  label,
  attached,
}) => (
  <View style={[styles.chip, attached && styles.chipOn]}>
    <Text style={[styles.chipText, attached && styles.chipTextOn]}>
      {label}
    </Text>
  </View>
);

const Composer: React.FC = () => {
  const [text, setText] = useState('');
  const busy = useClaudeStore((s) => s.session?.busy === true);
  const queued = useClaudeStore((s) => s.session?.queued_prompts ?? 0);
  const sending = useClaudeStore((s) => s.sending);
  const lastDelivery = useClaudeStore((s) => s.lastDelivery);
  const promptError = useClaudeStore((s) => s.promptError);
  const sendPrompt = useClaudeStore((s) => s.sendPrompt);
  const interrupt = useClaudeStore((s) => s.interrupt);

  const canSend = !sending && text.trim().length > 0;
  const send = async () => {
    if (!canSend) return;
    const sent = text;
    if (await sendPrompt(sent)) {
      setText((current) => (current === sent ? '' : current));
    }
  };

  const status = promptError
    ? promptError
    : lastDelivery
    ? DELIVERY_TEXT[lastDelivery]
    : queued > 0
    ? `${queued} queued`
    : null;

  return (
    <View style={styles.composer}>
      {status ? (
        <Text style={[styles.status, promptError ? styles.statusError : null]}>
          {status}
        </Text>
      ) : null}
      <View style={styles.composerRow}>
        <TextInput
          style={styles.input}
          value={text}
          onChangeText={setText}
          placeholder="Message Claude"
          placeholderTextColor={OkenaColors.textTertiary}
          multiline
        />
        {busy ? (
          <Pressable
            style={[styles.sendBtn, styles.interruptBtn]}
            onPress={() => void interrupt()}
            accessibilityRole="button"
          >
            <Text style={styles.sendText}>Stop</Text>
          </Pressable>
        ) : null}
        <Pressable
          style={[styles.sendBtn, !canSend && styles.disabled]}
          disabled={!canSend}
          onPress={() => void send()}
          accessibilityRole="button"
        >
          <Text style={styles.sendText}>Send</Text>
        </Pressable>
      </View>
    </View>
  );
};

const ChatRowContent: React.FC<{ row: ClaudeChatRow }> = ({ row }) => {
  switch (row.kind) {
    case 'user':
      return (
        <View style={styles.userBubble}>
          {row.viaChannel ? <Text style={styles.via}>via Okena</Text> : null}
          <Text style={styles.userText} selectable>
            {row.text}
          </Text>
        </View>
      );
    case 'assistant':
      return (
        <Text style={styles.assistantText} selectable>
          {row.text}
        </Text>
      );
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
  const failed = row.result?.is_error === true;
  const title = row.use
    ? `${row.use.name} — ${row.use.summary}`
    : 'Tool result';
  return (
    <Pressable
      style={styles.tool}
      disabled={row.result === undefined}
      onPress={() => setExpanded((value) => !value)}
      accessibilityRole="button"
      accessibilityState={{ expanded }}
    >
      <Text
        style={[styles.toolTitle, failed && styles.toolFailed]}
        numberOfLines={expanded ? undefined : 1}
      >
        {`${row.result === undefined ? '⋯' : expanded ? '▾' : '▸'} ${title}`}
      </Text>
      {expanded && row.result ? (
        <Text
          style={[styles.toolResult, failed && styles.toolFailed]}
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
  flexSpacer: { flex: 1 },
  appBar: {
    height: 96,
    paddingTop: 44,
    paddingHorizontal: 8,
    flexDirection: 'row',
    alignItems: 'center',
    backgroundColor: OkenaColors.surface,
    borderBottomWidth: StyleSheet.hairlineWidth,
    borderBottomColor: OkenaColors.border,
  },
  appBarBtn: { paddingHorizontal: 8, paddingVertical: 4 },
  appBarIcon: { color: OkenaColors.textPrimary, fontSize: 26 },
  title: {
    color: OkenaColors.textPrimary,
    fontSize: 17,
    fontWeight: '600',
    flexShrink: 1,
    marginLeft: 8,
  },
  chip: {
    paddingHorizontal: 6,
    paddingVertical: 2,
    borderRadius: 8,
    marginLeft: 6,
    borderWidth: StyleSheet.hairlineWidth,
    borderColor: OkenaColors.borderLight,
  },
  chipOn: { borderColor: OkenaColors.success },
  chipText: { color: OkenaColors.textTertiary, fontSize: 11 },
  chipTextOn: { color: OkenaColors.success },
  hint: {
    color: OkenaColors.textSecondary,
    fontSize: 12,
    padding: 10,
    backgroundColor: OkenaColors.surfaceElevated,
  },
  errorBanner: {
    color: OkenaColors.error,
    fontSize: 12,
    padding: 10,
    backgroundColor: OkenaColors.surfaceElevated,
  },
  listContent: { padding: 12 },
  empty: {
    color: OkenaColors.textTertiary,
    fontSize: 14,
    textAlign: 'center',
    padding: 24,
  },
  userBubble: {
    alignSelf: 'flex-end',
    maxWidth: '85%',
    backgroundColor: OkenaColors.accentSoft,
    borderRadius: 12,
    paddingHorizontal: 12,
    paddingVertical: 8,
    marginVertical: 4,
  },
  via: { color: OkenaColors.textTertiary, fontSize: 10, marginBottom: 2 },
  userText: { color: OkenaColors.textPrimary, fontSize: 14 },
  assistantText: {
    color: OkenaColors.textPrimary,
    fontSize: 14,
    marginVertical: 4,
  },
  tool: { paddingVertical: 4 },
  toolTitle: {
    color: OkenaColors.textSecondary,
    fontSize: 12,
    fontFamily: TerminalTheme.fontFamily,
  },
  toolResult: {
    color: OkenaColors.textSecondary,
    fontSize: 11,
    fontFamily: TerminalTheme.fontFamily,
    backgroundColor: OkenaColors.surface,
    borderRadius: 6,
    padding: 8,
    marginTop: 4,
  },
  toolFailed: { color: OkenaColors.error },
  dialogs: { maxHeight: '55%', flexGrow: 0 },
  dialogsContent: { paddingHorizontal: 8, paddingTop: 8 },
  error: {
    color: OkenaColors.error,
    fontSize: 12,
    paddingHorizontal: 12,
    paddingBottom: 4,
  },
  composer: {
    borderTopWidth: StyleSheet.hairlineWidth,
    borderTopColor: OkenaColors.border,
    backgroundColor: OkenaColors.surface,
    padding: 8,
  },
  status: { color: OkenaColors.textSecondary, fontSize: 12, marginBottom: 6 },
  statusError: { color: OkenaColors.error },
  composerRow: { flexDirection: 'row', alignItems: 'flex-end' },
  input: {
    flex: 1,
    maxHeight: 140,
    backgroundColor: OkenaColors.surfaceElevated,
    borderRadius: 8,
    paddingHorizontal: 10,
    paddingVertical: 8,
    color: OkenaColors.textPrimary,
    fontSize: 14,
  },
  sendBtn: {
    marginLeft: 8,
    paddingHorizontal: 14,
    paddingVertical: 10,
    borderRadius: 8,
    backgroundColor: OkenaColors.accent,
  },
  interruptBtn: { backgroundColor: OkenaColors.error },
  disabled: { opacity: 0.4 },
  sendText: { color: '#ffffff', fontSize: 14, fontWeight: '600' },
});

export default ClaudeScreen;
