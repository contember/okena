/**
 * ServerListScreen.tsx — saved-server list + "add server" sheet.
 *
 * Ported from `mobile/lib/src/screens/server_list_screen.dart`:
 *   - header: "Servers" large title + a circular "+" add button,
 *   - empty state: terminal glyph, "No servers yet", "Add a server to get
 *     started", and an "Add Server" button,
 *   - list: one card per saved server — a letter avatar, the display name, and
 *     (when a label is set) the `host:port` subtitle; tapping connects.
 *   - add sheet: a bottom `Modal` with Host / Port / Label fields.
 *
 * The Dart screen used a `Dismissible` (swipe) to delete. RN core has no
 * swipe-to-dismiss without extra native deps, so deletion is exposed via a
 * long-press that reveals an inline delete affordance — same `removeServer`
 * behavior, no new dependencies.
 *
 * Reads/writes the connection store; holds only local UI state (sheet
 * visibility + field values + which row is in "delete" mode).
 */

import React, { useState } from 'react';
import {
  ActivityIndicator,
  FlatList,
  KeyboardAvoidingView,
  Modal,
  Platform,
  Pressable,
  StyleSheet,
  Text,
  TextInput,
  View,
} from 'react-native';

import {
  createSavedServer,
  savedServerDisplayName,
  type SavedServer,
} from '../models';
import { useConnectionStore } from '../state';
import { OkenaColors, OkenaTypography } from '../theme';

const DEFAULT_PORT = '19100';

export const ServerListScreen: React.FC = () => {
  const servers = useConnectionStore((s) => s.servers);
  const loaded = useConnectionStore((s) => s.loaded);
  const connectTo = useConnectionStore((s) => s.connectTo);
  const addServer = useConnectionStore((s) => s.addServer);
  const removeServer = useConnectionStore((s) => s.removeServer);

  const [sheetOpen, setSheetOpen] = useState(false);
  // Which server (by key) currently shows its delete affordance (long-pressed).
  const [pendingDeleteKey, setPendingDeleteKey] = useState<string | null>(null);

  const serverKey = (s: SavedServer) => `${s.host}:${s.port}`;

  const renderItem = ({ item }: { item: SavedServer }) => {
    const name = savedServerDisplayName(item);
    const initial = name.charAt(0).toUpperCase() || '?';
    const showDelete = pendingDeleteKey === serverKey(item);

    return (
      <View style={styles.cardRow}>
        <Pressable
          style={({ pressed }) => [styles.card, pressed && styles.cardPressed]}
          onPress={() => {
            if (showDelete) {
              setPendingDeleteKey(null);
              return;
            }
            connectTo(item);
          }}
          onLongPress={() => setPendingDeleteKey(showDelete ? null : serverKey(item))}
        >
          <View style={styles.avatar}>
            <Text style={styles.avatarText}>{initial}</Text>
          </View>
          <View style={styles.cardBody}>
            <Text style={styles.cardName} numberOfLines={1}>
              {name}
            </Text>
            {item.label !== undefined && (
              <Text style={styles.cardSub} numberOfLines={1}>
                {item.host}:{item.port}
              </Text>
            )}
          </View>
          {showDelete ? (
            <Pressable
              hitSlop={8}
              style={styles.deleteBtn}
              onPress={() => {
                setPendingDeleteKey(null);
                removeServer(item);
              }}
            >
              <Text style={styles.deleteBtnText}>Delete</Text>
            </Pressable>
          ) : (
            <Text style={styles.chevron}>{'›'}</Text>
          )}
        </Pressable>
      </View>
    );
  };

  return (
    <View style={styles.root}>
      {/* Header */}
      <View style={styles.header}>
        <Text style={OkenaTypography.largeTitle}>Servers</Text>
        <Pressable
          style={({ pressed }) => [styles.addButton, pressed && styles.addButtonPressed]}
          onPress={() => setSheetOpen(true)}
          accessibilityLabel="Add server"
        >
          <Text style={styles.addButtonPlus}>+</Text>
        </Pressable>
      </View>

      {/* Content */}
      {servers.length === 0 ? (
        loaded ? (
          <EmptyState onAdd={() => setSheetOpen(true)} />
        ) : (
          <View style={styles.center}>
            <ActivityIndicator color={OkenaColors.textSecondary} />
          </View>
        )
      ) : (
        <FlatList
          data={servers}
          keyExtractor={serverKey}
          renderItem={renderItem}
          contentContainerStyle={styles.listContent}
          keyboardShouldPersistTaps="handled"
        />
      )}

      <AddServerSheet
        visible={sheetOpen}
        onClose={() => setSheetOpen(false)}
        onAdd={(server) => {
          addServer(server);
          setSheetOpen(false);
        }}
      />
    </View>
  );
};

