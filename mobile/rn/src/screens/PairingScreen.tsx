/**
 * PairingScreen.tsx — connect → pair → connected flow.
 *
 * Ported from `mobile/lib/src/screens/pairing_screen.dart`:
 *   - header: a back button (which disconnects), the active server's display
 *     name, and a "Connecting" subtitle,
 *   - a centered {@link StatusIndicator},
 *   - body, by phase:
 *       • connecting (not yet pairing, no error): spinner + "Connecting to
 *         server...",
 *       • pairing: "Pair with Server" heading, a centered code input
 *         (XXXX-XXXX), and a "Pair" button (spinner while submitting),
 *       • error: a red ✕ badge, the error message in a tinted box, and a "Try
 *         Again" button that reconnects to the active server.
 *
 * Additionally shows the pinned TLS cert fingerprint for the active server when
 * present (RN ships TLS-on; the Dart model had no fingerprint) — so the user can
 * verify it against the desktop app's pairing dialog.
 *
 * On success (status → connected) `App.tsx`'s connection→nav binding routes to
 * the workspace; nothing to do here.
 */

import React, { useState } from 'react';
import {
  ActivityIndicator,
  Pressable,
  ScrollView,
  StyleSheet,
  Text,
  TextInput,
  View,
} from 'react-native';

import { savedServerDisplayName } from '../models';
import { useConnectionStore, selectIsPairing } from '../state';
import { OkenaColors, OkenaTypography } from '../theme';
import { StatusIndicator } from '../components/StatusIndicator';

export const PairingScreen: React.FC = () => {
  const status = useConnectionStore((s) => s.status);
  const activeServer = useConnectionStore((s) => s.activeServer);
  const isPairing = useConnectionStore(selectIsPairing);
  const pair = useConnectionStore((s) => s.pair);
  const disconnect = useConnectionStore((s) => s.disconnect);
  const connectTo = useConnectionStore((s) => s.connectTo);

  const [code, setCode] = useState('');
  const [submitting, setSubmitting] = useState(false);

  const isError = status.kind === 'error';
  const errorMessage = isError ? status.message : null;
  const showCodeInput = isPairing;

  const submitCode = async () => {
    const trimmed = code.trim();
    if (trimmed.length === 0 || submitting) return;
    setSubmitting(true);
    try {
      await pair(trimmed);
    } finally {
      setSubmitting(false);
    }
  };

  const tryAgain = () => {
    if (!activeServer) return;
    // Mirrors the Dart "Try Again": disconnect then reconnect the same server.
    disconnect();
    connectTo(activeServer);
  };

  return (
    <View style={styles.root}>
      {/* Header */}
      <View style={styles.header}>
        <Pressable
          hitSlop={8}
          style={styles.backButton}
          onPress={disconnect}
          accessibilityLabel="Back"
        >
          <Text style={styles.backChevron}>{'‹'}</Text>
        </Pressable>
        <View style={styles.headerText}>
          <Text style={OkenaTypography.headline} numberOfLines={1}>
            {activeServer ? savedServerDisplayName(activeServer) : 'Server'}
          </Text>
          <Text style={styles.headerSub}>Connecting</Text>
        </View>
      </View>

      {/* Body */}
      <ScrollView
        contentContainerStyle={styles.body}
        keyboardShouldPersistTaps="handled"
      >
        <View style={styles.statusWrap}>
          <StatusIndicator status={status} />
        </View>

        {!showCodeInput && !isError && (
          <View style={styles.connectingBlock}>
            <ActivityIndicator size="large" color={OkenaColors.textSecondary} />
            <Text style={styles.connectingText}>Connecting to server...</Text>
          </View>
        )}

        {showCodeInput && (
          <View>
            <Text style={[OkenaTypography.largeTitle, styles.centerText]}>
              Pair with Server
            </Text>
            <Text style={[styles.pairSub, styles.centerText]}>
              Check the Okena desktop app for the pairing code.
            </Text>

            <TextInput
              style={styles.codeInput}
              value={code}
              onChangeText={(t) => setCode(t.toUpperCase())}
              placeholder="XXXX-XXXX"
              placeholderTextColor={OkenaColors.textTertiary}
              autoCapitalize="characters"
              autoCorrect={false}
              autoFocus
              textAlign="center"
              onSubmitEditing={() => void submitCode()}
              returnKeyType="go"
            />

            <Pressable
              disabled={submitting}
              style={({ pressed }) => [
                styles.primaryButton,
                submitting && styles.primaryButtonDisabled,
                pressed && !submitting && styles.primaryButtonPressed,
              ]}
              onPress={() => void submitCode()}
            >
              {submitting ? (
                <ActivityIndicator color="#ffffff" />
              ) : (
                <Text style={styles.primaryButtonText}>Pair</Text>
              )}
            </Pressable>

            <FingerprintNote fingerprint={activeServer?.fingerprint} />
          </View>
        )}

        {isError && (
          <View>
            <View style={styles.errorBadge}>
              <Text style={styles.errorBadgeMark}>{'✕'}</Text>
            </View>
            <View style={styles.errorBox}>
              <Text style={styles.errorText}>{errorMessage ?? 'Connection failed'}</Text>
            </View>
            <Pressable
              style={({ pressed }) => [styles.outlineButton, pressed && styles.outlineButtonPressed]}
              onPress={tryAgain}
            >
              <Text style={styles.outlineButtonText}>Try Again</Text>
            </Pressable>
          </View>
        )}
      </ScrollView>
    </View>
  );
};