// ── Empty state ─────────────────────────────────────────────────────────────

const EmptyState: React.FC<{ onAdd: () => void }> = ({ onAdd }) => (
  <View style={styles.center}>
    <View style={styles.emptyGlyph}>
      <Text style={styles.emptyGlyphText}>{'⌨'}</Text>
    </View>
    <Text style={[OkenaTypography.title, styles.emptyTitle]}>No servers yet</Text>
    <Text style={styles.emptySub}>Add a server to get started</Text>
    <Pressable
      style={({ pressed }) => [styles.primaryButton, styles.emptyButton, pressed && styles.primaryButtonPressed]}
      onPress={onAdd}
    >
      <Text style={styles.primaryButtonText}>Add Server</Text>
    </Pressable>
  </View>
);

// ── Add-server bottom sheet ───────────────────────────────────────────────────

const AddServerSheet: React.FC<{
  visible: boolean;
  onClose: () => void;
  onAdd: (server: SavedServer) => void;
}> = ({ visible, onClose, onAdd }) => {
  const [host, setHost] = useState('');
  const [port, setPort] = useState(DEFAULT_PORT);
  const [label, setLabel] = useState('');

  // Reset fields each time the sheet is opened.
  const reset = () => {
    setHost('');
    setPort(DEFAULT_PORT);
    setLabel('');
  };

  const trimmedHost = host.trim();
  const canSubmit = trimmedHost.length > 0;

  const submit = () => {
    if (!canSubmit) return;
    const parsedPort = parseInt(port.trim(), 10);
    const server = createSavedServer({
      host: trimmedHost,
      port: Number.isFinite(parsedPort) ? parsedPort : 19100,
      label: label.trim() === '' ? undefined : label.trim(),
    });
    onAdd(server);
    reset();
  };

  const close = () => {
    reset();
    onClose();
  };

  return (
    <Modal
      visible={visible}
      transparent
      animationType="slide"
      onShow={reset}
      onRequestClose={close}
    >
      {/* Tap the dimmed backdrop to dismiss. */}
      <Pressable style={styles.backdrop} onPress={close}>
        <KeyboardAvoidingView
          behavior={Platform.OS === 'ios' ? 'padding' : undefined}
          style={styles.sheetWrap}
        >
          {/* Stop taps inside the sheet from bubbling to the backdrop. */}
          <Pressable style={styles.sheet} onPress={() => {}}>
            <View style={styles.dragHandle} />
            <Text style={OkenaTypography.title}>Add Server</Text>
            <Text style={styles.sheetSub}>
              Enter the host and port of your Okena desktop app
            </Text>

            <Field
              label="Host"
              value={host}
              onChangeText={setHost}
              placeholder="192.168.1.100"
              autoFocus
              keyboardType="url"
              autoCapitalize="none"
              autoCorrect={false}
              onSubmitEditing={submit}
            />
            <Field
              label="Port"
              value={port}
              onChangeText={setPort}
              placeholder={DEFAULT_PORT}
              keyboardType="number-pad"
              onSubmitEditing={submit}
            />
            <Field
              label="Label (optional)"
              value={label}
              onChangeText={setLabel}
              placeholder="My laptop"
              autoCapitalize="words"
              onSubmitEditing={submit}
            />

            <Pressable
              disabled={!canSubmit}
              style={({ pressed }) => [
                styles.primaryButton,
                !canSubmit && styles.primaryButtonDisabled,
                pressed && canSubmit && styles.primaryButtonPressed,
              ]}
              onPress={submit}
            >
              <Text style={styles.primaryButtonText}>Add Server</Text>
            </Pressable>
          </Pressable>
        </KeyboardAvoidingView>
      </Pressable>
    </Modal>
  );
};

const Field: React.FC<
  React.ComponentProps<typeof TextInput> & { label: string }
> = ({ label, ...inputProps }) => (
  <View style={styles.field}>
    <Text style={styles.fieldLabel}>{label}</Text>
    <TextInput
      {...inputProps}
      style={styles.input}
      placeholderTextColor={OkenaColors.textTertiary}
    />
  </View>
);

const styles = StyleSheet.create({
  root: {
    flex: 1,
    backgroundColor: OkenaColors.background,
    paddingTop: 44, // approximate top safe-area inset (no SafeAreaView dep)
  },
  center: {
    flex: 1,
    alignItems: 'center',
    justifyContent: 'center',
    paddingHorizontal: 24,
  },

  // Header
  header: {
    flexDirection: 'row',
    alignItems: 'center',
    justifyContent: 'space-between',
    paddingHorizontal: 24,
    paddingTop: 20,
    paddingBottom: 8,
  },
  addButton: {
    width: 36,
    height: 36,
    borderRadius: 18,
    alignItems: 'center',
    justifyContent: 'center',
    backgroundColor: OkenaColors.surfaceElevated,
    borderWidth: StyleSheet.hairlineWidth,
    borderColor: OkenaColors.border,
  },
  addButtonPressed: { opacity: 0.6 },
  addButtonPlus: {
    color: OkenaColors.accent,
    fontSize: 22,
    lineHeight: 24,
    fontWeight: '400',
  },

  // List
  listContent: { paddingHorizontal: 16, paddingTop: 8, paddingBottom: 24 },
  cardRow: { marginBottom: 8 },
  card: {
    flexDirection: 'row',
    alignItems: 'center',
    padding: 16,
    borderRadius: 14,
    backgroundColor: OkenaColors.surface,
    borderWidth: StyleSheet.hairlineWidth,
    borderColor: OkenaColors.border,
  },
  cardPressed: { opacity: 0.7 },
  avatar: {
    width: 40,
    height: 40,
    borderRadius: 10,
    alignItems: 'center',
    justifyContent: 'center',
    backgroundColor: '#7c7fff1f', // accent @ ~12%
    marginRight: 14,
  },
  avatarText: { ...OkenaTypography.headline, color: OkenaColors.accent },
  cardBody: { flex: 1 },
  cardName: { ...OkenaTypography.body, fontWeight: '500' },
  cardSub: {
    ...OkenaTypography.caption,
    marginTop: 3,
    color: OkenaColors.textTertiary,
    fontFamily: 'JetBrainsMono',
  },
  chevron: { color: OkenaColors.textTertiary, fontSize: 18, marginLeft: 8 },
  deleteBtn: {
    paddingHorizontal: 12,
    paddingVertical: 6,
    borderRadius: 8,
    backgroundColor: '#f8717126', // error @ ~15%
    marginLeft: 8,
  },
  deleteBtnText: { color: OkenaColors.error, fontWeight: '600', fontSize: 13 },

  // Empty state
  emptyGlyph: {
    width: 80,
    height: 80,
    borderRadius: 40,
    alignItems: 'center',
    justifyContent: 'center',
    backgroundColor: '#7c7fff26', // accent @ ~15%
  },
  emptyGlyphText: { fontSize: 36, color: OkenaColors.accent },
  emptyTitle: { marginTop: 20 },
  emptySub: { ...OkenaTypography.body, color: OkenaColors.textSecondary, marginTop: 8 },
  emptyButton: { marginTop: 28, width: 180 },

  // Primary button
  primaryButton: {
    height: 48,
    borderRadius: 12,
    alignItems: 'center',
    justifyContent: 'center',
    backgroundColor: OkenaColors.accent,
  },
  primaryButtonPressed: { opacity: 0.85 },
  primaryButtonDisabled: { opacity: 0.4 },
  primaryButtonText: { color: '#ffffff', fontSize: 16, fontWeight: '600' },

  // Add-server sheet
  backdrop: { flex: 1, backgroundColor: '#000000aa', justifyContent: 'flex-end' },
  sheetWrap: { width: '100%' },
  sheet: {
    backgroundColor: OkenaColors.surface,
    borderTopLeftRadius: 16,
    borderTopRightRadius: 16,
    paddingHorizontal: 24,
    paddingTop: 8,
    paddingBottom: 32,
  },
  dragHandle: {
    width: 36,
    height: 4,
    borderRadius: 2,
    backgroundColor: OkenaColors.textTertiary,
    alignSelf: 'center',
    marginBottom: 20,
    opacity: 0.4,
  },
  sheetSub: { ...OkenaTypography.callout, color: OkenaColors.textTertiary, marginTop: 4, marginBottom: 24 },
  field: { marginBottom: 14 },
  fieldLabel: { ...OkenaTypography.caption, color: OkenaColors.textSecondary, marginBottom: 6 },
  input: {
    height: 44,
    borderRadius: 10,
    paddingHorizontal: 14,
    backgroundColor: OkenaColors.surfaceElevated,
    borderWidth: StyleSheet.hairlineWidth,
    borderColor: OkenaColors.border,
    color: OkenaColors.textPrimary,
    fontSize: 15,
  },
});

export default ServerListScreen;