/** Small footnote showing the pinned TLS cert fingerprint, when known. */
const FingerprintNote: React.FC<{ fingerprint?: string }> = ({ fingerprint }) => {
  if (!fingerprint) return null;
  return (
    <View style={styles.fingerprintWrap}>
      <Text style={styles.fingerprintLabel}>TLS certificate fingerprint</Text>
      <Text style={styles.fingerprintValue} selectable>
        {fingerprint}
      </Text>
    </View>
  );
};

const styles = StyleSheet.create({
  root: {
    flex: 1,
    backgroundColor: OkenaColors.background,
    paddingTop: 44, // approximate top safe-area inset (no SafeAreaView dep)
  },

  // Header
  header: {
    flexDirection: 'row',
    alignItems: 'center',
    paddingLeft: 4,
    paddingRight: 16,
    paddingTop: 4,
  },
  backButton: {
    width: 40,
    height: 40,
    alignItems: 'center',
    justifyContent: 'center',
  },
  backChevron: { color: OkenaColors.accent, fontSize: 28, lineHeight: 30 },
  headerText: { flex: 1, marginLeft: 4 },
  headerSub: {
    ...OkenaTypography.caption2,
    color: OkenaColors.textTertiary,
    marginTop: 1,
  },

  // Body
  body: {
    flexGrow: 1,
    justifyContent: 'center',
    padding: 24,
  },
  statusWrap: { alignItems: 'center', marginBottom: 40 },
  centerText: { textAlign: 'center' },

  // Connecting
  connectingBlock: { alignItems: 'center' },
  connectingText: {
    ...OkenaTypography.body,
    color: OkenaColors.textSecondary,
    textAlign: 'center',
    marginTop: 20,
  },

  // Pairing
  pairSub: {
    ...OkenaTypography.body,
    color: OkenaColors.textSecondary,
    marginTop: 8,
    marginBottom: 32,
  },
  codeInput: {
    fontSize: 28,
    letterSpacing: 8,
    fontFamily: 'JetBrainsMono',
    fontWeight: '500',
    color: OkenaColors.textPrimary,
    height: 56,
    borderRadius: 10,
    backgroundColor: OkenaColors.surfaceElevated,
    borderWidth: StyleSheet.hairlineWidth,
    borderColor: OkenaColors.border,
    marginBottom: 24,
  },

  // Primary button
  primaryButton: {
    height: 48,
    borderRadius: 12,
    alignItems: 'center',
    justifyContent: 'center',
    backgroundColor: OkenaColors.accent,
  },
  primaryButtonPressed: { opacity: 0.85 },
  primaryButtonDisabled: { opacity: 0.5 },
  primaryButtonText: { color: '#ffffff', fontSize: 16, fontWeight: '600' },

  // Fingerprint
  fingerprintWrap: {
    marginTop: 28,
    alignItems: 'center',
  },
  fingerprintLabel: {
    ...OkenaTypography.caption2,
    color: OkenaColors.textTertiary,
    marginBottom: 4,
  },
  fingerprintValue: {
    fontFamily: 'JetBrainsMono',
    fontSize: 11,
    color: OkenaColors.textSecondary,
    textAlign: 'center',
  },

  // Error
  errorBadge: {
    width: 64,
    height: 64,
    borderRadius: 32,
    alignItems: 'center',
    justifyContent: 'center',
    alignSelf: 'center',
    backgroundColor: '#f871711a', // error @ ~10%
  },
  errorBadgeMark: { color: OkenaColors.error, fontSize: 32 },
  errorBox: {
    marginTop: 20,
    padding: 16,
    borderRadius: 12,
    backgroundColor: '#f8717114', // error @ ~8%
    borderWidth: StyleSheet.hairlineWidth,
    borderColor: '#f8717133', // error @ ~20%
  },
  errorText: { ...OkenaTypography.body, color: OkenaColors.error, textAlign: 'center' },
  outlineButton: {
    height: 48,
    borderRadius: 12,
    alignItems: 'center',
    justifyContent: 'center',
    marginTop: 24,
    borderWidth: StyleSheet.hairlineWidth,
    borderColor: OkenaColors.accent,
  },
  outlineButtonPressed: { opacity: 0.6 },
  outlineButtonText: { color: OkenaColors.accent, fontSize: 16, fontWeight: '600' },
});

export default PairingScreen;
